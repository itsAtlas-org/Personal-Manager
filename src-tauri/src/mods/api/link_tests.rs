// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `shell.openLink` (PR C, K2): which addresses a mod may open (the URL table), that the PARSED
//! address is the only thing shown or opened, who is asked each time (the confirm policy), and what
//! the question shows.

use serde_json::{json, Value};

use super::call_fixtures::*;
use super::confirm::complete;
use super::link_calls::{check_link, question_for_test};
use super::permissions::{self, Class, PolicyTier};
use super::test_support::TestHost;
use super::{limits, Code, ModError, Reply};

const LINKS: &[&str] = &["links"];

fn accepted(raw: &str) -> String {
    check_link(raw)
        .unwrap_or_else(|e| panic!("{raw:?} was refused: {e}"))
        .to_string()
}

fn refused(raw: &str) {
    let e = check_link(raw).expect_err(&format!("{raw:?} was accepted"));
    assert_eq!(e.code, Code::BadArgs, "{raw:?}");
    // One sentence for every refusal: PM's own, which never repeats the address.
    assert_eq!(
        e.message, "A mod may open only an https:// address to a website.",
        "{raw:?}"
    );
}

/// The table: what a mod may open, and the tricks it may not.
#[test]
fn the_url_table() {
    // Accepted, in the parsed form (lower-cased host, punycode, default port and the root path).
    for (raw, shown) in [
        ("https://example.com", "https://example.com/"),
        (
            "https://example.com/a/b?c=d#e",
            "https://example.com/a/b?c=d#e",
        ),
        ("HTTPS://EXAMPLE.COM/Path", "https://example.com/Path"),
        ("https://example.com:443/x", "https://example.com/x"),
        (
            "https://sub.example.co.uk:8443/x",
            "https://sub.example.co.uk:8443/x",
        ),
        (
            "https://b\u{fc}cher.example/",
            "https://xn--bcher-kva.example/",
        ),
        ("https://example.com./x", "https://example.com./x"),
        // An empty user name and password say nothing: the parsed address has no userinfo at all, and
        // that is what is shown and opened.
        ("https://@example.com/", "https://example.com/"),
        ("https://:@example.com/", "https://example.com/"),
        // A backslash is a slash to the parser, so this is a path on example.com, and that is what
        // is shown and opened, not a way to say evil.com.
        (
            "https://example.com\\@evil.com/",
            "https://example.com/@evil.com/",
        ),
    ] {
        assert_eq!(accepted(raw), shown, "{raw}");
    }
    // Refused. Userinfo: the host before the @ is not the host.
    for raw in [
        "https://user@example.com/",
        "https://trusted.com@evil.com/",
        "https://user:password@example.com/",
        "https://example.com:443@evil.com/",
    ] {
        refused(raw);
    }
    // Refused. An IP address in any spelling the parser reads as one.
    for raw in [
        "https://127.0.0.1/",
        "https://127.0.0.1:443/",
        "https://10.0.0.1/",
        "https://192.168.1.1/",
        "https://169.254.169.254/latest/meta-data",
        "https://93.184.216.34/",
        "https://2130706433/",
        "https://0x7f.1/",
        "https://0177.0.0.1/",
        "https://127.1/",
        "https://[::1]/",
        "https://[::ffff:127.0.0.1]/",
        "https://[2001:db8::1]:8443/",
        "https://0/",
    ] {
        refused(raw);
    }
    // Refused. localhost, and a name with one label.
    for raw in [
        "https://localhost/",
        "https://LOCALHOST/",
        "https://localhost./",
        "https://app.localhost/",
        "https://intranet/",
        "https://intranet./",
        "https://nas/share",
    ] {
        refused(raw);
    }
    // Refused. Port tricks.
    for raw in [
        "https://example.com:99999/",
        "https://example.com:-1/",
        "https://example.com:abc/",
        "https://example.com:443:443/",
    ] {
        refused(raw);
    }
    // Refused. Other schemes, and no scheme.
    for raw in [
        "http://example.com/",
        "mailto:someone@example.com",
        "file:///etc/passwd",
        "javascript:alert(1)",
        "data:text/html,<p>x</p>",
        "ftp://example.com/",
        "ws://example.com/",
        "//example.com/",
        "example.com",
        "/path",
        "",
        "https:",
        "https://",
    ] {
        refused(raw);
    }
    // Refused. Control characters, hidden spaces, and the padding the parser would silently trim.
    for raw in [
        "https://exa\tmple.com/",
        "https://example.com/\n",
        "https://example.com/a\u{0}b",
        "https://example.com/a\rb",
        " https://example.com/",
        "https://example.com/ ",
        "\u{7f}https://example.com/",
    ] {
        refused(raw);
    }
    // Refused. Too long: past 2,048 bytes.
    let at_limit = format!(
        "https://example.com/{}",
        "a".repeat(limits::LINK_MAX_BYTES - 20)
    );
    assert_eq!(at_limit.len(), limits::LINK_MAX_BYTES);
    accepted(&at_limit);
    refused(&format!("{at_limit}a"));
}

