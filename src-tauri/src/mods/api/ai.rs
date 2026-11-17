// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `ai.complete` (Mods spec §1.6, plan X5): a mod asks PM's AI model to answer, and pays for it from
//! its own monthly cap. Experimental (D13): outside the API freeze, listed so in the Developer tab.
//!
//! **The flow**, each step refusing with a fixed PM sentence under a `MOD-####` code (D-25):
//! 1. The arguments are parsed strictly (`{messages:[{role,content}], maxTokens}`, unknown fields
//!    refused), sized, and `maxTokens` is clamped, never refused for being large.
//! 2. The route is PM's own Background route ([`ModelEdge::resolve`]); none is `MOD-5004`. Only a
//!    route PM can vouch for may carry a mod's text ([`serves_a_mod`]).
//! 3. What only the network can say: the local arm's address class (a PUBLIC "local" endpoint is a
//!    paid hosted service PM cannot price, so it fails closed) and fresh prices, both bounded and
//!    both before any money is held. A lookup that fails or runs long leaves the route unpriced,
//!    which refuses; nothing here can make a call cheaper.
//! 4. ONE blocking step under the DB guard: `recheck` (the grant and the mod, as of now), the worst
//!    case price over EVERY model the route may fall through to, the cap check and the booking
//!    ([`money::reserve`]), so N parallel calls cannot each see the same headroom. The guard is
//!    never held across an await (I-14).
//! 5. The model call, under [`limits::CALL_TIMEOUT`], bounded by `maxTokens` on every leg.
//! 6. The settle: what the call cost replaces the booking, and the usage row is written with the
//!    mod's id, whether or not the mod still exists. Then `still_current`, and only then the reply.
//!
//! **A booking never leaks.** It is handed to a [`Booking`] guard the moment it is made. Every
//! normal end settles it through the guard's slot, and a call dropped mid-flight (a future cancelled
//! at an await) settles it from the guard's `Drop` as a call that may have reached the wire (the
//! whole booking counts). Only a process kill leaves a row, which counts until the month ends (plan
//! decision 4): the cap errs high, never low.
//!
//! **A failure costs the booking** (a timeout or any model error on a priced route settles as spent)
//! because a request that left may have been billed and PM cannot always tell: the cap errs high.
//!
//! **What the mod gets back** is `{text, truncated, model?}` and nothing else. Provider text never
//! reaches a mod (`openrouter.rs` embeds provider detail in its errors): every failure is one of
//! [`ModError`]'s fixed sentences, and the model's name is told only when it is one of the route's
//! own listed models. PM adds nothing to the mod's prompt and trusts nothing from it: the messages
//! go to the model exactly as written, and the answer is handed to the mod and kept nowhere in PM,
//! so a mod's text never becomes part of a prompt PM builds for itself.

use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};

use super::handlers::{parse, within};
use super::limits;
use super::{blocking, recheck, still_current, Authorised, Code, Host, ModError, Reply};
use crate::llm_gateway::{CallMeta, LlmOutcome, RoutePlan};
use crate::local_slot::EndpointClass;
use crate::mods::ai as money;
use crate::openrouter::{ChatMessage, Completion};

/// The longest PM waits for what only the network can say before a call is booked: a local
/// endpoint's address class, and a price refresh. Past it the answer is "don't know", which prices
/// as unknown and refuses.
const PREP_TIMEOUT: Duration = Duration::from_secs(15);

/// What PM counts for each message beyond its text, in bytes: a chat template wraps every message in
/// a few tokens. Bytes bound the prompt's tokens from above (a token is at least one byte, no
/// tokenizer needed), and this keeps that true for the wrapping. Over-reserving fails safe.
const MESSAGE_OVERHEAD_BYTES: u64 = 16;
/// The same, once per call, for the template's own start and end.
const CALL_OVERHEAD_BYTES: u64 = 32;

/// The longest answer text PM hands a mod, in bytes. `maxTokens` bounds the model, but a server
/// that ignores it must not be able to send a mod (or PM's reply cap) more: the text is cut here,
/// at a character boundary, and `truncated` says so. Sixteen bytes a token is past any real one.
const MAX_REPLY_BYTES: usize = limits::MAX_AI_OUTPUT_TOKENS as usize * 16;

