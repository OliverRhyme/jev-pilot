//! Speaking MCP. Pure: nothing here touches a device.
#![cfg(feature = "mcp")]
use jev_pilot::mcp::Sessions;

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

/// A run is named by the device it drives, because that is what it is. Two
/// runs on one phone are two processes taking turns at the same screen, and an
/// identifier of their own would make that look like an ordinary thing to ask
/// for.
#[test]
fn a_run_is_named_by_the_device_it_drives() {
    use jev_pilot::mcp::which_run;

    // One run in flight, and nothing to say about which: there is only one.
    assert_eq!(which_run(None, &["abc123"]), Ok("abc123".to_owned()));
    assert_eq!(which_run(Some("abc123"), &["abc123"]), Ok("abc123".to_owned()));

    // Two devices, two runs. Now it matters which.
    let torn = which_run(None, &["abc123", "def456"]).expect_err("ambiguous");
    assert!(torn.contains("abc123") && torn.contains("def456"), "got {torn}");

    // Nothing running at all.
    assert!(which_run(None, &[]).is_err());
    assert!(which_run(Some("abc123"), &["def456"]).is_err());
}

/// A run that has finished is not still driving anything, so the device is
/// free. Otherwise one completed run would block that phone for the rest of
/// the conversation.
#[test]
fn a_finished_run_leaves_the_device_free() {
    let mut sessions = finished_with("abc123");

    assert_eq!(sessions.still_running(), Vec::<String>::new());
}

/// A conversation that ends takes its runs with it. Left going, a run carries
/// on tapping at somebody's phone with nothing watching it and nothing able to
/// answer it.
#[test]
fn ending_the_conversation_ends_the_runs_it_started() {
    let child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("a child that keeps going");
    let id = child.id();
    {
        let mut sessions = Sessions::default();
        sessions.remember("abc123".to_owned(), std::env::temp_dir(), child);
        assert_eq!(sessions.still_running(), ["abc123"]);
    }

    // Reaped by the drop, so the pid is no longer a live `sleep`.
    let alive = std::process::Command::new("ps")
        .args(["-p", &id.to_string(), "-o", "comm="])
        .output()
        .expect("ps runs");
    let named = String::from_utf8_lossy(&alive.stdout);
    assert!(!named.contains("sleep"), "left running: {named}");
}

/// A session holding one run that has ended, as a conversation has afterwards.
fn finished_with(device: &str) -> Sessions {
    let mut sessions = Sessions::default();
    let mut child = std::process::Command::new("true")
        .spawn()
        .expect("a child that exits at once");
    // Waited for here, so the test is about a run that has definitely ended
    // rather than about how quickly `true` gets round to it.
    child.wait().expect("it exits");
    sessions.remember(device.to_owned(), std::env::temp_dir(), child);
    sessions
}
