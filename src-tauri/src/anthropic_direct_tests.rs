// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Tests of Anthropic's wire. No test reads a real key, the real network or the host time zone: the
//! pure parts are tested as functions, the stream and the key check run against a scripted server on
//! loopback (threads and sockets, never a fork), and the fixtures are Anthropic's real replies.

use super::fixtures::*;
use super::*;
use crate::direct_test_support::{serve, Reply};
use crate::secret::Secret;
use std::sync::{Arc, Mutex};

/// Not a real key, and not shaped like one of anyone's.
const KEY: &str = "test-key-not-real-0123456789";

fn msg(role: &str, text: &str) -> ChatMessage {
    ChatMessage {
        role: role.into(),
        content: text.into(),
    }
}

fn row(id: &str) -> &'static DirectModel {
    direct_models::find(id).unwrap_or_else(|| panic!("{id} is not in the catalogue"))
}

const SONNET: &str = "anthropic/claude-sonnet-5-5";
const HAIKU: &str = "anthropic/claude-haiku-5-5";
const OPUS: &str = "anthropic/claude-opus-5-5";
const FABLE: &str = "anthropic/claude-fable-5-2";
const ALL_FOUR: [&str; 4] = [FABLE, OPUS, SONNET, HAIKU];

fn arm(model: &str, role: Option<Role>, effort: Option<&str>) -> CloudArm {
    CloudArm {
        effort: effort.map(str::to_string),
        role,
        ..CloudArm::new(
            CloudProvider::Anthropic,
            Secret::from(KEY.to_string()),
            vec![model.to_string()],
        )
    }
}

fn near(a: Option<f64>, b: f64) -> bool {
    a.is_some_and(|a| (a - b).abs() < 1e-9)
}

/// An event in the stream's own shape: `event:` line, `data:` line, blank line.
fn ev(v: Value) -> String {
    format!("event: {}\ndata: {v}\n\n", v["type"].as_str().unwrap())
}

fn start(model: &str, usage: Value) -> String {
    ev(json!({"type":"message_start","message":{"model":model,"usage":usage}}))
}

fn text_block(index: u32, deltas: &[&str]) -> String {
    let mut out = ev(
        json!({"type":"content_block_start","index":index,"content_block":{"type":"text","text":""}}),
    );
    for d in deltas {
        out += &ev(
            json!({"type":"content_block_delta","index":index,"delta":{"type":"text_delta","text":d}}),
        );
    }
    out + &ev(json!({"type":"content_block_stop","index":index}))
}

fn thinking_block(index: u32) -> String {
    ev(
        json!({"type":"content_block_start","index":index,"content_block":{"type":"thinking","thinking":"","signature":""}}),
    ) + &ev(
        json!({"type":"content_block_delta","index":index,"delta":{"type":"thinking_delta","thinking":""}}),
    ) + &ev(
        json!({"type":"content_block_delta","index":index,"delta":{"type":"signature_delta","signature":"sig"}}),
    ) + &ev(json!({"type":"content_block_stop","index":index}))
}

fn finish_with(stop: &str, usage: Value) -> String {
    ev(json!({"type":"message_delta","delta":{"stop_reason":stop},"usage":usage}))
        + &ev(json!({"type":"message_stop"}))
}

/// Feed a whole canned stream through a `Turn`, line by line, and finish it.
fn read(model: &str, stream: &str) -> (Result<Completion>, Vec<String>) {
    let mut turn = Turn::new(row(model), None);
    let mut tokens = Vec::new();
    for line in stream.lines() {
        match turn.feed_line(line.trim()) {
            Ok(Some(t)) => tokens.push(t),
            Ok(None) => {}
            Err(e) => return (Err(e), tokens),
        }
    }
    (turn.finish(), tokens)
}

fn ok(model: &str, stream: &str) -> (Completion, Vec<String>) {
    let (c, t) = read(model, stream);
    (c.expect("a completion"), t)
}

// ---- to_wire -----------------------------------------------------------------------------------

#[test]
fn system_messages_hoist_in_order_and_the_rest_map_in_order() {
    let w = to_wire(
        &[
            msg("system", "profile"),
            msg("user", "hi"),
            msg("assistant", "hello"),
            msg("system", "late rule"),
            msg("user", "again"),
        ],
        None,
    )
    .unwrap();
    assert_eq!(
        w.system,
        vec![
            json!({"type":"text","text":"profile"}),
            json!({"type":"text","text":"late rule"})
        ]
    );
    assert_eq!(
        w.messages,
        vec![
            json!({"role":"user","content":"hi"}),
            json!({"role":"assistant","content":"hello"}),
            json!({"role":"user","content":"again"}),
        ]
    );
}

#[test]
fn no_system_message_means_no_system_key_at_all() {
    let w = to_wire(&[msg("user", "hi")], None).unwrap();
    let body = chat_body(row(SONNET), None, None, w, true, 100);
    assert!(body.get("system").is_none(), "an empty array may 400");
}

#[test]
fn a_blank_message_of_any_role_is_dropped() {
    let w = to_wire(
        &[
            msg("system", "   "),
            msg("user", "first"),
            msg("assistant", ""),
            msg("user", "\n\t"),
            msg("user", "second"),
        ],
        None,
    )
    .unwrap();
    assert!(w.system.is_empty());
    // The dropped assistant turn also let the two user messages become neighbours, which merge.
    assert_eq!(w.messages.len(), 1);
    assert_eq!(
        w.messages[0]["content"],
        json!([{"type":"text","text":"first"},{"type":"text","text":"second"}])
    );
}

/// The chat's real shape: the learned profile is the cache seam, the grounding contract follows, then
/// the per-turn context and the turn itself. The two user messages MERGE into one message of two
/// blocks: the live probe never captured two in a row, so PM doesn't depend on the API combining them.
#[test]
fn chats_real_shape_marks_the_profile_and_merges_the_two_user_messages() {
    let w = to_wire(
        &[
            msg("system", "profile"),
            msg("system", "contract"),
            msg("user", "context"),
            msg("user", "turn"),
        ],
        Some(0),
    )
    .unwrap();
    assert_eq!(
        w.system,
        vec![
            json!({"type":"text","text":"profile","cache_control":{"type":"ephemeral"}}),
            json!({"type":"text","text":"contract"}),
        ]
    );
    assert_eq!(w.messages.len(), 1);
    assert_eq!(w.messages[0]["role"], "user");
    assert_eq!(
        w.messages[0]["content"],
        json!([{"type":"text","text":"context"},{"type":"text","text":"turn"}])
    );
}

fn marks_in(w: &Wire) -> usize {
    let body = chat_body(row(SONNET), None, None, to_wire_clone(w), true, 10);
    body.to_string().matches("cache_control").count()
}

fn to_wire_clone(w: &Wire) -> Wire {
    Wire {
        system: w.system.clone(),
        messages: w.messages.clone(),
    }
}

#[test]
fn the_cache_mark_is_on_exactly_the_block_named_and_never_twice() {
    let list = [
        msg("system", "a"),
        msg("system", "b"),
        msg("user", "c"),
        msg("assistant", "d"),
        msg("user", "e"),
    ];
    for i in 0..list.len() {
        let w = to_wire(&list, Some(i)).unwrap();
        assert_eq!(marks_in(&w), 1, "mark at {i}");
    }
    assert_eq!(marks_in(&to_wire(&list, None).unwrap()), 0);
    // On system block 1 only.
    let w = to_wire(&list, Some(1)).unwrap();
    assert!(w.system[0].get("cache_control").is_none());
    assert_eq!(w.system[1]["cache_control"], json!({"type":"ephemeral"}));
    // A mark on a non-system message goes on that message's last text block.
    let w = to_wire(&list, Some(3)).unwrap();
    assert_eq!(
        w.messages[1]["content"],
        json!([{"type":"text","text":"d","cache_control":{"type":"ephemeral"}}])
    );
    assert!(!w.system.iter().any(|b| b.get("cache_control").is_some()));
}

#[test]
fn a_mark_whose_message_was_dropped_moves_to_the_nearest_earlier_system_block_or_nowhere() {
    let list = [
        msg("system", "profile"),
        msg("system", "  "),
        msg("user", "turn"),
    ];
    let w = to_wire(&list, Some(1)).unwrap();
    assert_eq!(w.system.len(), 1);
    assert_eq!(w.system[0]["cache_control"], json!({"type":"ephemeral"}));
    // Nothing earlier survives: nowhere.
    let list = [msg("system", ""), msg("user", "turn")];
    assert_eq!(marks_in(&to_wire(&list, Some(0)).unwrap()), 0);
    // An index past the end finds the nearest earlier one too.
    let list = [msg("system", "profile"), msg("user", "turn")];
    assert_eq!(marks_in(&to_wire(&list, Some(9)).unwrap()), 1);
}

#[test]
fn a_list_that_starts_with_an_assistant_turn_gets_a_pm_written_user_line_first() {
    let w = to_wire(&[msg("assistant", "earlier"), msg("user", "now")], None).unwrap();
    assert_eq!(w.messages.len(), 3);
    assert_eq!(w.messages[0]["role"], "user");
    assert_eq!(w.messages[0]["content"], LEADING_STUB);
    assert_eq!(w.messages[1]["content"], "earlier");
}

#[test]
fn a_list_that_does_not_end_on_the_user_is_refused_before_any_request() {
    for list in [
        vec![msg("user", "a"), msg("assistant", "b")],
        vec![msg("system", "only a rule")],
        vec![],
        // Blank turns don't count: the last real one is the assistant's.
        vec![msg("user", "a"), msg("assistant", "b"), msg("user", " ")],
    ] {
        let e = to_wire(&list, None).err().expect("refused");
        assert!(e.to_string().contains("can't continue a reply"), "{e}");
    }
}

#[test]
fn an_unknown_role_is_an_error_not_a_guess() {
    let e = to_wire(&[msg("tool", "x"), msg("user", "y")], None)
        .err()
        .expect("refused");
    assert!(e.to_string().contains("can't take"), "{e}");
}

// ---- thinking and the body ----------------------------------------------------------------------

const LEVELS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

#[test]
fn thinking_is_switched_off_only_for_haiku_in_background_at_low() {
    for id in ALL_FOUR {
        let m = row(id);
        for role in [None, Some(Role::Chat), Some(Role::Background)] {
            for effort in [
                None,
                Some("low"),
                Some("medium"),
                Some("high"),
                Some("xhigh"),
                Some("max"),
            ] {
                let got = thinking_setting(m, role, effort);
                let expected =
                    (id == HAIKU && role == Some(Role::Background) && effort == Some("low"))
                        .then_some("disabled");
                assert_eq!(got, expected, "{id} {role:?} {effort:?}");
            }
        }
    }
}