/// The roles a mod may write. PM adds none: an `assistant` message is the mod's own few-shot text,
/// and `system` is the mod's own instruction, both its to write.
const ROLES: [&str; 3] = ["system", "user", "assistant"];

// --- the seam -----------------------------------------------------------------------------------

/// What a call needs from the model side of PM, so the handler's rules (the cap's concurrency
/// guarantee above all) are proven with no model. The app is the real one ([`AppHandle`]'s
/// `llm_gateway` route); a test scripts its own (`test_support::FakeModel`).
///
/// [`AppHandle`]: tauri::AppHandle
pub trait ModelEdge: Send + Sync {
    /// The route a mod's call would run on now (the Background role, like PM's own background
    /// jobs): the store and the keychain are read, so this is called from a blocking task. `None`
    /// when nothing can answer.
    fn resolve(&self) -> crate::error::Result<Option<RoutePlan>>;

    /// The class of the address a local endpoint resolves to now, or `None` when it cannot be told
    /// (counted as public by the caller). A DNS lookup: the caller bounds the wait.
    fn local_class(&self, base_url: &str) -> impl Future<Output = Option<EndpointClass>> + Send;

    /// Make sure the cached prices are fresh, for a route with a cloud arm. A user with no OpenRouter
    /// key contacts nothing. Best effort: the caller prices from whatever the cache holds.
    fn refresh_prices(&self) -> impl Future<Output = ()> + Send;

    /// Run the call on `plan`, every leg bounded by `max_tokens`. No cache prefix, nothing added
    /// to `messages`.
    fn complete(
        &self,
        plan: &RoutePlan,
        messages: &[ChatMessage],
        max_tokens: u32,
    ) -> impl Future<Output = crate::error::Result<LlmOutcome>> + Send;
}

impl ModelEdge for tauri::AppHandle {
    fn resolve(&self) -> crate::error::Result<Option<RoutePlan>> {
        crate::llm_gateway::resolve(self, crate::llm_gateway::Role::Background)
    }

    fn local_class(&self, base_url: &str) -> impl Future<Output = Option<EndpointClass>> + Send {
        crate::local_ai::endpoint_class(base_url)
    }

    async fn refresh_prices(&self) {
        // Its own key gate stops a local-only user; a failure leaves the cache as it was.
        let _ = crate::commands::ensure_pricing_fresh(self).await;
    }

    fn complete(
        &self,
        plan: &RoutePlan,
        messages: &[ChatMessage],
        max_tokens: u32,
    ) -> impl Future<Output = crate::error::Result<LlmOutcome>> + Send {
        crate::llm_gateway::complete_capped(self, plan, messages, false, Some(max_tokens))
    }
}

/// Whether `plan` may carry a mod's text. A cloud arm goes through OpenRouter, whose body pins
/// zero-data-retention on every request (`openrouter::chat_body`, held by its tests), and a local arm
/// stays on the user's own endpoint; a direct provider (outside ZDR by design) would be a new
/// [`RoutePlan`] variant and is not one. The match is exhaustive on purpose: adding a variant fails
/// to compile here until someone decides whether a mod may use it (I-13: a mod call never runs on
/// a non-ZDR arm).
pub(super) fn serves_a_mod(plan: &RoutePlan) -> bool {
    match plan {
        RoutePlan::Cloud(_)
        | RoutePlan::LocalOnly(_)
        | RoutePlan::LocalThenCloud { .. }
        | RoutePlan::CloudForPower { .. } => true,
    }
}

// --- the arguments ------------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Args {
    messages: Vec<Message>,
    #[serde(rename = "maxTokens")]
    max_tokens: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Message {
    role: String,
    content: String,
}

/// The arguments as the call runs them.
struct Request {
    messages: Vec<ChatMessage>,
    /// `maxTokens`, clamped to [`limits::MAX_AI_OUTPUT_TOKENS`].
    max_tokens: u32,
    /// The bound on the prompt's tokens the price is booked on.
    prompt_bytes: u64,
}

