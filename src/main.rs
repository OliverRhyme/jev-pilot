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
    // A copy moved aside by an update is removed once it is no longer running.
    // Best effort: on Windows it may still be held for a moment, and the next
    // start tries again.
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::fs::remove_file(jev_pilot::update::set_aside(&exe));
    }
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("jev-pilot: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

/// Bring this copy up to date with the latest release.
///
/// Asks GitHub which release is latest first, and does nothing when this is
/// it. The running program is moved aside before the installer runs, because
/// a running program cannot be overwritten on Windows and may not be on Linux,
/// and is put back if the installer fails.
fn update() -> Result<(), Box<dyn core::error::Error>> {
    use jev_pilot::update::{Plan, is_newer, plan, set_aside};

    let current = env!("CARGO_PKG_VERSION");
    match latest_release() {
        Some(tag) if !is_newer(current, &tag) => {
            println!("jev-pilot {current} is the latest release");
            return Ok(());
        }
        Some(tag) => println!("updating jev-pilot {current} to {tag}"),
        None => println!("could not tell which release is latest; running the installer anyway"),
    }

    let exe = std::env::current_exe()?;
    let windows = cfg!(windows);
    let home = std::env::var_os(if windows { "USERPROFILE" } else { "HOME" })
        .map(PathBuf::from)
        .ok_or("cannot tell where your home directory is")?;
    match plan(&exe, &home, windows) {
        Plan::FromSource { hint } => {
            println!("{hint}");
            Ok(())
        }
        Plan::Installer { program, args } => {
            let aside = set_aside(&exe);
            std::fs::rename(&exe, &aside)?;
            let installed = std::process::Command::new(&program).args(&args).status();
            match installed {
                Ok(status) if status.success() => {
                    let _ = std::fs::remove_file(&aside);
                    Ok(())
                }
                failed => {
                    // Nothing was put in its place, so the old copy goes back.
                    if !exe.exists() {
                        std::fs::rename(&aside, &exe)?;
                    }
                    match failed {
                        Ok(status) => Err(format!("the installer failed ({status})").into()),
                        Err(error) => Err(format!("could not run {program}: {error}").into()),
                    }
                }
            }
        }
        _ => Err("this copy cannot be updated in place".into()),
    }
}

/// The tag of the latest release, when GitHub can be asked.
#[cfg(feature = "http")]
fn latest_release() -> Option<String> {
    let body: serde_json::Value = ureq::get(jev_pilot::update::LATEST_API)
        .header(
            "User-Agent",
            concat!("jev-pilot/", env!("CARGO_PKG_VERSION")),
        )
        .header("Accept", "application/vnd.github+json")
        .call()
        .ok()?
        .body_mut()
        .read_json()
        .ok()?;
    body.get("tag_name")?.as_str().map(str::to_owned)
}

#[cfg(not(feature = "http"))]
fn latest_release() -> Option<String> {
    None
}

/// Serve MCP over stdin and stdout until the client goes away.
///
/// One thread. Nothing here is compute, and each run is a separate process
/// with a loop that waits on one thing at a time.
#[cfg(feature = "mcp")]
fn serve_mcp() -> Result<(), Box<dyn core::error::Error>> {
    use rmcp::ServiceExt as _;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let serving = jev_pilot::mcp::Pilot::new()
            .serve(rmcp::transport::stdio())
            .await?;
        serving.waiting().await?;
        Ok(())
    })
}

#[cfg(not(feature = "mcp"))]
fn serve_mcp() -> Result<(), Box<dyn core::error::Error>> {
    Err("this build has no MCP server; it is built with the `mcp` feature".into())
}

fn run() -> Result<(), Box<dyn core::error::Error>> {
    match cli::parse(std::env::args().skip(1))? {
        Invocation::Help => {
            print!("{}", cli::USAGE);
            Ok(())
        }
        Invocation::Devices => list_devices(),
        Invocation::Mcp => serve_mcp(),
        Invocation::Version => {
            println!("jev-pilot {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Invocation::Update => update(),
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
            plan,
            keys,
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
                then: plan,
                keys,
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
    then: Vec<String>,
    keys: Option<String>,
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
        then: steps_in_order,
        keys,
        desk_dir,
    } = plan;
    let goal = &*goal;
    let dir = desk_dir.unwrap_or_else(|| std::env::temp_dir().join("jev-pilot-desk"));
    let supplied: Vec<Box<str>> = texts.iter().map(|(field, _)| field.clone()).collect();
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

    // Every request kept as sent, when asked for, so a step that went wrong
    // can be replayed against the model with its wording changed.
    let judge = jev_pilot::pilot::Recorded::optionally(
        SystemOne::new(ApiKey::from_env()?),
        std::env::var_os("JEV_PILOT_REQUESTS").map(PathBuf::from),
    );
    let choosing = Rc::clone(&desk);
    let writing = Rc::clone(&desk);

    let mut pilot = Pilot::new(device, judge, &Android)
        .with_floors(floors)
        .about(app)
        .confirming(accept)
        .following(steps_in_order.iter().map(String::as_str))
        .entering_keys(keys.as_deref().unwrap_or_default())
        .supplying(&supplied)
        .limited_to(steps)
        .escalating_to(move |impasse: &Impasse<'_>| choosing.choose(impasse))
        .writing_with(move |request: &Writing<'_>| writing.compose(request))
        .watching(move |step: &jev_pilot::pilot::StepReport<'_>| {
            report(step);
            transcribe(&transcript, step);
        });

    let ending = pilot.pursue(goal)?;
    println!("\nending : {ending:?}");
    // Printed with the ending, which is all a caller of the MCP server is
    // shown of it: a goal met before anything was done is often a stale
    // screen left by an earlier run rather than work this one did.
    if matches!(
        ending,
        jev_pilot::pilot::Ending::Finished(jev_pilot::act::Outcome::Achieved)
    ) && pilot.actions_taken() == 0
    {
        println!(
            "note   : nothing was done; the goal already held on the screen the run started on"
        );
    }
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
        // A step that chose nothing because the goal was met did not refuse
        // anything; saying so kept a run that ended achieved from reading as
        // one that ended on a refusal.
        None if step.goal_met == jev_pilot::judgment::Progress::Achieved => println!(
            "   -> finished: the goal is met   op {:.2} / target {target}",
            step.operation_confidence.get()
        ),
        None => println!(
            "   -> refused   op {:.2} / target {target}",
            step.operation_confidence.get()
        ),
    }
}
