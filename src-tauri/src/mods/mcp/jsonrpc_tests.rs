// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The protocol's messages over a real store: the handshake, what gets no reply, what is refused, the
//! tool list (exactly five reads, filtered by scope) and the way a tool's failure is told.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::jsonrpc::{handle, MAX_LINE_BYTES, PROTOCOL_VERSIONS};
use super::test_support::{open_vault, pair_app, result_json, App};
use super::tools::TOOLS;
use super::SCOPES;
use crate::mods::api::test_support::TestHost;

fn rig(held: &[&str]) -> (TestHost, App) {
    let host = TestHost::new();
    let here = open_vault(&host);
    let app = pair_app(&host, &here, held);
    (host, app)
}

async fn line(host: &TestHost, app: &App, text: &str) -> Option<Value> {
    handle(host, app.bound, text)
        .await
        .map(|reply| serde_json::from_str(&reply).expect("a reply is one JSON line"))
}

async fn say(host: &TestHost, app: &App, message: Value) -> Value {
    line(host, app, &message.to_string())
        .await
        .expect("a reply")
}

fn code(reply: &Value) -> i64 {
    reply["error"]["code"].as_i64().expect("an error code")
}

fn names(reply: &Value) -> BTreeSet<String> {
    reply["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .collect()
}

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|n| (*n).to_owned()).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn initialize_answers_with_the_servers_shape_and_the_clients_version_when_known() {
    let (host, app) = rig(&["tasks.read"]);
    for version in PROTOCOL_VERSIONS {
        let reply = say(
            &host,
            &app,
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                   "params": {"protocolVersion": version, "capabilities": {}}}),
        )
        .await;
        assert_eq!(reply["jsonrpc"], "2.0");
        assert_eq!(reply["id"], 1);
        assert_eq!(reply["result"]["protocolVersion"], version);
        assert_eq!(reply["result"]["serverInfo"]["name"], "Personal Manager");
        // Tools and nothing else: no resources, prompts, logging or sampling.
        assert_eq!(reply["result"]["capabilities"], json!({"tools": {}}));
    }
    // A version PM doesn't know gets the newest it does; no params at all is fine.
    let reply = say(
        &host,
        &app,
        json!({"jsonrpc": "2.0", "id": "a", "method": "initialize",
               "params": {"protocolVersion": "1999-01-01"}}),
    )
    .await;
    assert_eq!(reply["id"], "a");
    assert_eq!(reply["result"]["protocolVersion"], PROTOCOL_VERSIONS[0]);
    let reply = say(
        &host,
        &app,
        json!({"jsonrpc": "2.0", "id": 2, "method": "initialize"}),
    )
    .await;
    assert_eq!(reply["result"]["protocolVersion"], PROTOCOL_VERSIONS[0]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn notifications_and_responses_get_no_reply() {
    let (host, app) = rig(&["tasks.read"]);
    for message in [
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 1}}),
        // Even a notification of a method PM doesn't have, or one that is a request elsewhere.
        json!({"jsonrpc": "2.0", "method": "nonsense"}),
        json!({"jsonrpc": "2.0", "method": "tools/list"}),
        // A response to something PM never asked.
        json!({"jsonrpc": "2.0", "id": 5, "result": {}}),
        json!({"jsonrpc": "2.0", "id": 5, "error": {"code": 1, "message": "x"}}),
    ] {
        assert_eq!(
            line(&host, &app, &message.to_string()).await,
            None,
            "{message}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ping_and_an_unknown_method() {
    let (host, app) = rig(&["tasks.read"]);
    let reply = say(
        &host,
        &app,
        json!({"jsonrpc": "2.0", "id": 7, "method": "ping"}),
    )
    .await;
    assert_eq!(
        (reply["id"].clone(), reply["result"].clone()),
        (json!(7), json!({}))
    );
    for method in [
        "resources/list",
        "prompts/list",
        "logging/setLevel",
        "tools/delete",
        "",
    ] {
        let reply = say(
            &host,
            &app,
            json!({"jsonrpc": "2.0", "id": 8, "method": method}),
        )
        .await;
        assert_eq!(code(&reply), -32601, "{method}");
        assert_eq!(reply["id"], 8);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn what_is_not_a_request_is_refused_in_the_protocols_words() {
    let (host, app) = rig(&["tasks.read"]);
    // Not JSON.
    for junk in ["", "not json", "{", "{\"jsonrpc\":"] {
        let reply = line(&host, &app, junk).await.unwrap();
        assert_eq!(
            (code(&reply), reply["id"].clone()),
            (-32700, Value::Null),
            "{junk:?}"
        );
    }
    // A batch: one line is one message.
    let reply = line(
        &host,
        &app,
        r#"[{"jsonrpc":"2.0","id":1,"method":"ping"},{"jsonrpc":"2.0","id":2,"method":"ping"}]"#,
    )
    .await
    .unwrap();
    assert_eq!(code(&reply), -32600);
    let reply = line(&host, &app, "[]").await.unwrap();
    assert_eq!(code(&reply), -32600);
    // Not an object, an id of the wrong kind, a method that isn't text, no method at all.
    for bad in [
        "7",
        "\"ping\"",
        "null",
        r#"{"jsonrpc":"2.0","id":1.5,"method":"ping"}"#,
        r#"{"jsonrpc":"2.0","id":null,"method":"ping"}"#,
        r#"{"jsonrpc":"2.0","id":true,"method":"ping"}"#,
        r#"{"jsonrpc":"2.0","id":{"a":1},"method":"ping"}"#,
        r#"{"jsonrpc":"2.0","id":1,"method":7}"#,
        r#"{"jsonrpc":"2.0","id":1}"#,
    ] {
        let reply = line(&host, &app, bad).await.unwrap();
        assert_eq!(code(&reply), -32600, "{bad}");
    }
    // Params that aren't an object.
    for params in [json!([1]), json!(5), json!("x"), json!(null), json!(true)] {
        let reply = say(
            &host,
            &app,
            json!({"jsonrpc": "2.0", "id": 3, "method": "tools/list", "params": params}),
        )
        .await;
        assert_eq!(code(&reply), -32602, "{params}");
        assert_eq!(reply["id"], 3);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_line_past_the_cap_is_refused_without_being_read() {
    let (host, app) = rig(&["tasks.read"]);
    let padding = "a".repeat(MAX_LINE_BYTES);
    let big = format!(r#"{{"jsonrpc":"2.0","id":1,"method":"ping","params":{{"x":"{padding}"}}}}"#);
    let reply = line(&host, &app, &big).await.unwrap();
    assert_eq!(code(&reply), -32600);
    // Just under it is read like any other.
    let ok = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"ping","params":{{"x":"{}"}}}}"#,
        "a".repeat(1000)
    );
    assert!(line(&host, &app, &ok)
        .await
        .unwrap()
        .get("result")
        .is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_tool_list_is_exactly_the_five_reads_and_nothing_that_writes() {
    let (host, app) = rig(&SCOPES);
    let reply = say(
        &host,
        &app,
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
    )
    .await;
    assert_eq!(
        names(&reply),
        set(&["search", "tasks", "events", "projects", "milestones"])
    );
    let write_words = [
        "add", "create", "update", "delete", "remove", "set", "write", "save", "send", "edit",
        "complete", "import", "reorder", "move", "put", "post", "patch", "insert", "open",
    ];
    for tool in reply["result"]["tools"].as_array().unwrap() {
        let name = tool["name"].as_str().unwrap();
        for word in write_words {
            assert!(
                !name.to_lowercase().contains(word),
                "{name} reads like a write"
            );
        }
        let schema = &tool["inputSchema"];
        assert_eq!(schema["type"], "object", "{name}");
        assert_eq!(schema["additionalProperties"], false, "{name}");
        for (field, _) in schema["properties"].as_object().unwrap() {
            for word in write_words {
                assert!(!field.to_lowercase().contains(word), "{name}.{field}");
            }
        }
        // Every description tells the app's model that what comes back is data.
        assert!(
            tool["description"].as_str().unwrap().contains("untrusted"),
            "{name}"
        );
    }
    // The registry itself holds the same five, so a sixth cannot be added without this test knowing.
    assert_eq!(
        TOOLS.iter().map(|t| t.name).collect::<BTreeSet<_>>(),
        ["search", "tasks", "events", "projects", "milestones"]
            .into_iter()
            .collect()
    );
    // And every tool needs one of the four offered scopes.
    assert!(TOOLS.iter().all(|t| SCOPES.contains(&t.scope)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_tool_list_is_filtered_by_the_clients_scopes() {
    let cases: [(&[&str], &[&str]); 5] = [
        (&["tasks.read"], &["tasks"]),
        (&["projects.read"], &["projects", "milestones"]),
        (&["calendar.read"], &["events"]),
        (&["documents.text"], &["search"]),
        (&["tasks.read", "calendar.read"], &["tasks", "events"]),
    ];
    for (held, shown) in cases {
        let (host, app) = rig(held);
        let reply = say(
            &host,
            &app,
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
        )
        .await;
        assert_eq!(names(&reply), set(shown), "{held:?}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_tasks_schema_offers_the_project_filter_only_with_projects_read() {
    for (held, offered) in [
        (&["tasks.read"][..], false),
        (&["tasks.read", "calendar.read"][..], false),
        (&["tasks.read", "projects.read"][..], true),
    ] {
        let (host, app) = rig(held);
        let reply = say(
            &host,
            &app,
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
        )
        .await;
        let tasks = reply["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "tasks")
            .expect("the tasks tool");
        let properties = tasks["inputSchema"]["properties"].as_object().unwrap();
        assert_eq!(properties.contains_key("project"), offered, "{held:?}");
        assert!(properties.contains_key("limit") && properties.contains_key("after"));
        // What the schema does not offer is what the call refuses.
        if !offered {
            let call = say(
                &host,
                &app,
                json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
                       "params": {"name": "tasks", "arguments": {"project": "Alpha"}}}),
            )
            .await;
            assert!(
                call.get("error").is_some() || call["result"]["isError"] == true,
                "{held:?}"
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unknown_tool_and_one_the_scopes_hide_answer_alike() {
    let (host, app) = rig(&["tasks.read"]);
    let call = |name: &str| json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": name, "arguments": {}}});
    let unknown = say(&host, &app, call("nonesuch")).await;
    // Every tool the client does not hold, including ones with real names.
    for hidden in ["search", "events", "projects", "milestones"] {
        let reply = say(&host, &app, call(hidden)).await;
        assert_eq!(reply, unknown, "{hidden}");
    }
    assert_eq!(code(&unknown), -32602);
    assert_eq!(unknown["error"]["data"]["code"], "MOD-1001");
    // Names that are not tools at all.
    for name in ["Tasks", "tasks ", "tasks.list", "documents.search", ""] {
        assert_eq!(say(&host, &app, call(name)).await, unknown, "{name:?}");
    }
    // The one it holds works.
    let ok = say(&host, &app, call("tasks")).await;
    assert_eq!(ok["result"]["isError"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tool_call_must_name_a_tool_and_take_an_object() {
    let (host, app) = rig(&["tasks.read"]);
    for params in [
        json!({}),
        json!({"name": 5}),
        json!({"name": null}),
        json!({"arguments": {}}),
        json!({"name": "tasks", "arguments": []}),
        json!({"name": "tasks", "arguments": "{}"}),
        json!({"name": "tasks", "arguments": 3}),
    ] {
        let reply = say(
            &host,
            &app,
            json!({"jsonrpc": "2.0", "id": 9, "method": "tools/call", "params": params}),
        )
        .await;
        assert_eq!(code(&reply), -32602, "{params}");
    }
    // Arguments left out, or null, are none.
    for params in [
        json!({"name": "tasks"}),
        json!({"name": "tasks", "arguments": null}),
    ] {
        let reply = say(
            &host,
            &app,
            json!({"jsonrpc": "2.0", "id": 9, "method": "tools/call", "params": params}),
        )
        .await;
        assert_eq!(reply["result"]["isError"], false, "{params}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tools_own_failure_is_a_result_the_model_reads_and_a_refusal_is_a_protocol_error() {
    let (host, app) = rig(&["tasks.read"]);
    host.sql(|c| {
        c.execute(
            "INSERT INTO tasks(title, ordinal) VALUES ('Call Sam', 0)",
            [],
        )
        .unwrap()
    });
    let call = |args: Value| {
        json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call",
               "params": {"name": "tasks", "arguments": args}})
    };
    // The happy path: one text item holding the JSON, not an error.
    let reply = say(&host, &app, call(json!({}))).await;
    assert_eq!(reply["result"]["isError"], false);
    assert_eq!(reply["result"]["content"][0]["type"], "text");
    assert_eq!(result_json(&reply)["tasks"][0]["title"], "Call Sam");
    // A bad argument is the tool's failure: a result with isError, PM's sentence, no protocol error.
    for args in [
        json!({"limit": 0}),
        json!({"limit": 201}),
        json!({"nope": 1}),
        json!({"after": "x"}),
    ] {
        let reply = say(&host, &app, call(args.clone())).await;
        assert!(reply.get("error").is_none(), "{args}");
        assert_eq!(reply["result"]["isError"], true, "{args}");
        assert!(
            reply["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .len()
                > 3
        );
    }
    // PM locked is a refusal of the connection: a protocol error with the stable code.
    host.0
        .locked
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let reply = say(&host, &app, call(json!({}))).await;
    assert_eq!(code(&reply), -32000);
    assert_eq!(reply["error"]["data"]["code"], "MOD-5001");
    let reply = say(
        &host,
        &app,
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
    )
    .await;
    assert_eq!(reply["error"]["data"]["code"], "MOD-5001");
}
