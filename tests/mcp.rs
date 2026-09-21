//! Speaking MCP over a pipe. Pure: nothing here touches a device.

use jev_pilot::mcp::{Sessions, handle};

fn ask(method: &str, params: &serde_json::Value) -> serde_json::Value {
    handle(
        &serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}),
        &mut Sessions::default(),
    )
    .expect("a request is answered")
}

/// The handshake names the protocol the client asked for when it is one this
/// server speaks, because a client that asked for an older one is entitled to
/// be answered in it rather than told the newest.
#[test]
fn the_handshake_answers_in_the_version_the_client_asked_for() {
    let answer = ask(
        "initialize",
        &serde_json::json!({"protocolVersion": "2024-11-05"}),
    );

    assert_eq!(answer["result"]["protocolVersion"], "2024-11-05");
    assert_eq!(answer["result"]["serverInfo"]["name"], "jev-pilot");
}

/// A version this server does not speak is answered with one it does, rather
/// than with a refusal: the client can then decide whether that will do.
#[test]
fn an_unknown_protocol_version_is_answered_with_a_known_one() {
    let answer = ask(
        "initialize",
        &serde_json::json!({"protocolVersion": "1999-01-01"}),
    );

    assert_eq!(answer["result"]["protocolVersion"], jev_pilot::mcp::PROTOCOL);
}

/// A notification has no id and wants no answer. Replying to one is a protocol
/// error, and some clients close the connection over it.
#[test]
fn a_notification_is_not_answered() {
    let quiet = handle(
        &serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        &mut Sessions::default(),
    );

    assert!(quiet.is_none(), "got {quiet:?}");
}

/// A method this server does not have is an error reply, not a silence and not
/// a panic.
#[test]
fn an_unknown_method_is_an_error_reply() {
    let answer = ask("nonesuch/method", &serde_json::json!({}));

    assert_eq!(answer["error"]["code"], -32601);
    assert!(answer.get("result").is_none(), "{answer}");
}

/// The tools a client is offered. Named for what they do to a device, because
/// that is the thing a person approving a call needs to know: looking at a
/// screen and moving money through one are not the same kind of permission.
#[test]
fn the_tools_offered_say_what_they_touch() {
    let answer = ask("tools/list", &serde_json::json!({}));
    let tools = answer["result"]["tools"].as_array().expect("a list");

    let named: Vec<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().expect("a name"))
        .collect();
    assert_eq!(
        named,
        [
            "observe",
            "devices",
            "helper",
            "start_run",
            "run_status",
            "answer_run",
            "stop_run",
        ],
    );

    for tool in tools {
        assert!(
            tool["description"].as_str().is_some_and(|d| d.len() > 30),
            "{} needs a description worth reading",
            tool["name"],
        );
        assert_eq!(
            tool["inputSchema"]["type"], "object",
            "{} needs a schema",
            tool["name"],
        );
    }
}

/// Starting a run is the one that can move money, so it says so where a person
/// approving the call will see it.
#[test]
fn starting_a_run_is_marked_as_changing_the_device() {
    let answer = ask("tools/list", &serde_json::json!({}));
    let tools = answer["result"]["tools"].as_array().expect("a list");
    let find = |name: &str| {
        tools
            .iter()
            .find(|tool| tool["name"] == name)
            .unwrap_or_else(|| panic!("{name} is offered"))
            .clone()
    };

    assert_eq!(find("start_run")["annotations"]["readOnlyHint"], false);
    assert_eq!(find("start_run")["annotations"]["destructiveHint"], true);
    assert_eq!(find("observe")["annotations"]["readOnlyHint"], true);
    assert_eq!(find("devices")["annotations"]["readOnlyHint"], true);
}

/// A tool nobody has is an error inside the result rather than a transport
/// fault, which is how MCP asks for a tool that failed to be reported.
#[test]
fn calling_a_tool_that_does_not_exist_is_reported_as_a_failed_call() {
    let answer = ask(
        "tools/call",
        &serde_json::json!({"name": "nonesuch", "arguments": {}}),
    );

    assert_eq!(answer["result"]["isError"], true);
    let said = answer["result"]["content"][0]["text"]
        .as_str()
        .expect("something said");
    assert!(said.contains("nonesuch"), "got {said}");
}

/// Every tool runs the command line rather than driving the loop in this
/// process, so a server cannot drift from the thing it is a face for. What is
/// worth testing without a device is that it builds the right invocation.
#[test]
fn observing_runs_the_command_line_that_observes() {
    assert_eq!(
        jev_pilot::mcp::invocation("observe", &serde_json::json!({})),
        Some(vec!["observe".to_owned()]),
    );
    assert_eq!(
        jev_pilot::mcp::invocation("observe", &serde_json::json!({"device": "abc123"})),
        Some(vec![
            "--device".to_owned(),
            "abc123".to_owned(),
            "observe".to_owned(),
        ]),
    );
}

