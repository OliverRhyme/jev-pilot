//! The two-dimensional action space: what to do, and what to do it to.

use jev_pilot::act::{Act, Catalog, Decision, Direction, Floors, Indecision, Operation, Outcome};
use jev_pilot::device::{Command, command_for};
use jev_pilot::judgment::Confidence;
use jev_pilot::platform::{Android, Ios, Platform};
use jev_pilot::step::StepAnswers;

const SETTINGS: &str = include_str!("fixtures/settings.xml");

fn answers(value: serde_json::Value) -> StepAnswers {
    serde_json::from_value(value).expect("answers parse")
}

/// Operation and target are asked as separate heads of one request. A flat list
/// of every operation crossed with every element would grow multiplicatively
/// and could express pairings that make no sense, like scrolling a button.
#[test]
fn the_operation_head_offers_operations_and_the_target_head_offers_elements() {
    let snapshot = Android.parse_hierarchy(SETTINGS).expect("fixture parses");
    let catalog = Catalog::for_screen(&snapshot, &Android);

    let operations =
        serde_json::to_value(catalog.operation_question("Turn on Wi-Fi")).expect("serializes");
    let targets = serde_json::to_value(
        catalog
            .tap_target_question("Turn on Wi-Fi")
            .expect("rows are tappable"),
    )
    .expect("serializes");

    let ops = operations["criteria"].as_object().expect("a criteria map");
    assert!(ops.contains_key("tap"), "got {ops:?}");
    assert!(ops.contains_key("scroll_down"), "got {ops:?}");
    assert!(ops.contains_key("done"), "got {ops:?}");
    assert!(
        !ops.keys().any(|k| k.starts_with('A')),
        "operations are named, not indexed"
    );

    let rows = targets["criteria"].as_object().expect("a criteria map");
    assert_eq!(
        rows.len(),
        snapshot.len(),
        "every row is a candidate target"
    );
    assert!(rows.contains_key("A3"));
}

/// iOS has no system back, so it must not appear among the operations offered.
#[test]
fn only_operations_the_platform_supports_are_offered() {
    assert!(Android.operations().contains(&Operation::Back));
    assert!(!Ios.operations().contains(&Operation::Back));
    for platform in [&Android as &dyn Platform, &Ios] {
        assert!(platform.operations().contains(&Operation::ScrollDown));
        assert!(platform.operations().contains(&Operation::Done));
    }
}

/// Target heads are answered speculatively, in parallel with the operation. Only
/// the head matching the chosen operation is consumed; the rest are discarded.
#[test]
fn the_target_matching_the_chosen_operation_is_the_one_used() {
    let snapshot = Android.parse_hierarchy(SETTINGS).expect("fixture parses");
    let catalog = Catalog::for_screen(&snapshot, &Android);

    let act = catalog
        .resolve(
            &answers(serde_json::json!({
                "operation": { "type": "choice", "choice": "tap", "confidence": 0.99 },
                "tap_target": { "type": "choice", "choice": "A3", "confidence": 0.99 },
                "goal_met": { "type": "score", "score": 0.2, "confidence": 0.9 },
                "is_error_screen": { "type": "noul", "noul": 0.01 }
            })),
            &Floors::new(Confidence::ZERO),
        )
        .expect("a decision");

    let expected = snapshot.refs().nth(2).expect("a third row").0;
    assert_eq!(act, Decision::Ready(Act::Tap(expected)));
}

/// An operation that needs no target ignores the target heads entirely, even
/// when they were answered.
#[test]
fn an_operation_without_a_target_ignores_the_speculative_heads() {
    let snapshot = Android.parse_hierarchy(SETTINGS).expect("fixture parses");
    let catalog = Catalog::for_screen(&snapshot, &Android);

    for (choice, expected) in [
        ("scroll_down", Act::Scroll(Direction::Down)),
        ("done", Act::Finish(Outcome::Achieved)),
        ("blocked", Act::Finish(Outcome::Blocked)),
    ] {
        let act = catalog
            .resolve(
                &answers(serde_json::json!({
                    "operation": { "type": "choice", "choice": choice, "confidence": 0.99 },
                    "tap_target": { "type": "choice", "choice": "A7", "confidence": 0.99 },
                    "goal_met": { "type": "score", "score": 0.2, "confidence": 0.9 },
                    "is_error_screen": { "type": "noul", "noul": 0.01 }
                })),
                &Floors::new(Confidence::ZERO),
            )
            .expect("a decision");
        assert_eq!(act, Decision::Ready(expected), "for {choice}");
    }
}

