//! Drive a real Android device toward a goal.
//!
//! ```sh
//! TYPESAFE_API_KEY=... cargo run --example drive -- <serial> "<goal>" [criteria...]
//! ```
//!
//! # One loop, two ways to answer it
//!
//! There is a single loop and a single escalation seam. Jev decides while it
//! is confident; when it is not, the run asks — and the question goes to *both*
//! the terminal and a file at once. Whichever answers first resolves it.
//!
//! That is what makes System Two swappable without restarting. A person can
//! watch a run and type an answer; a reasoning model or another agent can
//! watch the same directory and write `answer.json`; either can take over from
//! the other mid-run, at any step, with the other still able to answer the
//! next one. Neither is configured in advance, because a run cannot know in
//! advance which of them will be at the keyboard.
//!
//! Either way the answer resolves through the same catalog Jev was offered, so
//! an outside decider inherits every constraint: it cannot name an operation
//! the platform lacks, a row that is not on screen, or a coordinate.
use core::fmt::Write as _;
use jev_pilot::{
    act::Operation,
    client::http::SystemOne,
    credential::ApiKey,
    device::adb::AdbDevice,
    judgment::Confidence,
    pilot::{Impasse, Pilot, Resolution, Writing},
    platform::Android,
};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

const OPERATIONS: &[Operation] = &[
    Operation::Tap,
    Operation::TypeText,
    Operation::DoubleTap,
    Operation::LongPress,
    Operation::SwipeLeft,
    Operation::SwipeRight,
    Operation::ScrollUp,
    Operation::ScrollDown,
    Operation::Back,
    Operation::Home,
    Operation::AppSwitcher,
    Operation::Submit,
    Operation::Wait,
    Operation::Done,
    Operation::Blocked,
];

/// Where a question goes, and where an answer may come from.
///
/// Both channels are live for every question. The terminal is read on its own
/// thread, because a blocking read there would stop the file from being
/// noticed, and the whole point is that either may answer.
struct Desk {
    dir: PathBuf,
    typed: Receiver<String>,
}

impl Desk {
    fn new(dir: PathBuf) -> std::io::Result<Self> {
        std::fs::create_dir_all(&dir)?;
        let (sender, typed) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in std::io::stdin().lines() {
                let Ok(line) = line else { return };
                if sender.send(line).is_err() {
                    return;
                }
            }
        });
        Ok(Self { dir, typed })
    }

    /// Put a question to both channels and wait for the first answer.
    fn ask(&self, question: &serde_json::Value, prompt: &str) -> std::io::Result<Answer> {
        let ask = self.dir.join("ask.json");
        let answer = self.dir.join("answer.json");
        let _ = std::fs::remove_file(&answer);
        std::fs::write(&ask, serde_json::to_string_pretty(question)?)?;

        print!("{prompt}");
        std::io::stdout().flush()?;

        // Drain anything typed before the question existed: it answered
        // something else.
        while self.typed.try_recv().is_ok() {}

        loop {
            if let Ok(raw) = std::fs::read_to_string(&answer)
                && let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&raw)
            {
                let _ = std::fs::remove_file(&ask);
                let _ = std::fs::remove_file(&answer);
                println!("[answered from {}]", answer.display());
                return Ok(Answer::File(parsed));
            }
            match self.typed.try_recv() {
                Ok(line) => {
                    let _ = std::fs::remove_file(&ask);
                    return Ok(Answer::Typed(line));
                }
                Err(TryRecvError::Empty) => std::thread::sleep(Duration::from_millis(100)),
                // The terminal is gone; the file is still a way to answer.
                Err(TryRecvError::Disconnected) => {
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }
    }

    /// Put an impasse to whoever is there, and resolve their answer.
    fn choose(&self, impasse: &Impasse<'_>) -> Result<Resolution, std::io::Error> {
        let reply = self.ask(
            &serde_json::json!({
                "kind": "which_action",
                "step": impasse.step,
                "goal": impasse.goal,
                "why": impasse.because.to_string(),
                "leaning": impasse.leaning.key(),
                "operation_confidence": impasse.operation_confidence.get(),
                "row_confidence": impasse.target_confidence.map(Confidence::get),
                "torn_among": impasse.alternatives.iter().take(5)
                    .map(|(n, p)| serde_json::json!([n, p])).collect::<Vec<_>>(),
                "previous_action": impasse.previous,
                "operations": impasse.operations.iter().map(|o| o.key()).collect::<Vec<_>>(),
                "rows": impasse.rows,
            }),
            &describe(impasse),
        )?;
        Ok(match reply {
            Answer::Typed(line) => parse_typed(&line).unwrap_or(Resolution::Stop),
            Answer::File(value) => resolve_json(&value),
        })
    }

    /// Ask what belongs in a field.
    fn compose(&self, request: &Writing<'_>) -> Result<Box<str>, std::io::Error> {
        let prompt = format!(
            "\n  \u{2500}\u{2500} step {}: what should go in {}?\n     goal: {}\n     text: ",
            request.step,
            request.field.describe(),
            request.goal,
        );
        let reply = self.ask(
            &serde_json::json!({
                "kind": "what_to_type",
                "step": request.step,
                "goal": request.goal,
                "field": request.field.describe(),
                "previous_action": request.previous,
                "rows": request.rows,
            }),
            &prompt,
        )?;
        let text = match reply {
            Answer::Typed(line) => line.trim().to_owned(),
            Answer::File(value) => value
                .get("text")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        };
        Ok(Box::<str>::from(text))
    }
}

