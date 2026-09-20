//! Reading a command line. Pure: nothing here touches a device.

use jev_pilot::act::Operation;
use jev_pilot::cli::{Invocation, parse};

fn parsed(words: &[&str]) -> Invocation {
    parse(words.iter().map(|w| (*w).to_owned())).expect("a valid command line")
}

/// The common case is one argument: what you want done.
#[test]
fn a_goal_is_the_whole_command_line() {
    let Invocation::Run {
        goal,
        device,
        accept,
        ..
    } = parsed(&["Open the Wi-Fi settings"])
    else {
        panic!("expected a run");
    };

    assert_eq!(&*goal, "Open the Wi-Fi settings");
    assert!(device.is_none(), "one attached device needs no naming");
    assert!(accept.is_empty());
}

/// A goal is not optional: without one there is nothing to pursue, and
/// defaulting to something would drive a stranger's phone somewhere.
#[test]
fn a_run_without_a_goal_is_refused() {
    assert!(parse(Vec::new()).is_err());
    assert!(parse(["--device".to_owned(), "abc123".to_owned()]).is_err());
}

#[test]
fn a_device_and_acceptance_criteria_can_be_named() {
    let Invocation::Run {
        goal,
        device,
        accept,
        steps,
        floors,
        ..
    } = parsed(&[
        "--device",
        "abc123",
        "--accept",
        "Wi-Fi is on",
        "--accept",
        "The network list is showing",
        "--steps",
        "20",
        "--floor",
        "0.75",
        "Turn Wi-Fi on",
    ])
    else {
        panic!("expected a run");
    };

    assert_eq!(&*goal, "Turn Wi-Fi on");
    assert_eq!(device.as_deref(), Some("abc123"));
    assert_eq!(accept, ["Wi-Fi is on", "The network list is showing"]);
    assert_eq!(steps, 20);
    assert!((floors.for_operation(Operation::Tap).get() - 0.75).abs() < f64::EPSILON);
}

/// A floor outside 0..=1 is not a probability, and a step limit of zero would
/// end every run before it began.
#[test]
fn nonsense_limits_are_refused_rather_than_clamped() {
    assert!(parse(["--floor".to_owned(), "1.5".to_owned(), "go".to_owned()]).is_err());
    assert!(parse(["--floor".to_owned(), "high".to_owned(), "go".to_owned()]).is_err());
    assert!(parse(["--steps".to_owned(), "0".to_owned(), "go".to_owned()]).is_err());
}

/// A mistyped flag must not be swallowed as the goal: the run would then
/// pursue "--devce abc123" on whatever phone happened to be attached.
#[test]
fn an_unknown_flag_is_refused() {
    let error = parse(["--devce".to_owned(), "abc".to_owned(), "go".to_owned()]).unwrap_err();

    assert!(format!("{error}").contains("--devce"), "{error}");
}

#[test]
fn the_helper_is_managed_by_its_own_subcommand() {
    assert!(matches!(
        parsed(&["helper"]),
        Invocation::Helper { install: false, .. }
    ));
    assert!(matches!(
        parsed(&["helper", "install"]),
        Invocation::Helper { install: true, .. }
    ));
    assert!(matches!(
        parsed(&["helper", "install", "--device", "abc"]),
        Invocation::Helper {
            install: true,
            device: Some(_)
        }
    ));
}

#[test]
fn devices_and_help_are_their_own_subcommands() {
    assert!(matches!(parsed(&["devices"]), Invocation::Devices));
    assert!(matches!(parsed(&["--help"]), Invocation::Help));
    assert!(matches!(parsed(&["-h"]), Invocation::Help));
}

/// A goal that looks like a flag is still a goal once `--` has ended the
/// options, which is the only way to pursue one starting with a dash.
#[test]
fn a_goal_can_follow_the_end_of_options() {
    let Invocation::Run { goal, .. } = parsed(&["--", "--not-a-flag"]) else {
        panic!("expected a run");
    };
    assert_eq!(&*goal, "--not-a-flag");
}

/// Seeing what the loop believes is on screen had no route but provoking a
/// stall or leaving the tool for `uiautomator dump` — the slow reader the
/// helper exists to replace. It is the first thing wanted when a run behaves
/// oddly, and it costs no model call.
#[test]
fn the_catalog_can_be_looked_at_without_running_anything() {
    assert!(matches!(
        parsed(&["observe"]),
        Invocation::Observe { device: None }
    ));
    assert!(matches!(
        parsed(&["observe", "--device", "abc123"]),
        Invocation::Observe { device: Some(_) }
    ));
}

/// Looking is not driving: it takes no goal and refuses one, so a mistyped
/// command cannot quietly start acting on a phone.
#[test]
fn looking_at_a_screen_takes_no_goal() {
    assert!(parse(["observe".to_owned(), "Turn Wi-Fi on".to_owned()]).is_err());
}

/// `--floor` lowers what an ordinary gesture needs, because a run driving a
/// busy screen is asked about every second tap otherwise. It must not lower
/// what it takes to *end* the run: a verdict of "this cannot be done" is
/// final, and a run that gives up on a 0.41 guess has answered the question
/// wrongly rather than cheaply. Observed on a launcher, at step one.
#[test]
fn a_lowered_floor_does_not_let_a_run_quit_on_a_guess() {
    let Invocation::Run { floors, .. } = parse([
        "--floor".to_owned(),
        "0.2".to_owned(),
        "transfer some money".to_owned(),
    ])
    .expect("parses") else {
        panic!("a goal is a run");
    };

    assert!((floors.for_operation(Operation::Tap).get() - 0.2).abs() < f64::EPSILON);
    assert!(
        floors.for_operation(Operation::Blocked).get() >= 0.6,
        "ending the run keeps its own floor",
    );
    assert!(
        floors.for_operation(Operation::Home).get() >= 0.6,
        "leaving the app keeps its own floor",
    );
}

/// Raising the floor raises all of them: asking for more care means more care
/// everywhere, never less of it somewhere.
#[test]
fn a_raised_floor_raises_the_costly_actions_too() {
    let Invocation::Run { floors, .. } = parse([
        "--floor".to_owned(),
        "0.9".to_owned(),
        "transfer some money".to_owned(),
    ])
    .expect("parses") else {
        panic!("a goal is a run");
    };

    assert!((floors.for_operation(Operation::Blocked).get() - 0.9).abs() < f64::EPSILON);
    assert!((floors.for_operation(Operation::Tap).get() - 0.9).abs() < f64::EPSILON);
}
