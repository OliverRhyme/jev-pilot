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
    assert!(
        joined.contains("--accept A receipt is on screen"),
        "{joined}"
    );
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
    let wrote =
        |arguments: serde_json::Value| jev_pilot::mcp::answer_from(&arguments).expect("an answer");

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
    assert_eq!(
        which_run(Some("abc123"), &["abc123"]),
        Ok("abc123".to_owned())
    );

    // Two devices, two runs. Now it matters which.
    let torn = which_run(None, &["abc123", "def456"]).expect_err("ambiguous");
    assert!(
        torn.contains("abc123") && torn.contains("def456"),
        "got {torn}"
    );

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

/// A run that stops to ask should hand the question back as the answer to the
/// call, rather than leaving the caller to go looking for it. There is no way
/// for a server to call into a client's model — sampling did that and is
/// deprecated — so the next best thing is not to return until there is
/// something to say.
#[tokio::test]
async fn a_call_waits_until_the_run_wants_something() {
    use jev_pilot::mcp::{Waited, until_it_wants_something};

    let desk = std::env::temp_dir().join("jev-pilot-test-waiting");
    let _ = std::fs::remove_dir_all(&desk);
    std::fs::create_dir_all(&desk).expect("a desk");

    // Nothing yet, so it comes back having waited rather than hanging for ever.
    assert!(matches!(
        until_it_wants_something(&desk, std::time::Duration::from_millis(120), None).await,
        Waited::StillGoing,
    ));

    // The run asks, and the question is the answer to the call.
    std::fs::write(desk.join("ask.json"), r#"{"kind":"which_action"}"#).expect("asked");
    let Waited::Asking(question) =
        until_it_wants_something(&desk, std::time::Duration::from_secs(2), None).await
    else {
        panic!("it should come back with the question");
    };
    assert!(question.contains("which_action"), "got {question}");
}

/// A run that has finished says so, rather than being waited on until the call
/// gives up.
#[tokio::test]
async fn a_call_stops_waiting_once_the_run_is_over() {
    use jev_pilot::mcp::{Waited, until_it_wants_something};

    let desk = std::env::temp_dir().join("jev-pilot-test-over");
    let _ = std::fs::remove_dir_all(&desk);
    std::fs::create_dir_all(&desk).expect("a desk");
    std::fs::write(
        desk.join("run.log"),
        "step 1  4 rows\n\nending : Finished(Achieved)\n",
    )
    .expect("a log");

    let Waited::Ended(how) =
        until_it_wants_something(&desk, std::time::Duration::from_secs(2), None).await
    else {
        panic!("it should come back saying it is over");
    };
    assert!(how.contains("Finished(Achieved)"), "got {how}");
}

/// Asking after a run that is over should say how it went. The steps alone
/// cannot say it: the last one looks exactly like the last one of a run that
/// is still going.
#[test]
fn a_status_says_how_a_finished_run_went() {
    let desk = std::env::temp_dir().join("jev-pilot-test-status-over");
    let _ = std::fs::remove_dir_all(&desk);
    std::fs::create_dir_all(&desk).expect("a desk");
    std::fs::write(desk.join("steps.jsonl"), "{\"step\":1}\n").expect("a step");
    std::fs::write(
        desk.join("run.log"),
        "step 1  4 rows\n\nending : OutOfSteps { limit: 1 }\n",
    )
    .expect("a log");

    let said = jev_pilot::mcp::status_of(&desk);
    assert!(said.contains("OutOfSteps"), "got {said}");
    assert!(
        said.contains("{\"step\":1}"),
        "the steps are still there: {said}"
    );
}

/// Answering must not come straight back with the question that was just
/// answered. The run takes the answer off its desk in its own time, so for a
/// moment after the answer is written the old question is still lying there —
/// and a caller told to answer it again would answer it for ever.
#[tokio::test]
async fn answering_does_not_come_back_with_the_question_it_answered() {
    use jev_pilot::mcp::{Waited, until_it_wants_something};

    let desk = std::env::temp_dir().join("jev-pilot-test-answered");
    let _ = std::fs::remove_dir_all(&desk);
    std::fs::create_dir_all(&desk).expect("a desk");
    let asked = r#"{"kind":"which_action","step":3}"#;
    std::fs::write(desk.join("ask.json"), asked).expect("asked");

    // The run has not picked the answer up yet, so the stale question is still
    // there. Waiting on it should not report it.
    assert!(matches!(
        until_it_wants_something(&desk, std::time::Duration::from_millis(120), Some(asked)).await,
        Waited::StillGoing,
    ));

    // A different question is a real one.
    std::fs::write(desk.join("ask.json"), r#"{"kind":"which_action","step":4}"#).expect("asked");
    let Waited::Asking(question) =
        until_it_wants_something(&desk, std::time::Duration::from_secs(2), Some(asked)).await
    else {
        panic!("the next question should come back");
    };
    assert!(question.contains("\"step\":4"), "got {question}");
}

/// Writing the answer is only half of it: the run is asleep on its desk until
/// something tells it to look. The server keeps the run's input open for
/// exactly that, so an answer costs a pipe write rather than a wait.
#[test]
fn answering_wakes_the_run_rather_than_leaving_it_to_look() {
    use std::io::{BufRead as _, BufReader};

    let mut child = std::process::Command::new("cat")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("a stand-in for a run");
    let said = child.stdout.take().expect("its output");

    let mut sessions = jev_pilot::mcp::Sessions::default();
    sessions.remember(
        "a-device".to_owned(),
        std::env::temp_dir().join("jev-pilot-test-nudge"),
        child,
    );

    assert!(
        sessions.nudge("a-device"),
        "it should have an input to write to"
    );
    let mut heard = String::new();
    BufReader::new(said)
        .read_line(&mut heard)
        .expect("it woke up");
    assert_eq!(heard, "\n", "a nudge is an empty line, never an answer");

    assert!(!sessions.nudge("no-such-device"));
}

/// A plan and keypad keys reach the command line as its own flags, in order.
#[test]
fn a_run_can_be_given_its_steps_and_keys() {
    let args = jev_pilot::mcp::invocation(
        "start_run",
        &serde_json::json!({
            "goal": "send fifty pesos",
            "plan": ["Log in", "Open Transfer"],
            "keys": "246810",
        }),
    )
    .expect("a run is startable");

    let joined = args.join(" ");
    assert!(
        joined.contains("--then Log in --then Open Transfer"),
        "{joined}"
    );
    assert!(joined.contains("--keys 246810"), "{joined}");
    assert_eq!(args.last().map(String::as_str), Some("send fifty pesos"));
}

/// The command line is found beside the server by its file name, and on
/// Windows that name ends in `.exe`. Looked for without it, the server found
/// nothing beside itself and fell back to whatever `jev-pilot` was on the path.
#[test]
fn the_command_line_is_looked_for_under_its_platform_file_name() {
    let beside = jev_pilot::mcp::command_line_beside(
        std::path::Path::new("C:/tools/jev-pilot-mcp.exe"),
        ".exe",
    );
    assert_eq!(beside, std::path::Path::new("C:/tools/jev-pilot.exe"));

    let beside = jev_pilot::mcp::command_line_beside(
        std::path::Path::new("/usr/local/bin/jev-pilot-mcp"),
        "",
    );
    assert_eq!(beside, std::path::Path::new("/usr/local/bin/jev-pilot"));
}