/// An answer written as JSON, resolved through the same catalog a typed one is.
fn resolve_json(value: &serde_json::Value) -> Resolution {
    let Some(name) = value.get("operation").and_then(serde_json::Value::as_str) else {
        return Resolution::Stop;
    };
    let Some(operation) = OPERATIONS.iter().find(|o| o.key() == name).copied() else {
        return Resolution::Stop;
    };
    Resolution::Choose {
        operation,
        target: value
            .get("target")
            .and_then(serde_json::Value::as_u64)
            .and_then(|n| usize::try_from(n).ok()),
    }
}

enum Answer {
    Typed(String),
    File(serde_json::Value),
}

/// Read `tap 3`, `back`, `done` and the like, as a person would type them.
fn parse_typed(line: &str) -> Option<Resolution> {
    let mut words = line.split_whitespace();
    let name = words.next()?;
    if name == "stop" {
        return Some(Resolution::Stop);
    }
    let operation = OPERATIONS.iter().find(|o| o.key() == name).copied()?;
    Some(Resolution::Choose {
        operation,
        target: words.next().and_then(|n| n.parse().ok()),
    })
}

fn main() -> Result<(), Box<dyn core::error::Error>> {
    let mut args = std::env::args().skip(1);
    let serial = args
        .next()
        .ok_or("usage: drive <serial> <goal> [criteria...]")?;
    let goal = args
        .next()
        .ok_or("usage: drive <serial> <goal> [criteria...]")?;
    // Everything after the goal is an acceptance criterion: a specific claim
    // that must hold before success is accepted.
    let criteria: Vec<String> = args.collect();

    let dir = std::env::var("DRIVE_DESK").map_or_else(
        |_| std::env::temp_dir().join("jev-pilot-desk"),
        PathBuf::from,
    );
    let desk = Rc::new(Desk::new(dir.clone())?);

    // The helper reads a screen in ~60ms where `uiautomator dump` takes ~2.5s,
    // and dispatches gestures in-process. Absent, everything still works:
    // `cargo run --example helper -- <serial> install` puts one on a device.
    let device = AdbDevice::new(serial).with_helper()?;
    println!("device : {}", device.serial());
    match device.reader().why() {
        None => println!("screens: accessibility helper"),
        Some(why) => println!("screens: uiautomator CLI ({why})"),
    }
    println!("goal   : {goal}");
    println!(
        "asking : this terminal, or {}",
        dir.join("answer.json").display()
    );
    if !criteria.is_empty() {
        println!("accept : {}", criteria.join(" / "));
    }
    println!();

    let judge = SystemOne::new(ApiKey::from_env()?);
    let floor = Confidence::new(0.6).ok_or("floor must be a probability")?;

    let choosing = Rc::clone(&desk);
    let writing = Rc::clone(&desk);

    let mut pilot = Pilot::new(device, judge, &Android)
        .requiring(floor)
        .confirming(criteria)
        .limited_to(15)
        .escalating_to(move |impasse: &Impasse<'_>| choosing.choose(impasse))
        .writing_with(move |request: &Writing<'_>| writing.compose(request))
        .watching(|step| {
            println!(
                "step {}  {} rows  goal_met {}  error {:.2}",
                step.index,
                step.rows.len(),
                step.goal_met,
                step.is_error_screen
            );
            let target = step
                .target_confidence
                .map_or_else(|| "-".to_owned(), |c| format!("{:.2}", c.get()));
            match step.chosen {
                Some(act) => println!(
                    "   -> {act:?}   op {:.2} / target {target}",
                    step.operation_confidence.get()
                ),
                None => println!(
                    "   -> refused   op {:.2} / target {target}",
                    step.operation_confidence.get()
                ),
            }
        });

    let ending = pilot.pursue(&goal)?;
    println!("\nending : {ending:?}");
    let _ = std::fs::remove_file(Path::new(&dir).join("ask.json"));
    Ok(())
}

/// The impasse, written out for whoever is reading the terminal.
fn describe(impasse: &Impasse<'_>) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "\n  ── step {}: {}", impasse.step, impasse.because);
    let _ = writeln!(out, "     goal      : {}", impasse.goal);
    if let Some(previous) = impasse.previous {
        let _ = writeln!(out, "     last did  : {previous}");
    }
    let _ = write!(
        out,
        "     leaning   : {} at {:.2}",
        impasse.leaning,
        impasse.operation_confidence.get()
    );
    match impasse.target_confidence {
        Some(target) => {
            let _ = writeln!(out, ", row at {:.2}", target.get());
        }
        None => {
            let _ = writeln!(out);
        }
    }
    for (index, row) in impasse.rows.iter().enumerate() {
        let _ = writeln!(out, "     [{index}] {row}");
    }
    let _ = write!(
        out,
        "     tap <n> | type <n> | back | scroll_down | done | stop: "
    );
    out
}
