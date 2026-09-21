//! The two-dimensional action space: what to do, and what to do it to.

use jev_pilot::act::{Act, Catalog, Decision, Direction, Floors, Operation, Outcome};
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
    let catalog = Catalog::for_screen(&launcher, &Android).returning_to(Some("com.android.settings"));

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