/// Both heads must clear the floor. A confident operation aimed at a target the
/// model was unsure of is still a guess about where to tap.
#[test]
fn an_uncertain_target_is_refused_even_when_the_operation_is_certain() {
    let snapshot = Android.parse_hierarchy(SETTINGS).expect("fixture parses");
    let catalog = Catalog::for_screen(&snapshot, &Android);
    let floor = Confidence::new(0.6).expect("valid floor");

    let outcome = catalog.resolve(
        &answers(serde_json::json!({
            "operation": { "type": "choice", "choice": "tap", "confidence": 0.99 },
            "tap_target": { "type": "choice", "choice": "A3", "confidence": 0.20 },
            "goal_met": { "type": "score", "score": 0.2, "confidence": 0.9 },
            "is_error_screen": { "type": "noul", "noul": 0.01 }
        })),
        &Floors::new(floor),
    );

    assert!(outcome.is_err(), "got {outcome:?}");
}

/// The target head is answered in parallel with the operation, so it cannot see
/// which operation was chosen — but it must still see the goal. Without it the
/// question reduces to "which row looks important?", and the answer spreads
/// across every plausible row instead of concentrating on the one that advances
/// the task.
#[test]
fn the_target_question_states_the_goal_it_is_serving() {
    let snapshot = Android.parse_hierarchy(SETTINGS).expect("fixture parses");
    let catalog = Catalog::for_screen(&snapshot, &Android);

    let wire = serde_json::to_value(
        catalog
            .tap_target_question("Open the Storage screen")
            .expect("rows exist"),
    )
    .expect("serializes");

    let rendered = wire["instructions"].to_string();
    assert!(
        rendered.contains("Open the Storage screen"),
        "the target head must carry the goal: {rendered}"
    );
}

/// Typing has to say *where*, because a field that is not focused does not
/// receive it.
///
/// The point was being resolved to prove the field was reachable and then
/// thrown away, so the text went to whatever happened to hold focus — nothing,
/// on a freshly opened form. Measured on a Flutter app: setting the text with
/// nothing focused left the field empty, while tapping it first and then
/// setting the text filled it.
#[test]
fn typing_carries_the_point_that_focuses_the_field() {
    let snapshot = Android
        .parse_hierarchy(include_str!("fixtures/flutter-form.xml"))
        .expect("a screen");
    let (field, element) = snapshot
        .refs()
        .find(|(_, element)| element.editable)
        .expect("the form's field");
    let bounds = element.bounds;

    let command = command_for(
        &Act::TypeText {
            into: field,
            text: "09171234567".into(),
        },
        &snapshot,
    )
    .expect("reachable")
    .expect("a command");

    let Command::TypeText { at, text } = command else {
        panic!("expected typing, got {command:?}");
    };
    assert_eq!(&*text, "09171234567");
    assert!(
        at.x > bounds.left && at.x < bounds.right && at.y > bounds.top && at.y < bounds.bottom,
        "the point must land in the field: {at:?} not inside {bounds:?}"
    );
}

const LAUNCHER: &str = include_str!("fixtures/helper-home.xml");

/// A run that has wandered out of the app it was asked about needs one action
/// that puts it back. Without it the way home is a hunt across a launcher, and
/// every icon there looks as plausible as the right one — which is how a
/// banking goal ends up opening a wallet.
#[test]
fn leaving_the_app_offers_a_way_back_into_it() {
    let launcher = Android.parse_hierarchy(LAUNCHER).expect("fixture parses");
    let catalog =
        Catalog::for_screen(&launcher, &Android).returning_to(Some("com.android.settings"));

    assert!(catalog.operations().contains(&Operation::Return));

    let Decision::Ready(act) = catalog
        .act_from(Operation::Return, None)
        .expect("return is offered")
    else {
        panic!("return needs no text");
    };
    assert_eq!(
        command_for(&act, &launcher).expect("return resolves"),
        Some(Command::Launch("com.android.settings".into())),
    );
}

