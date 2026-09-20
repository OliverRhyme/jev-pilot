//! Drive a device, handing every impasse to an external decider over files.
//!
//! ```sh
//! TYPESAFE_API_KEY=... HELPER_TOKEN=... \
//!   cargo run --example handoff -- <serial> "<goal>" /tmp/handoff
//! ```
//!
//! On an impasse the run writes `<dir>/ask.json` and waits for
//! `<dir>/answer.json`. Whatever writes that file is System Two: a reasoning
//! model, a person at another terminal, an agent. The mechanism does not care
//! which, and neither does the loop — it resumes either way.
//!
//! The answer is resolved through the same catalog Jev was offered, so an
//! external decider inherits every constraint: it cannot name an operation the
//! platform lacks, a row that is not on screen, or a coordinate.
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
use std::time::{Duration, Instant};

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

/// Write a question, wait for the answer file, remove both.
fn exchange(dir: &Path, question: &serde_json::Value) -> std::io::Result<serde_json::Value> {
    let ask = dir.join("ask.json");
    let answer = dir.join("answer.json");
    let _ = std::fs::remove_file(&answer);

    let mut file = std::fs::File::create(&ask)?;
    file.write_all(serde_json::to_string_pretty(question)?.as_bytes())?;
    file.write_all(b"\n")?;
    drop(file);

    let deadline = Instant::now() + Duration::from_secs(300);
    while Instant::now() < deadline {
        if let Ok(raw) = std::fs::read_to_string(&answer)
            && let Ok(parsed) = serde_json::from_str(&raw)
        {
            let _ = std::fs::remove_file(&ask);
            let _ = std::fs::remove_file(&answer);
            return Ok(parsed);
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    let _ = std::fs::remove_file(&ask);
    Err(std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        "no answer arrived",
    ))
}

fn main() -> Result<(), Box<dyn core::error::Error>> {
    let mut args = std::env::args().skip(1);
    let serial = args.next().ok_or("usage: handoff <serial> <goal> <dir>")?;
    let goal = args.next().ok_or("usage: handoff <serial> <goal> <dir>")?;
    let dir = PathBuf::from(args.next().unwrap_or_else(|| "/tmp/handoff".to_owned()));
    // Everything after the directory is an acceptance criterion: a specific
    // claim that must hold before success is accepted.
    let criteria: Vec<String> = args.collect();
    std::fs::create_dir_all(&dir)?;

    let mut device = AdbDevice::new(serial);
    if let Ok(token) = std::env::var("HELPER_TOKEN") {
        device = device.through_helper(18888, "/dump_xml", Some(token.trim()))?;
    }

    let ask_dir = dir.clone();
    let write_dir = dir.clone();
    let mut pilot = Pilot::new(device, SystemOne::new(ApiKey::from_env()?), &Android)
        .requiring(Confidence::new(0.6).ok_or("floor must be a probability")?)
        .confirming(criteria.clone())
        .limited_to(15)
        .escalating_to(move |impasse: &Impasse<'_>| {
            let reply = exchange(
                &ask_dir,
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
            )?;

            let Some(name) = reply.get("operation").and_then(serde_json::Value::as_str) else {
                return Ok(Resolution::Stop);
            };
            let Some(operation) = OPERATIONS.iter().find(|o| o.key() == name).copied() else {
                return Ok(Resolution::Stop);
            };
            Ok::<_, std::io::Error>(Resolution::Choose {
                operation,
                target: reply
                    .get("target")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|n| usize::try_from(n).ok()),
            })
        })
        .writing_with(move |request: &Writing<'_>| {
            let reply = exchange(
                &write_dir,
                &serde_json::json!({
                    "kind": "what_to_type",
                    "step": request.step,
                    "goal": request.goal,
                    "field": request.field.describe(),
                    "previous_action": request.previous,
                    "rows": request.rows,
                }),
            )?;
            Ok::<_, std::io::Error>(
                reply
                    .get("text")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .into(),
            )
        })
        .watching(|step| {
            println!(
                "step {}  {} rows  goal_met {:.2}  op {:.2}  {}",
                step.index,
                step.rows.len(),
                step.goal_met,
                step.operation_confidence.get(),
                step.chosen
                    .map_or_else(|| "refused".to_owned(), |act| format!("{act:?}"))
            );
        });

    for criterion in &criteria {
        println!("must    : {criterion}");
    }
    println!("ending : {:?}", pilot.pursue(&goal)?);
    Ok(())
}
