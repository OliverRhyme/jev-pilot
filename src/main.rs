//! `jev-pilot`: drive an Android device toward a goal.
//!
//! ```sh
//! jev-pilot "Turn Wi-Fi on"
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
use std::io::Write as _;

use jev_pilot::{
    act::Catalog,
    cli::{self, Invocation},
    client::http::SystemOne,
    credential::ApiKey,
    desk::Desk,
    device::Device as _,
    device::adb::{Adb, AdbDevice},
    device::helper::BUNDLED,
    pilot::{Impasse, Pilot, Writing},
    platform::Android,
};
use std::path::PathBuf;
use std::rc::Rc;

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("jev-pilot: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn core::error::Error>> {
    match cli::parse(std::env::args().skip(1))? {
        Invocation::Help => {
            print!("{}", cli::USAGE);
            Ok(())
        }
        Invocation::Devices => list_devices(),
        Invocation::Observe { device } => observe(device.as_deref()),
        Invocation::Helper { device, install } => manage_helper(device.as_deref(), install),
        Invocation::Run {
            goal,
            device,
            accept,
            steps,
            floors,
            app,
            texts,
            desk,
        } => pursue(
            device.as_deref(),
            Plan {
                goal,
                accept,
                steps,
                floors,
                app,
                texts,
                desk_dir: desk,
            },
        ),
    }
}

/// The attached devices, as adb reports them.
fn attached() -> Result<Vec<String>, Box<dyn core::error::Error>> {
    let output = std::process::Command::new("adb")
        .args(Adb::devices_args())
        .output()
        .map_err(|error| format!("could not run adb: {error}"))?;
    Ok(Adb::parse_devices(&String::from_utf8_lossy(&output.stdout)))
}

fn list_devices() -> Result<(), Box<dyn core::error::Error>> {
    let devices = attached()?;
    if devices.is_empty() {
        println!("no devices attached and authorised");
        return Ok(());
    }
    for serial in devices {
        println!("{serial}");
    }
    Ok(())
}

/// The device to drive.
///
/// One attached device needs no naming. Two do: picking one silently would
/// drive whichever phone happened to enumerate first, which is somebody's.
fn choose_device(named: Option<&str>) -> Result<String, Box<dyn core::error::Error>> {
    if let Some(serial) = named {
        return Ok(serial.to_owned());
    }
    let mut devices = attached()?;
    match devices.len() {
        0 => Err("no devices attached and authorised; check `adb devices`".into()),
        1 => Ok(devices.remove(0)),
        _ => Err(format!(
            "{} devices attached; name one with --device (see `jev-pilot devices`)",
            devices.len()
        )
        .into()),
    }
}

/// Print the catalog the next step would be offered, and act on nothing.
///
/// The first thing wanted when a run behaves oddly, and it costs no model
/// call: the alternative was provoking a stall or leaving for `uiautomator
/// dump`, the slow reader the helper exists to replace.
fn observe(named: Option<&str>) -> Result<(), Box<dyn core::error::Error>> {
    let mut device = AdbDevice::new(choose_device(named)?).with_helper()?;
    let started = std::time::Instant::now();
    let screen = device.observe()?;
    let read_in = started.elapsed();

    match device.reader().why() {
        None => println!("read by : accessibility helper"),
        Some(why) => println!("read by : uiautomator CLI ({why})"),
    }
    println!("took    : {}ms", read_in.as_millis());
    println!(
        "keyboard: {}",
        if screen.keyboard_open() { "up" } else { "down" }
    );
    println!("app     : {}", screen.app().unwrap_or("unknown"));
    match screen.quiet_for_ms() {
        Some(quiet) => println!("quiet   : {quiet}ms since the screen last changed"),
        None => println!("quiet   : this reader cannot say"),
    }

    let refused: Vec<&str> = screen.unavailable().collect();
    if !refused.is_empty() {
        println!("\non screen but not available:");
        for control in refused {
            println!("  {control}");
        }
    }

    let says: Vec<&str> = screen.notices().collect();
    if !says.is_empty() {
        println!("\nthe screen says:");
        for notice in says {
            println!("  {notice}");
        }
    }

    let catalog = Catalog::for_screen(&screen, &Android);
    println!("\noperations offered:");
    let offered: Vec<&str> = catalog.operations().iter().map(|o| o.key()).collect();
    println!("  {}", offered.join(" "));

    println!("\n{} rows:", screen.refs().count());
    for (index, (handle, element)) in screen.refs().enumerate() {
        let where_it_taps = match screen.tap_point(handle) {
            Ok(point) => format!("taps {},{}", point.x, point.y),
            Err(error) => format!("UNREACHABLE: {error}"),
        };
        println!(
            "  [{index:2}] {:<52} {where_it_taps}{}",
            element.describe().chars().take(52).collect::<String>(),
            if element.editable { "  [editable]" } else { "" },
        );
    }
    Ok(())
}

fn manage_helper(named: Option<&str>, install: bool) -> Result<(), Box<dyn core::error::Error>> {
    let device = AdbDevice::new(choose_device(named)?);
    let provision = device.helper_provision()?;
    println!("device : {}", device.serial());
    println!("bundled: {} v{}", BUNDLED.package, BUNDLED.version_name);
    println!("state  : {provision:?} — {}", provision.advice());

    if provision.is_ready() || !install {
        if !provision.is_ready() {
            println!("\n{HELPER_PITCH}");
            println!("To install it:  jev-pilot helper install");
        }
        return Ok(());
    }

    println!("\ninstalling...");
    device.install_helper()?;
    let after = device.helper_provision()?;
    println!("state  : {after:?} — {}", after.advice());
    Ok(())
}