/// On the app it was asked about, going "back into" it is not an action — it
/// is a no-op the model can pick over doing the work.
#[test]
fn a_run_that_has_not_left_is_not_offered_a_way_back() {
    let settings = Android.parse_hierarchy(SETTINGS).expect("fixture parses");
    let catalog =
        Catalog::for_screen(&settings, &Android).returning_to(Some("com.android.settings"));

    assert!(!catalog.operations().contains(&Operation::Return));
}

/// With a keyboard up, the rows it covers are absent from the catalog rather
/// than marked unavailable, and the button that commits a form is usually
/// among them. Telling the judge the keyboard is open does not tell it that
/// closing the keyboard brings a row back — so closing it is offered as
/// something to choose, on the screens where there is a keyboard to close.
///
/// Measured on a transfer form: keyboard up, five rows and no commit; the
/// same screen with the keyboard down, those five and `Continue`.
#[test]
fn a_screen_under_a_keyboard_offers_closing_it() {
    let snapshot = Android.parse_hierarchy(SETTINGS).expect("fixture parses");
    let covered = Catalog::for_screen(&snapshot.with_keyboard_open(true), &Android);

    assert!(covered.operations().contains(&Operation::CloseKeyboard));

    let settled = Android.parse_hierarchy(SETTINGS).expect("fixture parses");
    let clear = Catalog::for_screen(&settled, &Android);
    assert!(
        !clear.operations().contains(&Operation::CloseKeyboard),
        "there is no keyboard to close",
    );
}

/// `back` and `close_keyboard` resolve to the same gesture, so offering both
/// asks the model to choose between two spellings of one action and splits its
/// confidence across them. Measured on a form with the keyboard up: back at
/// 0.57, then — the keyboard now gone — back again at 0.43, which navigated
/// off the form and discarded what had been typed into it.
///
/// While there is a keyboard, the one on offer is the one that says what it
/// does to the keyboard.
#[test]
fn going_back_is_not_offered_twice_under_two_names() {
    let snapshot = Android.parse_hierarchy(SETTINGS).expect("fixture parses");
    let covered = Catalog::for_screen(&snapshot.with_keyboard_open(true), &Android);

    assert!(covered.operations().contains(&Operation::CloseKeyboard));
    assert!(
        !covered.operations().contains(&Operation::Back),
        "with a keyboard up, back is how you close it, and it is already offered as that",
    );

    let settled = Android.parse_hierarchy(SETTINGS).expect("fixture parses");
    let clear = Catalog::for_screen(&settled, &Android);
    assert!(clear.operations().contains(&Operation::Back));
}

/// A screen that offers the same thing twice splits the model's probability
/// mass across both, and the split reads as uncertainty about what to act on.
/// It is not: it is certainty about what to do, divided by an accident of how
/// the screen names its controls.
///
/// Measured on a dialog offering `Close` and `CLOSE`: tap at 0.94, row at
/// 0.34, and the run stopped to ask which of two identical buttons to press.
#[test]
fn mass_split_across_rows_of_the_same_name_is_not_uncertainty() {
    use jev_pilot::snapshot::{Bounds, Element, Snapshot};

    let row = |label: &str, top| Element {
        label: label.into(),
        detail: None,
        editable: false,
        bounds: Bounds::from_origin_size(0, top, 400, 80),
    };
    let snapshot = Snapshot::new(vec![
        row("Keep editing", 0),
        row("Close", 100),
        row("CLOSE", 200),
    ])
    .expect("a screen");
    let catalog = Catalog::for_screen(&snapshot, &Android);

    let decided = catalog.resolve(
        &answers(serde_json::json!({
            "operation": { "type": "choice", "choice": "tap", "confidence": 0.94 },
            "tap_target": {
                "type": "choice", "choice": "A2", "confidence": 0.34,
                "probabilities": { "A1": 0.32, "A2": 0.34, "A3": 0.34 }
            },
            "goal_met": { "type": "score", "score": 0.2, "confidence": 0.9 },
            "is_error_screen": { "type": "noul", "noul": 0.01 },
        })),
        &Floors::new(Confidence::new(0.6).expect("a floor")),
    );

    let Ok(Decision::Ready(act)) = decided else {
        panic!("the two Closes are one choice worth 0.68: {decided:?}");
    };
    assert!(matches!(act, Act::Tap(_)), "got {act:?}");
}

