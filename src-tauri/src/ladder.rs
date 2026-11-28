// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The walk down a person's list, as pure policy.
//!
//! A reply is tried on the rows of the list top to bottom. This module decides, with no I/O, WHEN
//! another row may be tried after one that did not answer ([`next`]) and runs the loop itself over
//! closures ([`walk`]), so the one rule that matters (a request bills at most one attempt, I-41) is
//! a plain test over every sequence of outcomes rather than an argument about the executors.
//!
//! The threat model: these guards catch MISTAKES by PM and real provider replies. They are not a lock
//! against a hostile local user and do not defend against a vendor lying about its own usage.
//!
//! THE RULE (R3). A row is tried after another only when the earlier one is PROVEN to have billed
//! nothing: PM refused it itself before sending ([`Billing::NeverLeft`]), a connect failure proved
//! nothing left, or the provider answered with an error status before any body streamed
//! ([`Billing::Rejected`]). Anything that may have billed ([`Billing::MaybeBilled`]) and any refusal
//! by the model ends the walk. The class is wording; billing is the gate.
//!
//! R3b. A daily limit is a brake, not a switch of meter: limits are per provider with no total, so
//! a provider the limit paused never moves a request to another company's paid row. Only a Local row
//! (free) may answer after it, and once the brake is on no cloud row is tried at all.

use std::future::Future;

use crate::error::{
    Attempt, AttemptEntry, AttemptOutcome, Billing, Error, FailureClass, SkipReason,
};

/// What a rung is, as far as the walk cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// This computer: free, and the only rung a limit's brake lets through.
    Local,
    /// A company, billed.
    Cloud,
}

/// Whether to go on to the next rung after a failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Next {
    Walk,
    Stop,
}

/// Whether a failure that ended an attempt lets the walk go on. PURE, and total over its inputs.
///
/// - `MaybeBilled` always stops: it may have cost money.
/// - A refusal by the model (`Refused`) or a failure after the reply began (`StoppedPartway`) stops
///   whatever the billing says: the same prompt would be refused again, and a reply that began is not
///   to be re-asked of another company.
/// - `LimitReached` walks only to a Local rung (R3b).
/// - `Other` walks only when nothing left this computer (a connect failure, where the offline person's
///   second row is the whole point); an unclassified error RESPONSE (`Rejected`) stops, because PM
///   does not know what it meant.
/// - Every other class walks.
/// - With the brake on (an earlier rung was paused by the limit), only a Local rung may follow.
pub(crate) fn next(
    billing: Billing,
    class: FailureClass,
    brake: bool,
    next_rung_is_local: bool,
) -> Next {
    if billing == Billing::MaybeBilled {
        return Next::Stop;
    }
    let by_class = match class {
        FailureClass::Refused | FailureClass::StoppedPartway => Next::Stop,
        FailureClass::LimitReached if !next_rung_is_local => Next::Stop,
        FailureClass::Other if billing == Billing::Rejected => Next::Stop,
        _ => Next::Walk,
    };
    if by_class == Next::Walk && brake && !next_rung_is_local {
        return Next::Stop;
    }
    by_class
}

/// What the loop needs to know about an attempt's failure.
pub(crate) trait Failed {
    fn billing(&self) -> Billing;
    fn class(&self) -> FailureClass;
}

impl Failed for Error {
    /// A typed provider failure says its own; anything else (a locked vault, a database error, a
    /// plain string) is a failure PM cannot place, so it may have billed and nothing follows it.
    fn billing(&self) -> Billing {
        self.provider_failure()
            .map_or(Billing::MaybeBilled, |f| f.billing)
    }

    fn class(&self) -> FailureClass {
        self.provider_failure()
            .map_or(FailureClass::Other, |f| f.class)
    }
}

/// How a walk ended.
#[derive(Debug)]
pub(crate) enum End<T, E> {
    /// The rung at this index answered.
    Answered {
        // Which row answered: read by the tests; the executors tag the answer from the attempts.
        #[allow(dead_code)]
        rung: usize,
        value: T,
    },
    /// A failure, or the limit's brake, ended it with rungs possibly left untried.
    Stopped(E),
    /// Every rung was skipped or walked past. `None` when nothing was even tried.
    Exhausted(Option<E>),
}

