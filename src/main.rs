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
use core::fmt::Write as _;
use jev_pilot::{
    act::{Catalog, Operation},
    cli::{self, Invocation},
    client::http::SystemOne,
    credential::ApiKey,
    device::Device as _,
    device::adb::{Adb, AdbDevice},
    device::helper::BUNDLED,
    judgment::Confidence,
    pilot::{Impasse, Pilot, Resolution, Writing},
    platform::Android,
};
use std::io::Write as _;
use std::path::PathBuf;
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
    /// Text the caller supplied up front, by the field it belongs in.
    ///
    /// Consulted before anyone is asked. A scripted run knows the words it
    /// means to type — they are in the goal it was given — and stopping to ask
    /// for each one is what keeps such a run from finishing unattended.
    texts: Vec<(Box<str>, Box<str>)>,
}

impl Desk {
    fn new(dir: PathBuf, texts: Vec<(Box<str>, Box<str>)>) -> std::io::Result<Self> {
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
        Ok(Self { dir, typed, texts })
    }

    /// Text supplied for a field, matched by name.
    ///
    /// Contains rather than equals: a field is named by whatever the screen
    /// calls it, which is a hint, a label or a caption, and rarely the short
    /// name a caller would type. Case is ignored for the same reason.
    fn supplied(&self, field: &str) -> Option<&str> {
        let field = field.to_lowercase();
        self.texts
            .iter()
            .find(|(name, _)| field.contains(&name.to_lowercase()))
            .map(|(_, text)| &**text)
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

        // A question nobody is there to answer must not hold a run open for
        // ever. A person at the terminal has as long as they like; a run with
        // nothing attached to its desk gives up and says so.
        let deadline = std::time::Instant::now() + Duration::from_secs(WAIT_SECONDS);
        while std::time::Instant::now() < deadline {
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
        let _ = std::fs::remove_file(&ask);
        Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            format!("nobody answered within {WAIT_SECONDS}s"),
        ))
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
                "screen_says": impasse.says,
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
        if let Some(text) = self.supplied(&request.field.describe()) {
            println!(
                "\n  \u{2500}\u{2500} step {}: typing the text given for {}",
                request.step,
                request.field.describe(),
            );
            return Ok(text.into());
        }
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

/// How long a question waits for an answer before the run gives up.
const WAIT_SECONDS: u64 = 300;

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
        .watching(report);

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

/// One line per step, and one more for what it chose.
fn report(step: &jev_pilot::pilot::StepReport<'_>) {
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
}