/// Rows that merely both look plausible are still a split. Only rows that
/// name the same thing are the same choice.
#[test]
fn mass_split_across_rows_of_different_names_stays_uncertainty() {
    let snapshot = Android.parse_hierarchy(SETTINGS).expect("fixture parses");
    let catalog = Catalog::for_screen(&snapshot, &Android);

    let decided = catalog.resolve(
        &answers(serde_json::json!({
            "operation": { "type": "choice", "choice": "tap", "confidence": 0.94 },
            "tap_target": {
                "type": "choice", "choice": "A2", "confidence": 0.34,
                "probabilities": { "A1": 0.32, "A2": 0.34, "A3": 0.34 }
            },
            "goal_met": { "type": "score", "score": 0.2, "confidence": 0.9 },
            "is_error_screen": { "type": "noul", "noul": 0.01 },
        })),
        &Floors::new(Confidence::new(0.6).expect("a floor")),
    );

    assert!(
        matches!(decided, Err(Indecision::TooUncertain { .. })),
        "three different rows are three choices: {decided:?}",
    );
}

// ── The browser's vocabulary ────────────────────────────────────────────────

mod browser {
    use jev_pilot::act::{Catalog, Decision, DialogReply, Indecision, Operation};
    use jev_pilot::platform::{HierarchyError, Platform};
    use jev_pilot::snapshot::{
        Bounds, Dialog, DialogKind, Element, NodeKey, Snapshot, Tab, TabKey,
    };

    /// A platform with the browser's operations, standing in for the real one.
    struct Web;

