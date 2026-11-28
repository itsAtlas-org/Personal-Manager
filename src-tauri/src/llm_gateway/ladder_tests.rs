// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The ladder's executors, run with fake adapters and no network. The loop is [`walk_ladder`], which
//! is what `complete` and `stream_chat` call with their real attempts; here the attempt is a script,
//! but it runs inside the REAL per-attempt wire mark ([`attempt`]) and the real class-to-billing
//! rules, so what a rung proves about its billing is decided by the code that decides it in
//! production. The only requests made are to a closed or a silent port on this computer.
//!
//! THREAT MODEL. These tests hold the guards that catch MISTAKES by PM and real provider replies:
//! a request billed twice, a reply to a row nobody listed, a limit turning into a switch of meter,
//! Local AI off becoming a cloud bill. They do not stand for a hostile local user or a vendor lying
//! about usage.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use super::*;
use crate::entry_health::{Clock, Health};
use crate::error::{
    AttemptEntry, AttemptOutcome, Billing, Error, FailureClass, ProviderFailure, SkipReason,
};
use crate::openrouter::Usage;

const FAKE: &str = "fake-abcdefghijklmnop";

// ---- the world ---------------------------------------------------------------------------------

/// A clock the test moves by hand.
struct Fake {
    t0: Instant,
    w0: SystemTime,
    by: Mutex<Duration>,
}

impl Fake {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            t0: Instant::now(),
            w0: SystemTime::now(),
            by: Mutex::new(Duration::ZERO),
        })
    }
    fn advance(&self, d: Duration) {
        *self.by.lock().unwrap() += d;
    }
}

impl Clock for Fake {
    fn now(&self) -> Instant {
        self.t0 + *self.by.lock().unwrap()
    }
    fn wall(&self) -> SystemTime {
        self.w0 + *self.by.lock().unwrap()
    }
}

fn health() -> (Health, Arc<Fake>) {
    let clock = Fake::new();
    (Health::with_clock(clock.clone()), clock)
}

fn arm(p: CloudProvider, models: &[&str]) -> Rung {
    Rung::Cloud(CloudArm::new(
        p,
        Secret::from(FAKE.to_string()),
        models.iter().map(|m| m.to_string()).collect(),
    ))
}

fn local() -> Rung {
    Rung::Local(LocalArm {
        base_url: "http://127.0.0.1:1".into(),
        model: "llama3".into(),
        token: None,
    })
}

fn unusable(p: CloudProvider, why: SkipReason) -> Rung {
    Rung::Unusable {
        entry: AttemptEntry {
            provider: Some(p),
            model: Some("m".into()),
            local_elsewhere: false,
        },
        why,
    }
}

use CloudProvider::{Anthropic, Gemini, OpenAi, OpenRouter};

fn typed(class: FailureClass, billing: Billing, provider: CloudProvider) -> Error {
    Error::Provider(Box::new(ProviderFailure {
        class,
        billing,
        provider,
        model: None,
        message: format!("{} said {}.", provider.as_str(), class.as_str()),
    }))
}

/// A request that cannot connect: nothing of it leaves this computer.
async fn leaves_nothing() {
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let client = reqwest::Client::new();
    let _ = crate::wire_mark::send(client.post(format!("http://127.0.0.1:{port}/"))).await;
}

/// A request that connects and is written: whatever comes back, it left.
async fn leaves_something() {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        if let Ok((s, _)) = l.accept() {
            drop(s);
        }
    });
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    let _ =
        crate::wire_mark::send(client.post(format!("http://127.0.0.1:{port}/")).body("x")).await;
}

/// What a scripted rung does when it is asked.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Say {
    /// Answers having sent a request.
    Answers,
    /// Fails with this class and billing; a billed one sends first, one that never left tries to
    /// connect to nothing, one rejected by a response sends first (a response came back).
    Fails(FailureClass, Billing),
    /// Fails like a transport error before any connection: a plain error, no class PM can place.
    ConnectFails,
    /// Fails after a request left, with an error PM cannot place (a reset, a timeout): it may have
    /// billed, so it must STOP the list.
    PlainErrorAfterSend,
    /// A rate limit (429, unbilled) that asked to be retried after this many seconds.
    RateLimitedAfter(u64),
    /// A local row answers.
    LocalAnswers,
    /// A local row fails, nothing of it shown.
    LocalFails,
    /// A local row fails after an answer token reached the UI.
    LocalFailsAfterAnAnswer,
    /// A local row fails because Local AI was switched off under it.
    LocalFailsAiOff,
}

fn completion(model: Option<&str>) -> Completion {
    Completion {
        text: "a reply".into(),
        model: model.map(str::to_string),
        usage: Usage::default(),
        truncated: false,
    }
}

/// One scripted attempt on a rung: the real `attempt` wrapper (own wire mark, class-to-billing),
/// with the script standing in for the provider.
async fn scripted(rung: &Rung, say: Say) -> std::result::Result<LlmOutcome, RungFailure> {
    match rung {
        Rung::Cloud(arm) => {
            // The real walk's own wrapper: its holder mark carries a Retry-After to the failure.
            let out = rung_call(
                attempt(arm, async {
                    match say {
                        Say::Answers => {
                            leaves_something().await;
                            Ok(completion(arm.models.first().map(String::as_str)))
                        }
                        Say::Fails(class, billing) => {
                            match billing {
                                Billing::NeverLeft => leaves_nothing().await,
                                _ => leaves_something().await,
                            }
                            Err(typed(class, billing, arm.provider))
                        }
                        Say::ConnectFails => {
                            leaves_nothing().await;
                            Err(Error::Other("could not reach the provider".into()))
                        }
                        Say::PlainErrorAfterSend => {
                            leaves_something().await;
                            Err(Error::Other("the connection was reset".into()))
                        }
                        Say::RateLimitedAfter(secs) => {
                            leaves_something().await;
                            crate::wire_mark::note_retry_after_for(Duration::from_secs(secs));
                            Err(typed(
                                FailureClass::RateLimited,
                                Billing::Rejected,
                                arm.provider,
                            ))
                        }
                        other => panic!("a cloud row was scripted {other:?}"),
                    }
                }),
                || false,
            )
            .await;
            out.map(|completion| LlmOutcome {
                completion,
                meta: CallMeta::cloud(Duration::from_millis(1), arm.provider),
            })
        }
        Rung::Local(l) => match say {
            Say::LocalAnswers => Ok(LlmOutcome {
                completion: completion(Some(&l.model)),
                meta: CallMeta::local(Duration::from_millis(1)),
            }),
            Say::LocalFails => Err(RungFailure::local(
                Error::Other("no model".into()),
                false,
                true,
            )),
            Say::LocalFailsAfterAnAnswer => {
                Err(RungFailure::local(Error::Other("cut".into()), true, true))
            }
            Say::LocalFailsAiOff => Err(RungFailure::local(
                Error::Other("Local AI is off".into()),
                false,
                false,
            )),
            other => panic!("a local row was scripted {other:?}"),
        },
        Rung::Unusable { .. } => Err(RungFailure::never_tried()),
    }
}

