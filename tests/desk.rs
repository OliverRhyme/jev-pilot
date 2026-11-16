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
    let reply = desk
        .ask(&serde_json::json!({"kind": "which_action"}), "")
        .expect("an answer");
    let took = began.elapsed();

    match reply {
        Answer::File(value) => assert_eq!(value["operation"], "back"),
        Answer::Typed(line) => panic!("an empty line is a nudge, not an answer: {line:?}"),
    }
    assert!(
        took < Duration::from_secs(5),
        "it waited for the next look: {took:?}"
    );
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
    let reply = desk
        .ask(&serde_json::json!({"kind": "which_action"}), "")
        .expect("an answer");
    let took = began.elapsed();

    match reply {
        Answer::Typed(line) => assert_eq!(line, "tap 3"),
        Answer::File(value) => panic!("nothing wrote a file: {value}"),
    }
    assert!(
        took < Duration::from_secs(5),
        "it waited for the next look: {took:?}"
    );
}

/// A person reading the question in a terminal is told which rows are out of
/// reach, as the file is, so that neither answers with one.
#[test]
fn the_question_marks_the_rows_that_are_covered() {
    use jev_pilot::act::{Indecision, Operation};
    use jev_pilot::judgment::Confidence;
    use jev_pilot::pilot::Impasse;

    let rows = vec!["Result A".to_owned(), "Result B".to_owned()];
    let impasse = Impasse {
        goal: "Open result B",
        step: 1,
        because: &Indecision::Covered,
        leaning: Operation::Tap,
        operation_confidence: Confidence::new(0.9).expect("valid"),
        target_confidence: None,
        alternatives: &[],
        rows: &rows,
        covered: &[1],
        keyboard_open: false,
        unavailable: &[],
        fields: &[],
        tabs: &[],
        destinations: &[],
        says: &[],
        operations: &[Operation::Tap],
        previous: None,
        lately: &[],
    };

    let asked = jev_pilot::desk::describe(&impasse);

    let line = |label: &str| {
        asked
            .lines()
            .find(|line| line.contains(label))
            .unwrap_or_default()
            .to_owned()
    };
    assert!(line("Result B").contains("covered"), "{asked}");
    assert!(!line("Result A").contains("covered"), "{asked}");
}

/// An answer that says to type, and says what, has said everything. Measured
/// over MCP: `type_text` into the amount field with the words "50" was
/// answered, and the run then stopped a second time to ask what to type.
#[test]
fn words_given_with_an_answer_are_the_words_typed() {
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        use jev_pilot::device::{Command, Device};
        use jev_pilot::pilot::{Impasse, Judge, Pilot, Writing};
        use jev_pilot::platform::Android;
        use jev_pilot::snapshot::{Bounds, Element, Snapshot};
        use jev_pilot::step::{StepAnswers, StepQuestions};
        use std::rc::Rc;

        struct Form(Vec<Command>);
        impl Device for Form {
            type Error = std::convert::Infallible;
            fn observe(&mut self) -> Result<Snapshot, Self::Error> {
                Ok(Snapshot::new(vec![Element {
                    label: "Amount".into(),
                    detail: Some(format!("{} typed", self.0.len()).into()),
                    editable: true,
                    bounds: Bounds {
                        left: 0,
                        top: 0,
                        right: 100,
                        bottom: 50,
                    },
                }])
                .expect("a screen"))
            }
            fn perform(&mut self, command: &Command) -> Result<(), Self::Error> {
                self.0.push(command.clone());
                Ok(())
            }
        }
        struct Unsure(std::cell::Cell<bool>);
        impl Judge for Unsure {
            type Error = std::convert::Infallible;
            fn evaluate(
                &self,
                _state: serde_json::Value,
                _questions: &StepQuestions<'_>,
            ) -> Result<StepAnswers, Self::Error> {
                let first = !self.0.replace(true);
                let (operation, confidence) = if first {
                    ("type_text", 0.3)
                } else {
                    ("done", 0.99)
                };
                Ok(serde_json::from_value(serde_json::json!({
                    "operation": { "type": "choice", "choice": operation, "confidence": confidence },
                    "type_field": { "type": "choice", "choice": "A1", "confidence": confidence },
                    "goal_met": { "type": "score", "score": if first { 0.2 } else { 1.9 }, "confidence": 0.9 },
                    "is_error_screen": { "type": "noul", "noul": 0.01 },
                }))
                .expect("parses"))
            }
        }

        let (dir, sender, desk) = desk("jev-pilot-test-desk-words");
        let nudge = sender.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            std::fs::write(
                dir.join("answer.json"),
                r#"{"operation":"type_text","target":0,"text":"50"}"#,
            )
            .expect("answered");
            nudge.send(String::new()).expect("nudged");
        });
        let desk = Rc::new(desk);
        let (choosing, writing) = (Rc::clone(&desk), Rc::clone(&desk));
        let mut pilot = Pilot::new(
            Form(Vec::new()),
            Unsure(std::cell::Cell::new(false)),
            &Android,
        )
        .requiring(jev_pilot::judgment::Confidence::new(0.6).expect("valid"))
        .escalating_to(move |impasse: &Impasse<'_>| choosing.choose(impasse))
        .writing_with(move |request: &Writing<'_>| writing.compose(request));
        let _ = pilot.pursue("Send 50");
        let typed = pilot.device().0.iter().find_map(|command| match command {
            Command::TypeText { text, .. } => Some(text.to_string()),
            _ => None,
        });
        drop(sender);
        let _ = done.send(typed);
    });

    let typed = finished
        .recv_timeout(Duration::from_secs(10))
        .expect("the words were already given, so nothing was asked again");
    assert_eq!(typed.as_deref(), Some("50"));
}