    impl Platform for Web {
        fn name(&self) -> &'static str {
            "Web"
        }
        fn operations(&self) -> &'static [Operation] {
            &[
                Operation::Tap,
                Operation::TypeText,
                Operation::Submit,
                Operation::ScrollUp,
                Operation::ScrollDown,
                Operation::Back,
                Operation::Navigate,
                Operation::SwitchTab,
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

    fn row(label: &str, editable: bool, top: i32) -> Element {
        Element {
            label: label.into(),
            detail: None,
            editable,
            bounds: Bounds::from_origin_size(0, top, 100, 20),
        }
    }

    fn page() -> Snapshot {
        Snapshot::new(vec![row("Search", true, 0), row("Go", false, 40)])
            .expect("fits")
            .at_url(Some("https://example.test/".into()))
    }

    fn tab(key: &str, title: &str) -> Tab {
        Tab {
            key: TabKey::new(key),
            title: title.into(),
            url: format!("https://example.test/{key}").into(),
        }
    }

    fn dialog(kind: DialogKind) -> Dialog {
        Dialog {
            kind,
            message: "Delete this draft?".into(),
            default_prompt: None,
        }
    }

    fn offered(catalog: &Catalog) -> Vec<Operation> {
        catalog.operations().to_vec()
    }

    /// Switching tab is only an option when there is another tab to switch to.
    #[test]
    fn switching_tab_is_offered_only_when_another_tab_exists() {
        let alone = Catalog::for_screen(&page(), &Web);
        assert!(!offered(&alone).contains(&Operation::SwitchTab));

        let with_one = page().with_tabs(vec![tab("t2", "Results")]);
        let catalog = Catalog::for_screen(&with_one, &Web);
        assert!(offered(&catalog).contains(&Operation::SwitchTab));
    }

    /// Navigating is only an option when a supplied address is somewhere else.
    #[test]
    fn navigating_is_offered_only_when_a_supplied_address_is_elsewhere() {
        let here: Box<str> = "https://example.test/".into();
        let there: Box<str> = "https://example.test/other".into();

        let nothing = Catalog::for_screen(&page(), &Web);
        assert!(!offered(&nothing).contains(&Operation::Navigate));

        let only_here =
            Catalog::for_screen(&page(), &Web).navigating_to(std::slice::from_ref(&here));
        assert!(!offered(&only_here).contains(&Operation::Navigate));

        let elsewhere = Catalog::for_screen(&page(), &Web).navigating_to(&[here, there.clone()]);
        assert!(offered(&elsewhere).contains(&Operation::Navigate));
        let Decision::Ready(act) = elsewhere
            .act_from(Operation::Navigate, Some(0))
            .expect("the one address elsewhere is offered")
        else {
            panic!("navigating needs no words");
        };
        assert_eq!(act, jev_pilot::act::Act::Navigate(there));
    }

    /// The page cannot be touched under a dialog, so the dialog's own answers
    /// are what is offered, with waiting and the verdicts.
    #[test]
    fn under_a_dialog_only_its_answers_and_the_verdicts_are_offered() {
        let under = page()
            .with_tabs(vec![tab("t2", "Results")])
            .with_dialog(Some(dialog(DialogKind::Confirm)));
        let catalog = Catalog::for_screen(&under, &Web)
            .accepting_text()
            .navigating_to(&["https://example.test/other".into()])
            .returning_to(Some("elsewhere"));

        let mut got = offered(&catalog);
        got.sort_by_key(|operation| operation.key());
        let mut want = vec![
            Operation::AcceptDialog,
            Operation::DismissDialog,
            Operation::Wait,
            Operation::Done,
            Operation::Blocked,
        ];
        want.sort_by_key(|operation| operation.key());
        assert_eq!(got, want);
        assert!(
            catalog.tap_target_question("Delete the draft").is_none(),
            "no row can be tapped under a dialog, so none is asked about"
        );
    }

    /// And the answers are only offered under one.
    #[test]
    fn the_dialog_operations_are_not_offered_without_a_dialog() {
        let catalog = Catalog::for_screen(&page(), &Web);
        assert!(!offered(&catalog).contains(&Operation::AcceptDialog));
        assert!(!offered(&catalog).contains(&Operation::DismissDialog));
    }

    /// A confirm is accepted as it stands, and dismissing it needs no words.
    #[test]
    fn a_confirm_is_answered_without_words() {
        let under = page().with_dialog(Some(dialog(DialogKind::Confirm)));
        let catalog = Catalog::for_screen(&under, &Web);

        assert_eq!(
            catalog.act_from(Operation::AcceptDialog, None),
            Ok(Decision::Ready(jev_pilot::act::Act::Dialog(
                DialogReply::Accept { words: None }
            ))),
        );
        assert_eq!(
            catalog.act_from(Operation::DismissDialog, None),
            Ok(Decision::Ready(jev_pilot::act::Act::Dialog(
                DialogReply::Dismiss
            ))),
        );
    }

    /// A prompt is answered with words, and a System One model selects and does
    /// not write: accepting one needs text from elsewhere, so it is offered
    /// only where text can be supplied, and comes back needing it.
    #[test]
    fn a_prompt_is_accepted_only_with_words_from_elsewhere() {
        let under = page().with_dialog(Some(dialog(DialogKind::Prompt)));

        let mute = Catalog::for_screen(&under, &Web);
        assert!(!offered(&mute).contains(&Operation::AcceptDialog));
        assert!(offered(&mute).contains(&Operation::DismissDialog));
        assert_eq!(
            mute.act_from(Operation::AcceptDialog, None),
            Err(Indecision::NotOffered {
                what: "accept_dialog".into()
            }),
        );

        let writing = Catalog::for_screen(&under, &Web).accepting_text();
        assert!(offered(&writing).contains(&Operation::AcceptDialog));
        assert_eq!(
            writing.act_from(Operation::AcceptDialog, None),
            Ok(Decision::NeedsPromptText),
        );
    }

    /// A platform that does not list Return never gets it, whatever the
    /// screen says about where the run started.
    #[test]
    fn return_is_never_offered_on_a_platform_that_does_not_list_it() {
        let catalog = Catalog::for_screen(&page(), &Web).returning_to(Some("com.elsewhere"));
        assert!(!offered(&catalog).contains(&Operation::Return));
    }

    /// A native drop-down is a field to type the option into, never something
    /// to tap: tapping it opens a pop-up nothing can read.
    #[test]
    fn a_field_only_row_is_a_field_and_never_a_tap_target() {
        let snapshot = Snapshot::new(vec![
            row("Country", false, 0),
            row("Search", true, 40),
            row("Go", false, 80),
        ])
        .expect("fits")
        .with_node_keys(vec![
            Some(NodeKey::new("1")),
            Some(NodeKey::new("2")),
            Some(NodeKey::new("3")),
        ])
        .with_field_only(vec![true, false, false]);
        let catalog = Catalog::for_screen(&snapshot, &Web).accepting_text();

        let fields: Vec<_> = catalog.fields_offered().map(|(_, text)| text).collect();
        assert_eq!(fields, ["Country", "Search"]);
        let rows: Vec<_> = catalog.rows().map(|(_, text)| text).collect();
        assert_eq!(rows, ["Search", "Go"]);
        assert_eq!(catalog.targets(), 2);

        // An escalation numbers every row, and may not tap this one either.
        assert_eq!(
            catalog.act_from(Operation::Tap, Some(0)),
            Err(Indecision::NotOffered {
                what: "row 0".into()
            }),
        );
        assert!(catalog.act_from(Operation::Tap, Some(1)).is_ok());
        assert!(matches!(
            catalog.act_from(Operation::TypeText, Some(0)),
            Ok(Decision::NeedsText { .. })
        ));
    }
}

mod browser_heads {
    use jev_pilot::act::{Act, Catalog, Decision, Floors, Indecision, Operation};
    use jev_pilot::judgment::Confidence;
    use jev_pilot::platform::{HierarchyError, Platform};
    use jev_pilot::snapshot::{Bounds, Element, Snapshot, Tab, TabKey};
    use jev_pilot::step::{StepAnswers, StepQuestions};