impl Request {
    fn parse(args: &Value) -> Result<Self, ModError> {
        let Args {
            messages,
            max_tokens,
        } = parse(args)?;
        if messages.is_empty() || messages.len() > limits::MAX_AI_MESSAGES {
            return Err(ModError::bad_args(format!(
                "Send between 1 and {} messages.",
                limits::MAX_AI_MESSAGES
            )));
        }
        if messages.iter().any(|m| !ROLES.contains(&m.role.as_str())) {
            // The role is the mod's own text, but a refusal quotes none of it.
            return Err(ModError::bad_args(
                "A message's role must be system, user or assistant.",
            ));
        }
        let text_bytes: usize = messages.iter().map(|m| m.content.len()).sum();
        if text_bytes > limits::MAX_AI_PROMPT_BYTES {
            return Err(ModError::bad_args(format!(
                "The messages are past {} KiB.",
                limits::MAX_AI_PROMPT_BYTES >> 10
            )));
        }
        if messages.iter().all(|m| m.content.trim().is_empty()) {
            return Err(ModError::bad_args("Send some text to answer."));
        }
        if max_tokens == 0 {
            return Err(ModError::bad_args("maxTokens must be at least 1."));
        }
        let prompt_bytes = CALL_OVERHEAD_BYTES
            + messages
                .iter()
                .map(|m| (m.role.len() + m.content.len()) as u64 + MESSAGE_OVERHEAD_BYTES)
                .sum::<u64>();
        Ok(Self {
            messages: messages
                .into_iter()
                .map(|m| ChatMessage {
                    role: m.role,
                    content: m.content,
                })
                .collect(),
            max_tokens: max_tokens.min(limits::MAX_AI_OUTPUT_TOKENS),
            prompt_bytes,
        })
    }
}

// --- the booking guard --------------------------------------------------------------------------

type Slot = Arc<Mutex<Option<money::Reservation>>>;

fn lock(slot: &Mutex<Option<money::Reservation>>) -> MutexGuard<'_, Option<money::Reservation>> {
    slot.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Holds a call's booking until it is settled. The booking lives in a slot the blocking steps share
/// (so one that never ran leaves it here, and one that did has taken it): whoever takes it settles
/// it, once. A call dropped while a booking is still here settles it as spent ([`money::settle_unknown`]):
/// it may have reached the wire.
struct Booking<H: Host> {
    host: H,
    slot: Slot,
}

impl<H: Host> Booking<H> {
    fn new(host: &H) -> Self {
        Self {
            host: host.clone(),
            slot: Arc::new(Mutex::new(None)),
        }
    }
}

impl<H: Host> Drop for Booking<H> {
    fn drop(&mut self) {
        let Some(res) = lock(&self.slot).take() else {
            return;
        };
        // A short synchronous write: a drop cannot await. The guard is taken here and nowhere an
        // await is pending, so the non-reentrant DB mutex is never taken twice by one thread.
        // A closed or locked store: the booking row stays and counts until the month ends.
        let store = self.host.store();
        if let Ok(mut conn) = store.conn() {
            let epoch = store.store_epoch();
            if let Err(e) = money::settle_unknown(&mut conn, epoch, res) {
                eprintln!("mods: ai.complete could not settle a dropped call: {e}");
            }
        }
    }
}

// --- the handler --------------------------------------------------------------------------------

/// `ai.complete({messages, maxTokens})` → `{text, truncated, model?}`.
pub(super) async fn complete<H: Host>(
    host: &H,
    auth: &Authorised,
    args: &Value,
) -> Result<Reply, ModError> {
    run(host, auth, args, limits::CALL_TIMEOUT).await
}