const HELPER_PITCH: &str = "\
The helper is strongly recommended. Without it every screen read costs about
2.5s instead of about 60ms, every gesture spawns a process on the device, text
outside ASCII cannot be typed at all, and `uiautomator dump` reports boxes
whose bottom edge lies above their top for rows scrolled off screen.

It is an accessibility service: once enabled it can read every screen on that
device. It answers only on loopback, only to a caller holding a token this
host generates per run, and it sends nothing anywhere. The source is in
`helper/` and it is built from that source, not downloaded.";

/// Everything a run needs beyond the device it is pointed at.
///
/// One value rather than eight arguments: they arrive together from the
/// command line and travel together to the loop, and a list this long is read
/// by position, which is how the wrong two get swapped.
struct Plan {
    goal: Box<str>,
    accept: Vec<String>,
    steps: u32,
    floors: jev_pilot::act::Floors,
    app: Option<Box<str>>,
    texts: Vec<(Box<str>, Box<str>)>,
    desk_dir: Option<PathBuf>,
}

fn pursue(named: Option<&str>, plan: Plan) -> Result<(), Box<dyn core::error::Error>> {
    let Plan {
        goal,
        accept,
        steps,
        floors,
        app,
        texts,
        desk_dir,
    } = plan;
    let goal = &*goal;
    let dir = desk_dir.unwrap_or_else(|| std::env::temp_dir().join("jev-pilot-desk"));
    let desk = Rc::new(Desk::new(dir.clone(), texts)?);
    let transcript = dir.join("steps.jsonl");
    let _ = std::fs::remove_file(&transcript);

    let device = AdbDevice::new(choose_device(named)?).with_helper()?;
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
    if !accept.is_empty() {
        println!("accept : {}", accept.join(" / "));
    }
    println!();

    let judge = SystemOne::new(ApiKey::from_env()?);
    let choosing = Rc::clone(&desk);
    let writing = Rc::clone(&desk);

    let mut pilot = Pilot::new(device, judge, &Android)
        .with_floors(floors)
        .about(app)
        .confirming(accept)
        .limited_to(steps)
        .escalating_to(move |impasse: &Impasse<'_>| choosing.choose(impasse))
        .writing_with(move |request: &Writing<'_>| writing.compose(request))
        .watching(move |step: &jev_pilot::pilot::StepReport<'_>| {
            report(step);
            transcribe(&transcript, step);
        });

    let ending = pilot.pursue(goal)?;
    println!("\nending : {ending:?}");
    // Said at the end as well as the start: a run can lose the helper part way
    // through, and the reason is the only account of why it went slow.
    let reader = pilot.device().reader();
    match (reader.uses_helper(), reader.borrowed(), reader.why()) {
        (true, 0, _) => println!("screens: accessibility helper throughout"),
        (true, borrowed, Some(why)) => println!(
            "screens: accessibility helper, and {borrowed} screen(s) it could not see\n\
             reason : {why}"
        ),
        (_, _, Some(why)) => println!("screens: uiautomator CLI ({why})"),
        (_, _, None) => {}
    }
    let _ = std::fs::remove_file(dir.join("ask.json"));
    Ok(())
}


/// One line per step, and one more for what it chose.
/// Append one step to the run's transcript.
///
/// A run that ends early otherwise leaves only its ending: "blocked", with no
/// way to ask blocked by what, because only an impasse ever prints a screen.
/// One line per step, as JSON, so the answer is on disk when the question is
/// asked afterwards rather than needing the run done again.
fn transcribe(to: &std::path::Path, step: &jev_pilot::pilot::StepReport<'_>) {
    
    let line = serde_json::json!({
        "step": step.index,
        "app": step.app,
        "rows": step.rows,
        "screen_says": step.says,
        "unavailable": step.unavailable,
        "repeating": step.repeating,
        "acted": step.acted,
        "read_ms": step.read_ms,
        "step_ms": step.step_ms,
        "waited_ms": step.waited_ms,
        "judged_ms": step.judged_ms,
        "settled_ms": step.settled_ms,
        "chosen": step.chosen.map(|act| format!("{act:?}")),
        "operation_confidence": step.operation_confidence.get(),
        "target_confidence": step.target_confidence.map(jev_pilot::judgment::Confidence::get),
        "goal_met": step.goal_met.to_string(),
        "is_error_screen": step.is_error_screen,
    });
    // A transcript that cannot be written must not end the run it is
    // describing: it is an account of the work, not the work.
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(to)
        && let Ok(line) = serde_json::to_string(&line)
    {
        let _ = writeln!(file, "{line}");
    }
}

fn report(step: &jev_pilot::pilot::StepReport<'_>) {
    println!(
        "step {}  {} rows  goal_met {}  error {:.2}  \
         {}ms (read {} judge {} settle {}{})",
        step.index,
        step.rows.len(),
        step.goal_met,
        step.is_error_screen,
        // The work, not the wall clock: a step that stopped to ask spent most
        // of its time on the person answering.
        step.step_ms.saturating_sub(step.waited_ms),
        step.read_ms,
        step.judged_ms,
        step.settled_ms,
        if step.waited_ms > 0 {
            format!(", asked {}ms", step.waited_ms)
        } else {
            String::new()
        },
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
}