/// What a request meets: the memory, a region pause, the person's limit and the windows.
struct World {
    health: Health,
    clock: Arc<Fake>,
    paused: Vec<CloudProvider>,
    limited: Vec<CloudProvider>,
    windows: Vec<Option<i64>>,
    estimate: i64,
}

impl World {
    fn new() -> Self {
        let (health, clock) = health();
        Self {
            health,
            clock,
            paused: vec![],
            limited: vec![],
            windows: vec![],
            estimate: 100,
        }
    }
}

/// Walk `rungs` with `script[i]` as rung i's behaviour. Returns the result and the rungs asked, in
/// order.
async fn run(
    world: &World,
    rungs: &[Rung],
    script: &[Say],
    start: Start,
) -> (Result<LlmOutcome>, Vec<usize>) {
    let (out, asked, _) = run_noted(world, rungs, script, start).await;
    (out, asked)
}

/// [`run`], and what the walk says became of each row (the notes the chat's strip is told).
async fn run_noted(
    world: &World,
    rungs: &[Rung],
    script: &[Say],
    start: Start,
) -> (Result<LlmOutcome>, Vec<usize>, Vec<crate::error::Attempt>) {
    let asked = Rc::new(RefCell::new(Vec::new()));
    let log = asked.clone();
    let paused = world.paused.clone();
    let limited = world.limited.clone();
    let paused_fn = move |a: &CloudArm| paused.contains(&a.provider);
    let limit_fn = move |p: CloudProvider| {
        limited
            .contains(&p)
            .then(|| crate::daily_limit::test_refusal(p))
    };
    let (out, notes) = walk_ladder(
        Checks {
            health: &world.health,
            estimate: world.estimate,
            windows: &world.windows,
            paused: &paused_fn,
            limit: &limit_fn,
        },
        rungs,
        start,
        |i, rung| {
            log.borrow_mut().push(i);
            scripted(rung, script[i])
        },
    )
    .await;
    let asked = asked.borrow().clone();
    (out, asked, notes)
}

fn walk_class(class: FailureClass) -> Say {
    Say::Fails(class, Billing::Rejected)
}

// ---- the walk past each class ------------------------------------------------------------------

/// Every class that walks, from the row that failed to the next: the second row answers, says so in
/// the tag (the first note), and is the provider and model the outcome names. Nothing but the two
/// rows is asked.
#[tokio::test]
async fn a_row_that_failed_without_billing_walks_to_the_next_for_every_class_that_walks() {
    for class in [
        FailureClass::Busy,
        FailureClass::RateLimited,
        FailureClass::ModelGone,
        FailureClass::ContextTooLong,
        FailureClass::KeyBad,
        FailureClass::NoCredit,
        FailureClass::NeedsRetention,
        FailureClass::SetupStale,
        FailureClass::Paused,
    ] {
        let w = World::new();
        let rungs = [
            arm(Anthropic, &["claude-a"]),
            arm(Gemini, &["gemini-b"]),
            arm(OpenAi, &["gpt-c"]),
        ];
        let (out, asked) = run(
            &w,
            &rungs,
            &[walk_class(class), Say::Answers, Say::Answers],
            Start::default(),
        )
        .await;
        let out = out.unwrap_or_else(|e| panic!("{class:?}: {e}"));
        assert_eq!(asked, vec![0, 1], "{class:?}");
        assert_eq!(out.meta.cloud_provider, Some(Gemini), "{class:?}");
        assert_eq!(
            out.completion.model.as_deref(),
            Some("gemini-b"),
            "{class:?}"
        );
        assert_eq!(
            out.meta.fallback,
            Some(FallbackReason::ListWalk(class)),
            "{class:?}"
        );
        assert_eq!(out.meta.provider, Provider::Cloud);
    }
}

/// The walk hands the chat's strip every row's part, in order, ending on the row that answered: a
/// failed row, a row passed over before any request, and the answer.
#[tokio::test]
async fn the_walk_keeps_every_rows_part_in_order_for_the_strip() {
    use crate::error::{AttemptOutcome as O, SkipReason};
    let mut w = World::new();
    w.paused = vec![Gemini];
    let rungs = [
        arm(Anthropic, &["claude-a"]),
        arm(Gemini, &["gemini-b"]),
        arm(OpenAi, &["gpt-c"]),
    ];
    let (out, asked, notes) = run_noted(
        &w,
        &rungs,
        &[walk_class(FailureClass::Busy), Say::Answers, Say::Answers],
        Start::default(),
    )
    .await;
    out.unwrap();
    assert_eq!(asked, vec![0, 2], "the paused row is never asked");
    let parts: Vec<_> = notes.iter().map(|a| a.outcome).collect();
    assert_eq!(
        parts,
        vec![
            O::Failed(FailureClass::Busy),
            O::Skipped(SkipReason::Paused),
            O::Answered
        ]
    );
    assert_eq!(notes[2].entry.model.as_deref(), Some("gpt-c"));
}

/// A connect failure (nothing left) is the offline person's first row: the next one is tried.
#[tokio::test]
async fn a_connect_failure_walks_on_because_nothing_left() {
    let w = World::new();
    let rungs = [arm(Anthropic, &["a"]), arm(Gemini, &["g"])];
    let (out, asked) = run(
        &w,
        &rungs,
        &[Say::ConnectFails, Say::Answers],
        Start::default(),
    )
    .await;
    let out = out.unwrap();
    assert_eq!(asked, vec![0, 1]);
    assert_eq!(
        out.meta.fallback,
        Some(FallbackReason::ListWalk(FailureClass::Other))
    );
}

/// THE rule. After a failure that may have billed nothing follows, whatever its class: not the next
/// row, not any later one. The error is the failing row's own, so the card can speak to it.
#[tokio::test]
async fn a_failure_that_may_have_billed_stops_the_list() {
    for class in [
        FailureClass::Busy,
        FailureClass::StoppedPartway,
        FailureClass::Other,
    ] {
        let w = World::new();
        let rungs = [arm(Anthropic, &["a"]), arm(Gemini, &["g"]), local()];
        let (out, asked) = run(
            &w,
            &rungs,
            &[
                Say::Fails(class, Billing::MaybeBilled),
                Say::Answers,
                Say::LocalAnswers,
            ],
            Start::default(),
        )
        .await;
        assert_eq!(asked, vec![0], "{class:?}");
        let e = out.err().expect("stopped");
        assert_eq!(e.provider_failure().unwrap().class, class);
        assert_eq!(e.provider_failure().unwrap().billing, Billing::MaybeBilled);
    }
}

/// A refusal by the model is billed and would be refused again: it stops even when the status said
/// nothing was billed.
#[tokio::test]
async fn a_refusal_by_the_model_stops_the_list() {
    let w = World::new();
    let rungs = [arm(Anthropic, &["a"]), arm(Gemini, &["g"])];
    let (out, asked) = run(
        &w,
        &rungs,
        &[walk_class(FailureClass::Refused), Say::Answers],
        Start::default(),
    )
    .await;
    assert_eq!(asked, vec![0]);
    assert_eq!(
        out.err().unwrap().provider_failure().unwrap().class,
        FailureClass::Refused
    );
}