/// What a browser run can switch to and go to is put to whoever answers, each
/// numbered on its own so that `switch_tab 1` and `navigate 1` mean the second
/// tab and the second address, whatever number the rows reach.
#[test]
fn the_question_numbers_tabs_and_addresses_on_their_own() {
    use jev_pilot::act::{Indecision, Operation};
    use jev_pilot::judgment::Confidence;
    use jev_pilot::pilot::{Impasse, Resolution};

    let rows = vec!["Open".to_owned(), "Close".to_owned(), "Menu".to_owned()];
    let tabs = vec![
        "Results — https://example.test/r".to_owned(),
        "Cart — https://example.test/c".to_owned(),
    ];
    let destinations = vec![
        "https://example.test/".to_owned(),
        "https://example.test/pricing".to_owned(),
    ];
    let impasse = Impasse {
        goal: "Find the price",
        step: 2,
        because: &Indecision::NoProgress { repeated: 3 },
        leaning: Operation::Tap,
        operation_confidence: Confidence::new(0.4).expect("valid"),
        target_confidence: None,
        alternatives: &[],
        rows: &rows,
        covered: &[],
        keyboard_open: false,
        unavailable: &[],
        fields: &[],
        tabs: &tabs,
        destinations: &destinations,
        says: &[],
        operations: &[Operation::Tap, Operation::SwitchTab, Operation::Navigate],
        previous: None,
        lately: &[],
    };

    let asked = jev_pilot::desk::describe(&impasse);
    assert!(
        asked.contains("switch_tab 1 -> Cart — https://example.test/c"),
        "{asked}"
    );
    assert!(
        asked.contains("navigate 0 -> https://example.test/"),
        "{asked}"
    );
    assert!(asked.contains("switch_tab <n>"), "{asked}");
    assert!(asked.contains("navigate <n>"), "{asked}");

    let (dir, sender, desk) = desk("jev-pilot-test-desk-tabs");
    let reader = std::thread::spawn({
        let ask = dir.join("ask.json");
        move || {
            for _ in 0..200 {
                if let Ok(raw) = std::fs::read_to_string(&ask) {
                    // The desk drains whatever was typed before it began
                    // waiting, so the answer has to come after it has.
                    std::thread::sleep(Duration::from_millis(50));
                    sender.send("navigate 1".to_owned()).expect("answered");
                    return serde_json::from_str::<serde_json::Value>(&raw).expect("json");
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            panic!("nothing was asked");
        }
    });
    let resolution = desk.choose(&impasse).expect("answered");
    let written = reader.join().expect("the question was read");

    assert_eq!(
        resolution,
        Resolution::Choose {
            operation: Operation::Navigate,
            target: Some(1)
        }
    );
    assert_eq!(written["tabs"], serde_json::json!(tabs));
    assert_eq!(written["destinations"], serde_json::json!(destinations));
}

/// A phone's question is the one it has always been: nothing about tabs or
/// addresses, in the file or at the terminal.
#[test]
fn a_phones_question_says_nothing_of_tabs_or_addresses() {
    use jev_pilot::act::{Indecision, Operation};
    use jev_pilot::judgment::Confidence;
    use jev_pilot::pilot::Impasse;

    let rows = vec!["Result A".to_owned()];
    let impasse = Impasse {
        goal: "Open it",
        step: 1,
        because: &Indecision::Covered,
        leaning: Operation::Tap,
        operation_confidence: Confidence::new(0.9).expect("valid"),
        target_confidence: None,
        alternatives: &[],
        rows: &rows,
        covered: &[],
        keyboard_open: false,
        unavailable: &[],
        fields: &[],
        tabs: &[],
        destinations: &[],
        says: &[],
        operations: &[Operation::Tap],
        previous: None,
        lately: &[],
    };

    let asked = jev_pilot::desk::describe(&impasse);

    assert!(!asked.contains("switch_tab"), "{asked}");
    assert!(!asked.contains("navigate"), "{asked}");
    assert!(
        asked.contains("tap <n> | type <n> | back | scroll_down | done | stop: "),
        "{asked}"
    );
}

/// A prompt takes its words from the text supplied for a key that matches the
/// prompt's own question, by containment and ignoring case, as a field does.
/// Without a match it asks, as a field does.
#[test]
fn a_prompt_takes_its_words_from_the_text_supplied_for_a_matching_key() {
    use jev_pilot::act::{DialogReply, Operation};
    use jev_pilot::device::{Command, Device};
    use jev_pilot::pilot::{Pilot, Writing};
    use jev_pilot::platform::{HierarchyError, Platform};
    use jev_pilot::snapshot::{Bounds, Dialog, DialogKind, Element, Snapshot};
    use jev_pilot::step::{StepAnswers, StepQuestions};
    use std::rc::Rc;

    struct Web;
    impl Platform for Web {
        fn name(&self) -> &'static str {
            "Web"
        }
        fn operations(&self) -> &'static [Operation] {
            &[
                Operation::AcceptDialog,
                Operation::DismissDialog,
                Operation::Wait,
                Operation::Done,
                Operation::Blocked,
            ]
        }
        fn parse_hierarchy(&self, _raw: &str) -> Result<Snapshot, HierarchyError> {
            unreachable!("these tests build snapshots by hand")
        }
    }
    struct Page(Vec<Command>);
    impl Device for Page {
        type Error = std::convert::Infallible;
        fn observe(&mut self) -> Result<Snapshot, Self::Error> {
            let rows = ["Save", "Cancel"]
                .iter()
                .enumerate()
                .map(|(n, label)| Element {
                    label: (*label).into(),
                    detail: None,
                    editable: false,
                    bounds: Bounds::from_origin_size(
                        0,
                        i32::try_from(n).expect("small") * 40,
                        50,
                        20,
                    ),
                })
                .collect();
            let snapshot = Snapshot::new(rows).expect("a page");
            Ok(if self.0.is_empty() {
                snapshot.with_dialog(Some(Dialog {
                    kind: DialogKind::Prompt,
                    message: "Name this draft".into(),
                    default_prompt: None,
                }))
            } else {
                snapshot.with_keyboard_open(true)
            })
        }
        fn perform(&mut self, command: &Command) -> Result<(), Self::Error> {
            self.0.push(command.clone());
            Ok(())
        }
    }
    struct Accepts(std::cell::Cell<bool>);
    impl jev_pilot::pilot::Judge for Accepts {
        type Error = std::convert::Infallible;
        fn evaluate(
            &self,
            _state: serde_json::Value,
            _questions: &StepQuestions<'_>,
        ) -> Result<StepAnswers, Self::Error> {
            let first = !self.0.replace(true);
            let operation = if first { "accept_dialog" } else { "done" };
            Ok(serde_json::from_value(serde_json::json!({
                "operation": { "type": "choice", "choice": operation, "confidence": 0.99 },
                "goal_met": { "type": "score", "score": if first { 0.1 } else { 1.9 }, "confidence": 0.9 },
                "is_error_screen": { "type": "noul", "noul": 0.01 },
            }))
            .expect("parses"))
        }
    }

    let dir = std::env::temp_dir().join("jev-pilot-test-desk-prompt");
    let _ = std::fs::remove_dir_all(&dir);
    let desk = Desk::at(
        dir,
        std::sync::mpsc::channel().1,
        vec![("NAME THIS".into(), "Quarterly plan".into())],
        Duration::from_millis(20),
    )
    .expect("a desk");
    let desk = Rc::new(desk);
    let writing = Rc::clone(&desk);
    let mut pilot = Pilot::new(Page(Vec::new()), Accepts(std::cell::Cell::new(false)), &Web)
        .writing_with(move |request: &Writing<'_>| writing.compose(request));

    pilot.pursue("Name the draft").expect("the run completes");

    assert_eq!(
        pilot.device().0.first(),
        Some(&Command::Dialog(DialogReply::Accept {
            words: Some("Quarterly plan".into())
        })),
    );
}
