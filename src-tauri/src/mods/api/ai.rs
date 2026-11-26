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
//! at an await) settles it from the guard's `Drop` the way a failed call is settled (below). A call
//! dropped while the booking step itself is still running (a blocking task outlives its future)
//! is released by that step, which sees the guard's flag ([`Slot::put`]). Only
//! a process kill leaves a row, which counts until the month ends (plan decision 4): the cap errs
//! high, never low. The one other way a row can stay is a store taken or swapped under the call:
//! a booking belongs to the store (epoch) it was made in, so a settle in any other store is refused
//! and the row counts to month's end the same way. The settle takes its connection before the
//! reservation leaves the slot, so a shut store loses nothing PM still holds.
//!
//! **A failure costs the booking only if a request may have left** (a timeout or a model error on a
//! priced route): a request that left may have been billed and PM cannot always tell, so it settles
//! as spent. A request that provably never reached the provider (Local AI switched off, no
//! connection, a deadline hit while still queued behind a chat) costs nothing and its booking is
//! released (plan D-19), so being offline cannot drain a cap. [`WireMark`] is how the model call
//! says which; with no word, it counts as spent.
//!
//! **What the mod gets back** is `{text, truncated, model?}` and nothing else. Provider text never
//! reaches a mod (`openrouter.rs` embeds provider detail in its errors): every failure is one of
//! [`ModError`]'s fixed sentences, and the model's name is told only when it is one of the route's
//! own listed models. PM adds nothing to the mod's prompt and trusts nothing from it: the messages
//! go to the model exactly as written, and the answer is handed to the mod and kept nowhere in PM,
//! so a mod's text never becomes part of a prompt PM builds for itself.

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};

use super::handlers::{parse, within};
use super::limits;
use super::{blocking, recheck, still_current, Authorised, Code, Host, ModError, Reply};
use crate::llm_gateway::{CallMeta, CloudArm, CloudProvider, LlmOutcome, RoutePlan};
use crate::local_slot::EndpointClass;
use crate::mods::ai as money;
use crate::openrouter::{ChatMessage, Completion};
use crate::wire_mark::WireMark;

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

    /// The Background role's whole cloud model list, in the person's order, whether or not
    /// auto-switch is on (so a plan with auto-switch off carries only the first of it). It is what
    /// the money prompt names: the call may use more of it later (the person turns auto-switch on,
    /// or moves a model up), and the prompt has to stay true then. A blocking store read.
    fn listed_models(&self) -> crate::error::Result<Vec<String>>;

    /// The class of the address a local endpoint resolves to now, or `None` when it cannot be told
    /// (counted as public by the caller). A DNS lookup: the caller bounds the wait.
    fn local_class(&self, base_url: &str) -> impl Future<Output = Option<EndpointClass>> + Send;

    /// Make sure the cached prices are fresh, for a route with a cloud arm. A user with no OpenRouter
    /// key contacts nothing. Best effort: the caller prices from whatever the cache holds.
    fn refresh_prices(&self) -> impl Future<Output = ()> + Send;

    /// Run the call on `plan`, every leg bounded by `max_tokens`. No cache prefix, nothing added
    /// to `messages`. `wire` is told when a paid request is about to be written, and when one
    /// failed before it connected, so the caller can give the booking back for a call that never
    /// left. Only a request that can be BILLED is announced (the OpenRouter one: a local endpoint
    /// is on the person's own side and costs nothing), and a new paid arm must announce itself
    /// too, or a call that fails on it would be given back as free.
    fn complete(
        &self,
        plan: &RoutePlan,
        messages: &[ChatMessage],
        max_tokens: u32,
        wire: &WireMark,
    ) -> impl Future<Output = crate::error::Result<LlmOutcome>> + Send;
}

impl ModelEdge for tauri::AppHandle {
    fn resolve(&self) -> crate::error::Result<Option<RoutePlan>> {
        crate::llm_gateway::resolve(self, crate::llm_gateway::Role::Background)
    }

    fn listed_models(&self) -> crate::error::Result<Vec<String>> {
        crate::llm_gateway::background_model_list(self)
    }

    fn local_class(&self, base_url: &str) -> impl Future<Output = Option<EndpointClass>> + Send {
        crate::local_ai::endpoint_class(base_url)
    }

    async fn refresh_prices(&self) {
        // Its own key gate stops a local-only user; a failure leaves the cache as it was.
        let _ = crate::commands::ensure_pricing_fresh(self).await;
    }