/// A list that ran out is ONE error that says what became of each row, and the card can still read
/// the last failure from it.
#[tokio::test]
async fn a_list_that_ran_out_says_what_became_of_every_row() {
    let w = World::new();
    let rungs = [
        unusable(Gemini, SkipReason::NoKey),
        arm(Anthropic, &["a"]),
        arm(OpenAi, &["o"]),
    ];
    let (out, asked) = run(
        &w,
        &rungs,
        &[
            Say::Answers,
            walk_class(FailureClass::NoCredit),
            walk_class(FailureClass::Busy),
        ],
        Start::default(),
    )
    .await;
    assert_eq!(asked, vec![1, 2]);
    let Error::Exhausted { attempts, last } = out.err().unwrap() else {
        panic!("expected an exhausted list");
    };
    assert_eq!(
        attempts.iter().map(|a| a.outcome).collect::<Vec<_>>(),
        vec![
            AttemptOutcome::Skipped(SkipReason::NoKey),
            AttemptOutcome::Failed(FailureClass::NoCredit),
            AttemptOutcome::Failed(FailureClass::Busy),
        ]
    );
    assert_eq!(last.provider_failure().unwrap().class, FailureClass::Busy);
    let e = Error::Exhausted { attempts, last };
    assert!(
        e.to_string().ends_with("Nothing else is in your list."),
        "{e}"
    );
}

// ---- rows passed over ---------------------------------------------------------------------------

/// A row that is not turned on or has no key is passed over, never asked, and the answer says so.
#[tokio::test]
async fn an_unusable_row_is_passed_over_and_said() {
    let w = World::new();
    let rungs = [unusable(Anthropic, SkipReason::NoKey), arm(Gemini, &["g"])];
    let (out, asked) = run(&w, &rungs, &[Say::Answers, Say::Answers], Start::default()).await;
    let out = out.unwrap();
    assert_eq!(asked, vec![1]);
    assert_eq!(
        out.meta.fallback,
        Some(FallbackReason::ListSkip(SkipReason::NoKey))
    );
}

/// A row paused where the person is (a free Gemini key in the UK) is passed over, never asked.
#[tokio::test]
async fn a_row_paused_where_you_are_is_passed_over() {
    let mut w = World::new();
    w.paused = vec![Gemini];
    let rungs = [arm(Gemini, &["g"]), arm(Anthropic, &["a"])];
    let (out, asked) = run(&w, &rungs, &[Say::Answers, Say::Answers], Start::default()).await;
    let out = out.unwrap();
    assert_eq!(asked, vec![1]);
    assert_eq!(out.meta.cloud_provider, Some(Anthropic));
    assert_eq!(
        out.meta.fallback,
        Some(FallbackReason::ListSkip(SkipReason::Paused))
    );
}

/// A prompt over a row's window by more than a tenth is not sent there; one within the margin is.
#[tokio::test]
async fn a_prompt_too_long_for_a_rows_window_skips_it_and_a_near_fit_does_not() {
    let mut w = World::new();
    w.estimate = 1_100;
    w.windows = vec![Some(1_000), Some(1_000_000)];
    let rungs = [arm(Gemini, &["g"]), arm(Anthropic, &["a"])];
    // 1_100 is exactly 10% over 1_000: not MORE than ten percent, so it is sent.
    let (out, asked) = run(&w, &rungs, &[Say::Answers, Say::Answers], Start::default()).await;
    assert_eq!(asked, vec![0]);
    assert_eq!(out.unwrap().meta.fallback, None);

    w.estimate = 1_101;
    let (out, asked) = run(&w, &rungs, &[Say::Answers, Say::Answers], Start::default()).await;
    assert_eq!(asked, vec![1]);
    assert_eq!(
        out.unwrap().meta.fallback,
        Some(FallbackReason::ListSkip(SkipReason::TooLong))
    );

    // An unknown window never skips.
    w.windows = vec![None, None];
    let (_, asked) = run(&w, &rungs, &[Say::Answers, Say::Answers], Start::default()).await;
    assert_eq!(asked, vec![0]);
}

/// An OpenRouter run's window is the largest of its models, and only when every one is known.
#[test]
fn an_openrouter_runs_window_is_the_largest_known_or_nothing() {
    let run = |models: &[&str]| match arm(OpenRouter, models) {
        Rung::Cloud(a) => a,
        _ => unreachable!(),
    };
    let known = |m: &str| match m {
        "small" => Some(8_000),
        "big" => Some(200_000),
        _ => None,
    };
    assert_eq!(arm_window(&run(&["small", "big"]), &known), Some(200_000));
    assert_eq!(arm_window(&run(&["small", "mystery"]), &known), None);
    // A direct row's is the catalogue's `max_input`.
    let direct = crate::direct_models::catalogue()
        .providers
        .iter()
        .flat_map(|s| s.models.iter())
        .next()
        .expect("a catalogue row");
    let a = CloudArm::new(
        Anthropic,
        Secret::from(FAKE.to_string()),
        vec![direct.id.clone()],
    );
    assert_eq!(arm_window(&a, &known), Some(i64::from(direct.max_input)));
    // A model the catalogue does not list has no window, so it never skips.
    let a = CloudArm::new(
        Anthropic,
        Secret::from(FAKE.to_string()),
        vec!["not/listed".into()],
    );
    assert_eq!(arm_window(&a, &known), None);
}

// ---- the daily limit is a brake -----------------------------------------------------------------

/// R3b. A limit-paused row never moves a request to another company's paid row, by itself; the
/// answer is the limit's own refusal, which the card turns into its offers.
#[tokio::test]
async fn a_limit_paused_row_stops_before_a_paid_row() {
    let mut w = World::new();
    w.limited = vec![Anthropic];
    let rungs = [
        arm(Anthropic, &["a"]),
        arm(Gemini, &["g"]),
        arm(OpenAi, &["o"]),
    ];
    let (out, asked) = run(&w, &rungs, &[Say::Answers; 3], Start::default()).await;
    assert!(asked.is_empty(), "nothing is sent: {asked:?}");
    let e = out.err().expect("stopped");
    let f = e.provider_failure().expect("the limit's typed refusal");
    assert_eq!(f.class, FailureClass::LimitReached);
    assert_eq!(f.billing, Billing::NeverLeft);
}

/// ...but a Local row after it answers, because it is free; and a paid row after THAT local one is
/// not tried if the local one fails.
#[tokio::test]
async fn a_limit_paused_row_lets_a_local_row_answer_and_nothing_paid_follows() {
    let mut w = World::new();
    w.limited = vec![Anthropic];
    let rungs = [arm(Anthropic, &["a"]), local(), arm(Gemini, &["g"])];
    let (out, asked) = run(
        &w,
        &rungs,
        &[Say::Answers, Say::LocalAnswers, Say::Answers],
        Start::default(),
    )
    .await;
    let out = out.unwrap();
    assert_eq!(asked, vec![1]);
    assert_eq!(out.meta.provider, Provider::Local);
    assert_eq!(
        out.meta.fallback,
        Some(FallbackReason::ListSkip(SkipReason::LimitReached))
    );

    let (out, asked) = run(
        &w,
        &rungs,
        &[Say::Answers, Say::LocalFails, Say::Answers],
        Start::default(),
    )
    .await;
    assert_eq!(
        asked,
        vec![1],
        "the paid row after the brake is never asked"
    );
    assert_eq!(
        out.err().unwrap().provider_failure().unwrap().class,
        FailureClass::LimitReached
    );
}