/// [`complete`] with the model call's deadline given (a test's short one).
pub(super) async fn run<H: Host>(
    host: &H,
    auth: &Authorised,
    args: &Value,
    limit: Duration,
) -> Result<Reply, ModError> {
    let request = Request::parse(args)?;

    // 2. The route.
    let plan = {
        let host = host.clone();
        blocking(move || host.model().resolve().map_err(ModError::internal)).await?
    };
    still_current(host, auth)?;
    let plan = plan.ok_or_else(ModError::no_model)?;
    if !serves_a_mod(&plan) {
        return Err(ModError::no_model());
    }

    // 3. What only the network can say, before any money is held.
    let class = match local_url(&plan) {
        Some(url) => tokio::time::timeout(PREP_TIMEOUT, host.model().local_class(url))
            .await
            .ok()
            .flatten(),
        None => None,
    };
    if has_cloud_arm(&plan) {
        let _ = tokio::time::timeout(PREP_TIMEOUT, host.model().refresh_prices()).await;
    }
    still_current(host, auth)?;

    // 4. Price, check the cap and book, in one step.
    let plan = Arc::new(plan);
    let booking = Booking::new(host);
    {
        let (host, auth, plan, slot) = (
            host.clone(),
            auth.clone(),
            Arc::clone(&plan),
            Arc::clone(&booking.slot),
        );
        let (prompt_bytes, max_tokens) = (request.prompt_bytes, request.max_tokens);
        blocking(move || reserve(&host, &auth, &plan, class, prompt_bytes, max_tokens, &slot))
            .await?;
    }

    // 5. The model call.
    let answered = within(limit, async {
        host.model()
            .complete(&plan, &request.messages, request.max_tokens)
            .await
            .map_err(|_| {
                // What a provider said can echo the prompt, so none of it is logged or sent on.
                eprintln!("mods: an ai.complete model call failed");
                ModError::model_call_failed()
            })
    })
    .await;

    // 6. Settle, whatever happened, then ask whether the mod is still there to hear it.
    let outcome = match answered {
        Ok(outcome) => outcome,
        Err(e) => {
            settle_unknown(host, &booking.slot).await;
            return Err(e);
        }
    };
    // The served model is the provider's word: it prices and is told only when it is one of the
    // route's own models (a name PM did not list prices as unknown, which books the whole hold).
    let LlmOutcome {
        mut completion,
        meta,
    } = outcome;
    completion.model = completion
        .model
        .take()
        .filter(|m| plan.models().contains(m));
    let text = std::mem::take(&mut completion.text);
    let (truncated, model) = (completion.truncated, completion.model.clone());
    settle(host, &booking.slot, &plan, completion, meta).await;
    still_current(host, auth)?;
    // A vault switched under the call: its session belongs to the store it began in, so the answer
    // is not delivered (nothing of the new store was read, and the old store was settled above).
    if host.store().store_epoch() != auth.ticket.store_epoch {
        return Err(ModError::session_ended());
    }
    Ok(reply(text, truncated, model))
}

/// The local arm's endpoint, if the plan has one.
fn local_url(plan: &RoutePlan) -> Option<&str> {
    match plan {
        RoutePlan::LocalOnly(local) | RoutePlan::LocalThenCloud { local, .. } => {
            Some(&local.base_url)
        }
        RoutePlan::Cloud(_) | RoutePlan::CloudForPower { .. } => None,
    }
}

/// Whether the plan can spend: it has a cloud arm.
fn has_cloud_arm(plan: &RoutePlan) -> bool {
    !matches!(plan, RoutePlan::LocalOnly(_))
}

/// Step 4, in a blocking task: under one DB guard, recheck, price, check the cap and book. The
/// booking goes into `slot` before anything else can fail, and is released (it never reached the
/// wire) if the call stopped being current.
fn reserve<H: Host>(
    host: &H,
    auth: &Authorised,
    plan: &RoutePlan,
    class: Option<EndpointClass>,
    prompt_bytes: u64,
    max_tokens: u32,
    slot: &Mutex<Option<money::Reservation>>,
) -> Result<(), ModError> {
    let store = host.store();
    let mut conn = store.conn().map_err(|_| ModError::locked())?;
    let facts = recheck(host, &conn, auth)?;
    // Read under this guard: the epoch of the store the booking is made in.
    let epoch = store.store_epoch();
    let cost = money::RouteCost::of(plan, class);
    let usd = money::worst_case_usd(&cost, prompt_bytes, max_tokens, |m| {
        money::model_price(&conn, m)
    })
    .map_err(|_| ModError::price_unknown())?;
    let month = money::civil_month(crate::commands::resolve_zone(&conn), chrono::Utc::now());
    let reservation = money::reserve(
        &mut conn,
        epoch,
        facts.id.as_str(),
        &month,
        usd,
        cost.is_cloud(),
    )
    .map_err(reserve_refusal)?;
    *lock(slot) = Some(reservation);
    #[cfg(test)]
    host.at(super::Point::Booked);
    if let Err(stopped) = still_current(host, auth) {
        if let Some(reservation) = lock(slot).take() {
            if let Err(e) = money::release(&mut conn, epoch, reservation) {
                eprintln!("mods: ai.complete could not release a booking: {e}");
            }
        }
        return Err(stopped);
    }
    Ok(())
}