#[test]
fn every_body_is_the_safe_core_for_every_model_role_and_level() {
    const ALLOWED: [&str; 7] = [
        "model",
        "max_tokens",
        "stream",
        "system",
        "messages",
        "output_config",
        "thinking",
    ];
    let list = [
        msg("system", "profile"),
        msg("user", "context"),
        msg("user", "turn"),
    ];
    for id in ALL_FOUR {
        for role in [Role::Chat, Role::Background] {
            let mut levels: Vec<Option<&str>> = vec![None];
            levels.extend(row(id).effort_levels.iter().map(|l| Some(l.as_str())));
            for level in levels {
                let a = arm(id, Some(role), level);
                let p = prepare(&a, &list, Some(0), None).unwrap();
                let obj = p.body.as_object().unwrap();
                for k in obj.keys() {
                    assert!(ALLOWED.contains(&k.as_str()), "{id}: stray key {k}");
                }
                assert_eq!(p.body["model"], row(id).wire);
                assert_eq!(p.body["stream"], true);
                match level {
                    Some(l) => assert_eq!(p.body["output_config"], json!({"effort": l})),
                    None => assert!(obj.get("output_config").is_none()),
                }
                let disabled = id == HAIKU && role == Role::Background && level == Some("low");
                if disabled {
                    assert_eq!(p.body["thinking"], json!({"type": "disabled"}));
                } else {
                    assert!(obj.get("thinking").is_none(), "{id} {role:?} {level:?}");
                }
                let limit = p.body["max_tokens"].as_u64().unwrap();
                assert!((1..=u64::from(row(id).max_output)).contains(&limit));
                assert_eq!(
                    p.body["messages"].as_array().unwrap().last().unwrap()["role"],
                    "user"
                );
                for banned in [
                    concat!("\"temper", "ature\""),
                    concat!("\"top", "_p\""),
                    concat!("\"top", "_k\""),
                    "\"tools\"",
                    "\"metadata\"",
                    concat!("\"fall", "backs\""),
                    "\"speed\"",
                    "\"service_tier\"",
                    "\"inference_geo\"",
                ] {
                    assert!(!p.body.to_string().contains(banned), "{id}: {banned}");
                }
            }
        }
    }
}

#[test]
fn a_callers_bound_lowers_the_limit_and_a_bound_of_zero_sends_one() {
    let m = row(SONNET);
    assert_eq!(output_limit(m, None), 128_000);
    assert_eq!(output_limit(m, Some(500)), 500);
    assert_eq!(output_limit(m, Some(9_999_999)), 128_000);
    assert_eq!(output_limit(m, Some(0)), 1);
    let a = arm(SONNET, Some(Role::Chat), None);
    let p = prepare(&a, &[msg("user", "x")], None, Some(0)).unwrap();
    assert_eq!(p.body["max_tokens"], 1);
}

#[test]
fn a_stored_effort_the_model_does_not_offer_never_reaches_the_wire() {
    let a = arm(SONNET, Some(Role::Chat), Some("none"));
    assert_eq!(wire_effort(row(SONNET), &a), None);
    let p = prepare(&a, &[msg("user", "x")], None, None).unwrap();
    assert!(p.body.get("output_config").is_none());
    // And the same stale level on Haiku in the background doesn't switch thinking off either.
    let a = arm(HAIKU, Some(Role::Background), Some("none"));
    assert!(prepare(&a, &[msg("user", "x")], None, None)
        .unwrap()
        .body
        .get("thinking")
        .is_none());
}

/// The arm the real builder makes carries its role, so the default Haiku background job is the cheap
/// one: stored effort `low` (the background default) and role Background switch thinking off.
#[test]
fn the_default_haiku_background_arm_sends_thinking_disabled_and_low() {
    let dir = tempfile::tempdir().unwrap();
    let conn = crate::db::open(
        &dir.path().join("t.sqlite"),
        "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff",
    )
    .unwrap();
    let inputs =
        direct_providers::arm_inputs(&conn, CloudProvider::Anthropic, Role::Background).unwrap();
    assert_eq!(inputs.models, vec![HAIKU.to_string()]);
    assert_eq!(inputs.effort.as_deref(), Some("low"));
    let a = CloudArm {
        effort: inputs.effort,
        role: Some(Role::Background),
        ..arm(&inputs.models[0], None, None)
    };
    let p = prepare(
        &a,
        &[msg("system", "rule"), msg("user", "x")],
        Some(0),
        Some(2000),
    )
    .unwrap();
    assert_eq!(p.body["thinking"], json!({"type":"disabled"}));
    assert_eq!(p.body["output_config"], json!({"effort":"low"}));
    assert_eq!(p.body["max_tokens"], 2000);
    // Chat's default omits the effort and never disables thinking.
    let chat = direct_providers::arm_inputs(&conn, CloudProvider::Anthropic, Role::Chat).unwrap();
    assert_eq!(chat.models, vec![SONNET.to_string()]);
    assert_eq!(chat.effort, None);
}

// ---- the stream, through the Turn ----------------------------------------------------------------

#[test]
fn bobbys_real_plain_stream_reads_whole_with_padding_and_a_placeholder_output_count() {
    let (c, tokens) = ok(HAIKU, STREAM_HAIKU);
    assert_eq!(c.text, "1\n2\n3\n4\n5");
    assert_eq!(tokens, ["1\n2\n3\n4", "\n5"]);
    assert!(!c.truncated && !c.usage.refused);
    assert_eq!(c.model.as_deref(), Some("claude-haiku-5-5"));
    // The final usage is the delta's: 23 in, 11 out. `message_start`'s output of 8 is a placeholder.
    assert_eq!(c.usage.prompt_tokens, Some(23));
    assert_eq!(c.usage.completion_tokens, Some(11));
    assert_eq!(c.usage.cached_tokens, Some(0));
    assert_eq!(c.usage.cache_write_tokens, Some(0));
}

#[test]
fn bobbys_real_thinking_stream_skips_the_empty_thinking_block() {
    let (c, tokens) = ok(SONNET, STREAM_SONNET_THINKING);
    assert_eq!(c.text, "£18");
    assert_eq!(tokens, ["£", "18"]);
    // Output includes the 32 thinking tokens; they are billed as output and never added again.
    assert_eq!(c.usage.completion_tokens, Some(36));
    assert_eq!(c.usage.prompt_tokens, Some(45));
}

/// A canned stream with a cache hit: the prompt is the TOTAL input (uncached + written + read), and
/// the cost is the Sonnet formula over it.
#[test]
fn total_input_is_the_sum_of_the_three_counters_and_priced_on_the_model() {
    let stream = start(
        "claude-sonnet-5-5",
        json!({"input_tokens":13,"cache_creation_input_tokens":0,"cache_read_input_tokens":1383,"output_tokens":1}),
    ) + &thinking_block(0)
        + &text_block(1, &["Hel", "lo"])
        + &ev(json!({"type":"ping"}))
        + &finish_with("end_turn", json!({"output_tokens":42}));
    let (c, tokens) = ok(SONNET, &stream);
    assert_eq!(c.text, "Hello");
    assert_eq!(tokens, ["Hel", "lo"]);
    assert_eq!(c.usage.prompt_tokens, Some(1396));
    assert_eq!(c.usage.cached_tokens, Some(1383));
    assert_eq!(c.usage.cache_write_tokens, Some(0));
    assert_eq!(c.usage.completion_tokens, Some(42));
    let settled = settle(c, row(SONNET));
    assert!(
        near(settled.usage.cost, 0.0005843),
        "{:?}",
        settled.usage.cost
    );
    assert_eq!(settled.model.as_deref(), Some(SONNET));
}

#[test]
fn a_reply_that_stops_at_the_limit_with_only_thinking_is_truncated_and_empty() {
    let stream = start(
        "claude-fable-5-2",
        json!({"input_tokens":10,"output_tokens":1}),
    ) + &thinking_block(0)
        + &finish_with("max_tokens", json!({"output_tokens":4000}));
    let (c, _) = ok(FABLE, &stream);
    assert!(c.truncated && c.text.is_empty());
    assert!(!c.usage.refused);
    // The chat path turns this into "stopped before it wrote an answer" (a test in `conversations`).
    assert_eq!(c.usable_text(), None);
    assert_eq!(
        c.unusable_reason(),
        Some("the model's reply was cut off before it finished")
    );
}

#[test]
fn a_context_window_stop_is_truncated_and_keeps_the_text() {
    let stream = start("claude-opus-5-5", json!({"input_tokens":5}))
        + &text_block(0, &["part"])
        + &finish_with("model_context_window_exceeded", json!({"output_tokens":9}));
    let (c, _) = ok(OPUS, &stream);
    assert!(c.truncated);
    assert_eq!(c.text, "part");
}

#[test]
fn a_refusal_before_any_output_is_ok_with_usage_a_flag_and_no_text() {
    let stream = start(
        "claude-fable-5-2",
        json!({"input_tokens":900,"output_tokens":1}),
    ) + &ev(
        json!({"type":"message_delta","delta":{"stop_reason":"refusal","stop_details":{"type":"refusal","category":"cyber","explanation":"This request was declined."}},"usage":{"input_tokens":900,"output_tokens":0}}),
    ) + &ev(json!({"type":"message_stop"}));
    let (c, tokens) = ok(FABLE, &stream);
    assert!(tokens.is_empty());
    assert!(c.text.is_empty() && !c.truncated);
    assert!(c.usage.refused);
    assert_eq!(c.usage.prompt_tokens, Some(900));
    // It is BILLED: settle prices it, so the usage row gets a cost.
    let settled = settle(c, row(FABLE));
    assert!(settled.usage.cost.is_some());
    assert_eq!(settled.model.as_deref(), Some(FABLE));
    assert!(settled.usage.refused);
    // The reader's reasons: nothing is usable, and the sentence says what happened without echoing
    // Anthropic's explanation.
    assert_eq!(settled.usable_text(), None);
    assert_eq!(
        settled.unusable_reason(),
        Some("Anthropic's safety system stopped this reply")
    );
    // (The chat path's sentence for it is held by a test in `conversations`.)
}