/// A limit met INSIDE a row's own attempt (the race with the pre-check) is the same brake.
#[tokio::test]
async fn a_limit_met_inside_an_attempt_is_the_same_brake() {
    let w = World::new();
    let rungs = [arm(Anthropic, &["a"]), arm(Gemini, &["g"])];
    let (out, asked) = run(
        &w,
        &rungs,
        &[
            Say::Fails(FailureClass::LimitReached, Billing::NeverLeft),
            Say::Answers,
        ],
        Start::default(),
    )
    .await;
    assert_eq!(asked, vec![0]);
    assert_eq!(
        out.err().unwrap().provider_failure().unwrap().class,
        FailureClass::LimitReached
    );
}

// ---- this computer, anywhere in the list --------------------------------------------------------

/// A Local row that is the LAST row answers when the cloud rows before it failed without billing: the
/// offline person's [Claude, this computer].
#[tokio::test]
async fn a_local_tail_answers_after_a_connect_failure() {
    let w = World::new();
    let rungs = [arm(Anthropic, &["a"]), local()];
    let (out, asked) = run(
        &w,
        &rungs,
        &[Say::ConnectFails, Say::LocalAnswers],
        Start::default(),
    )
    .await;
    let out = out.unwrap();
    assert_eq!(asked, vec![0, 1]);
    assert_eq!(out.meta.provider, Provider::Local);
    assert_eq!(out.meta.cloud_provider, None);
    assert_eq!(out.completion.model.as_deref(), Some("llama3"));
}

/// [Claude, this computer, OpenRouter]: free local is tried before the second paid company, and when
/// it answers OpenRouter is never asked; when it fails plainly the walk goes on to OpenRouter.
#[tokio::test]
async fn a_local_middle_row_is_tried_before_the_second_paid_company() {
    let w = World::new();
    let rungs = [arm(Anthropic, &["a"]), local(), arm(OpenRouter, &["o/m"])];
    let (out, asked) = run(
        &w,
        &rungs,
        &[Say::ConnectFails, Say::LocalAnswers, Say::Answers],
        Start::default(),
    )
    .await;
    assert_eq!(out.unwrap().meta.provider, Provider::Local);
    assert_eq!(asked, vec![0, 1]);

    let (out, asked) = run(
        &w,
        &rungs,
        &[Say::ConnectFails, Say::LocalFails, Say::Answers],
        Start::default(),
    )
    .await;
    assert_eq!(asked, vec![0, 1, 2]);
    assert_eq!(out.unwrap().meta.cloud_provider, Some(OpenRouter));
}

/// A local row that failed AFTER an answer token reached the UI stops: nothing after it is asked.
/// And a local row that failed because Local AI was switched off under it stops too, never a cloud
/// bill (the switch said no more local work, not "try the cloud").
#[tokio::test]
async fn a_local_row_that_showed_an_answer_or_lost_local_ai_stops_the_list() {
    for say in [Say::LocalFailsAfterAnAnswer, Say::LocalFailsAiOff] {
        let w = World::new();
        let rungs = [local(), arm(Anthropic, &["a"])];
        let (out, asked) = run(&w, &rungs, &[say, Say::Answers], Start::default()).await;
        assert_eq!(asked, vec![0], "{say:?}");
        assert!(out.is_err(), "{say:?}");
    }
}

/// A Local head that failed hands over to the rest of its list: the cloud rows are walked, the local
/// row heads the record, and the model it displaced is kept. The answer is tagged with the LOCAL
/// reason when the first cloud row answers (the plain local strip), and with the list's own note
/// when a later row also failed or was passed over (so the chat tells every row and the Usage line
/// counts the reply). R11: this test used to pin the local tag for the second case too.
#[tokio::test]
async fn a_local_heads_tail_is_walked_and_tagged_with_why_local_failed() {
    let w = World::new();
    let rungs = [arm(Anthropic, &["a"]), arm(Gemini, &["g"])];
    // The first cloud row answers: the local reason leads, nothing else to tell.
    let start = Start::after_local(
        FallbackReason::HardFailure(LocalFailKind::Timeout),
        "llama3".into(),
        false,
    );
    let (out, asked, notes) = run_noted(&w, &rungs, &[Say::Answers, Say::Answers], start).await;
    let out = out.unwrap();
    assert_eq!(asked, vec![0]);
    assert_eq!(
        out.meta.fallback,
        Some(FallbackReason::HardFailure(LocalFailKind::Timeout))
    );
    assert_eq!(out.meta.displaced_local_model.as_deref(), Some("llama3"));
    assert_eq!(notes.len(), 2);

    // A cloud row failed before the answer: the reply is tagged by THAT row's note, the displaced
    // model is still kept, and the record names the local row first.
    let start = Start::after_local(
        FallbackReason::HardFailure(LocalFailKind::Timeout),
        "llama3".into(),
        false,
    );
    let (out, asked, notes) = run_noted(
        &w,
        &rungs,
        &[walk_class(FailureClass::NoCredit), Say::Answers],
        start,
    )
    .await;
    let out = out.unwrap();
    assert_eq!(asked, vec![0, 1]);
    assert_eq!(
        out.meta.fallback,
        Some(FallbackReason::ListWalk(FailureClass::NoCredit))
    );
    assert_eq!(out.meta.displaced_local_model.as_deref(), Some("llama3"));
    assert_eq!(out.meta.cloud_provider, Some(Gemini));
    assert_eq!(notes.len(), 3);
    assert_eq!(notes[0].entry.provider, None);

    // And when the tail runs out, the record begins with the local row.
    let start = Start::after_local(FallbackReason::Cooldown, "llama3".into(), false);
    let (out, _) = run(
        &w,
        &rungs,
        &[
            walk_class(FailureClass::NoCredit),
            walk_class(FailureClass::Busy),
        ],
        start,
    )
    .await;
    let Error::Exhausted { attempts, .. } = out.err().unwrap() else {
        panic!("expected an exhausted list");
    };
    assert_eq!(attempts.len(), 3);
    assert_eq!(attempts[0].entry.provider, None);
    assert_eq!(attempts[0].entry.model.as_deref(), Some("llama3"));
    // The seeded local step says WHY in the local strip's own words, never a company's "it failed".
    assert_eq!(
        attempts[0].outcome,
        AttemptOutcome::LocalFailed("your local model was resting after repeated errors")
    );
    assert_eq!(
        attempts[0].sentence(),
        "This computer didn't answer (your local model was resting after repeated errors)."
    );
    for (reason, words) in [
        (
            FallbackReason::HardFailure(LocalFailKind::Timeout),
            "your local model timed out",
        ),
        (
            FallbackReason::HardFailure(LocalFailKind::Refused),
            "your local model wasn't reachable",
        ),
        (
            FallbackReason::HardFailure(LocalFailKind::ModelLoading),
            "your local model was still loading",
        ),
        (
            FallbackReason::HardFailure(LocalFailKind::ServerError(500)),
            "your local model couldn't answer",
        ),
    ] {
        let seed = Start::after_local(reason, "llama3".into(), false).seed;
        assert_eq!(seed[0].outcome, AttemptOutcome::LocalFailed(words));
        assert!(!seed[0].sentence().contains("it failed"));
    }
}

