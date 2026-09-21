//! The escalation seam: a question put to a terminal and a file at once.

use std::time::Duration;

use jev_pilot::desk::{Answer, Desk};

/// A desk whose file is only ever looked at on purpose.
///
/// The interval is long enough that nothing in these tests could reach the
/// file by waiting for the next look: anything that arrives in time arrived
/// because something woke the desk up.
fn desk(name: &str) -> (std::path::PathBuf, std::sync::mpsc::Sender<String>, Desk) {
    let dir = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    let (sender, typed) = std::sync::mpsc::channel();
    let desk = Desk::at(dir.clone(), typed, Vec::new(), Duration::from_secs(30)).expect("a desk");
    (dir, sender, desk)
}

/// An answer written to the desk should not have to wait for the next look.
///
/// Whatever writes the file can say so on the same channel a person types on,
/// and an empty line means exactly that: look again, now. Without it the run
/// sits idle for up to a whole interval on every impasse, which is time added
/// to the slowest thing a run does.
#[test]
fn a_nudge_makes_the_desk_read_its_file_at_once() {
    let (dir, sender, desk) = desk("jev-pilot-test-desk-nudge");

    std::thread::spawn(move || {
        // Long enough that `ask` is certainly waiting by now.
        std::thread::sleep(Duration::from_millis(50));
        std::fs::write(dir.join("answer.json"), r#"{"operation":"back"}"#).expect("answered");
        sender.send(String::new()).expect("nudged");
    });

    let began = std::time::Instant::now();
    let reply = desk.ask(&serde_json::json!({"kind": "which_action"}), "").expect("an answer");
    let took = began.elapsed();

    match reply {
        Answer::File(value) => assert_eq!(value["operation"], "back"),
        Answer::Typed(line) => panic!("an empty line is a nudge, not an answer: {line:?}"),
    }
    assert!(took < Duration::from_secs(5), "it waited for the next look: {took:?}");
}

/// A person typing an answer is still answering, and still does not wait for
/// the next look at the file.
#[test]
fn a_typed_answer_still_comes_straight_back() {
    let (_dir, sender, desk) = desk("jev-pilot-test-desk-typed");

    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        sender.send("tap 3".to_owned()).expect("typed");
    });

    let began = std::time::Instant::now();
    let reply = desk.ask(&serde_json::json!({"kind": "which_action"}), "").expect("an answer");
    let took = began.elapsed();

    match reply {
        Answer::Typed(line) => assert_eq!(line, "tap 3"),
        Answer::File(value) => panic!("nothing wrote a file: {value}"),
    }
    assert!(took < Duration::from_secs(5), "it waited for the next look: {took:?}");
}