#[test]
fn a_refusal_halfway_drops_the_partial_text_but_the_sink_already_had_it() {
    let stream = start(
        "claude-sonnet-5-5",
        json!({"input_tokens":50,"output_tokens":1}),
    ) + &text_block(0, &["Here is"])
        + &finish_with("refusal", json!({"output_tokens":7}));
    let (c, tokens) = ok(SONNET, &stream);
    assert_eq!(tokens, ["Here is"]);
    assert!(c.text.is_empty() && c.usage.refused);
    assert_eq!(c.usage.completion_tokens, Some(7));
    assert_eq!(c.usage.prompt_tokens, Some(50));
    assert!(settle(c, row(SONNET)).usage.cost.is_some());
}

#[test]
fn a_text_blocks_own_first_text_is_kept() {
    let stream = start("claude-sonnet-5-5", json!({"input_tokens":5}))
        + &ev(
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"Hi"}}),
        )
        + &ev(
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":" there"}}),
        )
        + &finish_with("end_turn", json!({"output_tokens":3}));
    let (c, tokens) = ok(SONNET, &stream);
    assert_eq!(c.text, "Hi there");
    assert_eq!(tokens, ["Hi", " there"]);
}

#[test]
fn an_error_event_after_the_200_is_the_mapped_sentence_and_drops_the_partial() {
    let stream = start("claude-sonnet-5-5", json!({"input_tokens":5}))
        + &text_block(0, &["partial"])
        + &ev(json!({"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}));
    let (c, tokens) = read(SONNET, &stream);
    assert_eq!(tokens, ["partial"]);
    assert_eq!(
        c.err().unwrap().to_string(),
        "Anthropic is having trouble right now. Try again shortly."
    );
    // The spend cap arrives as a rate-limit TYPE with its code, and the code wins.
    let stream = start("claude-sonnet-5-5", json!({"input_tokens":5}))
        + &ev(
            json!({"type":"error","error":{"type":"rate_limit_error","message":"You have reached your API usage limits","details":{"error_code":"enforced_spend_limit_reached"}}}),
        );
    let said = read(SONNET, &stream).0.err().unwrap().to_string();
    assert!(said.contains("monthly spend cap"), "{said}");
    assert!(!said.contains("rate-limiting"), "{said}");
}

#[test]
fn a_stream_with_no_message_stop_is_an_error_with_text_or_without() {
    let cut = "Anthropic's reply was cut off before the end, so PM didn't keep it. Try again.";
    let with_text =
        start("claude-sonnet-5-5", json!({"input_tokens":5})) + &text_block(0, &["half"]);
    assert_eq!(read(SONNET, &with_text).0.err().unwrap().to_string(), cut);
    assert_eq!(read(SONNET, "").0.err().unwrap().to_string(), cut);
    // A stop reason without the end marker is still not the end.
    let no_marker = start("claude-sonnet-5-5", json!({"input_tokens":5}))
        + &text_block(0, &["half"])
        + &ev(
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":2}}),
        );
    assert_eq!(read(SONNET, &no_marker).0.err().unwrap().to_string(), cut);
}

#[test]
fn unknown_events_blocks_and_deltas_are_ignored() {
    let stream = start("claude-sonnet-5-5", json!({"input_tokens":5}))
        + &ev(json!({"type":"future_event","x":1}))
        + &ev(
            json!({"type":"content_block_start","index":0,"content_block":{"type":"fallback","from":{"model":"a"},"to":{"model":"b"}}}),
        )
        + &ev(json!({"type":"content_block_stop","index":0}))
        + &ev(
            json!({"type":"content_block_start","index":1,"content_block":{"type":"future_block"}}),
        )
        + &ev(
            json!({"type":"content_block_delta","index":1,"delta":{"type":"future_delta","text":"NOT THE REPLY"}}),
        )
        + &text_block(2, &["kept"])
        + &ev(
            json!({"type":"content_block_delta","index":2,"delta":{"type":"another_delta","text":"NOT THE REPLY"}}),
        )
        + &finish_with("end_turn", json!({"output_tokens":3}));
    let (c, _) = ok(SONNET, &stream);
    assert_eq!(c.text, "kept");
    // A data line that isn't JSON is ignored too.
    let mut turn = Turn::new(row(SONNET), None);
    assert!(turn.feed_line("data: not json at all").unwrap().is_none());
    assert!(turn.feed_line("event: message_start").unwrap().is_none());
    assert!(turn.feed_line(": a comment").unwrap().is_none());
}

#[test]
fn usage_is_merged_by_the_larger_of_each_field_and_never_added() {
    let stream = start(
        "claude-sonnet-5-5",
        json!({"input_tokens":20,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":1}),
    ) + &text_block(0, &["x"])
        + &finish_with(
            "end_turn",
            json!({"input_tokens":25,"cache_creation_input_tokens":500,"cache_read_input_tokens":0,"output_tokens":80}),
        );
    let (c, _) = ok(SONNET, &stream);
    assert_eq!(c.usage.prompt_tokens, Some(525));
    assert_eq!(c.usage.cache_write_tokens, Some(500));
    assert_eq!(c.usage.completion_tokens, Some(80));
    // A 0 placeholder in a later event never lowers a figure.
    let stream = start(
        "claude-sonnet-5-5",
        json!({"input_tokens":20,"output_tokens":1}),
    ) + &text_block(0, &["x"])
        + &finish_with("end_turn", json!({"output_tokens":80,"input_tokens":0}));
    let (c, _) = ok(SONNET, &stream);
    assert_eq!(c.usage.prompt_tokens, Some(20));
    assert_eq!(c.usage.completion_tokens, Some(80));
    // With no delta carrying an output count there is NO output count: the start's is a placeholder,
    // and booking it would price the reply as an exact under-count (see the no-delta test).
    let stream = start(
        "claude-sonnet-5-5",
        json!({"input_tokens":20,"output_tokens":3}),
    ) + &text_block(0, &["x"])
        + &ev(json!({"type":"message_stop"}));
    assert_eq!(ok(SONNET, &stream).0.usage.completion_tokens, None);
}

#[test]
fn a_stream_with_no_usage_at_all_has_no_cost_never_zero() {
    let stream = start("claude-sonnet-5-5", json!({}))
        + &text_block(0, &["x"])
        + &ev(json!({"type":"message_stop"}));
    let (c, _) = ok(SONNET, &stream);
    assert!(c.usage.prompt_tokens.is_none() && c.usage.completion_tokens.is_none());
    assert_eq!(settle(c, row(SONNET)).usage.cost, None);
}

#[test]
fn a_redacted_thinking_block_and_odd_stop_reasons_are_normal() {
    let stream = start("claude-sonnet-5-5", json!({"input_tokens":5}))
        + &ev(
            json!({"type":"content_block_start","index":0,"content_block":{"type":"redacted_thinking","data":"x"}}),
        )
        + &ev(json!({"type":"content_block_stop","index":0}))
        + &text_block(1, &["answer"])
        + &finish_with("end_turn", json!({"output_tokens":3}));
    assert_eq!(ok(SONNET, &stream).0.text, "answer");
    for stop in ["pause_turn", "tool_use", "stop_sequence", "something_new"] {
        let stream = start("claude-sonnet-5-5", json!({"input_tokens":5}))
            + &text_block(0, &["kept"])
            + &finish_with(stop, json!({"output_tokens":3}));
        let (c, _) = ok(SONNET, &stream);
        assert_eq!(c.text, "kept", "{stop}");
        assert!(!c.truncated && !c.usage.refused, "{stop}");
    }
}

// ---- cost and settle ----------------------------------------------------------------------------

fn usage_of(prompt: i64, cached: i64, written: i64, out: i64) -> Usage {
    Usage {
        prompt_tokens: Some(prompt),
        completion_tokens: Some(out),
        cached_tokens: Some(cached),
        cache_write_tokens: Some(written),
        ..Usage::default()
    }
}

fn cost(id: &str, u: &Usage) -> Option<f64> {
    direct_models::cost_usd(row(id), u, "2026-11-26")
}

#[test]
fn the_cost_follows_bobbys_real_cache_figures_and_the_four_price_rows() {
    // His real cache write (input 13, creation 1383, read 0, out 4) and read (13, 0, 1383, 4), Haiku.
    assert!(near(cost(HAIKU, &usage_of(1396, 0, 1383, 4)), 0.000176175));
    assert!(near(cost(HAIKU, &usage_of(1396, 1383, 0, 4)), 0.00001713));
    assert!(near(cost(SONNET, &usage_of(2000, 1500, 0, 800)), 0.00915));
    assert!(near(cost(FABLE, &usage_of(1200, 0, 1000, 100)), 0.0156));
    assert!(near(cost(OPUS, &usage_of(1000, 0, 0, 500)), 0.014));
    assert_eq!(cost(SONNET, &Usage::default()), None);
    // `ordinary` is never negative.
    assert!(cost(SONNET, &usage_of(100, 90, 90, 0)).unwrap() >= 0.0);
}

#[test]
fn haikus_long_request_tier_is_the_whole_request_at_five_times_from_100001_tokens() {
    assert!(near(cost(HAIKU, &usage_of(100_000, 0, 0, 1000)), 0.0105));
    assert!(near(cost(HAIKU, &usage_of(100_001, 0, 0, 1000)), 0.0525005));
    // Mostly cache READS still take the tier, at 5x the read price (0.05).
    let long_cached = cost(HAIKU, &usage_of(100_001, 100_000, 0, 0)).unwrap();
    assert!((long_cached - (1.0 * 0.5 + 100_000.0 * 0.05) / 1e6).abs() < 1e-12);
    // The other three rows never take a tier, at any size.
    for id in [FABLE, OPUS, SONNET] {
        let m = row(id);
        let p = |n: i64| cost(id, &usage_of(n, 0, 0, 0)).unwrap();
        assert!(
            (p(900_000) - 900_000.0 * m.price_in / 1e6).abs() < 1e-9,
            "{id}"
        );
    }
}

#[test]
fn settle_always_names_a_catalogue_id() {
    let c = |served: Option<&str>| Completion {
        text: String::new(),
        model: served.map(str::to_string),
        usage: usage_of(10, 0, 0, 1),
        truncated: false,
    };
    assert_eq!(
        settle(c(Some("claude-haiku-5-5")), row(SONNET))
            .model
            .as_deref(),
        Some(HAIKU)
    );
    // A dated snapshot of a listed model maps to that row (the trailing date is a harmless fallback).
    assert_eq!(
        settle(c(Some("claude-haiku-5-5-2027-01-01")), row(SONNET))
            .model
            .as_deref(),
        Some(HAIKU)
    );
    // A future or missing served name falls back to the REQUESTED row, never None, never raw.
    for served in [
        Some("claude-new-9"),
        Some("anthropic/claude-haiku-5-5"),
        None,
    ] {
        let settled = settle(c(served), row(SONNET));
        assert_eq!(settled.model.as_deref(), Some(SONNET), "{served:?}");
    }
}

// ---- errors -------------------------------------------------------------------------------------

fn body(error_type: &str, message: &str) -> String {
    json!({"type":"error","error":{"type":error_type,"message":message},"request_id":"req_01"})
        .to_string()
}

fn body_code(error_type: &str, message: &str, code: &str) -> String {
    json!({"type":"error","error":{"type":error_type,"message":message,"details":{"error_code":code,"appeal_url":"https://appeal.example/x"}},"request_id":"req_01"})
        .to_string()
}

fn classified(status: u16, raw: &str) -> Failure {
    classify(Some(status), &fields_from_body(raw))
}

fn said(status: u16, raw: &str, model: &str, effort: Option<&str>) -> String {
    let f = fields_from_body(raw);
    let failure = classify(Some(status), &f);
    failure_text(&failure, Some(status), &f, row(model), effort)
}

const CREDIT_TEXT: &str = "Your credit balance is too low to access the Anthropic API. Please go to Plans & Billing to upgrade or purchase credits.";

#[test]
fn every_error_in_the_table_maps_to_its_failure_and_its_sentence() {
    let cases: Vec<(u16, String, Failure)> = vec![
        // E1
        (401, body("authentication_error", "invalid x-api-key"), Failure::KeyRejected),
        // E2
        (402, body("billing_error", "There is an issue with your billing or payment information."), Failure::OutOfCredit),
        // E3: the text is verified, the status is not, so it runs as a 402 and as a 400.
        (400, body("invalid_request_error", CREDIT_TEXT), Failure::OutOfCredit),
        (402, body("invalid_request_error", CREDIT_TEXT), Failure::OutOfCredit),
        // E4: the spend cap is a 429, but the CODE wins.
        (429, body_code("rate_limit_error", "You have reached your API usage limits: your organization has crossed its monthly API usage threshold. You will regain access on 2026-09-01 at 00:00 UTC.", "enforced_spend_limit_reached"), Failure::SpendCap),
        // E5
        (400, body("invalid_request_error", "You have reached your specified API usage limits. You will regain access on 2026-12-01 at 00:00 UTC."), Failure::OwnSpendLimit),
        (400, body("invalid_request_error", "You have reached your specified workspace API usage limits. You will regain access on 2026-12-01 at 00:00 UTC."), Failure::OwnSpendLimit),
        // E6
        (429, body_code("rate_limit_error", "ramping", "ramp_limit_exceeded"), Failure::RampLimit),
        // E7
        (429, body("rate_limit_error", "Number of request tokens has exceeded your per-minute rate limit"), Failure::RateLimited),
        // E8: verbatim from Anthropic's retention page.
        (400, body("invalid_request_error", "In order to access this model, your organization or workspace must have data retention enabled."), Failure::NeedsRetention),
        // E9
        (400, body_code("invalid_request_error", "Your organization is on hold.", "organization_on_hold"), Failure::OnHold),
        // E10
        (400, body("invalid_request_error", "max_tokens: 200000 > 128000, which is the maximum allowed number of output tokens for claude-sonnet-5-5"), Failure::OutputLimitStale),
        // E11: the context message wins even when it names max_tokens.
        (400, body("invalid_request_error", "prompt is too long: 1050000 tokens > 1000000 maximum"), Failure::ContextTooLong),
        (400, body("invalid_request_error", "input length and `max_tokens` exceed context limit: 900000 + 128000 > 1000000"), Failure::ContextTooLong),
        // E12 (and Anthropic's real one from the probe)
        (400, body("invalid_request_error", "output_config.effort: this value is not supported for this model"), Failure::EffortRejected),
        (400, XHIGH_400.to_string(), Failure::EffortRejected),
        // E13
        (400, body("invalid_request_error", "thinking.type: disabled is not supported for this model"), Failure::SetupStale),
        // E14
        (413, body("request_too_large", "Request exceeds the maximum allowed number of bytes."), Failure::TooLarge),
        // E15
        (403, body("permission_error", "Your API key does not have permission to use the specified resource."), Failure::NotAllowed),
        // E16
        (404, body("not_found_error", "model: claude-sonnet-5-5"), Failure::ModelUnavailable),
        // E17-E20
        (529, body("overloaded_error", "Overloaded"), Failure::Overloaded),
        (500, body("api_error", "Internal server error"), Failure::Overloaded),
        (504, body("timeout_error", "Request timed out"), Failure::Overloaded),
        (503, body("overloaded_error", "Overloaded"), Failure::Overloaded),
        // E21: a plain-text body still classifies, by phrase.
        (400, "Error: this model needs data retention enabled".to_string(), Failure::NeedsRetention),
        // Anthropic's real blank and sampling 400s are not mapped to anything special.
        (400, BLANK_400.to_string(), Failure::Other),
        (400, TEMPERATURE_400.to_string(), Failure::Other),
    ];
    for (status, raw, want) in cases {
        let got = classified(status, &raw);
        assert_eq!(got, want, "{status} {raw}");
        for id in ALL_FOUR {
            let sentence = said(status, &raw, id, Some("xhigh"));
            for banned in ["req_01", "appeal", "sk-ant"] {
                assert!(!sentence.contains(banned), "{status}: {sentence}");
            }
        }
    }
}

#[test]
fn the_failure_sentences_read_as_the_table_says() {
    let s = |status, raw: String, model| said(status, &raw, model, Some("xhigh"));
    assert!(s(401, body("authentication_error", "x"), SONNET)
        .starts_with("Anthropic rejected your key"));
    assert!(s(402, body("billing_error", "x"), SONNET).contains("out of credit"));
    assert!(s(
        400,
        body(
            "invalid_request_error",
            "You have reached your specified API usage limits."
        ),
        SONNET
    )
    .contains("usage limit you set"));
    assert!(s(429, body("rate_limit_error", "x"), SONNET).contains("rate-limiting"));
    // The retention sentence names the REQUESTED model from the catalogue, not the error text.
    let retention = s(
        400,
        body(
            "invalid_request_error",
            "must have data retention enabled for Opus",
        ),
        FABLE,
    );
    assert!(retention.contains("Claude Fable 5.2"), "{retention}");
    assert!(retention.contains("30-day data retention"), "{retention}");
    assert!(s(
        400,
        body("invalid_request_error", "prompt is too long"),
        OPUS
    )
    .contains("longer than Claude Opus 5.5's limit"));
    assert!(s(
        400,
        body("invalid_request_error", "effort not allowed"),
        OPUS
    )
    .contains("doesn't take effort 'xhigh'"));
    assert!(s(404, body("not_found_error", "x"), HAIKU).contains("can't use Claude Haiku 5.5"));
    // An unrecognised answer is the generic sentence with the redacted detail, cut at 200.
    let long = "x ".repeat(400);
    let other = s(418, body("teapot", &long), SONNET);
    assert!(
        other.starts_with("Anthropic request failed (418): "),
        "{other}"
    );
    assert!(other.chars().count() <= "Anthropic request failed (418): ".len() + 200);
}

#[test]
fn an_error_that_echoes_a_key_never_shows_it() {
    let echoed = body(
        "teapot",
        "bad key sk-ant-api03-AbCdEfGhIjKlMnOpQrStUvWxYz0123456789 was sent",
    );
    let sentence = said(418, &echoed, SONNET, None);
    assert!(sentence.contains("[key]"), "{sentence}");
    assert!(
        !sentence.contains("AbCdEf") && !sentence.contains("sk-ant"),
        "{sentence}"
    );
}

#[test]
fn a_mid_stream_error_type_maps_through_the_same_table() {
    for (kind, want) in [
        ("authentication_error", Failure::KeyRejected),
        ("billing_error", Failure::OutOfCredit),
        ("permission_error", Failure::NotAllowed),
        ("not_found_error", Failure::ModelUnavailable),
        ("request_too_large", Failure::TooLarge),
        ("rate_limit_error", Failure::RateLimited),
        ("api_error", Failure::Overloaded),
        ("timeout_error", Failure::Overloaded),
        ("overloaded_error", Failure::Overloaded),
    ] {
        let f = fields_from_error(&json!({"type": kind, "message": "m"}));
        assert_eq!(classify(status_of_type(kind), &f), want, "{kind}");
    }
    // A type PM has never heard of is the generic sentence, not a guess.
    let f = fields_from_error(&json!({"type": "brand_new_error", "message": "m"}));
    assert_eq!(
        classify(status_of_type("brand_new_error"), &f),
        Failure::Other
    );
}

#[test]
fn the_retry_is_for_a_refused_request_only_and_capped() {
    let d = |status, after: Option<&str>, f: &Failure| retry_delay(status, after, f);
    assert_eq!(
        d(429, None, &Failure::RateLimited),
        Some(Duration::from_secs(2))
    );
    assert_eq!(
        d(429, Some("7"), &Failure::RateLimited),
        Some(Duration::from_secs(7))
    );
    assert_eq!(
        d(429, Some("60"), &Failure::RampLimit),
        Some(Duration::from_secs(20))
    );
    assert_eq!(
        d(529, Some("not a number"), &Failure::Overloaded),
        Some(Duration::from_secs(2))
    );
    assert_eq!(
        d(500, None, &Failure::Overloaded),
        Some(Duration::from_secs(2))
    );
    // Never for a spend cap, credit, retention, an ordinary 4xx, or an overload that is a 400.
    for f in [
        Failure::SpendCap,
        Failure::OutOfCredit,
        Failure::OwnSpendLimit,
        Failure::NeedsRetention,
        Failure::KeyRejected,
        Failure::Other,
    ] {
        assert_eq!(d(429, None, &f), None, "{f:?}");
    }
    assert_eq!(d(400, None, &Failure::Overloaded), None);
}

// ---- the key check, pure -------------------------------------------------------------------------

#[test]
fn the_listing_verdict_follows_the_status_and_whose_body_it_is() {
    let ours = |t: &str, m: &str| body(t, m);
    assert_eq!(interpret(200, "{}"), KeyOutcome::Ok);
    assert_eq!(
        interpret(401, &ours("authentication_error", "x")),
        KeyOutcome::Rejected
    );
    // A 401 or 403 from a proxy's page is no verdict on the key.
    assert_eq!(
        interpret(401, "<html>login</html>"),
        KeyOutcome::Unexpected(401)
    );
    assert_eq!(
        interpret(403, "<html>blocked</html>"),
        KeyOutcome::Unexpected(403)
    );
    assert_eq!(
        interpret(
            403,
            &ours(
                "permission_error",
                "Your API key does not have permission to use the specified resource."
            )
        ),
        KeyOutcome::Restricted
    );
    assert_eq!(interpret(402, ""), KeyOutcome::NoCredit);
    assert_eq!(
        interpret(400, &ours("invalid_request_error", CREDIT_TEXT)),
        KeyOutcome::NoCredit
    );
    for status in [429, 500, 503, 529] {
        assert_eq!(
            interpret(status, &ours("overloaded_error", "x")),
            KeyOutcome::Busy
        );
    }
    assert_eq!(
        interpret(
            400,
            &body_code("invalid_request_error", "hold", "organization_on_hold")
        ),
        KeyOutcome::Unexpected(400)
    );
    assert_eq!(interpret(418, ""), KeyOutcome::Unexpected(418));
}

#[test]
fn the_test_chats_reply_is_read_by_the_failure_it_is() {
    assert_eq!(read_probe(200, HAIKU_LOW_200), Probe::Works);
    // A 200 that stopped at the output limit with no answer still proves a chat.
    assert_eq!(read_probe(200, MAX_TOKENS_200), Probe::Works);
    for (status, raw) in [
        (402, body("billing_error", "x")),
        (400, body("invalid_request_error", CREDIT_TEXT)),
        (
            429,
            body_code("rate_limit_error", "cap", "enforced_spend_limit_reached"),
        ),
        (
            400,
            body(
                "invalid_request_error",
                "You have reached your specified API usage limits.",
            ),
        ),
    ] {
        assert_eq!(read_probe(status, &raw), Probe::NoCredit, "{status}");
    }
    let forbidden = body(
        "permission_error",
        "Your API key does not have permission to use the specified resource.",
    );
    assert_eq!(read_probe(403, &forbidden), Probe::Refused);
    assert_eq!(
        read_probe(401, &body("authentication_error", "x")),
        Probe::Refused
    );
    assert_eq!(read_probe(403, "<html>proxy</html>"), Probe::Untried);
    assert_eq!(
        read_probe(404, &body("not_found_error", "model: x")),
        Probe::ModelGone
    );
    for (status, raw) in [
        (
            400,
            body("invalid_request_error", "must have data retention enabled"),
        ),
        (429, body("rate_limit_error", "slow down")),
        (
            429,
            body_code("rate_limit_error", "ramp", "ramp_limit_exceeded"),
        ),
        (529, body("overloaded_error", "x")),
        (400, BLANK_400.to_string()),
        (503, "<html>down</html>".to_string()),
    ] {
        assert_eq!(read_probe(status, &raw), Probe::Untried, "{status}");
    }
}

#[test]
fn the_listing_is_read_through_the_catalogue_and_mythos_never_gets_in() {
    let listing = format!(
        r#"{{"data":[{HAIKU_E},{FABLE_E},{{"type":"model","id":"claude-mythos-5-2","line":"mythos"}},{{"type":"model","id":"claude-sonnet-5-5","lifecycle":"retired"}},{{"type":"model","id":"claude-opus-4-1"}}],"has_more":true}}"#,
        HAIKU_E = MODEL_ENTRY_HAIKU_5_5,
        FABLE_E = MODEL_ENTRY_FABLE_5_2,
    );
    let (listed, total) = listed_models(&listing);
    assert_eq!(listed, vec![FABLE.to_string(), HAIKU.to_string()]);
    assert_eq!(total, 5, "Mythos and unknown ids count in the total only");
    assert!(!listed.iter().any(|id| names_mythos(id)));
    assert_eq!(listed_models("not json"), (Vec::new(), 0));
}

#[test]
fn the_test_chat_tries_the_cheapest_and_never_fable_unless_it_is_alone() {
    let ids = |listed: &[String]| -> Vec<String> {
        probe_order(listed).iter().map(|m| m.id.clone()).collect()
    };
    assert_eq!(ids(&[]), [HAIKU, SONNET, OPUS], "the listing was refused");
    let all: Vec<String> = ALL_FOUR.iter().map(|s| s.to_string()).collect();
    assert_eq!(ids(&all), [HAIKU, SONNET, OPUS]);
    assert_eq!(ids(&[FABLE.to_string()]), [FABLE]);
    assert_eq!(ids(&[FABLE.to_string(), OPUS.to_string()]), [OPUS]);
    assert_eq!(ids(&[SONNET.to_string(), OPUS.to_string()]), [SONNET, OPUS]);
}

/// The weekly provider canary sends these bodies (tests/golden/request_bodies.json), so they are built
/// here by the real builders and compared byte for byte. Haiku is the canary's model, and the arm is
/// the cheap background job at `low`, the one row of the matrix where PM also sends `thinking`.
#[test]
fn request_bodies_match_the_canary_golden() {
    use crate::golden_request_bodies::{assert_matches, SYSTEM, USER};
    let say = [msg("system", SYSTEM), msg("user", USER)];
    let a = arm(HAIKU, Some(Role::Background), Some("low"));
    let stream = prepare(&a, &say, Some(0), Some(64)).unwrap();
    assert_eq!(
        stream.body["thinking"],
        json!({"type": "disabled"}),
        "the canary's body is the one that carries thinking, so it covers that field too"
    );
    let m = row(HAIKU);
    let plain = chat_body(
        m,
        wire_effort(m, &a),
        thinking_setting(m, a.role, wire_effort(m, &a)),
        to_wire(&say, None).unwrap(),
        false,
        64,
    );
    assert_matches(
        "anthropic",
        &[
            ("stream", stream.body),
            ("plain", plain),
            ("key_check", probe_body(m)),
        ],
    );
}

#[test]
fn the_test_chat_body_is_the_smallest_proof_a_chat_works() {
    for id in ALL_FOUR {
        let b = probe_body(row(id));
        let keys: Vec<&str> = b.as_object().unwrap().keys().map(String::as_str).collect();
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        assert_eq!(
            sorted,
            ["max_tokens", "messages", "model", "output_config"],
            "{id}"
        );
        assert_eq!(b["max_tokens"], 32);
        assert_eq!(b["output_config"], json!({"effort": "low"}));
        assert_eq!(b["messages"][0]["role"], "user");
    }
}

#[test]
fn a_key_is_the_right_shape_to_try() {
    assert!(well_formed("sk-ant-api03-abcdefghijklmnopqrst"));
    assert!(well_formed("  a-future-key-shape-with-no-prefix-0123  "));
    assert!(!well_formed("short"));
    assert!(!well_formed(&"k".repeat(301)));
    assert!(!well_formed("has a space inside the key 0123456789"));
    assert!(!well_formed("tab\tinside-the-key-0123456789"));
    assert!(!well_formed("AIzaSyAbcdefghijklmnopqrstuvwxyz0123456"));
    // Another company's `sk-` key is never sent here, whatever its shape: OpenAI's project,
    // service-account and admin keys, a plain legacy one, and OpenRouter's.
    for key in [
        "sk-proj-abcdefghijklmnopqrst",
        "sk-svcacct-abcdefghijklmnop",
        "sk-admin-abcdefghijklmnop",
        "sk-abcdefghijklmnopqrstuvwxyz0123",
        "sk-or-v1-abcdefghijklmnop",
    ] {
        assert!(!well_formed(key), "{key}");
    }
}

// ---- Mythos, in three layers ----------------------------------------------------------------------

#[test]
fn mythos_is_never_offered_saved_or_called() {
    // Layer 1: no catalogue row names it, in any field.
    for m in direct_models::models(CloudProvider::Anthropic) {
        for field in [&m.id, &m.wire, &m.name] {
            assert!(!names_mythos(field), "{}", m.id);
        }
    }
    assert!(direct_models::find("anthropic/claude-mythos-5-2").is_none());
    assert!(direct_models::find_wire("claude-mythos-5-2").is_none());
    // Layer 2: a stored id the catalogue doesn't know is dropped, and a role can't be set to it.
    let dir = tempfile::tempdir().unwrap();
    let conn = crate::db::open(
        &dir.path().join("t.sqlite"),
        "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff",
    )
    .unwrap();
    crate::db::set_setting(
        &conn,
        "direct_anthropic_chat_models",
        r#"["anthropic/claude-mythos-5-2"]"#,
    )
    .unwrap();
    assert_eq!(
        direct_providers::models_for(&conn, CloudProvider::Anthropic, Role::Chat).unwrap(),
        vec![SONNET.to_string()]
    );
    let e = direct_providers::set_role_model(
        &conn,
        CloudProvider::Anthropic,
        Role::Chat,
        &["anthropic/claude-mythos-5-2".to_string()],
        None,
    )
    .unwrap_err();
    assert!(e.to_string().contains("doesn't list"), "{e}");
    // Layer 3: an arm naming it (a hand-edit that got past the others) is refused by name.
    for id in ["anthropic/claude-mythos-5-2", "CLAUDE-MYTHOS-preview"] {
        let e = requested_model(&arm(id, None, None)).expect_err("refused");
        assert!(e.to_string().contains("Mythos"), "{e}");
    }
    // And nothing reaches the network: `prepare` stops first.
    let e = prepare(
        &arm("anthropic/claude-mythos-5-2", None, None),
        &[msg("user", "x")],
        None,
        None,
    )
    .expect_err("refused");
    assert!(e.to_string().contains("Mythos"), "{e}");
}

#[test]
fn a_model_not_in_the_catalogue_is_refused_before_the_network() {
    for id in ["", "not/a-model", "openai/gpt-6.2-sol", "claude-sonnet-5-5"] {
        let e = requested_model(&arm(id, None, None)).expect_err("refused");
        assert!(e.to_string().contains("no longer lists"), "{id}: {e}");
    }
}

#[test]
fn the_roles_take_every_level_the_models_offer_and_refuse_default() {
    let dir = tempfile::tempdir().unwrap();
    let conn = crate::db::open(
        &dir.path().join("t.sqlite"),
        "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff",
    )
    .unwrap();
    for id in ALL_FOUR {
        for level in LEVELS {
            direct_providers::set_role_model(
                &conn,
                CloudProvider::Anthropic,
                Role::Chat,
                &[id.to_string()],
                Some(level),
            )
            .unwrap_or_else(|e| panic!("{id} {level}: {e}"));
            assert_eq!(
                direct_providers::effort_for(&conn, CloudProvider::Anthropic, Role::Chat)
                    .unwrap()
                    .as_deref(),
                Some(level)
            );
        }
        assert!(direct_providers::set_role_model(
            &conn,
            CloudProvider::Anthropic,
            Role::Chat,
            &[id.to_string()],
            Some("default"),
        )
        .is_err());
    }
}

/// Turning Anthropic off moves only the roles that were on Anthropic, in one step, and forgets only its
/// own consent; OpenAI's roles stay.
#[test]
fn opting_out_of_anthropic_moves_only_its_roles() {
    let dir = tempfile::tempdir().unwrap();
    let conn = crate::db::open(
        &dir.path().join("t.sqlite"),
        "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff",
    )
    .unwrap();
    for p in [CloudProvider::OpenAi, CloudProvider::Anthropic] {
        direct_providers::set_opt_in(&conn, p, true).unwrap();
    }
    direct_providers::set_role_provider(&conn, Role::Chat, CloudProvider::Anthropic).unwrap();
    direct_providers::set_role_provider(&conn, Role::Background, CloudProvider::OpenAi).unwrap();
    let moved = direct_providers::set_opt_in(&conn, CloudProvider::Anthropic, false).unwrap();
    assert_eq!(moved, vec![Role::Chat]);
    assert_eq!(
        crate::llm_gateway::role_cloud_provider(&conn, Role::Chat).unwrap(),
        CloudProvider::OpenRouter
    );
    assert_eq!(
        crate::llm_gateway::role_cloud_provider(&conn, Role::Background).unwrap(),
        CloudProvider::OpenAi
    );
}

/// A second provider saved after the first never takes a role that is no longer on OpenRouter: the
/// card's adopt rule is generic, and the Rust side of it is `set_role_provider` being called only for
/// a role still on OpenRouter. Pinned here as the rule's data: the Anthropic row holds no special case.
#[test]
fn anthropic_is_one_more_table_row_with_the_same_keys_as_the_others() {
    let d = direct_providers::provider(CloudProvider::Anthropic).unwrap();
    assert_eq!(d.name, "Anthropic");
    assert_eq!(d.opt_in, "direct_anthropic_enabled");
    assert_eq!(d.chat.models, "direct_anthropic_chat_models");
    assert_eq!(d.background.effort, "direct_anthropic_background_effort");
    assert_eq!(d.key_check, "direct_anthropic_key_check");
    assert!(d.shares_data.is_none() && !d.tiers);
    assert!(direct_providers::PROVIDERS
        .iter()
        .all(|p| direct_models::section(p.provider).is_some()));
}

// ---- loopback: the request and the stream through the real parser ------------------------------

fn prepared(model: &str, role: Role, effort: Option<&str>, list: &[ChatMessage]) -> Prepared {
    prepare(&arm(model, Some(role), effort), list, Some(0), None).unwrap()
}

fn user_list() -> Vec<ChatMessage> {
    vec![msg("system", "rule"), msg("user", "Count to five")]
}

type Waits = Arc<Mutex<Vec<Duration>>>;

/// `run` against a loopback URL, recording every wait it would have slept instead of sleeping.
async fn run_at(url: &str, p: &Prepared, tokens: &mut Vec<String>) -> (Result<Completion>, Waits) {
    let waits: Waits = Arc::default();
    let rec = waits.clone();
    let result = run(
        url,
        KEY,
        p,
        |t| tokens.push(t.to_string()),
        move |d| {
            rec.lock().unwrap().push(d);
            async {}
        },
    )
    .await;
    (result, waits)
}

fn chunks(text: &str, size: usize) -> Vec<Vec<u8>> {
    text.as_bytes().chunks(size).map(<[u8]>::to_vec).collect()
}

#[tokio::test]
async fn a_real_stream_goes_through_the_real_parser_and_the_request_is_the_safe_core() {
    let (base, seen) = serve(vec![Reply::Sse(chunks(STREAM_HAIKU, 97))]);
    let p = prepared(HAIKU, Role::Background, Some("low"), &user_list());
    let mut tokens = Vec::new();
    let (c, waits) = run_at(&format!("{base}/v1/messages"), &p, &mut tokens).await;
    let c = c.unwrap();
    assert_eq!(c.text, "1\n2\n3\n4\n5");
    assert_eq!(tokens.concat(), "1\n2\n3\n4\n5");
    assert_eq!(c.usage.completion_tokens, Some(11));
    assert!(waits.lock().unwrap().is_empty());

    let reqs = seen.lock().unwrap();
    assert_eq!(reqs.len(), 1);
    let r = &reqs[0];
    assert_eq!(r.path, "/v1/messages");
    assert_eq!(r.header("x-api-key"), Some(KEY));
    assert_eq!(r.header("anthropic-version"), Some("2026-10-21"));
    assert_eq!(r.header("accept"), Some("text/event-stream"));
    // None of the headers PM never sends (built from parts so this file can't match itself).
    for banned in [
        "authorization",
        concat!("ori", "gin"),
        concat!("user-", "agent"),
        concat!("anthropic-", "beta"),
        concat!("anthropic-", "workspace-id"),
        concat!("http-", "referer"),
        concat!("x-", "title"),
    ] {
        assert!(r.header(banned).is_none(), "{banned} was sent");
    }
    let sent = r.json();
    assert_eq!(sent["model"], "claude-haiku-5-5");
    assert_eq!(sent["stream"], true);
    assert_eq!(sent["thinking"], json!({"type":"disabled"}));
    assert_eq!(sent["output_config"], json!({"effort":"low"}));
    assert_eq!(
        sent["system"][0]["cache_control"],
        json!({"type":"ephemeral"})
    );
}

#[tokio::test]
async fn a_character_split_across_two_network_chunks_arrives_whole() {
    let stream = start(
        "claude-sonnet-5-5",
        json!({"input_tokens":5,"output_tokens":1}),
    ) + &text_block(0, &["caf\u{e9}"])
        + &finish_with("end_turn", json!({"output_tokens":2}));
    // Cut the bytes inside the two-byte "e acute" so no chunk holds it whole.
    let bytes = stream.as_bytes();
    let at = bytes.windows(2).position(|w| w == [0xc3, 0xa9]).unwrap() + 1;
    let split = vec![bytes[..at].to_vec(), bytes[at..].to_vec()];
    let (base, _) = serve(vec![Reply::Sse(split)]);
    let p = prepared(SONNET, Role::Chat, None, &user_list());
    let mut tokens = Vec::new();
    let (c, _) = run_at(&format!("{base}/v1/messages"), &p, &mut tokens).await;
    assert_eq!(c.unwrap().text, "caf\u{e9}");
    assert_eq!(tokens.concat(), "caf\u{e9}");
}

#[tokio::test]
async fn a_dropped_connection_mid_body_and_a_clean_end_without_the_marker_both_fail() {
    let half = start("claude-sonnet-5-5", json!({"input_tokens":5})) + &text_block(0, &["half"]);
    // The connection is cut with bytes still promised: a transport error, partial dropped.
    let (base, _) = serve(vec![Reply::SseCut(chunks(&half, 64))]);
    let p = prepared(SONNET, Role::Chat, None, &user_list());
    let mut tokens = Vec::new();
    let (c, waits) = run_at(&format!("{base}/v1/messages"), &p, &mut tokens).await;
    assert!(c.is_err(), "a cut body is an error");
    assert!(
        waits.lock().unwrap().is_empty(),
        "a stream that began is never retried"
    );
    // A clean close with text and no `message_stop`: the cut-off sentence.
    let (base, _) = serve(vec![Reply::Sse(chunks(&half, 64))]);
    let mut tokens = Vec::new();
    let (c, _) = run_at(&format!("{base}/v1/messages"), &p, &mut tokens).await;
    assert!(c
        .err()
        .unwrap()
        .to_string()
        .contains("cut off before the end"));
}

#[tokio::test]
async fn an_oversized_line_and_an_oversized_reply_are_errors() {
    // A line with no newline past the cap.
    let huge = vec![b'x'; MAX_SSE_LINE_BYTES + 10];
    let (base, _) = serve(vec![Reply::Sse(vec![b"data: ".to_vec(), huge])]);
    let p = prepared(SONNET, Role::Chat, None, &user_list());
    let mut tokens = Vec::new();
    let (c, _) = run_at(&format!("{base}/v1/messages"), &p, &mut tokens).await;
    assert!(c.err().unwrap().to_string().contains("oversized line"));
    // A reply past the size limit, one delta at a time.
    let big = "y".repeat(1024 * 1024);
    let mut stream = start("claude-sonnet-5-5", json!({"input_tokens":5}));
    stream += &ev(
        json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
    );
    for _ in 0..9 {
        stream += &ev(
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":big}}),
        );
    }
    let (base, _) = serve(vec![Reply::Sse(chunks(&stream, 256 * 1024))]);
    let mut tokens = Vec::new();
    let (c, _) = run_at(&format!("{base}/v1/messages"), &p, &mut tokens).await;
    assert!(c.err().unwrap().to_string().contains("size limit"));
}

#[tokio::test]
async fn a_refused_request_is_the_mapped_sentence_and_never_shows_the_key() {
    for (status, raw, wants) in [
        (
            429,
            body_code("rate_limit_error", "cap", "enforced_spend_limit_reached"),
            "monthly spend cap",
        ),
        (
            400,
            body("invalid_request_error", "In order to access this model, your organization or workspace must have data retention enabled."),
            "30-day data retention",
        ),
    ] {
        let (base, seen) = serve(vec![Reply::Http(status, raw)]);
        let p = prepared(FABLE, Role::Chat, None, &user_list());
        let mut tokens = Vec::new();
        let (c, waits) = run_at(&format!("{base}/v1/messages"), &p, &mut tokens).await;
        let said = c.err().unwrap().to_string();
        assert!(said.contains(wants), "{said}");
        assert!(!said.contains(KEY));
        assert!(waits.lock().unwrap().is_empty(), "{status} is never retried");
        assert_eq!(seen.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn a_rate_limit_before_the_stream_gets_one_retry_and_a_second_failure_is_the_sentence() {
    // 429 then 200: waited retry-after seconds once, then read the stream.
    let (base, seen) = serve(vec![
        Reply::HttpWith(
            429,
            vec![("retry-after".into(), "5".into())],
            body("rate_limit_error", "slow down"),
        ),
        Reply::Sse(chunks(STREAM_HAIKU, 4096)),
    ]);
    let p = prepared(HAIKU, Role::Chat, None, &user_list());
    let mut tokens = Vec::new();
    let (c, waits) = run_at(&format!("{base}/v1/messages"), &p, &mut tokens).await;
    assert_eq!(c.unwrap().text, "1\n2\n3\n4\n5");
    assert_eq!(*waits.lock().unwrap(), [Duration::from_secs(5)]);
    assert_eq!(seen.lock().unwrap().len(), 2);

    // 529 then 529: exactly one retry, then the Overloaded sentence.
    let (base, seen) = serve(vec![
        Reply::Http(529, body("overloaded_error", "Overloaded")),
        Reply::Http(529, body("overloaded_error", "Overloaded")),
        Reply::Http(529, body("overloaded_error", "never asked")),
    ]);
    let mut tokens = Vec::new();
    let (c, waits) = run_at(&format!("{base}/v1/messages"), &p, &mut tokens).await;
    assert!(c.err().unwrap().to_string().contains("having trouble"));
    assert_eq!(waits.lock().unwrap().len(), 1);
    assert_eq!(seen.lock().unwrap().len(), 2);

    // retry-after 60 is capped at 20 seconds.
    let (base, _) = serve(vec![
        Reply::HttpWith(
            429,
            vec![("retry-after".into(), "60".into())],
            body("rate_limit_error", "slow down"),
        ),
        Reply::Sse(chunks(STREAM_HAIKU, 4096)),
    ]);
    let mut tokens = Vec::new();
    let (c, waits) = run_at(&format!("{base}/v1/messages"), &p, &mut tokens).await;
    assert!(c.is_ok());
    assert_eq!(*waits.lock().unwrap(), [Duration::from_secs(20)]);
}

/// reqwest strips only `Authorization` and `Cookie` across hosts, never `x-api-key`, so a redirect
/// would carry the key away. The second listener stands for "somewhere else": it must see nothing.
#[tokio::test]
async fn a_redirect_is_never_followed_so_the_key_never_leaves() {
    let elsewhere = || serve(vec![Reply::Http(200, "{}".into())]);
    let redirect = |to: &str| {
        Reply::HttpWith(
            307,
            vec![("location".into(), format!("{to}/v1/messages"))],
            "{}".into(),
        )
    };

    // The chat.
    let (other, other_seen) = elsewhere();
    let (base, seen) = serve(vec![redirect(&other)]);
    let p = prepared(HAIKU, Role::Chat, None, &user_list());
    let mut tokens = Vec::new();
    let (c, waits) = run_at(&format!("{base}/v1/messages"), &p, &mut tokens).await;
    let said = c.err().unwrap().to_string();
    assert!(!said.is_empty() && !said.contains(KEY), "{said}");
    assert_eq!(seen.lock().unwrap().len(), 1);
    assert!(waits.lock().unwrap().is_empty(), "a 3xx is not retried");
    assert!(
        other_seen.lock().unwrap().is_empty(),
        "the chat followed a redirect"
    );

    // The key check's listing, and its test chat.
    let (other, other_seen) = elsewhere();
    let (c, seen) = check(vec![redirect(&other)]).await;
    assert_ne!(c.outcome, KeyOutcome::Ok);
    assert_eq!(seen.lock().unwrap().len(), 1);
    assert!(
        other_seen.lock().unwrap().is_empty(),
        "the listing followed a redirect"
    );

    let (other, other_seen) = elsewhere();
    let (c, seen) = check(vec![listing(&WIRES), redirect(&other)]).await;
    // A 3xx is an answer PM can't read: the chat was not tried, never a verdict on the key.
    assert_eq!(
        (c.outcome, c.note),
        (KeyOutcome::Ok, Some(KeyNote::ChatUntried))
    );
    assert_eq!(seen.lock().unwrap().len(), 2);
    assert!(
        other_seen.lock().unwrap().is_empty(),
        "the test chat followed a redirect"
    );
}

/// Half a usage is not a cost: an output count with no input fields would price the prompt as 0.
#[test]
fn a_usage_with_only_one_side_has_no_cost_never_a_confident_undercount() {
    let output_only = start("claude-sonnet-5-5", json!({}))
        + &text_block(0, &["x"])
        + &finish_with("end_turn", json!({"output_tokens":3}));
    let (c, _) = ok(SONNET, &output_only);
    assert_eq!(c.usage.completion_tokens, Some(3));
    assert_eq!(settle(c, row(SONNET)).usage.cost, None);

    let input_only = start("claude-sonnet-5-5", json!({"input_tokens":5}))
        + &text_block(0, &["x"])
        + &ev(json!({"type":"message_stop"}));
    let (c, _) = ok(SONNET, &input_only);
    assert_eq!(c.usage.prompt_tokens, Some(5));
    assert_eq!(settle(c, row(SONNET)).usage.cost, None);

    // Both sides present still price.
    let both = start("claude-sonnet-5-5", json!({"input_tokens":5}))
        + &text_block(0, &["x"])
        + &finish_with("end_turn", json!({"output_tokens":3}));
    let (c, _) = ok(SONNET, &both);
    assert!(settle(c, row(SONNET)).usage.cost.is_some());
}

/// `message_start`'s output count is a placeholder (8 before any text). A stream that ends with
/// `message_stop` but no `message_delta` has no real output count, so it must read as unknown cost, not
/// as a priced 8-token reply.
#[test]
fn a_stream_with_no_message_delta_has_no_output_count_and_no_cost() {
    let no_delta = start(
        "claude-sonnet-5-5",
        json!({"input_tokens":5,"output_tokens":8}),
    ) + &text_block(0, &["x"])
        + &ev(json!({"type":"message_stop"}));
    let (c, _) = ok(SONNET, &no_delta);
    assert_eq!(c.usage.prompt_tokens, Some(5));
    assert_eq!(
        c.usage.completion_tokens, None,
        "the placeholder is not a count"
    );
    assert_eq!(settle(c, row(SONNET)).usage.cost, None);

    // With the delta, the same start prices on the delta's count, never the placeholder's.
    let with_delta = start(
        "claude-sonnet-5-5",
        json!({"input_tokens":5,"output_tokens":8}),
    ) + &text_block(0, &["x"])
        + &finish_with("end_turn", json!({"output_tokens":3}));
    let (c, _) = ok(SONNET, &with_delta);
    assert_eq!(c.usage.completion_tokens, Some(3));
    assert!(settle(c, row(SONNET)).usage.cost.is_some());
}

/// PM asks only for the five-minute cache lifetime and has no price for a one-hour write (2x, not
/// 1.25x). A reply that reports one would be under-priced, so its cost is unknown instead.
#[test]
fn a_one_hour_cache_write_makes_the_cost_unknown_never_under_priced() {
    let hour = |one_hour: i64| {
        start(
            "claude-sonnet-5-5",
            json!({
                "input_tokens":5,
                "cache_creation_input_tokens":1000,
                "cache_creation":{"ephemeral_5m_input_tokens":1000 - one_hour,"ephemeral_1h_input_tokens":one_hour},
            }),
        ) + &text_block(0, &["x"])
            + &finish_with("end_turn", json!({"output_tokens":3}))
    };
    // Five-minute writes only (what PM asks for): priced, as the live probe showed.
    let (c, _) = ok(SONNET, &hour(0));
    assert!(!c.usage.output_partial);
    assert!(settle(c, row(SONNET)).usage.cost.is_some());
    // Any one-hour part: the tokens still show, the dollar figure is a dash.
    let (c, _) = ok(SONNET, &hour(400));
    assert_eq!(c.usage.prompt_tokens, Some(1005));
    assert_eq!(c.usage.completion_tokens, Some(3));
    assert_eq!(settle(c, row(SONNET)).usage.cost, None);

    // The same when the figure arrives on the delta, not the start.
    let late = start(
        "claude-sonnet-5-5",
        json!({"input_tokens":5,"cache_creation_input_tokens":1000}),
    ) + &text_block(0, &["x"])
        + &finish_with(
            "end_turn",
            json!({"output_tokens":3,"cache_creation":{"ephemeral_1h_input_tokens":1000}}),
        );
    let (c, _) = ok(SONNET, &late);
    assert_eq!(settle(c, row(SONNET)).usage.cost, None);
}

/// A mid-stream error event arrives after the 200, so the request is billed and PM never retries it.
#[tokio::test]
async fn an_error_event_in_the_stream_is_never_retried() {
    let stream = start("claude-sonnet-5-5", json!({"input_tokens":5}))
        + &text_block(0, &["x"])
        + &ev(json!({"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}));
    let (base, seen) = serve(vec![
        Reply::Sse(chunks(&stream, 128)),
        Reply::Sse(chunks(STREAM_HAIKU, 128)),
    ]);
    let p = prepared(SONNET, Role::Chat, None, &user_list());
    let mut tokens = Vec::new();
    let (c, waits) = run_at(&format!("{base}/v1/messages"), &p, &mut tokens).await;
    assert!(c.is_err());
    assert!(waits.lock().unwrap().is_empty());
    assert_eq!(seen.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_refusal_over_the_wire_is_ok_with_its_usage_so_the_spend_is_booked() {
    let stream = start(
        "claude-sonnet-5-5",
        json!({"input_tokens":50,"output_tokens":1}),
    ) + &text_block(0, &["Here is"])
        + &finish_with("refusal", json!({"output_tokens":7}));
    let (base, _) = serve(vec![Reply::Sse(chunks(&stream, 64))]);
    let p = prepared(SONNET, Role::Chat, None, &user_list());
    let mut tokens = Vec::new();
    let (c, _) = run_at(&format!("{base}/v1/messages"), &p, &mut tokens).await;
    let c = settle(c.unwrap(), p.model);
    assert!(c.usage.refused && c.text.is_empty());
    assert!(c.usage.cost.is_some());
    assert_eq!(tokens, ["Here is"]);
}

// ---- loopback: the key check ---------------------------------------------------------------------

fn urls_at(base: &str) -> CheckUrls {
    let leak = |tail: &str| -> &'static str { Box::leak(format!("{base}{tail}").into_boxed_str()) };
    CheckUrls {
        models: leak("/v1/models?limit=1000"),
        chat: leak("/v1/messages"),
    }
}

fn listing(wires: &[&str]) -> Reply {
    let data: Vec<Value> = wires
        .iter()
        .map(|w| json!({"type":"model","id":w,"lifecycle":"active"}))
        .collect();
    Reply::Http(200, json!({"data": data, "has_more": false}).to_string())
}

const WIRES: [&str; 4] = [
    "claude-fable-5-2",
    "claude-haiku-5-5",
    "claude-sonnet-5-5",
    "claude-opus-5-5",
];

async fn check(replies: Vec<Reply>) -> (KeyCheck, crate::direct_test_support::Seen) {
    let (base, seen) = serve(replies);
    (check_key_at(KEY, &urls_at(&base)).await, seen)
}

fn paths(seen: &crate::direct_test_support::Seen) -> Vec<String> {
    seen.lock()
        .unwrap()
        .iter()
        .map(|r| r.path.clone())
        .collect()
}

fn chat_ok() -> Reply {
    Reply::Http(200, HAIKU_LOW_200.to_string())
}

#[tokio::test]
async fn a_good_key_lists_four_and_one_test_chat_proves_it() {
    let (c, seen) = check(vec![listing(&WIRES), chat_ok()]).await;
    assert_eq!(c.outcome, KeyOutcome::Ok);
    assert_eq!(c.note, None);
    assert_eq!(c.listed.len(), 4);
    assert_eq!(c.listed_total, 4);
    assert_eq!(paths(&seen), ["/v1/models?limit=1000", "/v1/messages"]);
    let reqs = seen.lock().unwrap();
    // Both requests carry the key as x-api-key and the version, and nothing else of PM's.
    for r in reqs.iter() {
        assert_eq!(r.header("x-api-key"), Some(KEY));
        assert_eq!(r.header("anthropic-version"), Some("2026-10-21"));
        assert!(r.header("authorization").is_none());
        assert!(r.header(concat!("ori", "gin")).is_none());
    }
    // The chat was a POST of the probe body on Haiku.
    assert_eq!(reqs[1].json(), probe_body(row(HAIKU)));
}

#[tokio::test]
async fn no_credit_keeps_the_key_and_says_so_without_a_note() {
    let (c, _) = check(vec![
        listing(&WIRES),
        Reply::Http(400, body("invalid_request_error", CREDIT_TEXT)),
    ])
    .await;
    assert_eq!(c.outcome, KeyOutcome::NoCredit);
    assert!(c.outcome.keeps_key());
    assert_eq!(c.listed.len(), 4);
    assert_eq!(c.note, None);
}

#[tokio::test]
async fn a_key_that_lists_but_cannot_chat_is_read_only_and_one_that_cannot_list_has_no_access() {
    let forbidden = body(
        "permission_error",
        "Your API key does not have permission to use the specified resource.",
    );
    let (c, _) = check(vec![listing(&WIRES), Reply::Http(403, forbidden.clone())]).await;
    assert_eq!(
        (c.outcome, c.note),
        (KeyOutcome::Restricted, Some(KeyNote::ReadOnly))
    );
    let (c, _) = check(vec![
        Reply::Http(403, forbidden.clone()),
        Reply::Http(403, forbidden.clone()),
    ])
    .await;
    assert_eq!(
        (c.outcome, c.note),
        (KeyOutcome::Restricted, Some(KeyNote::NoAccess))
    );
    assert!(c.listed.is_empty());
    // A key whose listing is refused but whose chat works is restricted with no note.
    let (c, _) = check(vec![Reply::Http(403, forbidden), chat_ok()]).await;
    assert_eq!((c.outcome, c.note), (KeyOutcome::Restricted, None));
}

#[tokio::test]
async fn a_chat_that_could_not_be_tried_never_fails_the_key() {
    for second in [
        Reply::Http(429, body("rate_limit_error", "slow")),
        Reply::Http(529, body("overloaded_error", "x")),
        Reply::Drop,
    ] {
        let (c, seen) = check(vec![listing(&WIRES), second]).await;
        assert_eq!(c.outcome, KeyOutcome::Ok);
        assert_eq!(c.note, Some(KeyNote::ChatUntried));
        assert_eq!(paths(&seen).len(), 2, "the test chat is never retried");
    }
}

#[tokio::test]
async fn a_cheapest_model_that_is_gone_moves_on_to_the_next_and_never_to_fable() {
    let (c, seen) = check(vec![
        listing(&WIRES),
        Reply::Http(404, body("not_found_error", "model: claude-haiku-5-5")),
        chat_ok(),
    ])
    .await;
    assert_eq!((c.outcome, c.note), (KeyOutcome::Ok, None));
    {
        let reqs = seen.lock().unwrap();
        assert_eq!(reqs.len(), 3);
        assert_eq!(reqs[1].json()["model"], "claude-haiku-5-5");
        assert_eq!(reqs[2].json()["model"], "claude-sonnet-5-5");
    }
    // Both tries gone: the listing's verdict stands, untried, and Fable was never asked.
    let (c, seen) = check(vec![
        listing(&WIRES),
        Reply::Http(404, body("not_found_error", "gone")),
        Reply::Http(404, body("not_found_error", "gone")),
    ])
    .await;
    assert_eq!(
        (c.outcome, c.note),
        (KeyOutcome::Ok, Some(KeyNote::ChatUntried))
    );
    assert!(!seen
        .lock()
        .unwrap()
        .iter()
        .any(|r| r.body.contains("fable")));
}

#[tokio::test]
async fn a_rejected_unreachable_or_odd_listing_never_gets_a_test_chat() {
    for (listing_reply, outcome) in [
        (
            Reply::Http(401, body("authentication_error", "invalid x-api-key")),
            KeyOutcome::Rejected,
        ),
        (
            Reply::Http(529, body("overloaded_error", "x")),
            KeyOutcome::Busy,
        ),
        (Reply::Http(418, String::new()), KeyOutcome::Unexpected(418)),
        (
            Reply::Http(401, "<html>proxy</html>".into()),
            KeyOutcome::Unexpected(401),
        ),
        (Reply::Drop, KeyOutcome::Unreachable),
    ] {
        let (c, seen) = check(vec![listing_reply]).await;
        assert_eq!(c.outcome, outcome);
        assert!(!c.outcome.keeps_key() || outcome == KeyOutcome::NoCredit);
        assert_eq!(
            seen.lock().unwrap().len(),
            1,
            "{outcome:?}: no chat was sent"
        );
    }
}

#[tokio::test]
async fn a_key_that_does_not_list_haiku_is_probed_on_the_next_cheapest() {
    let (c, seen) = check(vec![
        listing(&["claude-sonnet-5-5", "claude-opus-5-5", "claude-mythos-5-2"]),
        chat_ok(),
    ])
    .await;
    assert_eq!(c.outcome, KeyOutcome::Ok);
    // In the catalogue's order (dearest first), not the probe's.
    assert_eq!(c.listed, vec![OPUS.to_string(), SONNET.to_string()]);
    assert_eq!(c.listed_total, 3, "Mythos counts in the total only");
    assert_eq!(seen.lock().unwrap()[1].json()["model"], "claude-sonnet-5-5");
}

#[tokio::test]
async fn a_key_that_lists_only_fable_probes_fable() {
    let (c, seen) = check(vec![listing(&["claude-fable-5-2"]), chat_ok()]).await;
    assert_eq!(c.outcome, KeyOutcome::Ok);
    assert_eq!(seen.lock().unwrap()[1].json()["model"], "claude-fable-5-2");
}

// ---- scans of this file's own text ---------------------------------------------------------------

/// This file's code, without its tests.
fn live_source() -> &'static str {
    include_str!("anthropic_direct.rs")
        .split("#[cfg(test)]\n#[path")
        .next()
        .unwrap()
}

fn function<'a>(src: &'a str, head: &str) -> &'a str {
    let at = src.find(head).unwrap_or_else(|| panic!("{head} not found"));
    let rest = &src[at + head.len()..];
    let end = rest
        .find("\nfn ")
        .or_else(|| rest.find("\nasync fn "))
        .or_else(|| rest.find("\npub(crate) "))
        .unwrap_or(rest.len());
    &rest[..end]
}

/// Every paid request announces itself on the wire mark (a plain `.send()` would make every failure
/// before the wire look unsent, free, to a mod's booking), and only the key check's two requests are
/// plain sends: they belong to no mod.
#[test]
fn the_anthropic_paid_request_goes_through_the_mark() {
    let src = live_source();
    let retry = function(src, "async fn send_with_one_retry<S, Fut>(");
    assert!(retry.contains("crate::wire_mark::send("));
    assert!(!retry.contains(".send()"));
    let run_fn = function(src, "async fn run<F, S, Fut>(");
    assert!(run_fn.contains("send_with_one_retry("));
    assert!(!run_fn.contains(".send()"));
    let complete = function(src, "pub(crate) async fn complete(");
    let stream = function(src, "pub(crate) async fn stream_chat<F: FnMut(&str)>(");
    assert!(complete.contains("run(") && stream.contains("run("));
    assert!(!complete.contains(".send()") && !stream.contains(".send()"));
    // The two plain sends of the whole file sit in the key check.
    assert_eq!(src.matches(".send()").count(), 2);
    assert!(function(src, "async fn send_probe(").contains(".send()"));
    assert!(function(src, "async fn check_key_at(").contains(".send()"));
    // Every request goes through this file's own no-redirect clients (a redirect would carry the
    // custom `x-api-key` header away), never the shared ones, which follow redirects.
    assert!(!src.contains(concat!("cloud_openai::", "http()")));
    assert!(!src.contains(concat!("cloud_openai::", "patient_http()")));
    assert!(function(src, "fn no_redirect_client(").contains("Policy::none()"));
}

/// The file names no sampling, fallback, prefill or beta field: the matrix test is the behavioural
/// proof, this stops a later edit adding one back. Names built from parts so this can't match itself,
/// and read as quoted JSON keys.
#[test]
fn anthropic_direct_names_no_sampling_fallback_beta_or_origin() {
    let src = live_source().to_ascii_lowercase();
    for needle in [
        concat!("\"temper", "ature\""),
        concat!("\"top", "_p\""),
        concat!("\"top", "_k\""),
        concat!("\"fall", "backs\""),
        concat!("\"budget", "_tokens\""),
        concat!("\"pre", "fill\""),
        concat!("anthropic-", "beta"),
        concat!("\"ori", "gin"),
        concat!("\"author", "ization"),
        concat!("\"user", "-agent"),
    ] {
        assert!(!src.contains(needle), "{needle} is in the live source");
    }
}

/// Only the one `thinking` form ever leaves PM, and `cache_control` is written in one place.
#[test]
fn the_wire_has_one_thinking_form_and_one_cache_mark() {
    let src = live_source();
    // Written to a body in one place each (`body[..] = ..`); the phrase scan in `classify` reads the
    // word `thinking` but never writes it.
    assert_eq!(src.matches("[\"thinking\"] =").count(), 1);
    assert_eq!(src.matches("[\"cache_control\"] =").count(), 2);
    assert_eq!(src.matches("fn ephemeral()").count(), 1);
}