/// The three ways a Local head hands over a request are checked at their source: the up-front
/// refusal when Local AI is off happens BEFORE any rung of a list that has a local row, in both
/// verbs, on `needs_local_ai`, and a list's tail asks the switch again before it walks.
#[test]
fn a_list_with_a_local_row_asks_the_local_ai_switch_before_any_rung() {
    let live = include_str!("../llm_gateway.rs")
        .split("#[cfg(test)]\nmod tests {")
        .next()
        .unwrap();
    for sig in ["pub async fn complete(", "pub async fn stream_chat<F>("] {
        let body = live
            .split(sig)
            .nth(1)
            .unwrap()
            .split("\n}\n")
            .next()
            .unwrap();
        let ladder = body.split("Plan::Ladder(ladder)").nth(1).expect(sig);
        let gate = ladder
            .find("local_ai_gate(")
            .unwrap_or_else(|| panic!("{sig}: a ladder is never run without asking the switch"));
        let run = ladder.find("_ladder(").unwrap();
        assert!(ladder[..gate].contains("needs_local_ai()"), "{sig}");
        assert!(gate < run, "{sig}: the switch is asked before the walk");
    }
    for sig in ["async fn tail_complete(", "async fn tail_stream<F>("] {
        let body = live
            .split(sig)
            .nth(1)
            .unwrap()
            .split("\n}\n")
            .next()
            .unwrap();
        let ladder = body
            .split("CloudTail::Ladder(rungs, elsewhere) =>")
            .nth(1)
            .expect(sig);
        assert!(
            ladder.find("local_ai_gate(rt)?").unwrap() < ladder.find("walk_").unwrap(),
            "{sig}"
        );
    }
}

/// A power-routed request is one attempt that does not walk: its arms in both verbs reach the
/// provider once, through the single-arm dispatch, and never a ladder, a tail or a walk.
#[test]
fn a_power_routed_request_makes_one_attempt_and_never_walks() {
    let live = include_str!("../llm_gateway.rs")
        .split("#[cfg(test)]\nmod tests {")
        .next()
        .unwrap();
    for (sig, dispatch) in [
        ("pub async fn complete_capped(", "cloud_complete_via("),
        ("async fn stream_route<F>(", "cloud_stream_via("),
    ] {
        let body = live
            .split(sig)
            .nth(1)
            .unwrap()
            .split("\n}\n")
            .next()
            .unwrap();
        let power = body.split("RoutePlan::CloudForPower").nth(1).expect(sig);
        assert!(power.contains(dispatch), "{sig}");
        for walking in ["walk_", "tail_", "_ladder(", "CloudTail"] {
            assert!(
                !power.contains(walking),
                "{sig} walks on a power route: {walking}"
            );
        }
    }
    // And the planner never builds a ladder for a power-routed role (S3a pins the shape; this pins
    // the type): a `Plan::Ladder` is never power-routed.
    let ladder = Plan::Ladder(Ladder::new(vec![arm(Anthropic, &["a"])]));
    assert!(!ladder.is_power_routed());
}

// ---- the wire mark ------------------------------------------------------------------------------

/// Two rungs: the first fails at connect, the second sends. Each attempt is judged by its OWN mark
/// (the first proves it never left, so the walk goes on), and the OUTER mark, which is what a mod's
/// booking is judged by, says "sent".
#[tokio::test]
async fn the_outer_mark_says_sent_when_a_later_rung_sent() {
    let w = World::new();
    let rungs = [arm(Anthropic, &["a"]), arm(Gemini, &["g"])];
    let outer = crate::wire_mark::WireMark::new();
    let (out, asked) = outer
        .scope(run(
            &w,
            &rungs,
            &[Say::ConnectFails, Say::Answers],
            Start::default(),
        ))
        .await;
    assert!(out.is_ok());
    assert_eq!(asked, vec![0, 1]);
    assert!(outer.may_have_reached_the_wire());
}

/// ...and when EVERY rung failed at connect, the outer mark still says nothing left, so a mod's
/// booking is given back.
#[tokio::test]
async fn the_outer_mark_says_never_left_when_no_rung_sent() {
    let w = World::new();
    let rungs = [arm(Anthropic, &["a"]), arm(Gemini, &["g"])];
    let outer = crate::wire_mark::WireMark::new();
    let (out, _) = outer
        .scope(run(
            &w,
            &rungs,
            &[Say::ConnectFails, Say::ConnectFails],
            Start::default(),
        ))
        .await;
    assert!(matches!(out.err().unwrap(), Error::Exhausted { .. }));
    assert!(!outer.may_have_reached_the_wire());
}

// ---- entry health -------------------------------------------------------------------------------

/// A row that failed in a way the memory keeps is skipped by the NEXT request, said so, and asked
/// again once its time has passed (a fake clock): no credit is ten minutes, busy a minute.
#[tokio::test]
async fn a_remembered_failure_is_skipped_until_its_time_has_passed() {
    let w = World::new();
    let rungs = [arm(Anthropic, &["a"]), arm(Gemini, &["g"])];
    // The first request pays one failing round trip and remembers it.
    let (out, asked) = run(
        &w,
        &rungs,
        &[walk_class(FailureClass::NoCredit), Say::Answers],
        Start::default(),
    )
    .await;
    assert_eq!(asked, vec![0, 1]);
    assert!(out.is_ok());
    // The second does not ask it, and says why.
    w.clock.advance(Duration::from_secs(60));
    let (out, asked) = run(&w, &rungs, &[Say::Answers, Say::Answers], Start::default()).await;
    assert_eq!(asked, vec![1]);
    assert_eq!(
        out.unwrap().meta.fallback,
        Some(FallbackReason::ListSkip(SkipReason::Down(
            FailureClass::NoCredit
        )))
    );
    // Past ten minutes it is asked again, and answering clears it.
    w.clock.advance(Duration::from_secs(10 * 60));
    let (out, asked) = run(&w, &rungs, &[Say::Answers, Say::Answers], Start::default()).await;
    assert_eq!(asked, vec![0]);
    assert_eq!(out.unwrap().meta.fallback, None);
}

#[tokio::test]
async fn a_busy_row_is_remembered_for_a_minute_only() {
    let w = World::new();
    let rungs = [arm(Anthropic, &["a"]), arm(Gemini, &["g"])];
    let _ = run(
        &w,
        &rungs,
        &[walk_class(FailureClass::Busy), Say::Answers],
        Start::default(),
    )
    .await;
    w.clock.advance(Duration::from_secs(30));
    let (_, asked) = run(&w, &rungs, &[Say::Answers, Say::Answers], Start::default()).await;
    assert_eq!(asked, vec![1]);
    w.clock.advance(Duration::from_secs(31));
    let (_, asked) = run(&w, &rungs, &[Say::Answers, Say::Answers], Start::default()).await;
    assert_eq!(asked, vec![0]);
}