/// A refused booking as the mod is told it.
fn reserve_refusal(e: money::ReserveError) -> ModError {
    match e {
        // Uninstalled since the call began: its session is over.
        money::ReserveError::ModGone => ModError::session_ended(),
        // A priced route with no cap yet: the page asks (state-based, so a mod moved from a local
        // route to a cloud one asks at that moment, however it was granted before).
        money::ReserveError::NeedsCap => {
            ModError::new(Code::NeedsPrompt, "This needs your permission first (ai).")
                .with_permission("ai")
        }
        money::ReserveError::CapReached => ModError::ai_cap_reached(),
        money::ReserveError::BadAmount => {
            ModError::internal("a worst-case price that was not a number")
        }
        money::ReserveError::Db(e) => ModError::internal(e),
    }
}

/// A finished call: its cost replaces the booking and its usage row is written (one step under the
/// one DB guard). Taken from the slot inside the step, so a guard that settled it first leaves
/// nothing to do. A failure leaves the booking, which counts: the cap errs high.
async fn settle<H: Host>(
    host: &H,
    slot: &Slot,
    plan: &Arc<RoutePlan>,
    completion: Completion,
    meta: CallMeta,
) {
    // A settle must not be skipped because the future was dropped at this await: the work is a
    // blocking task that runs to its end, and the guard's drop is the backstop if it never starts.
    let (host, slot, plan) = (host.clone(), Arc::clone(slot), Arc::clone(plan));
    let _ = blocking(move || {
        let Some(reservation) = lock(&slot).take() else {
            return Ok(());
        };
        let store = host.store();
        let mut conn = store.conn().map_err(|_| ModError::locked())?;
        let epoch = store.store_epoch();
        let served = completion
            .model
            .as_deref()
            .unwrap_or_else(|| plan.primary_model_id());
        if let Err(e) = money::settle_and_log(
            &mut conn,
            epoch,
            reservation,
            &completion,
            &meta,
            Some(served),
        ) {
            eprintln!("mods: ai.complete could not settle a call: {e}");
        }
        Ok(())
    })
    .await;
}

/// A call that may have reached the wire and ended with no answer: the whole booking counts as spent.
async fn settle_unknown<H: Host>(host: &H, slot: &Slot) {
    let (host, slot) = (host.clone(), Arc::clone(slot));
    let _ = blocking(move || {
        let Some(reservation) = lock(&slot).take() else {
            return Ok(());
        };
        let store = host.store();
        let mut conn = store.conn().map_err(|_| ModError::locked())?;
        let epoch = store.store_epoch();
        if let Err(e) = money::settle_unknown(&mut conn, epoch, reservation) {
            eprintln!("mods: ai.complete could not settle a failed call: {e}");
        }
        Ok(())
    })
    .await;
}

/// The reply: the text (cut to [`MAX_REPLY_BYTES`]), whether the answer was cut short, and the model
/// when it is one the route listed. Nothing else of the call.
fn reply(text: String, truncated: bool, model: Option<String>) -> Reply {
    let (text, cut) = cut_text(text);
    let mut value = json!({"text": text, "truncated": truncated || cut});
    if let Some(model) = model {
        value["model"] = Value::String(model);
    }
    Reply::value(value)
}

/// `text` cut to [`MAX_REPLY_BYTES`] at a character boundary, and whether it was cut.
fn cut_text(mut text: String) -> (String, bool) {
    if text.len() <= MAX_REPLY_BYTES {
        return (text, false);
    }
    let mut end = MAX_REPLY_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    (text, true)
}
