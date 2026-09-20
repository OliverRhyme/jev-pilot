//! Drive a device with a programmatic second opinion instead of a person.
//!
//! ```sh
//! TYPESAFE_API_KEY=... HELPER_TOKEN=... \
//!   cargo run --example assisted -- <serial> "<goal>" "<text to type>"
//! ```
//!
//! The escalation here is a crude stand-in for a reasoning model: it scores
//! each row by how much of the goal's wording it shares. That is far less than
//! an LLM would bring, and it is the point — it shows that the seam needs only
//! *something* able to choose among options Jev already enumerated, and that
//! the loop keeps going afterwards rather than ending at the first impasse.
use jev_pilot::{
    act::Operation,
    client::http::SystemOne,
    credential::ApiKey,
    device::adb::AdbDevice,
    judgment::Confidence,
    pilot::{Impasse, Pilot, Resolution, Writing},
    platform::Android,
};
use std::convert::Infallible;

/// Every operation, so a name from the distribution can be turned back into one.
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

/// How much of the goal's wording a row echoes.
fn overlap(row: &str, goal: &str) -> usize {
    let words: Vec<String> = goal
        .split_whitespace()
        .filter(|w| w.len() > 3)
        .map(str::to_lowercase)
        .collect();
    let row = row.to_lowercase();
    words.iter().filter(|w| row.contains(w.as_str())).count()
}

fn main() -> Result<(), Box<dyn core::error::Error>> {
    let mut args = std::env::args().skip(1);
    let serial = args
        .next()
        .ok_or("usage: assisted <serial> <goal> [text]")?;
    let goal = args
        .next()
        .ok_or("usage: assisted <serial> <goal> [text]")?;
    let text = args.next().unwrap_or_else(|| goal.clone());

    let mut device = AdbDevice::new(serial);
    if let Ok(token) = std::env::var("HELPER_TOKEN") {
        device = device.through_helper(18888, "/dump_xml", Some(token.trim()))?;
    }

    let words = text.clone();
    let mut pilot = Pilot::new(device, SystemOne::new(ApiKey::from_env()?), &Android)
        .requiring(Confidence::new(0.6).ok_or("floor must be a probability")?)
        .limited_to(12)
        .writing_with(move |request: &Writing<'_>| {
            println!("     writing {:?} into {}", words, request.field.describe());
            Ok::<_, Infallible>(words.as_str().into())
        })
        .escalating_to(|impasse: &Impasse<'_>| {
            // Confidence measures how concentrated the distribution is, which
            // is not the same as having a clear leader. A 0.55/0.10/0.08 spread
            // scores low and is not actually ambiguous; a 0.35/0.30 one is.
            // Taking a leader that doubles the runner-up is a general rule, not
            // a fact about this screen.
            println!(
                "     torn among: {}",
                impasse
                    .alternatives
                    .iter()
                    .take(4)
                    .map(|(n, p)| format!("{n} {p:.2}"))
                    .collect::<Vec<_>>()
                    .join(" · ")
            );
            if let [(top, first), (_, second), ..] = impasse.alternatives
                && *first >= second * 2.0
            {
                {
                    println!(
                        "     second opinion: {top} leads {first:.2} to {second:.2}, clear enough"
                    );
                    if let Some(operation) = OPERATIONS.iter().find(|o| o.key() == &**top) {
                        return Ok(Resolution::Choose {
                            operation: *operation,
                            target: impasse
                                .rows
                                .iter()
                                .enumerate()
                                .map(|(i, r)| (overlap(r, impasse.goal), i))
                                .max_by_key(|(score, _)| *score)
                                .map(|(_, i)| i),
                        });
                    }
                }
            }

            let best = impasse
                .rows
                .iter()
                .enumerate()
                .map(|(index, row)| (overlap(row, impasse.goal), index, row))
                .filter(|(score, ..)| *score > 0)
                .max_by_key(|(score, ..)| *score);

            // Only a row choice is second-guessed. When Jev was unsure about
            // the operation itself, wording overlap says nothing useful, and
            // guessing there would be worse than stopping.
            Ok::<_, Infallible>(match best {
                Some((score, index, row)) if impasse.leaning.needs_tap_target() => {
                    println!("     second opinion: row {index} ({row}), {score} words shared");
                    Resolution::Choose {
                        operation: Operation::Tap,
                        target: Some(index),
                    }
                }
                _ => {
                    println!("     second opinion: nothing matched the goal, stopping");
                    Resolution::Stop
                }
            })
        })
        .watching(|step| {
            println!(
                "step {}  {} rows  goal_met {:.2}",
                step.index,
                step.rows.len(),
                step.goal_met
            );
            match step.chosen {
                Some(act) => println!("   -> {act:?}  op {:.2}", step.operation_confidence.get()),
                None => println!("   -> refused  op {:.2}", step.operation_confidence.get()),
            }
        });

    println!("ending : {:?}", pilot.pursue(&goal)?);
    Ok(())
}