/// Busy is about one model: after the first Claude model is busy, the person's second Claude model is
/// still asked, and the answer stays with Anthropic, not with the next company in the list.
#[tokio::test]
async fn a_busy_model_does_not_take_the_providers_other_rows_out() {
    let w = World::new();
    let _ = run(
        &w,
        &[arm(Anthropic, &["fable"]), arm(Gemini, &["g"])],
        &[walk_class(FailureClass::Busy), Say::Answers],
        Start::default(),
    )
    .await;
    let rungs = [
        arm(Anthropic, &["fable"]),
        arm(Anthropic, &["haiku"]),
        arm(Gemini, &["g"]),
    ];
    let (out, asked) = run(
        &w,
        &rungs,
        &[Say::Answers, Say::Answers, Say::Answers],
        Start::default(),
    )
    .await;
    assert_eq!(
        asked,
        vec![1],
        "the busy model is skipped, its sibling is not"
    );
    assert_eq!(
        out.unwrap().meta.fallback,
        Some(FallbackReason::ListSkip(SkipReason::Down(
            FailureClass::Busy
        )))
    );
}

/// A failure that may have billed, and one that stops the walk, are never remembered as "down": the
/// memory is for failures that proved nothing was billed and are likely to repeat.
#[tokio::test]
async fn only_a_failure_that_walked_is_remembered() {
    let w = World::new();
    let rungs = [arm(Anthropic, &["a"]), arm(Gemini, &["g"])];
    let _ = run(
        &w,
        &rungs,
        &[
            Say::Fails(FailureClass::NoCredit, Billing::MaybeBilled),
            Say::Answers,
        ],
        Start::default(),
    )
    .await;
    assert!(w.health.is_down(Anthropic, "a").is_none());
}

/// If EVERY usable row is remembered as down, the memory is ignored for the request: a stale mark
/// can never leave a role with nothing to try.
#[tokio::test]
async fn when_every_usable_row_is_down_the_memory_is_ignored() {
    let w = World::new();
    w.health.mark(Anthropic, "a", FailureClass::NoCredit, None);
    w.health.mark(Gemini, "g", FailureClass::KeyBad, None);
    let rungs = [arm(Anthropic, &["a"]), arm(Gemini, &["g"])];
    let (out, asked) = run(&w, &rungs, &[Say::Answers, Say::Answers], Start::default()).await;
    assert_eq!(
        asked,
        vec![0],
        "tried in order, as if nothing were remembered"
    );
    assert_eq!(out.unwrap().meta.fallback, None);

    // One row that is usable and NOT down means the memory applies: the down row is skipped. (The
    // success above cleared Anthropic's mark, so it is marked again.)
    w.health.mark(Anthropic, "a", FailureClass::NoCredit, None);
    let rungs = [arm(Anthropic, &["a"]), arm(OpenAi, &["o"])];
    let (_, asked) = run(&w, &rungs, &[Say::Answers, Say::Answers], Start::default()).await;
    assert_eq!(asked, vec![1]);
}

/// A row the person's limit or a region pause already took out of the running is not "usable", so it
/// does not stop the memory being ignored for the rows that are.
#[tokio::test]
async fn rows_that_were_passed_over_for_another_reason_do_not_count_as_usable() {
    let mut w = World::new();
    w.paused = vec![OpenAi];
    w.health.mark(Anthropic, "a", FailureClass::NoCredit, None);
    let rungs = [arm(Anthropic, &["a"]), arm(OpenAi, &["o"])];
    // Anthropic is the only usable row, and it is down: the memory is ignored, so it is tried.
    let (_, asked) = run(&w, &rungs, &[Say::Answers, Say::Answers], Start::default()).await;
    assert_eq!(asked, vec![0]);
}

/// A usable Local row is a row that is never down, so remembered cloud rows are skipped even when
/// every one of them is down: the free row answers instead of a failing round trip.
#[tokio::test]
async fn a_usable_local_row_means_the_memory_still_applies_to_the_cloud() {
    let w = World::new();
    w.health.mark(Anthropic, "a", FailureClass::NoCredit, None);
    let rungs = [arm(Anthropic, &["a"]), local()];
    let (out, asked) = run(
        &w,
        &rungs,
        &[Say::Answers, Say::LocalAnswers],
        Start::default(),
    )
    .await;
    assert_eq!(asked, vec![1]);
    assert_eq!(out.unwrap().meta.provider, Provider::Local);
}

// ---- small pieces -------------------------------------------------------------------------------

/// An answer carries the model of the row that answered, never the plan's first.
#[test]
fn a_reply_without_a_model_is_booked_on_the_row_that_answered() {
    let mut c = completion(None);
    served_on(&mut c, Some(&"gemini-b".to_string()));
    assert_eq!(c.model.as_deref(), Some("gemini-b"));
    let mut c = completion(Some("what-the-server-said"));
    served_on(&mut c, Some(&"gemini-b".to_string()));
    assert_eq!(c.model.as_deref(), Some("what-the-server-said"));
    let live = include_str!("../llm_gateway.rs")
        .split("#[cfg(test)]\nmod tests {")
        .next()
        .unwrap();
    for sig in [
        "async fn attempt_complete_cloud(",
        "async fn attempt_stream_cloud<F>(",
    ] {
        let body = live
            .split(sig)
            .nth(1)
            .unwrap()
            .split("\n}\n")
            .next()
            .unwrap();
        assert!(body.contains("served_on("), "{sig}");
    }
}

/// A stream that has shown an answer never walks, whatever its adapter proved: the sink says so.
#[test]
fn a_stream_that_showed_an_answer_cannot_walk_on() {
    let seen = Rc::new(RefCell::new(Vec::<String>::new()));
    let tap = seen.clone();
    let sink = Sink::new(move |d: ChatDelta<'_>| {
        if let ChatDelta::Answer(t) = d {
            tap.borrow_mut().push(t.to_string());
        }
    });
    sink.answer("");
    assert!(!sink.answered(), "an empty delta is not a token");
    let failed = RungFailure::of(typed(FailureClass::Busy, Billing::Rejected, Anthropic));
    assert!(failed.walks());
    sink.answer("Hel");
    assert!(sink.answered());
    assert!(!failed.after_an_answer().walks());
    assert_eq!(*seen.borrow(), vec!["Hel".to_string()]);
}

/// The ONE copy of "a window is exceeded".
#[test]
fn the_window_margin_is_ten_percent() {
    assert!(!too_long(1_100, Some(1_000)));
    assert!(too_long(1_101, Some(1_000)));
    assert!(!too_long(1_000_000, None));
    assert!(!too_long(1_000_000, Some(0)));
}