/// The helper is reported on by default and installed only when asked, because
/// installing puts an app on somebody's phone.
#[test]
fn the_helper_is_only_installed_when_asked() {
    assert_eq!(
        jev_pilot::mcp::invocation("helper", &serde_json::json!({})),
        Some(vec!["helper".to_owned()]),
    );
    assert_eq!(
        jev_pilot::mcp::invocation("helper", &serde_json::json!({"install": true})),
        Some(vec!["helper".to_owned(), "install".to_owned()]),
    );
}

/// A goal, the app it is about, what must hold at the end, and the words to
/// type — each becoming the flag the command line already has for it.
#[test]
fn a_run_is_started_with_the_flags_the_command_line_already_has() {
    let args = jev_pilot::mcp::invocation(
        "start_run",
        &serde_json::json!({
            "goal": "send fifty pesos",
            "app": "com.example.wallet",
            "accept": ["A receipt is on screen"],
            "text": { "amount": "50" },
            "steps": 40,
            "floor": 0.35,
        }),
    )
    .expect("a run is startable");

    let joined = args.join(" ");
    assert!(joined.contains("--app com.example.wallet"), "{joined}");
    assert!(joined.contains("--accept A receipt is on screen"), "{joined}");
    assert!(joined.contains("--text amount=50"), "{joined}");
    assert!(joined.contains("--steps 40"), "{joined}");
    assert!(joined.contains("--floor 0.35"), "{joined}");
    assert_eq!(args.last().map(String::as_str), Some("send fifty pesos"));
}

/// A run with no goal is not a run.
#[test]
fn a_run_without_a_goal_is_refused() {
    assert_eq!(
        jev_pilot::mcp::invocation("start_run", &serde_json::json!({})),
        None,
    );
}

/// An answer becomes the file the desk already reads, because that is the seam
/// a run has always escalated through: a person at a terminal and a model over
/// MCP write the same thing.
#[test]
fn an_answer_becomes_what_the_desk_already_reads() {
    let wrote = |arguments: serde_json::Value| {
        jev_pilot::mcp::answer_from(&arguments).expect("an answer")
    };

    assert_eq!(
        wrote(serde_json::json!({"operation": "tap", "target": 3})),
        serde_json::json!({"operation": "tap", "target": 3}),
    );
    assert_eq!(
        wrote(serde_json::json!({"operation": "stop"})),
        serde_json::json!({"operation": "stop"}),
    );
    // Words, when what it asked for was words rather than an action.
    assert_eq!(
        wrote(serde_json::json!({"text": "0150002954"})),
        serde_json::json!({"text": "0150002954"}),
    );
}

/// An answer that says nothing is not an answer, and writing it would let a
/// run act on an empty file.
#[test]
fn an_empty_answer_is_refused() {
    assert_eq!(jev_pilot::mcp::answer_from(&serde_json::json!({})), None);
}

/// Runs are named by this server, not by the caller, so one conversation
/// cannot reach into another's run by guessing a name it was never given.
#[test]
fn runs_are_named_by_the_server() {
    let mut sessions = Sessions::default();
    let first = sessions.name_a_run();
    let second = sessions.name_a_run();

    assert_ne!(first, second);
    assert!(first.starts_with("run-"), "got {first}");
}

/// Asking after a run nobody started is a failed call, not a panic and not a
/// silence.
#[test]
fn asking_after_an_unknown_run_is_a_failed_call() {
    let answer = handle(
        &serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": "run_status", "arguments": { "run": "run-404" } },
        }),
        &mut Sessions::default(),
    )
    .expect("answered");

    assert_eq!(answer["result"]["isError"], true);
    let said = answer["result"]["content"][0]["text"].as_str().expect("text");
    assert!(said.contains("run-404"), "got {said}");
}

/// Requests arrive one JSON object per line and answers go back the same way.
/// A line that is not JSON is answered rather than ending the conversation:
/// one bad line is not a reason to hang up on a client.
#[test]
fn a_conversation_is_one_object_per_line() {
    let asked = concat!(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        "\n",
        "not json at all\n",
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#,
        "\n",
    );
    let mut said = Vec::new();

    jev_pilot::mcp::serve(asked.as_bytes(), &mut said).expect("the conversation runs to the end");

    let lines: Vec<serde_json::Value> = String::from_utf8(said)
        .expect("utf-8")
        .lines()
        .map(|line| serde_json::from_str(line).expect("each answer is one object"))
        .collect();
    // The handshake, the complaint about the bad line, and the ping. The
    // notification is not answered.
    assert_eq!(lines.len(), 3, "got {lines:#?}");
    assert_eq!(lines[0]["id"], 1);
    assert_eq!(lines[1]["error"]["code"], -32700);
    assert_eq!(lines[2]["id"], 2);
}
