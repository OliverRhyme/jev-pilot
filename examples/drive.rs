//! Drive a real Android device toward a goal.
//!
//! ```sh
//! TYPESAFE_API_KEY=... cargo run --example drive -- <serial> "Open Wi-Fi settings"
//! ```
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

/// Hand an impasse to whoever is at the terminal.
///
/// The person picks from exactly the options Jev was offered, so escalating
/// widens who decides without widening what may happen.
fn ask_the_operator(impasse: &Impasse<'_>) -> Result<Resolution, std::io::Error> {
    println!("\n  ── step {}: {}", impasse.step, impasse.because);
    println!("     goal      : {}", impasse.goal);
    if let Some(previous) = impasse.previous {
        println!("     last did  : {previous}");
    }
    print!(
        "     leaning   : {} at {:.2}",
        impasse.leaning,
        impasse.operation_confidence.get()
    );
    if let Some(target) = impasse.target_confidence {
        print!(", row at {:.2}", target.get());
    }
    println!();
    let near: Vec<String> = impasse
        .alternatives
        .iter()
        .take(3)
        .map(|(name, p)| format!("{name} {p:.2}"))
        .collect();
    if !near.is_empty() {
        println!("     torn among: {}", near.join(" · "));
    }
    for (index, row) in impasse.rows.iter().enumerate() {
        println!("     [{index}] {row}");
    }
    print!("     tap <n> | type <n> | submit | back | scroll_down | done | stop: ");
    std::io::stdout().flush()?;

    let mut line = String::new();
    if std::io::stdin().read_line(&mut line)? == 0 {
        return Ok(Resolution::Stop);
    }
    let mut words = line.split_whitespace();
    let pick = |op| Resolution::Choose {
        operation: op,
        target: None,
    };
    Ok(match (words.next(), words.next()) {
        (Some("tap"), Some(n)) => Resolution::Choose {
            operation: Operation::Tap,
            target: n.parse().ok(),
        },
        (Some("type"), Some(n)) => Resolution::Choose {
            operation: Operation::TypeText,
            target: n.parse().ok(),
        },
        (Some("submit"), _) => pick(Operation::Submit),
        (Some("back"), _) => pick(Operation::Back),
        (Some("scroll_down"), _) => pick(Operation::ScrollDown),
        (Some("done"), _) => pick(Operation::Done),
        _ => Resolution::Stop,
    })
}

/// Write the words Jev cannot. A person here; a reasoning model in a real run.
fn write_the_text(request: &Writing<'_>) -> Result<Box<str>, std::io::Error> {
    println!(
        "\n  ── step {} wants to type into: {}",
        request.step,
        request.field.describe()
    );
    println!("     goal: {}", request.goal);
    print!("     text to type: ");
    std::io::stdout().flush()?;

    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(line.trim().into())
}

fn main() -> Result<(), Box<dyn core::error::Error>> {
    let mut args = std::env::args().skip(1);
    let serial = args.next().ok_or("usage: drive <serial> <goal>")?;
    let goal = args.next().ok_or("usage: drive <serial> <goal>")?;

    // The helper reads a screen in ~50ms where `uiautomator dump` takes ~2.5s,
    // and dispatches gestures in-process. Absent, everything still works.
    // `cargo run --example helper -- <serial> install` puts one on a device.
    let device = AdbDevice::new(serial).with_helper()?;
    match device.reader().why() {
        None => println!("screens : accessibility helper"),
        Some(why) => println!("screens : uiautomator CLI ({why})"),
    }
    let judge = SystemOne::new(ApiKey::from_env()?);
    let floor = Confidence::new(0.6).ok_or("floor must be a probability")?;

    println!("device : {}", device.serial());
    println!("goal   : {goal}\n");

    let mut pilot = Pilot::new(device, judge, &Android)
        .escalating_to(ask_the_operator)
        .writing_with(write_the_text)
        .requiring(floor)
        .limited_to(10)
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
            if let Some(act) = step.chosen {
                println!(
                    "   -> {act:?}   op {:.2} / target {target}",
                    step.operation_confidence.get()
                );
            } else {
                println!(
                    "   -> refused   op {:.2} / target {target}",
                    step.operation_confidence.get()
                );
                for row in &step.rows {
                    println!("        {row}");
                }
            }
        });

    let ending = pilot.pursue(&goal)?;
    println!("\nending : {ending:?}");
    Ok(())
}