/// The walk's notes reach the chat sink as a `Walked` delta, which is not an answer token (so it can
/// never rule a later row out), and only for an answer the list's own first note tagged: a Local head
/// that fell to the list is the local strip's to tell.
#[test]
fn the_walk_is_told_to_the_sink_only_for_a_list_answer_and_never_counts_as_a_token() {
    let sink = Sink::new(|_: ChatDelta<'_>| {});
    sink.walked(&[]);
    assert!(!sink.answered(), "a notice is not an answer token");
    let src = include_str!("../llm_gateway.rs");
    let body = src
        .split("async fn walk_stream<'r, A, Fut>(")
        .nth(1)
        .and_then(|rest| rest.split("\n}\n").next())
        .expect("walk_stream is there");
    let told = body
        .find("tell(&notes)")
        .expect("walk_stream tells the sink");
    let guard = body
        .find("FallbackReason::ListWalk(_) | FallbackReason::ListSkip(_)")
        .expect("and only for a list's own note");
    assert!(guard < told, "the guard comes first");
}

/// A rate limit that asked to be retried after a time is remembered for THAT time (up to the cap),
/// carried from the reply's own header through the real attempt wrapper to the entry-health memory,
/// not the flat default minute (review round 1: the wait never reached the memory).
#[tokio::test]
async fn a_rate_limit_is_remembered_for_the_wait_its_reply_asked() {
    let w = World::new();
    let rungs = [arm(Anthropic, &["a"]), arm(Gemini, &["g"])];
    let (out, asked) = run(
        &w,
        &rungs,
        &[Say::RateLimitedAfter(240), Say::Answers],
        Start::default(),
    )
    .await;
    assert!(out.is_ok());
    assert_eq!(asked, vec![0, 1]);
    // Three minutes on, the default minute is long over, but the reply asked for four: still skipped.
    w.clock.advance(Duration::from_secs(180));
    let (_, asked) = run(&w, &rungs, &[Say::Answers, Say::Answers], Start::default()).await;
    assert_eq!(asked, vec![1]);
    // Past the four minutes it is asked again.
    w.clock.advance(Duration::from_secs(70));
    let (_, asked) = run(&w, &rungs, &[Say::Answers, Say::Answers], Start::default()).await;
    assert_eq!(asked, vec![0]);

    // And a short wait does not keep a usable row out for a whole minute.
    let w = World::new();
    let _ = run(
        &w,
        &rungs,
        &[Say::RateLimitedAfter(5), Say::Answers],
        Start::default(),
    )
    .await;
    w.clock.advance(Duration::from_secs(6));
    let (_, asked) = run(&w, &rungs, &[Say::Answers, Say::Answers], Start::default()).await;
    assert_eq!(asked, vec![0]);
}

/// A plain error after a request left (a reset, a timeout PM cannot place) may have billed: the real
/// wrapper stamps it `MaybeBilled` from the wire mark, and the walk STOPS (R3). Before this the walk
/// tests scripted only typed errors for a request that went out.
#[tokio::test]
async fn a_plain_error_after_a_request_left_stops_the_list() {
    let w = World::new();
    let rungs = [arm(Anthropic, &["a"]), arm(Gemini, &["g"])];
    let (out, asked) = run(
        &w,
        &rungs,
        &[Say::PlainErrorAfterSend, Say::Answers],
        Start::default(),
    )
    .await;
    assert_eq!(
        asked,
        vec![0],
        "nothing after a request that may have billed"
    );
    assert!(out.is_err());
}

/// R3's streaming guard, in one place: once an answer token reached the UI the failure STOPS, whatever
/// the adapter proved. `rung_failure` is what both streaming attempts call.
#[test]
fn a_failure_after_an_answer_token_never_walks() {
    let unbilled = || typed(FailureClass::Busy, Billing::Rejected, Anthropic);
    assert!(rung_failure(unbilled(), false, None).walks());
    assert!(!rung_failure(unbilled(), true, None).walks());
    assert_eq!(
        rung_failure(unbilled(), false, Some(Duration::from_secs(9))).retry_after,
        Some(Duration::from_secs(9))
    );
}

/// The guard above only protects a reply if the streaming attempts really ask the sink. Deleting
/// the `sink.answered()` call from either one must fail this scan (the mutation proof of round 1).
#[test]
fn both_streaming_attempts_ask_the_sink_whether_an_answer_was_shown() {
    let live = include_str!("../llm_gateway.rs")
        .split("#[cfg(test)]\nmod tests {")
        .next()
        .unwrap();
    for sig in [
        "async fn attempt_stream_cloud<F>(",
        "async fn attempt_stream<F>(",
    ] {
        let body = live
            .split(sig)
            .nth(1)
            .and_then(|rest| rest.split("\n}\n").next())
            .unwrap_or_else(|| panic!("{sig} is there"));
        assert!(body.contains("sink.answered()"), "{sig} must ask the sink");
    }
}

/// What the notes call a Local row follows where its server is, by the loopback test the 4c card
/// makes: the strip and the log must not say "this computer" for a LAN or public server.
#[test]
fn a_local_rungs_label_knows_whether_its_server_is_this_computer() {
    assert!(!local().label().local_elsewhere);
    assert_eq!(local().label().company(), "this computer");
    let away = Rung::Local(LocalArm {
        base_url: "http://192.168.1.20:11434".into(),
        model: "llama3".into(),
        token: None,
    });
    assert!(away.label().local_elsewhere);
    assert_eq!(away.label().company(), "your Local AI server");
    assert!(local_is_elsewhere(match &away {
        Rung::Local(l) => l,
        _ => unreachable!(),
    }));
}

// ---- Local AI off, a this-computer row below cloud rows -----------------------------------------

/// The rungs the REAL planner builds for a stored list while Local AI is off (or the role waits for
/// a model), keys present for every provider, so the tests below walk what a person's list really
/// plans, not a hand-built ladder. `None` when the planner says nothing can answer.
fn planned_while_local_is_off(
    entries: Vec<crate::route::Entry>,
    emptied: bool,
) -> Option<Vec<Rung>> {
    planned_while_local_is_off_at(entries, emptied, None)
}

/// [`planned_while_local_is_off`] with the stored Local AI server's arm (the base URL it points at).
fn planned_while_local_is_off_at(
    entries: Vec<crate::route::Entry>,
    emptied: bool,
    local: Option<LocalArm>,
) -> Option<Vec<Rung>> {
    use crate::list_plan::{plan, Inputs, KeyRead, Mode, Seat};
    let mut prefs = RoutingPrefs::uniform(ProviderPref::Cloud);
    prefs.local_ai_on = emptied;
    if emptied {
        prefs.emptied.chat = true;
    }
    let route = crate::route::RoleRoute {
        entries,
        derived: false,
    };
    let ctx = RuntimeContext::default();
    let mut seat = |_p: CloudProvider| Seat {
        opted_in: true,
        key: KeyRead::Key(Secret::from(FAKE.to_string())),
        tier: None,
    };
    let planned = plan(
        Inputs {
            role: Role::Chat,
            route: &route,
            prefs: &prefs,
            ctx: &ctx,
            local,
        },
        Mode::Person,
        &mut seat,
        &|_| true,
    )
    .unwrap()?;
    match planned {
        Plan::Ladder(l) => Some(l.rungs),
        Plan::Route(_) => panic!("a list that stops at a Local row is always a ladder"),
    }
}

fn or_row(model: &str) -> crate::route::Entry {
    crate::route::Entry::Cloud {
        provider: OpenRouter,
        model: model.into(),
        effort: None,
    }
}

fn claude_row() -> crate::route::Entry {
    crate::route::Entry::Cloud {
        provider: Anthropic,
        model: "claude-a".into(),
        effort: None,
    }
}

/// [Claude, This computer] with Local AI off answers from Claude: the rows ABOVE the Local row run
/// exactly as normal, and nothing says anything about the Local row to a reply that was answered.
#[tokio::test]
async fn local_ai_off_a_row_above_the_local_row_still_answers() {
    let rungs = planned_while_local_is_off(vec![claude_row(), crate::route::Entry::Local], false)
        .expect("Claude is usable");
    let w = World::new();
    let (out, asked) = run(
        &w,
        &rungs,
        &[Say::Answers, Say::LocalAnswers],
        Start::default(),
    )
    .await;
    let out = out.expect("Claude answers");
    assert_eq!(asked, vec![0]);
    assert_eq!(out.meta.provider, Provider::Cloud);
    assert_eq!(
        out.meta.fallback, None,
        "a first-row answer carries no note"
    );
}

/// [Claude (walk-class failure), This computer, OpenRouter] with Local AI off: Claude is asked, the
/// walk reaches the Local row and STOPS. The OpenRouter row after it is never even built, so the
/// fake adapter is asked exactly once, and the list says why it stopped.
#[tokio::test]
async fn local_ai_off_a_failed_row_walks_to_the_local_row_and_stops_there() {
    let rungs = planned_while_local_is_off(
        vec![claude_row(), crate::route::Entry::Local, or_row("v/a")],
        false,
    )
    .expect("Claude is usable");
    assert_eq!(rungs.len(), 2, "the row after the stop is never planned");
    assert!(rungs
        .iter()
        .all(|r| !matches!(r, Rung::Cloud(a) if a.provider == OpenRouter)));
    let w = World::new();
    // A script with a THIRD entry that would answer: it must never be asked.
    let (out, asked, notes) = run_noted(
        &w,
        &rungs,
        &[
            walk_class(FailureClass::Busy),
            Say::LocalAnswers,
            Say::Answers,
        ],
        Start::default(),
    )
    .await;
    assert_eq!(asked, vec![0], "only Claude is ever asked");
    let e = out.err().expect("the list ran out at the Local row");
    let sentence = e.to_string();
    assert!(
        sentence.ends_with(SkipReason::LocalAiOff.stop_sentence().unwrap()),
        "{sentence}"
    );
    assert!(sentence.contains("This computer was skipped (Local AI is off)"));
    assert!(matches!(
        notes.last().map(|a| a.outcome),
        Some(AttemptOutcome::Skipped(SkipReason::LocalAiOff))
    ));
    // The card speaks to Claude's failure (the last row that failed), and says why it stopped.
    match &e {
        Error::Exhausted { last, attempts } => {
            assert_eq!(
                last.provider_failure().map(|f| f.class),
                Some(FailureClass::Busy)
            );
            assert_eq!(
                crate::error::stop_note(attempts),
                SkipReason::LocalAiOff.stop_sentence()
            );
        }
        other => panic!("{other:?}"),
    }
}

/// With Local AI off and pointing at another machine, the stop note names the row as the rest of the
/// PR does ("Your Local AI server"), never "This computer" (lead item n3); on this computer it still
/// says "This computer".
#[test]
fn the_stop_note_names_a_local_ai_server_on_another_machine_as_one() {
    let arm = |base: &str| LocalArm {
        base_url: base.into(),
        model: "llama3".into(),
        token: None,
    };
    let stop_sentence = |base: &str| {
        let rungs = planned_while_local_is_off_at(
            vec![claude_row(), crate::route::Entry::Local],
            false,
            Some(arm(base)),
        )
        .expect("Claude is usable");
        let note = rungs.last().expect("the stop note").label();
        crate::error::Attempt {
            entry: note,
            outcome: AttemptOutcome::Skipped(SkipReason::LocalAiOff),
        }
        .sentence()
    };
    assert_eq!(
        stop_sentence("http://192.168.1.20:11434"),
        "Your Local AI server was skipped (Local AI is off)."
    );
    assert_eq!(
        stop_sentence("http://127.0.0.1:11434"),
        "This computer was skipped (Local AI is off)."
    );
}

/// A Local HEAD stops the role at once (the planner says nothing can answer), so no cloud row
/// behind it is built, whatever it holds.
#[test]
fn local_ai_off_a_local_head_stops_before_any_cloud_row() {
    assert!(
        planned_while_local_is_off(vec![crate::route::Entry::Local, claude_row()], false).is_none()
    );
    assert!(
        planned_while_local_is_off(vec![crate::route::Entry::Local, claude_row()], true).is_none()
    );
}

/// The role waiting for its model (emptied, Local AI on) stops at the Local row the same way, and says
/// the model is why.
#[tokio::test]
async fn a_role_waiting_for_its_model_stops_at_the_local_row_and_says_so() {
    let rungs = planned_while_local_is_off(
        vec![claude_row(), crate::route::Entry::Local, or_row("v/a")],
        true,
    )
    .expect("Claude is usable");
    let w = World::new();
    let (out, asked) = run(
        &w,
        &rungs,
        &[
            walk_class(FailureClass::Busy),
            Say::LocalAnswers,
            Say::Answers,
        ],
        Start::default(),
    )
    .await;
    assert_eq!(asked, vec![0]);
    assert!(out
        .err()
        .unwrap()
        .to_string()
        .ends_with(SkipReason::PickAModel.stop_sentence().unwrap()));
}

/// A request that may have billed stops the walk with its own error before it ever reaches the
/// Local row (R3 holds above the stop too), and a tripped daily limit walks only to the Local row,
/// which is the stop, so the limit's own refusal is the answer (R3b).
#[tokio::test]
async fn the_stop_at_the_local_row_leaves_the_billing_and_limit_rules_alone() {
    let rungs = planned_while_local_is_off(
        vec![claude_row(), crate::route::Entry::Local, or_row("v/a")],
        false,
    )
    .unwrap();
    let w = World::new();
    let (out, asked) = run(
        &w,
        &rungs,
        &[
            Say::Fails(FailureClass::Busy, Billing::MaybeBilled),
            Say::LocalAnswers,
        ],
        Start::default(),
    )
    .await;
    assert_eq!(asked, vec![0]);
    let e = out.err().unwrap();
    assert!(
        e.provider_failure().is_some(),
        "the billed failure is the error, not the list's: {e}"
    );
    let mut w = World::new();
    w.limited = vec![Anthropic];
    let (out, asked) = run(
        &w,
        &rungs,
        &[Say::Answers, Say::LocalAnswers],
        Start::default(),
    )
    .await;
    assert!(asked.is_empty(), "the paused row is not asked: {asked:?}");
    assert_eq!(
        out.err().unwrap().provider_failure().unwrap().class,
        FailureClass::LimitReached
    );
}