    async fn complete(
        &self,
        plan: &RoutePlan,
        messages: &[ChatMessage],
        max_tokens: u32,
        wire: &WireMark,
    ) -> crate::error::Result<LlmOutcome> {
        wire.scope(crate::llm_gateway::complete_capped(
            self,
            plan,
            messages,
            false,
            Some(max_tokens),
        ))
        .await
    }
}

/// Whether `plan` may carry a mod's text. A cloud arm goes through OpenRouter, whose body pins
/// zero-data-retention on every request (`openrouter::chat_body`, held by its tests), and a local arm
/// stays on the user's own endpoint. A direct provider (the person's own Anthropic, Gemini or OpenAI
/// key) is outside ZDR by design and is a [`CloudProvider`] on an ordinary cloud arm, not a route of
/// its own, so the arm's provider is asked too: a route with ANY non-OpenRouter arm, including the
/// cloud fallback of a local-first one, serves no mod. The match is exhaustive on purpose: adding a
/// variant fails to compile here until someone decides whether a mod may use it (I-13: a mod call
/// never runs on a non-ZDR arm). A new PAID arm must also announce its requests on the call's
/// [`WireMark`] (`wire_mark::send`), or a call that fails on it would be given back as free.
pub(super) fn serves_a_mod(plan: &RoutePlan) -> bool {
    let zdr_pinned = |arm: &CloudArm| arm.provider == CloudProvider::OpenRouter;
    match plan {
        RoutePlan::Cloud(arm) | RoutePlan::CloudForPower { cloud: arm, .. } => zdr_pinned(arm),
        RoutePlan::LocalOnly(_) => true,
        RoutePlan::LocalThenCloud { cloud, .. } => zdr_pinned(cloud),
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

/// Where a call's booking waits to be settled, shared by the [`Booking`] guard and the blocking
/// steps. A blocking task cannot be cancelled: it outlives a dropped call future, so a guard that
/// is dropped while `reserve` is still running can look at the slot before `reserve` has filled
/// it. The `abandoned` flag closes that: the guard sets it first, and `reserve` reads it after it
/// fills the slot, so one of the two always sees the other (see [`Slot::put`]).
#[derive(Default)]
pub(super) struct Slot {
    held: Mutex<Option<money::Reservation>>,
    abandoned: AtomicBool,
}

impl Slot {
    fn held(&self) -> MutexGuard<'_, Option<money::Reservation>> {
        self.held.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A slot that already holds a booking (a test's).
    #[cfg(test)]
    pub(super) fn with(reservation: money::Reservation) -> Self {
        let slot = Self::default();
        *slot.held() = Some(reservation);
        slot
    }

    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.held().is_none()
    }

    fn take(&self) -> Option<money::Reservation> {
        self.held().take()
    }

    /// The guard is gone: nobody will settle what lands here after this, so whoever puts a booking
    /// in must see it ([`Slot::put`]).
    fn abandon(&self) {
        self.abandoned.store(true, Ordering::SeqCst);
    }

    /// Stores a booking and says whether the guard had already been dropped. The flag is read after
    /// the booking is in the slot, and the guard sets it before it looks: if the guard looked first
    /// (and found nothing) the flag was already set, so `true` here means "release it yourself";
    /// if the booking was in first the guard's own look finds and settles it.
    fn put(&self, reservation: money::Reservation) -> bool {
        *self.held() = Some(reservation);
        self.abandoned.load(Ordering::SeqCst)
    }
}

/// Holds a call's booking until it is settled. The booking lives in a slot the blocking steps share
/// (so one that never ran leaves it here, and one that did has taken it): whoever takes it settles
/// it, once. A call dropped while a booking is still here settles it as a failed call, by what its
/// [`WireMark`] says ([`settle_failed_in`]); one dropped before `reserve` has booked leaves
/// `reserve` to release it ([`Slot::put`]).
struct Booking<H: Host> {
    host: H,
    slot: Arc<Slot>,
    /// What the model call's requests did, read at the end of a failed or dropped call.
    wire: WireMark,
}

impl<H: Host> Booking<H> {
    fn new(host: &H) -> Self {
        Self {
            host: host.clone(),
            slot: Arc::new(Slot::default()),
            wire: WireMark::new(),
        }
    }
}

impl<H: Host> Drop for Booking<H> {
    fn drop(&mut self) {
        // A short synchronous write: a drop cannot await. The guard is taken here and nowhere an
        // await is pending, so the non-reentrant DB mutex is never taken twice by one thread.
        // A closed or locked store: the booking stays in the slot, and its row counts until the
        // month ends. The flag goes up BEFORE the slot is looked at, never after.
        self.slot.abandon();
        settle_failed_in(
            &self.host,
            &self.slot,
            self.wire.may_have_reached_the_wire(),
        );
    }
}

/// A call that ended with no answer: if a request may have left, the whole booking counts as spent
/// ([`money::settle_unknown`]); if none did, it is deleted ([`money::release`]). The connection is
/// taken BEFORE the reservation leaves the slot, so a store that is shut right now leaves it there
/// for the guard's `Drop` instead of losing it.
pub(super) fn settle_failed_in<H: Host>(host: &H, slot: &Slot, reached_wire: bool) {
    // An empty slot is the normal end of every call (it was settled or never booked): that must
    // not touch the store at all, or each finished call would block an async worker on the DB
    // mutex behind whatever job holds it. A booking that lands after this look (a `reserve` still
    // running when the guard was dropped) is released by `reserve` itself, which sees the flag the
    // guard raised before looking ([`Slot::put`]).
    if slot.held().is_none() {
        return;
    }
    let store = host.store();
    let Ok(mut conn) = store.conn() else {
        return;
    };
    let Some(reservation) = slot.take() else {
        return;
    };
    let epoch = store.store_epoch();
    let done = if reached_wire {
        money::settle_unknown(&mut conn, epoch, reservation).map(|_| ())
    } else {
        money::release(&mut conn, epoch, reservation)
    };
    if let Err(e) = done {
        eprintln!("mods: ai.complete could not settle a failed call: {e}");
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

    // 5. The model call, its requests recorded on the booking's mark.
    let answered = within(limit, async {
        host.model()
            .complete(&plan, &request.messages, request.max_tokens, &booking.wire)
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
            settle_failed(host, &booking).await;
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

// --- what the money prompt is told ----------------------------------------------------------------

/// The shape of the route a mod's call would take now, in the words' own terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PromptRoute {
    /// Nothing can answer (Local AI off, no model picked), or the route may not carry a mod's text.
    None,
    /// A model on this machine or the person's own network.
    Local,
    /// The cloud, through the person's OpenRouter key.
    Cloud,
    /// A local model first, falling back to the cloud inside the call.
    LocalThenCloud,
    /// On battery, the request goes to the cloud instead of the local model.
    CloudForPower,
}

/// What `mod_ai_prompt_info` tells the page about the route, for the money prompt: PM's own facts
/// about the person's own settings, nothing the mod supplied.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptInfo {
    pub route: PromptRoute,
    /// The route has a cloud arm, so a call can cost money and the prompt asks for a monthly limit.
    pub paid: bool,
    /// PM cannot price the route (a public "local" endpoint, or no arm it can name): every call is
    /// refused with "price unknown" (MOD-5005), so a limit would change nothing.
    pub unpriced: bool,
    /// The Background role's cloud models: the person's whole list in their order, not only the
    /// one a call would send first (with auto-switch off that is just the first of them), so the
    /// prompt stays true when the list is used differently later. Empty when the route has no
    /// cloud arm. Plain text the person chose, never anything the mod supplied.
    pub cloud_models: Vec<String>,
    /// The Background role's local model, when Local AI is on and the role uses it. On battery it
    /// is the model the policy set aside, so the prompt can still name it.
    pub local_model: Option<String>,
    /// The limit the prompt starts from.
    pub suggested_cap_usd: f64,
}

/// [`PromptInfo`] from the resolved route, the class of its local arm's address (the same rule the
/// call itself prices by, so the prompt can never call a route free that the call would charge
/// for) and the Background role's whole model list (`listed`, [`ModelEdge::listed_models`]). Pure.
pub(crate) fn describe(
    plan: Option<&RoutePlan>,
    local_class: Option<EndpointClass>,
    listed: &[String],
) -> PromptInfo {
    let suggested_cap_usd = money::SUGGESTED_CAP_USD;
    // A route that cannot carry a mod's text is no route: the call answers "pick a model".
    let Some(plan) = plan.filter(|p| serves_a_mod(p)) else {
        return PromptInfo {
            route: PromptRoute::None,
            paid: false,
            unpriced: false,
            cloud_models: Vec::new(),
            local_model: None,
            suggested_cap_usd,
        };
    };
    let cost = money::RouteCost::of(plan, local_class);
    let route = match plan {
        RoutePlan::Cloud(_) => PromptRoute::Cloud,
        RoutePlan::LocalOnly(_) => PromptRoute::Local,
        RoutePlan::LocalThenCloud { .. } => PromptRoute::LocalThenCloud,
        RoutePlan::CloudForPower { .. } => PromptRoute::CloudForPower,
    };
    let named = |m: &String| !m.trim().is_empty();
    let (cloud_models, local_model) = match plan {
        RoutePlan::Cloud(cloud) => (cloud.models.clone(), None),
        RoutePlan::LocalOnly(local) => (Vec::new(), Some(local.model.clone())),
        RoutePlan::LocalThenCloud { local, cloud } => {
            (cloud.models.clone(), Some(local.model.clone()))
        }
        RoutePlan::CloudForPower {
            cloud,
            displaced_local_model,
        } => (cloud.models.clone(), Some(displaced_local_model.clone())),
    };
    // The whole list, then any model the route has that the list read missed (the setting moved
    // between the two reads): never fewer names than the call will really use.
    let cloud_models = if cloud_models.is_empty() {
        cloud_models
    } else {
        let mut all: Vec<String> = listed.to_vec();
        for m in cloud_models {
            if !all.contains(&m) {
                all.push(m);
            }
        }
        all
    };
    PromptInfo {
        route,
        paid: cost.is_cloud(),
        unpriced: cost == money::RouteCost::Unpriced,
        cloud_models: cloud_models.into_iter().filter(named).collect(),
        local_model: local_model.filter(named),
        suggested_cap_usd,
    }
}

/// What the prompt for `ai` needs: resolve the route as a call would (a blocking read of the store
/// and the keychain), look up the local arm's address class off the DB lock under a deadline, and
/// describe it. Reads only; contacts nothing but the local endpoint's name.
pub(crate) async fn prompt_info<H: Host>(host: &H) -> Result<PromptInfo, ModError> {
    let plan = {
        let host = host.clone();
        blocking(move || host.model().resolve().map_err(ModError::internal)).await?
    };
    let class = match plan.as_ref().and_then(local_url) {
        Some(url) => tokio::time::timeout(PREP_TIMEOUT, host.model().local_class(url))
            .await
            .ok()
            .flatten(),
        None => None,
    };
    // A list that can't be read falls back to the route's own names (never a failure: the route
    // already says what will answer).
    let listed = if plan.as_ref().is_some_and(has_cloud_arm) {
        let host = host.clone();
        blocking(move || Ok(host.model().listed_models().unwrap_or_default())).await?
    } else {
        Vec::new()
    };
    Ok(describe(plan.as_ref(), class, &listed))
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
    slot: &Slot,
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
    let month = money::current_month(&conn, chrono::Utc::now());
    let reservation = money::reserve(
        &mut conn,
        epoch,
        facts.id.as_str(),
        &month,
        usd,
        cost.is_cloud(),
    )
    .map_err(reserve_refusal)?;
    #[cfg(test)]
    host.at(super::Point::Booking);
    let abandoned = slot.put(reservation);
    #[cfg(test)]
    host.at(super::Point::Booked);
    // A call dropped while this step ran has nobody left to settle the booking: it is released
    // here, under the guard already held, as a call that stopped being current is.
    let current = if abandoned {
        Err(ModError::session_ended())
    } else {
        still_current(host, auth)
    };
    if let Err(stopped) = current {
        if let Some(reservation) = slot.take() {
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
    slot: &Arc<Slot>,
    plan: &Arc<RoutePlan>,
    completion: Completion,
    meta: CallMeta,
) {
    // A settle must not be skipped because the future was dropped at this await: the work is a
    // blocking task that runs to its end, and the guard's drop is the backstop if it never starts.
    let (host, slot, plan) = (host.clone(), Arc::clone(slot), Arc::clone(plan));
    let _ = blocking(move || {
        // The connection first: a store that is shut now leaves the reservation in the slot for
        // the guard's `Drop` rather than losing it.
        let store = host.store();
        let mut conn = store.conn().map_err(|_| ModError::locked())?;
        let Some(reservation) = slot.take() else {
            return Ok(());
        };
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

/// A call that ended with no answer (a model error or the deadline): see [`settle_failed_in`]. What
/// the call's requests did is read here, once the model future is gone, and the work itself runs
/// as a blocking task that is not skipped if this future is dropped at the await.
async fn settle_failed<H: Host>(host: &H, booking: &Booking<H>) {
    let (host, slot) = (host.clone(), Arc::clone(&booking.slot));
    let reached_wire = booking.wire.may_have_reached_the_wire();
    let _ = blocking(move || {
        settle_failed_in(&host, &slot, reached_wire);
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
