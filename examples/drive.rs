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
    pilot::{Impasse, Pilot, Resolution},
    platform::Android,
};
use std::io::Write as _;

/// Hand an impasse to whoever is at the terminal.
///
/// The person picks from exactly the options Jev was offered, so escalating
/// widens who decides without widening what may happen.
fn ask_the_operator(impasse: &Impasse<'_>) -> Result<Resolution, std::io::Error> {
    println!(
        "\n  ── step {} needs a second opinion: {}",
        impasse.step, impasse.because
    );
    if let Some(previous) = impasse.previous {
        println!("     last did: {previous}");
    }
    for (index, row) in impasse.rows.iter().enumerate() {
        println!("     [{index}] {row}");
    }
    print!("     tap <n>, back, scroll_down, done, or stop: ");
    std::io::stdout().flush()?;

    let mut line = String::new();
    if std::io::stdin().read_line(&mut line)? == 0 {
        return Ok(Resolution::Stop);
    }
    let mut words = line.split_whitespace();
    Ok(match (words.next(), words.next()) {
        (Some("tap"), Some(n)) => Resolution::Choose {
            operation: Operation::Tap,
            target: n.parse().ok(),
        },
        (Some("back"), _) => Resolution::Choose {
            operation: Operation::Back,
            target: None,
        },
        (Some("scroll_down"), _) => Resolution::Choose {
            operation: Operation::ScrollDown,
            target: None,
        },
        (Some("done"), _) => Resolution::Choose {
            operation: Operation::Done,
            target: None,
        },
        _ => Resolution::Stop,
    })
}

fn main() -> Result<(), Box<dyn core::error::Error>> {
    let mut args = std::env::args().skip(1);
    let serial = args.next().ok_or("usage: drive <serial> <goal>")?;
    let goal = args.next().ok_or("usage: drive <serial> <goal>")?;

    let device = AdbDevice::new(serial);
    let judge = SystemOne::new(ApiKey::from_env()?);
    let floor = Confidence::new(0.6).ok_or("floor must be a probability")?;

    println!("device : {}", device.serial());
    println!("goal   : {goal}\n");

    let mut pilot = Pilot::new(device, judge, &Android)
        .escalating_to(ask_the_operator)
        .requiring(floor)
        .limited_to(6)
        .watching(|step| {
            println!(
                "step {}  {} rows  goal_met {:.2}  error {:.2}",
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