/// The parsed form re-parses to itself, so what PM checks at the person's yes is exactly what it
/// showed at the question.
#[test]
fn the_parsed_address_parses_to_itself() {
    for raw in [
        "HTTPS://EXAMPLE.COM:443/a b?c d#e f",
        "https://b\u{fc}cher.example/\u{e9}",
        "https://example.com\\@evil.com/",
        "https://example.com/%2e%2e/x",
    ] {
        let once = accepted(raw);
        assert_eq!(accepted(&once), once, "{raw}");
    }
}

/// L7: an unsigned mod may read your data or reach outside, never both; `links` is outside.
#[test]
fn opening_a_link_counts_as_reaching_the_internet() {
    let links = permissions::policy("links").unwrap();
    assert_eq!(links.class, Class::Egress);
    assert_eq!(permissions::policy("internet").unwrap().class, links.class);
    // An unsigned mod holding a read cannot also hold `links`, and the other way round.
    assert!(permissions::excluded(
        PolicyTier::Unsigned,
        Class::Egress,
        [(PolicyTier::Unsigned, Class::Read)]
    ));
    assert!(permissions::excluded(
        PolicyTier::Unsigned,
        Class::Read,
        [(PolicyTier::Unsigned, Class::Egress)]
    ));
    // The row is `links`, class egress.
    let row = super::table::ROWS
        .iter()
        .find(|r| r.name == "shell.openLink")
        .unwrap();
    assert_eq!((row.permission, row.class), (Some("links"), Class::Egress));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unsigned_mod_that_reads_your_data_may_not_open_links() {
    let (host, s) = unsigned("acme.opener", "third", &["links", "tasks.read"]);
    let e = error(
        call(
            &host,
            &s,
            "shell.openLink",
            json!({"url": "https://example.com/"}),
        )
        .await,
    );
    assert_eq!(e.code, Code::NotGranted);
    assert!(host.opened_links().is_empty());
    // Links alone: asked each time, never opened unasked.
    let (host, s) = unsigned("acme.opener", "third", LINKS);
    let e = error(
        call(
            &host,
            &s,
            "shell.openLink",
            json!({"url": "https://example.com/"}),
        )
        .await,
    );
    assert_eq!(e.code, Code::NeedsConfirmation);
    assert!(host.opened_links().is_empty());
}

// --- who is asked -------------------------------------------------------------------------------

fn token(e: &ModError) -> String {
    assert_eq!(e.code, Code::NeedsConfirmation, "{e}");
    e.confirm.as_ref().expect("a question").token.clone()
}

async fn answer(host: &TestHost, token: &str, accept: bool) -> Result<Value, ModError> {
    match complete(host, "main", token, accept).await {
        Reply::Value { value } => Ok(value),
        Reply::Error { error } => Err(error),
        other => panic!("{other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_first_party_mod_opens_a_link_at_once_with_no_question() {
    let (host, s) = first("atlas.pinboard", LINKS);
    let v = value(
        call(
            &host,
            &s,
            "shell.openLink",
            json!({"url": "HTTPS://Example.COM/a?b=c"}),
        )
        .await,
    );
    assert_eq!(v, json!({"opened": true}));
    // What opened is the parsed address, once.
    assert_eq!(host.opened_links(), ["https://example.com/a?b=c"]);
    // A refused address opens nothing and is no question.
    assert_eq!(
        code(
            call(
                &host,
                &s,
                "shell.openLink",
                json!({"url": "http://example.com/"})
            )
            .await
        ),
        Code::BadArgs
    );
    assert_eq!(host.opened_links().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_link_call_with_extra_or_missing_fields_is_refused() {
    // Each on its own mod: a refused call still counts against the family's burst.
    for bad in [
        json!({}),
        json!({"url": "https://example.com/", "target": "x"}),
        json!({"url": 3}),
    ] {
        let (host, s) = first("atlas.pinboard", LINKS);
        assert_eq!(
            code(call(&host, &s, "shell.openLink", bad).await),
            Code::BadArgs
        );
        assert!(host.opened_links().is_empty());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_party_or_unsigned_mod_asks_every_time_and_the_yes_opens_the_parsed_address() {
    let makers: [(&str, MakeFixed); 3] = [
        ("second party", || second("acme.opener", LINKS)),
        ("unsigned", || unsigned("acme.opener", "third", LINKS)),
        // The unsigned dev-folder atlas.pinboard is NOT the first-party Pinboard.
        ("unsigned atlas.pinboard", || {
            unsigned("atlas.pinboard", "first", LINKS)
        }),
    ];
    for (who, make) in makers {
        let (host, s) = make();
        let raw = "HTTPS://Example.COM:443/Path\\x?q=1";
        let e = error(call(&host, &s, "shell.openLink", json!({"url": raw})).await);
        let t = token(&e);
        let shown = "https://example.com/Path/x?q=1";
        assert_eq!(
            e.confirm.clone().unwrap().summary,
            format!("Open this address in your browser?\n{shown}"),
            "{who}"
        );
        // Nothing opened until the yes, and the yes opens the parsed address exactly once.
        assert!(host.opened_links().is_empty(), "{who}");
        assert_eq!(
            answer(&host, &t, true).await.unwrap(),
            json!({"opened": true})
        );
        assert_eq!(host.opened_links(), [shown], "{who}");
        // A token is used once.
        assert_eq!(
            answer(&host, &t, true).await.unwrap_err().code,
            Code::Declined
        );
        assert_eq!(host.opened_links().len(), 1, "{who}");
        // The next link is a new question, even to the same address.
        let e = error(call(&host, &s, "shell.openLink", json!({"url": raw})).await);
        assert_eq!(e.code, Code::NeedsConfirmation, "{who}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_no_opens_nothing_and_a_yes_after_the_session_ended_does_nothing() {
    let (host, s) = second("acme.opener", LINKS);
    let t = token(&error(
        call(
            &host,
            &s,
            "shell.openLink",
            json!({"url": "https://example.com/"}),
        )
        .await,
    ));
    assert_eq!(
        answer(&host, &t, false).await.unwrap_err().code,
        Code::Declined
    );
    assert!(host.opened_links().is_empty());
    let t = token(&error(
        call(
            &host,
            &s,
            "shell.openLink",
            json!({"url": "https://example.com/"}),
        )
        .await,
    ));
    host.runtime().close(&s);
    assert_eq!(
        answer(&host, &t, true).await.unwrap_err().code,
        Code::Declined
    );
    assert!(host.opened_links().is_empty());
}

/// The question shows the whole address, not only the host: a path or a query is how data leaves.
#[test]
fn the_question_shows_the_full_address_clamped_at_300_characters_with_how_many_more() {
    let short = "https://example.com/a?b=c";
    assert_eq!(
        question_for_test(short),
        format!("Open this address in your browser?\n{short}")
    );
    let exactly = format!("https://example.com/{}", "a".repeat(300 - 20));
    assert_eq!(exactly.chars().count(), 300);
    assert!(!question_for_test(&exactly).contains("more characters"));
    let long = format!("https://example.com/{}", "b".repeat(1000));
    let total = long.chars().count();
    let q = question_for_test(&long);
    let mut lines = q.lines();
    assert_eq!(lines.next(), Some("Open this address in your browser?"));
    let shown = lines.next().unwrap();
    assert_eq!(shown.chars().count(), 300);
    assert!(long.starts_with(shown));
    assert_eq!(
        lines.next(),
        Some(format!("...and {} more characters", total - 300).as_str())
    );
    // Counted in characters, not bytes: an international address is not cut mid-letter.
    let wide = format!("https://example.com/{}", "\u{e9}".repeat(400));
    let q = question_for_test(&wide);
    assert_eq!(q.lines().nth(1).unwrap().chars().count(), 300);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_question_of_a_long_address_reaches_the_mod_as_the_full_clamped_text() {
    let (host, s) = second("acme.opener", LINKS);
    let long = format!("https://example.com/{}", "q".repeat(900));
    let e = error(call(&host, &s, "shell.openLink", json!({"url": long})).await);
    let summary = e.confirm.unwrap().summary;
    assert!(summary.contains("...and 620 more characters"), "{summary}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn links_are_rate_limited_by_their_family() {
    let (host, s) = first("atlas.pinboard", LINKS);
    for n in 0..limits::LINK_BURST {
        value(
            call(
                &host,
                &s,
                "shell.openLink",
                json!({"url": format!("https://example.com/{n}")}),
            )
            .await,
        );
    }
    let e = error(
        call(
            &host,
            &s,
            "shell.openLink",
            json!({"url": "https://example.com/x"}),
        )
        .await,
    );
    assert_eq!(e.code, Code::RateLimited);
    assert_eq!(host.opened_links().len() as u64, limits::LINK_BURST);
}

/// The yes runs the rules again on the stored address: an address that would no longer be allowed is
/// never opened, whatever token or question it came from.
#[test]
fn the_address_is_checked_again_when_the_person_says_yes() {
    use super::link_calls::{perform, read};
    use super::Authorised;
    let (host, s) = second("acme.opener", LINKS);
    let auth = Authorised {
        ticket: host.runtime().begin_call(&s).unwrap(),
        row: super::table::ROWS
            .iter()
            .find(|r| r.name == "shell.openLink")
            .unwrap(),
    };
    for stored in [
        "http://example.com/",
        "https://127.0.0.1/",
        "https://user@example.com/",
    ] {
        assert_eq!(read(stored).unwrap_err().code, Code::BadArgs, "{stored}");
        assert_eq!(
            perform(&host, &auth, stored).unwrap_err().code,
            Code::BadArgs,
            "{stored}"
        );
    }
    assert!(host.opened_links().is_empty());
    assert_eq!(
        perform(&host, &auth, "https://example.com/").unwrap(),
        json!({"opened": true})
    );
}