/// What the walk did, row by row, and how it ended.
#[derive(Debug)]
pub(crate) struct Walked<T, E> {
    pub attempts: Vec<Attempt>,
    pub end: End<T, E>,
}

/// Run the walk. `kind` and `label` describe a rung; `pre_check` says why a rung is passed over
/// without a request (not opted in, no key, paused, remembered down, too long); `limit` answers
/// with the typed refusal when the person's daily limit has paused a CLOUD rung; `attempt` makes the
/// one request. Closures, so the executors (streaming, complete, power) and the property test share
/// this one loop.
///
/// The loop, in the brief's words:
/// - a rung that is Cloud while the brake is on stops the walk with the limit's refusal;
/// - a `pre_check` skip is noted and the walk goes on;
/// - a Cloud rung the limit has paused sets the brake, is noted, and the walk goes on ONLY so a later
///   Local rung can answer;
/// - an answer ends it; a failure goes through [`next`].
pub(crate) async fn walk<R, T, E, Fut>(
    rungs: &[R],
    kind: impl Fn(&R) -> Kind,
    label: impl Fn(&R) -> AttemptEntry,
    mut pre_check: impl FnMut(&R) -> Option<SkipReason>,
    mut limit: impl FnMut(&R) -> Option<E>,
    mut attempt: impl FnMut(&R) -> Fut,
) -> Walked<T, E>
where
    E: Failed,
    Fut: Future<Output = Result<T, E>>,
{
    let mut attempts: Vec<Attempt> = Vec::new();
    let mut brake: Option<E> = None;
    let mut last: Option<E> = None;
    for (i, rung) in rungs.iter().enumerate() {
        let k = kind(rung);
        if k == Kind::Cloud {
            if let Some(refusal) = brake.take() {
                return Walked {
                    attempts,
                    end: End::Stopped(refusal),
                };
            }
        }
        if let Some(skip) = pre_check(rung) {
            attempts.push(Attempt {
                entry: label(rung),
                outcome: AttemptOutcome::Skipped(skip),
            });
            continue;
        }
        if k == Kind::Cloud {
            if let Some(refusal) = limit(rung) {
                attempts.push(Attempt {
                    entry: label(rung),
                    outcome: AttemptOutcome::Skipped(SkipReason::LimitReached),
                });
                brake.get_or_insert(refusal);
                continue;
            }
        }
        match attempt(rung).await {
            Ok(value) => {
                attempts.push(Attempt {
                    entry: label(rung),
                    outcome: AttemptOutcome::Answered,
                });
                return Walked {
                    attempts,
                    end: End::Answered { rung: i, value },
                };
            }
            Err(e) => {
                let (billing, class) = (e.billing(), e.class());
                attempts.push(Attempt {
                    entry: label(rung),
                    outcome: AttemptOutcome::Failed(class),
                });
                let next_is_local = rungs.get(i + 1).is_some_and(|r| kind(r) == Kind::Local);
                let braked = brake.is_some() || class == FailureClass::LimitReached;
                if next(billing, class, braked, next_is_local) == Next::Stop {
                    // A free row that failed while the limit's brake was on stops the walk only
                    // because of the brake (no paid row may follow it), so the person is told about
                    // the limit, the wall they are actually at, not about the free row's own trouble.
                    // A failure that may have billed is still its own.
                    let end = match (billing != Billing::MaybeBilled, brake.take()) {
                        (true, Some(refusal)) => End::Stopped(refusal),
                        _ => End::Stopped(e),
                    };
                    return Walked { attempts, end };
                }
                if class == FailureClass::LimitReached {
                    brake.get_or_insert(e);
                } else {
                    last = Some(e);
                }
            }
        }
    }
    let end = match brake {
        Some(refusal) => End::Stopped(refusal),
        None => End::Exhausted(last),
    };
    Walked { attempts, end }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLASSES: [FailureClass; 13] = [
        FailureClass::Busy,
        FailureClass::RateLimited,
        FailureClass::ModelGone,
        FailureClass::ContextTooLong,
        FailureClass::KeyBad,
        FailureClass::NoCredit,
        FailureClass::NeedsRetention,
        FailureClass::SetupStale,
        FailureClass::LimitReached,
        FailureClass::Paused,
        FailureClass::Refused,
        FailureClass::StoppedPartway,
        FailureClass::Other,
    ];
    const BILLINGS: [Billing; 3] = [Billing::NeverLeft, Billing::Rejected, Billing::MaybeBilled];

    /// Every `(billing, class, brake, next_is_local)` pair, against the table written out by hand.
    #[test]
    fn next_is_pinned_for_every_pair() {
        use FailureClass::*;
        for billing in BILLINGS {
            for class in CLASSES {
                for brake in [false, true] {
                    for local in [false, true] {
                        let expect_walk = match (billing, class) {
                            (Billing::MaybeBilled, _) => false,
                            (_, Refused | StoppedPartway) => false,
                            (_, LimitReached) => local,
                            (Billing::Rejected, Other) => false,
                            _ => !brake || local,
                        };
                        let got = next(billing, class, brake, local);
                        assert_eq!(
                            got == Next::Walk,
                            expect_walk,
                            "{billing:?} {class:?} brake={brake} next_local={local}"
                        );
                    }
                }
            }
        }
        // Spot checks that read as the table in the brief.
        assert_eq!(next(Billing::Rejected, KeyBad, false, false), Next::Walk);
        assert_eq!(next(Billing::NeverLeft, Other, false, false), Next::Walk);
        assert_eq!(next(Billing::MaybeBilled, Busy, false, true), Next::Stop);
        assert_eq!(next(Billing::NeverLeft, Refused, false, true), Next::Stop);
        assert_eq!(
            next(Billing::NeverLeft, LimitReached, true, false),
            Next::Stop
        );
        assert_eq!(
            next(Billing::NeverLeft, LimitReached, true, true),
            Next::Walk
        );
    }

    // ---- THE BILLING PROPERTY ---------------------------------------------------------------

    /// What a fake rung does when it is asked.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Out {
        Answered,
        WalkNeverLeft,
        WalkRejected,
        MaybeBilled,
        Refused,
        /// A cloud rung the limit has paused, found BEFORE the request.
        LimitBefore,
        /// A cloud rung that meets the limit inside its own attempt.
        LimitInAttempt,
    }
    const OUTS: [Out; 7] = [
        Out::Answered,
        Out::WalkNeverLeft,
        Out::WalkRejected,
        Out::MaybeBilled,
        Out::Refused,
        Out::LimitBefore,
        Out::LimitInAttempt,
    ];

    #[derive(Debug, Clone, Copy)]
    struct Rung {
        kind: Kind,
        out: Out,
    }

    #[derive(Debug)]
    struct Fail {
        billing: Billing,
        class: FailureClass,
    }
    impl Failed for Fail {
        fn billing(&self) -> Billing {
            self.billing
        }
        fn class(&self) -> FailureClass {
            self.class
        }
    }

    fn entry(r: &Rung) -> AttemptEntry {
        AttemptEntry {
            provider: (r.kind == Kind::Cloud).then_some(crate::llm_gateway::CloudProvider::Gemini),
            model: None,
            local_elsewhere: false,
        }
    }

    /// Run the real loop over a scripted list. Returns (calls made, billed calls, the walk).
    fn run(rungs: &[Rung]) -> (usize, usize, Walked<usize, Fail>) {
        let calls = std::cell::Cell::new(0usize);
        let billed = std::cell::Cell::new(0usize);
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let walked = rt.block_on(walk(
            rungs,
            |r| r.kind,
            entry,
            |_| None,
            |r| {
                (r.out == Out::LimitBefore && r.kind == Kind::Cloud).then_some(Fail {
                    billing: Billing::NeverLeft,
                    class: FailureClass::LimitReached,
                })
            },
            |r| {
                calls.set(calls.get() + 1);
                let out = r.out;
                let billed = &billed;
                async move {
                    match out {
                        Out::Answered => {
                            billed.set(billed.get() + 1);
                            Ok(1usize)
                        }
                        Out::WalkNeverLeft => Err(Fail {
                            billing: Billing::NeverLeft,
                            class: FailureClass::Other,
                        }),
                        Out::WalkRejected => Err(Fail {
                            billing: Billing::Rejected,
                            class: FailureClass::KeyBad,
                        }),
                        Out::MaybeBilled => {
                            billed.set(billed.get() + 1);
                            // A 500 is `Busy` in words and `MaybeBilled` in fact: the class would
                            // walk, only the billing stops it.
                            Err(Fail {
                                billing: Billing::MaybeBilled,
                                class: FailureClass::Busy,
                            })
                        }
                        Out::Refused => {
                            billed.set(billed.get() + 1);
                            Err(Fail {
                                billing: Billing::MaybeBilled,
                                class: FailureClass::Refused,
                            })
                        }
                        // Only reached for a rung that was not found paused before the request.
                        Out::LimitBefore | Out::LimitInAttempt => Err(Fail {
                            billing: Billing::NeverLeft,
                            class: FailureClass::LimitReached,
                        }),
                    }
                }
            },
        ));
        (calls.get(), billed.get(), walked)
    }

    /// Every list of up to four rungs, each Local or Cloud, each with every outcome. The cases that
    /// make no sense (a limit on a Local rung) are not generated.
    fn all_lists() -> Vec<Vec<Rung>> {
        let mut singles = Vec::new();
        for kind in [Kind::Local, Kind::Cloud] {
            for out in OUTS {
                let limit = matches!(out, Out::LimitBefore | Out::LimitInAttempt);
                if kind == Kind::Local && limit {
                    continue;
                }
                singles.push(Rung { kind, out });
            }
        }
        let mut lists: Vec<Vec<Rung>> = vec![vec![]];
        let mut frontier: Vec<Vec<Rung>> = vec![vec![]];
        for _ in 0..4 {
            let mut grown = Vec::new();
            for l in &frontier {
                for s in &singles {
                    let mut n = l.clone();
                    n.push(*s);
                    grown.push(n);
                }
            }
            lists.extend(grown.iter().cloned());
            frontier = grown;
        }
        lists
    }

    /// The test that carries R3: over every list of up to four rungs and every sequence of outcomes,
    /// the loop never lets a second billed attempt follow a first, and it obeys the limit's brake.
    #[test]
    fn a_request_bills_at_most_one_attempt_over_every_sequence() {
        let lists = all_lists();
        assert!(lists.len() > 20_000, "the enumeration ran {}", lists.len());
        for rungs in &lists {
            let (calls, billed, walked) = run(rungs);
            let ctx = format!("{rungs:?}");
            assert!(billed <= 1, "more than one billed attempt: {ctx}");

            // Where the walk should stop by the rules, worked out independently of the loop.
            let mut brake = false;
            let mut expected_calls = 0usize;
            let mut stop_at: Option<usize> = None;
            let mut stopped_by_brake = false;
            for (i, r) in rungs.iter().enumerate() {
                if r.kind == Kind::Cloud && brake {
                    stop_at = Some(i);
                    stopped_by_brake = true;
                    break;
                }
                if r.kind == Kind::Cloud && r.out == Out::LimitBefore {
                    brake = true;
                    continue;
                }
                expected_calls += 1;
                match r.out {
                    Out::Answered | Out::MaybeBilled | Out::Refused => {
                        stop_at = Some(i);
                        break;
                    }
                    Out::WalkNeverLeft | Out::WalkRejected => {}
                    Out::LimitInAttempt | Out::LimitBefore => {
                        brake = true;
                        let next_local = rungs.get(i + 1).is_some_and(|n| n.kind == Kind::Local);
                        if !next_local {
                            stop_at = Some(i);
                            break;
                        }
                    }
                }
            }
            assert_eq!(calls, expected_calls, "call count: {ctx}");

            match (&walked.end, stop_at) {
                (End::Answered { rung, .. }, Some(i)) => {
                    assert_eq!(*rung, i, "{ctx}");
                    assert_eq!(rungs[i].out, Out::Answered, "{ctx}");
                }
                (End::Stopped(_), Some(i)) => {
                    // The limit's brake stops BEFORE the paid rung is asked, so that rung never
                    // answers whatever it was scripted to do. Any other stop is a billed failure, a
                    // refusal or a limit met inside the attempt: never an answer either.
                    if stopped_by_brake {
                        assert!(brake, "{ctx}");
                    } else {
                        assert_ne!(rungs[i].out, Out::Answered, "{ctx}");
                    }
                }
                (End::Stopped(_), None) => {
                    // The list ran out with the brake on: the person's limit is the headline.
                    assert!(brake, "{ctx}");
                }
                (End::Exhausted(_), None) => assert!(!brake, "{ctx}"),
                (end, stop) => panic!("{end:?} but stop at {stop:?}: {ctx}"),
            }
            // A billed outcome always ends the walk: nothing is noted after it.
            if let Some(i) = walked
                .attempts
                .iter()
                .position(|a| a.outcome == AttemptOutcome::Answered)
            {
                assert_eq!(i + 1, walked.attempts.len(), "notes after an answer: {ctx}");
            }
        }
    }

    /// A rung the limit paused followed by a paid rung always stops; followed by Local it goes on and
    /// the Local row answers.
    #[test]
    fn a_limit_stops_before_a_paid_rung_and_lets_a_local_one_answer() {
        let paid = |out| Rung {
            kind: Kind::Cloud,
            out,
        };
        let local = |out| Rung {
            kind: Kind::Local,
            out,
        };
        for limited in [Out::LimitBefore, Out::LimitInAttempt] {
            let (calls, _, walked) = run(&[paid(limited), paid(Out::Answered)]);
            assert!(matches!(walked.end, End::Stopped(_)), "{limited:?}");
            assert_eq!(calls, usize::from(limited == Out::LimitInAttempt));

            let (_, billed, walked) = run(&[paid(limited), local(Out::Answered)]);
            assert!(
                matches!(walked.end, End::Answered { rung: 1, .. }),
                "{limited:?}"
            );
            assert_eq!(billed, 1);

            // Once the brake is on, a Local rung that fails still never lets a paid one follow.
            let (_, _, walked) = run(&[
                paid(limited),
                local(Out::WalkNeverLeft),
                paid(Out::Answered),
            ]);
            assert!(matches!(walked.end, End::Stopped(_)), "{limited:?}");
        }
    }

    /// The notes say what happened to each row, in order, for the strip and the log.
    #[test]
    fn the_attempts_say_what_happened_to_each_row_in_order() {
        let rungs = [
            Rung {
                kind: Kind::Cloud,
                out: Out::WalkRejected,
            },
            Rung {
                kind: Kind::Cloud,
                out: Out::WalkNeverLeft,
            },
            Rung {
                kind: Kind::Local,
                out: Out::Answered,
            },
        ];
        let (_, _, walked) = run(&rungs);
        let outcomes: Vec<_> = walked.attempts.iter().map(|a| a.outcome).collect();
        assert_eq!(
            outcomes,
            vec![
                AttemptOutcome::Failed(FailureClass::KeyBad),
                AttemptOutcome::Failed(FailureClass::Other),
                AttemptOutcome::Answered,
            ]
        );
    }

    /// A skipped row is noted and never attempted, and a list of only skips ends as `Exhausted` with
    /// nothing tried.
    #[test]
    fn a_skipped_row_is_noted_and_never_asked() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let rungs = [1u8, 2, 3];
        let asked = std::cell::Cell::new(0);
        let walked: Walked<(), Fail> = rt.block_on(walk(
            &rungs,
            |_| Kind::Cloud,
            |_| AttemptEntry {
                provider: Some(crate::llm_gateway::CloudProvider::Anthropic),
                model: None,
                local_elsewhere: false,
            },
            |r| {
                Some(match r {
                    1 => SkipReason::NoKey,
                    2 => SkipReason::Paused,
                    _ => SkipReason::Down(FailureClass::NoCredit),
                })
            },
            |_| None,
            |_| {
                asked.set(asked.get() + 1);
                async { Ok(()) }
            },
        ));
        assert_eq!(asked.get(), 0);
        assert!(matches!(walked.end, End::Exhausted(None)));
        assert_eq!(walked.attempts.len(), 3);
    }

    #[test]
    fn a_plain_error_is_a_failure_pm_cannot_place_so_nothing_follows_it() {
        let e = Error::Other("boom".into());
        assert_eq!(Failed::billing(&e), Billing::MaybeBilled);
        assert_eq!(Failed::class(&e), FailureClass::Other);
        assert_eq!(
            next(Failed::billing(&e), Failed::class(&e), false, true),
            Next::Stop
        );
    }
}