    struct Web;

    impl Platform for Web {
        fn name(&self) -> &'static str {
            "Web"
        }
        fn operations(&self) -> &'static [Operation] {
            &[
                Operation::Tap,
                Operation::Navigate,
                Operation::SwitchTab,
                Operation::Wait,
                Operation::Done,
                Operation::Blocked,
            ]
        }
        fn parse_hierarchy(&self, _raw: &str) -> Result<Snapshot, HierarchyError> {
            unreachable!("these tests build snapshots by hand")
        }
    }

    fn page(tabs: usize) -> Snapshot {
        Snapshot::new(vec![Element {
            label: "Go".into(),
            detail: None,
            editable: false,
            bounds: Bounds::from_origin_size(0, 0, 100, 20),
        }])
        .expect("fits")
        .at_url(Some("https://example.test/".into()))
        .with_tabs(
            (0..tabs)
                .map(|n| Tab {
                    key: TabKey::new(format!("t{n}")),
                    title: format!("Tab {n}").into(),
                    url: format!("https://example.test/{n}").into(),
                })
                .collect(),
        )
    }

    fn urls(n: usize) -> Vec<Box<str>> {
        (0..n)
            .map(|k| format!("https://example.test/dest{k}").into())
            .collect()
    }

    fn answers(value: serde_json::Value) -> StepAnswers {
        serde_json::from_value(value).expect("answers parse")
    }

    fn base(operation: &str) -> serde_json::Value {
        serde_json::json!({
            "operation": { "type": "choice", "choice": operation, "confidence": 0.99 },
            "goal_met": { "type": "score", "score": 0.2, "confidence": 0.9 },
            "is_error_screen": { "type": "noul", "noul": 0.01 },
        })
    }

    /// With several tabs or addresses the choice is a head of its own, asked
    /// in the same request, and its field name is the wire key.
    #[test]
    fn the_tab_and_destination_heads_are_asked_when_there_is_more_than_one_candidate() {
        let snapshot = page(2);
        let catalog = Catalog::for_screen(&snapshot, &Web).navigating_to(&urls(2));

        let wire = serde_json::to_value(StepQuestions::new("Find it", &catalog)).expect("wire");

        assert_eq!(wire["tab"]["type"], "choice");
        assert_eq!(wire["destination"]["type"], "choice");
        let tabs = wire["tab"]["criteria"].as_object().expect("a map");
        assert_eq!(tabs.len(), 2);
        assert!(tabs["A1"].as_str().expect("words").contains("Tab 0"));
        assert!(
            tabs["A1"]
                .as_str()
                .expect("words")
                .contains("https://example.test/0")
        );
        assert_eq!(
            wire["destination"]["criteria"]["A2"],
            "https://example.test/dest1"
        );
    }

    /// With exactly one candidate the head is skipped and that candidate used.
    #[test]
    fn a_single_candidate_is_used_without_being_asked_about() {
        let snapshot = page(1);
        let catalog = Catalog::for_screen(&snapshot, &Web).navigating_to(&urls(1));

        let wire = serde_json::to_value(StepQuestions::new("Find it", &catalog)).expect("wire");
        assert!(wire.get("tab").is_none());
        assert!(wire.get("destination").is_none());

        let floors = Floors::new(Confidence::ZERO);
        let switched = catalog.resolve(&answers(base("switch_tab")), &floors);
        assert!(
            matches!(switched, Ok(Decision::Ready(Act::SwitchTab(_)))),
            "{switched:?}"
        );
        assert_eq!(
            catalog.resolve(&answers(base("navigate")), &floors),
            Ok(Decision::Ready(Act::Navigate(
                "https://example.test/dest0".into()
            ))),
        );
    }

    /// With none to switch to, neither head is asked.
    #[test]
    fn no_head_is_asked_about_an_operation_that_is_not_offered() {
        let catalog = Catalog::for_screen(&page(0), &Web);
        let wire = serde_json::to_value(StepQuestions::new("Find it", &catalog)).expect("wire");
        assert!(wire.get("tab").is_none());
        assert!(wire.get("destination").is_none());
    }

    /// The head's answer picks which, and is held to the floor like a row.
    #[test]
    fn the_chosen_tab_and_destination_are_the_ones_acted_on() {
        let snapshot = page(3);
        let catalog = Catalog::for_screen(&snapshot, &Web).navigating_to(&urls(3));
        let floors = Floors::new(Confidence::new(0.6).expect("floor"));

        let mut tab = base("switch_tab");
        tab["tab"] = serde_json::json!({ "type": "choice", "choice": "A3", "confidence": 0.9 });
        let third = snapshot.tabs().nth(2).expect("three tabs").0;
        assert_eq!(
            catalog.resolve(&answers(tab.clone()), &floors),
            Ok(Decision::Ready(Act::SwitchTab(third))),
        );

        tab["tab"]["confidence"] = serde_json::json!(0.3);
        assert!(matches!(
            catalog.resolve(&answers(tab), &floors),
            Err(Indecision::TooUncertain { .. })
        ));

        let mut destination = base("navigate");
        destination["destination"] =
            serde_json::json!({ "type": "choice", "choice": "A2", "confidence": 0.9 });
        assert_eq!(
            catalog.resolve(&answers(destination), &floors),
            Ok(Decision::Ready(Act::Navigate(
                "https://example.test/dest1".into()
            ))),
        );

        // Naming nothing at all is not a guess to act on.
        assert_eq!(
            catalog.resolve(&answers(base("navigate")), &floors),
            Err(Indecision::NoTarget),
        );
    }

    /// An escalation numbers tabs and addresses on their own.
    #[test]
    fn an_escalation_numbers_tabs_and_destinations_separately_from_rows() {
        let snapshot = page(2);
        let catalog = Catalog::for_screen(&snapshot, &Web).navigating_to(&urls(2));
        let second = snapshot.tabs().nth(1).expect("two tabs").0;

        assert_eq!(
            catalog.act_from(Operation::SwitchTab, Some(1)),
            Ok(Decision::Ready(Act::SwitchTab(second))),
        );
        assert_eq!(
            catalog.act_from(Operation::Navigate, Some(1)),
            Ok(Decision::Ready(Act::Navigate(
                "https://example.test/dest1".into()
            ))),
        );
        assert_eq!(
            catalog.act_from(Operation::SwitchTab, Some(2)),
            Err(Indecision::NotOffered {
                what: "tab 2".into()
            }),
        );
        assert_eq!(
            catalog.act_from(Operation::Navigate, None),
            Err(Indecision::NoTarget),
        );
    }
}
