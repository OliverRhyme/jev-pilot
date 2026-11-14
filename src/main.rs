//! `jev-pilot`: drive an Android device, or a web browser, toward a goal.
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
    platform::{Android, Platform},
    snapshot::Snapshot,
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
    let invocation = cli::parse(std::env::args().skip(1))?;
    bridge::give_up_group(&invocation)?;
    match invocation {
        Invocation::Help => {
            print!("{}", cli::USAGE);
            Ok(())
        }
        Invocation::Devices { browser: None } => list_devices(),
        Invocation::Devices {
            browser: Some(browser),
        } => web::list_tabs(&browser),
        Invocation::Mcp => serve_mcp(),
        Invocation::Version => {
            println!("jev-pilot {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Invocation::Update => update(),
        Invocation::Extension(command) => bridge::command(command),
        Invocation::Host => bridge::host(),
        Invocation::Observe {
            device,
            browser: None,
        } => observe(device.as_deref()),
        Invocation::Observe {
            browser: Some(browser),
            ..
        } => web::observe(&browser),
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
            browser,
        } => {
            let plan = Plan {
                goal,
                accept,
                steps,
                floors,
                app,
                texts,
                then: plan,
                keys,
                desk_dir: desk,
            };
            match browser {
                Some(browser) => web::pursue(&browser, plan),
                None => pursue(device.as_deref(), plan),
            }
        }
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
    print_screen(&screen, &Android);
    Ok(())
}

/// What a screen offers and refuses, and what it says, as `observe` prints it.
fn print_screen(screen: &Snapshot, platform: &dyn Platform) {
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

    let tabs: Vec<String> = screen
        .tabs()
        .map(|(_, tab)| format!("{} — {}", tab.title, tab.url))
        .collect();
    if !tabs.is_empty() {
        println!("\nother tabs:");
        for tab in tabs {
            println!("  {tab}");
        }
    }

    let catalog = Catalog::for_screen(screen, platform);
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
    // Read before the device is touched, so a missing key costs nothing.
    let key = ApiKey::from_env()?;
    let device = AdbDevice::new(choose_device(named)?).with_helper()?;
    println!("device : {}", device.serial());
    match device.reader().why() {
        None => println!("screens: accessibility helper"),
        Some(why) => println!("screens: uiautomator CLI ({why})"),
    }
    drive(device, &Android, key, plan, &[], |device| {
        // Said at the end as well as the start: a run can lose the helper
        // part way through, and the reason is the only account of why it
        // went slow.
        let reader = device.reader();
        match (reader.uses_helper(), reader.borrowed(), reader.why()) {
            (true, 0, _) => println!("screens: accessibility helper throughout"),
            (true, borrowed, Some(why)) => println!(
                "screens: accessibility helper, and {borrowed} screen(s) it could not see\n\
                 reason : {why}"
            ),
            (_, _, Some(why)) => println!("screens: uiautomator CLI ({why})"),
            (_, _, None) => {}
        }
    })
}

/// Pursue a goal on any device, printing each step and keeping a transcript.
///
/// `urls` are the addresses `navigate` may go to, for a browser; `after` says
/// what only this kind of device can say once the run is over.
fn drive<D>(
    device: D,
    platform: &dyn Platform,
    key: ApiKey,
    plan: Plan,
    urls: &[Box<str>],
    after: impl FnOnce(&D),
) -> Result<(), Box<dyn core::error::Error>>
where
    D: jev_pilot::device::Device,
    D::Error: 'static,
{
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

    println!("goal   : {goal}");
    println!(
        "asking : this terminal, or {}",
        dir.join("answer.json").display()
    );
    if !accept.is_empty() {
        println!("accept : {}", accept.join(" / "));
    }
    if !urls.is_empty() {
        let shown: Vec<&str> = urls.iter().map(AsRef::as_ref).collect();
        println!("urls   : {}", shown.join(" "));
    }
    println!();

    // Every request kept as sent, when asked for, so a step that went wrong
    // can be replayed against the model with its wording changed.
    let judge = jev_pilot::pilot::Recorded::optionally(
        SystemOne::new(key),
        std::env::var_os("JEV_PILOT_REQUESTS").map(PathBuf::from),
    );
    let choosing = Rc::clone(&desk);
    let writing = Rc::clone(&desk);

    let mut pilot = Pilot::new(device, judge, platform)
        .with_floors(floors)
        .about(app)
        .confirming(accept)
        .following(steps_in_order.iter().map(String::as_str))
        .entering_keys(keys.as_deref().unwrap_or_default())
        .supplying(&supplied)
        .navigating_to(urls.iter().cloned())
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
    after(pilot.device());
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
        "url": step.url,
        "rows": step.rows,
        "screen_says": step.says,
        "unavailable": step.unavailable,
        "repeating": step.repeating,
        "acted": step.acted,
        "outcome": step.outcome,
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

/// Driving a web browser: finding or starting it, choosing the tab, and the
/// device a run drives it through.
#[cfg(feature = "browser")]
mod web {
    use super::{Plan, drive, print_screen};
    use jev_pilot::browser::device::BrowserDevice;
    use jev_pilot::browser::discover::{
        ActivePort, AttachError, Channel, Os, TabChoice, choose_tab, is_chrome, read_active_port,
        tab_line, user_data_dir,
    };
    use jev_pilot::browser::engine::{BrowserCommand, BrowserEngine as _, CdpEngine};
    use jev_pilot::browser::launch::{Fresh, Interrupted, LaunchError, find_chrome};
    use jev_pilot::browser::transport::{Connection, Socket, Wire, WireError};
    use jev_pilot::cli::{BrowserMode, BrowserTarget};
    use jev_pilot::credential::ApiKey;
    use jev_pilot::device::{Command, Device};
    use jev_pilot::platform::browser::Browser;
    use jev_pilot::platform::page::PageTab;
    use jev_pilot::snapshot::{Snapshot, TabKey};
    use serde_json::{Value, json};
    use std::path::PathBuf;
    use std::sync::{Mutex, MutexGuard, Once, PoisonError};
    use std::time::Duration;

    type Error = Box<dyn core::error::Error>;

    /// The connection a run drives: Chrome's websocket, or the bridge to
    /// its extension.
    pub enum AnyWire {
        /// A DevTools websocket.
        Socket(Socket),
        /// The extension, through the host.
        #[cfg(feature = "extension")]
        Extension(
            jev_pilot::browser::extension::ExtensionWire<
                jev_pilot::browser::host::channel::RunStream,
            >,
        ),
    }

    impl Wire for AnyWire {
        fn send(&mut self, text: &str) -> Result<(), WireError> {
            match self {
                Self::Socket(w) => w.send(text),
                #[cfg(feature = "extension")]
                Self::Extension(w) => w.send(text),
            }
        }

        fn receive(&mut self, timeout: Duration) -> Result<Option<String>, WireError> {
            match self {
                Self::Socket(w) => w.receive(timeout),
                #[cfg(feature = "extension")]
                Self::Extension(w) => w.receive(timeout),
            }
        }

        fn close(&mut self) {
            match self {
                Self::Socket(w) => w.close(),
                #[cfg(feature = "extension")]
                Self::Extension(w) => w.close(),
            }
        }
    }

    /// What Ctrl-C has to undo while a browser session is open.
    ///
    /// The run may be blocked on the browser or on the model when Ctrl-C
    /// comes, and would never reach its own clean-up, so the handler does it
    /// on its own thread. It holds this lock while it works, and a run ending
    /// normally holds it while it closes up, so the two never both clean up.
    static ON_INTERRUPT: Mutex<Option<Interrupted>> = Mutex::new(None);

    fn on_interrupt() -> MutexGuard<'static, Option<Interrupted>> {
        ON_INTERRUPT.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Note what Ctrl-C has to undo, handling Ctrl-C from now on.
    fn undo_on_interrupt(what: Interrupted) {
        static HANDLER: Once = Once::new();
        HANDLER.call_once(|| {
            let _ = ctrlc::set_handler(|| {
                let mut held = on_interrupt();
                match held.take() {
                    Some(what @ Interrupted::Fresh { .. }) => {
                        what.clean_up(|endpoint| {
                            if let Ok(socket) = Socket::connect(endpoint, Duration::from_secs(2)) {
                                let _ = Connection::new(AnyWire::Socket(socket)).send(
                                    "Browser.close",
                                    json!({}),
                                    None,
                                );
                            }
                        });
                        eprintln!(
                            "\ninterrupted: closed the fresh browser and removed its profile"
                        );
                    }
                    Some(Interrupted::Attached) => {
                        eprintln!("\n{}", jev_pilot::cli::LET_GO);
                    }
                    None => {}
                }
                std::process::exit(130);
            });
        });
        *on_interrupt() = Some(what);
    }

    /// The browser this run started, if it started one.
    ///
    /// Dropping it clears what Ctrl-C would undo, then closes the browser and
    /// removes its profile, with Ctrl-C held off until that is done. That
    /// covers every way out of a run, an error before the run began included,
    /// so the handler is never left pointing at a browser already gone.
    struct Started(Option<Fresh>);

    impl Started {
        /// A browser just started, reached at `port`: Ctrl-C undoes it from
        /// now on.
        fn new(fresh: Fresh, port: &ActivePort) -> Self {
            undo_on_interrupt(fresh.interrupted(port));
            Self(Some(fresh))
        }

        fn is_some(&self) -> bool {
            self.0.is_some()
        }
    }

    impl Drop for Started {
        fn drop(&mut self) {
            let mut held = on_interrupt();
            *held = None;
            drop(self.0.take());
        }
    }

    /// How long a fresh browser is given to open its debugging port.
    const FRESH_START: Duration = Duration::from_secs(20);

    /// How long the calls made before the engine takes over may take.
    const CALL: Duration = Duration::from_secs(10);

    /// A browser session: the device a run drives, and the browser this run
    /// started, if it started one.
    ///
    /// The fields drop in order, so the engine lets go of its tab before the
    /// browser it is in is reaped and its profile removed.
    pub struct Session {
        device: BrowserDevice<CdpEngine<AnyWire>>,
        fresh: Started,
    }

    impl Drop for Session {
        fn drop(&mut self) {
            let mut held = on_interrupt();
            *held = None;
            if let Some(fresh) = self.fresh.0.take() {
                let _ = self.device.engine_mut().close_browser();
                drop(fresh);
            }
        }
    }

    impl Device for Session {
        type Error = <BrowserDevice<CdpEngine<AnyWire>> as Device>::Error;

        fn observe(&mut self) -> Result<Snapshot, Self::Error> {
            self.device.observe()
        }

        fn perform(&mut self, command: &Command) -> Result<(), Self::Error> {
            self.device.perform(command)
        }

        fn outcome(&mut self) -> Option<Box<str>> {
            self.device.outcome()
        }

        fn refused(&mut self) -> bool {
            self.device.refused()
        }
    }

    impl Session {
        /// The driven tab's title and address, for the run's header.
        fn describe_tab(&mut self) -> Option<String> {
            let engine = self.device.engine_mut();
            let current = engine.current_tab();
            let tabs = engine.tabs().ok()?;
            let tab = tabs.into_iter().find(|t| t.key == current)?;
            Some(format!("{} — {}", tab.title, tab.url))
        }
    }

    /// A connection to a browser that has proved to be Chrome, with its tabs.
    struct Connected {
        wire: AnyWire,
        tabs: Vec<PageTab>,
        fresh: Started,
    }

    /// Connect to the browser the target names, once.
    ///
    /// In attach mode the handshake raises Chrome's prompt, so it is made as
    /// soon as the run starts and never retried; a retry would stack a second
    /// prompt on the first.
    fn connect(target: &BrowserTarget, goal: Option<&str>) -> Result<Connected, Error> {
        let (wire, fresh, binary) = match &target.mode {
            BrowserMode::Attach {
                channel,
                endpoint,
                allow_wait,
                ..
            } => {
                let endpoint = if let Some(endpoint) = endpoint {
                    endpoint.to_string()
                } else {
                    let channel: Channel = channel.parse().unwrap_or_default();
                    let home = home_dir().ok_or("cannot tell where your home directory is")?;
                    let dir = user_data_dir(Os::current(), channel, &home, &|name| {
                        std::env::var(name).ok()
                    });
                    read_active_port(&dir)?.endpoint()
                };
                undo_on_interrupt(Interrupted::Attached);
                eprintln!("Click Allow in Chrome (Enter chooses Cancel)");
                let socket = Socket::connect(&endpoint, Duration::from_secs(*allow_wait))
                    .map_err(AttachError::from)?;
                (AnyWire::Socket(socket), Started(None), None)
            }
            BrowserMode::Fresh { chrome, headless } => {
                let binary = find_chrome(
                    chrome.as_deref(),
                    std::env::var_os("JEV_PILOT_CHROME"),
                    Os::current(),
                    &|path| path.exists(),
                )
                .ok_or(LaunchError::NoBrowser)?;
                let (fresh, port) = Fresh::start(&binary, *headless, FRESH_START)?;
                let fresh = Started::new(fresh, &port);
                let socket = Socket::connect(&port.endpoint(), CALL).map_err(AttachError::from)?;
                (AnyWire::Socket(socket), fresh, Some(binary))
            }
            BrowserMode::Extension { .. } => {
                undo_on_interrupt(Interrupted::Attached);
                (
                    super::bridge::connect(target, goal.unwrap_or_default())?,
                    Started(None),
                    None,
                )
            }
        };
        let mut connection = Connection::new(wire);
        // Nothing is typed into a browser until it has said it is Chrome: a
        // stale port file can name a port another program now holds.
        let version = connection.call("Browser.getVersion", json!({}), None, CALL)?;
        if !is_chrome(&version) {
            let product = version
                .get("product")
                .and_then(Value::as_str)
                .unwrap_or("something unnamed");
            // A browser this run started is not a stale endpoint; say which
            // binary it was.
            return Err(match binary {
                Some(binary) => LaunchError::NotChrome {
                    binary,
                    product: product.into(),
                }
                .into(),
                None => AttachError::NotChrome {
                    product: product.into(),
                }
                .into(),
            });
        }
        let targets = connection.call("Target.getTargets", json!({}), None, CALL)?;
        Ok(Connected {
            wire: connection.into_wire(),
            tabs: page_tabs(&targets),
            fresh,
        })
    }

    fn page_tabs(targets: &Value) -> Vec<PageTab> {
        targets
            .get("targetInfos")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|t| t.get("type").and_then(Value::as_str) == Some("page"))
            .map(|t| {
                let s = |k: &str| t.get(k).and_then(Value::as_str).unwrap_or_default();
                PageTab {
                    key: TabKey::new(s("targetId")),
                    title: s("title").into(),
                    url: s("url").into(),
                    opened_by_run: false,
                }
            })
            .collect()
    }

    /// Whether tabs are to be listed by id and site alone.
    fn sites_only(target: &BrowserTarget) -> bool {
        matches!(
            target.mode,
            BrowserMode::Attach {
                sites_only: true,
                ..
            } | BrowserMode::Extension { sites_only: true }
        )
    }

    /// A tab that could not be chosen, as an error that lists the tabs the
    /// way the target asks.
    fn unchosen(target: &BrowserTarget) -> impl Fn(TabChoice) -> Error + '_ {
        move |choice| {
            if sites_only(target) {
                choice.listing(true).into()
            } else {
                choice.into()
            }
        }
    }

    fn home_dir() -> Option<PathBuf> {
        std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
    }

    /// Connect, and attach to the tab the target names.
    ///
    /// A fresh browser starts on one blank tab the run owns, and `--open`
    /// loads its address there; in attach mode `--open` opens a new tab, so
    /// none of the owner's is touched.
    fn session(target: &BrowserTarget, goal: Option<&str>) -> Result<Session, Error> {
        let Connected { wire, tabs, fresh } = connect(target, goal)?;
        let engine = match (&target.open, fresh.is_some()) {
            (Some(url), false) => CdpEngine::open(wire, url)?,
            (open, true) if target.tab.is_none() => {
                let tab = choose_tab(&tabs, None).map_err(unchosen(target))?;
                let mut engine = CdpEngine::attach(wire, &tab.key, true)?;
                if let Some(url) = open {
                    engine.perform(&BrowserCommand::Navigate(url.clone()))?;
                }
                engine
            }
            _ => {
                let tab = choose_tab(&tabs, target.tab.as_deref()).map_err(unchosen(target))?;
                CdpEngine::attach(wire, &tab.key, fresh.is_some())?
            }
        };
        Ok(Session {
            device: BrowserDevice::new(engine),
            fresh,
        })
    }

    /// `devices --browser …`: the browser's tabs, with their ids.
    pub fn list_tabs(target: &BrowserTarget) -> Result<(), Error> {
        let Connected { wire, tabs, fresh } = connect(target, None)?;
        if tabs.is_empty() {
            println!("no tabs open");
        }
        for tab in &tabs {
            println!("{}", tab_line(tab, sites_only(target)));
        }
        if fresh.is_some() {
            let _ = Connection::new(wire).send("Browser.close", json!({}), None);
        }
        drop(fresh);
        Ok(())
    }

    /// `observe --browser …`: what the next step would be offered on the tab.
    pub fn observe(target: &BrowserTarget) -> Result<(), Error> {
        let mut session = session(target, None)?;
        let started = std::time::Instant::now();
        let screen = session.observe()?;
        println!("read by : DevTools Protocol");
        println!("took    : {}ms", started.elapsed().as_millis());
        println!("tab     : {}", screen.url().unwrap_or("unknown"));
        print_screen(&screen, &Browser::default());
        Ok(())
    }

    /// A run in a browser.
    pub fn pursue(target: &BrowserTarget, plan: Plan) -> Result<(), Error> {
        // Read before Chrome is asked, so a missing key costs the owner no
        // click.
        let key = ApiKey::from_env()?;
        let mut session = session(target, Some(&plan.goal))?;
        println!(
            "browser: {}",
            match &target.mode {
                BrowserMode::Attach { channel, .. } => format!("attached to Chrome ({channel})"),
                BrowserMode::Fresh { headless: true, .. } => "fresh, headless".to_owned(),
                BrowserMode::Fresh { .. } => "fresh".to_owned(),
                BrowserMode::Extension { .. } =>
                    "Chrome, through the jev-pilot extension".to_owned(),
            }
        );
        if let Some(header) = super::bridge::development_header(target) {
            println!("{header}");
        }
        if let Some(tab) = session.describe_tab() {
            println!("tab    : {tab}");
        }
        drive(
            session,
            &Browser::default(),
            key,
            plan,
            &target.urls,
            |_| {},
        )
    }
}

/// Without the `browser` feature, every browser command says how to get one.
#[cfg(not(feature = "browser"))]
mod web {
    use super::Plan;
    use jev_pilot::cli::BrowserTarget;

    const REBUILD: &str =
        "this build cannot drive a browser; rebuild with `cargo install --features browser`";

    pub fn list_tabs(_: &BrowserTarget) -> Result<(), Box<dyn core::error::Error>> {
        Err(REBUILD.into())
    }

    pub fn observe(_: &BrowserTarget) -> Result<(), Box<dyn core::error::Error>> {
        Err(REBUILD.into())
    }

    pub fn pursue(_: &BrowserTarget, _: Plan) -> Result<(), Box<dyn core::error::Error>> {
        Err(REBUILD.into())
    }
}

/// The bridge to Chrome's jev-pilot extension: the host, the `extension`
/// commands, and a run's way onto the bridge.
#[cfg(feature = "extension")]
mod bridge {
    use jev_pilot::browser::discover::Os;
    use jev_pilot::browser::host::channel::{Place, connect_run, host_answers};
    use jev_pilot::browser::host::{Serve, serve};
    use jev_pilot::browser::install::{
        self, Found, InstallKind, Installer, Layout, NOT_SET_UP, Status, UNPROTECTED, UserHosts,
    };
    use jev_pilot::cli::{BrowserMode, BrowserTarget, ExtensionCommand, Invocation};
    use jev_pilot::peer::{Expected, Native};
    use std::path::{Path, PathBuf};

    type Error = Box<dyn core::error::Error>;

    fn env(name: &str) -> Option<String> {
        std::env::var(name).ok()
    }

    fn home() -> Result<PathBuf, Error> {
        std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
            .map(PathBuf::from)
            .ok_or_else(|| "cannot tell where your home directory is".into())
    }

    fn read(path: &Path) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }

    fn read_install(path: &Path) -> std::io::Result<String> {
        std::fs::read_to_string(path)
    }

    /// Where Windows keeps Program Files and ProgramData, as the system
    /// says.
    #[cfg_attr(
        not(windows),
        allow(clippy::unnecessary_wraps, reason = "Windows can fail")
    )]
    fn folders() -> Result<install::SystemFolders, Error> {
        #[cfg(windows)]
        return Ok(install::SystemFolders::of_this_system()?);
        #[cfg(not(windows))]
        Ok(install::SystemFolders::of_this_system())
    }

    /// The installed copy, or why there is none.
    fn installed() -> Result<Found, Error> {
        install::find_installed(Os::current(), &folders()?, &home()?, &env, &read_install)?
            .ok_or_else(|| NOT_SET_UP.into())
    }

    /// Whether the invocation speaks to the host: drives Chrome through the
    /// extension, or asks whether the bridge answers. Only the installed
    /// copy passes the host's check, so these run as it.
    fn through_extension(invocation: &Invocation) -> bool {
        if matches!(invocation, Invocation::Extension(ExtensionCommand::Status)) {
            return true;
        }
        let browser = match invocation {
            Invocation::Run { browser, .. }
            | Invocation::Observe { browser, .. }
            | Invocation::Devices { browser } => browser.as_ref(),
            _ => None,
        };
        matches!(
            browser.map(|b| &b.mode),
            Some(BrowserMode::Extension { .. })
        )
    }

    /// Give up the setgid install's group, first thing, in every mode but
    /// two: the host keeps it (a run reads the host's credentials as they
    /// were when it listened, and only the group may append to the audit
    /// log; the host starts no other program), and a run through the
    /// extension gives it up right after connecting. Before that, a run
    /// through the extension that is not the installed copy runs the
    /// installed copy instead.
    ///
    /// A development install is never made non-dumpable: its peer could not
    /// then read its executable to check its hash.
    pub fn give_up_group(invocation: &Invocation) -> Result<(), Error> {
        if through_extension(invocation) {
            return run_the_installed_copy();
        }
        if matches!(invocation, Invocation::Host) {
            return Ok(());
        }
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        jev_pilot::peer::drop_install_group()?;
        Ok(())
    }

    /// Carry on as the installed copy: exec it with the same arguments,
    /// unless this is it.
    fn run_the_installed_copy() -> Result<(), Error> {
        let found = installed()?;
        let this = std::env::current_exe()?;
        let installed = found.record.expected.path();
        if std::fs::canonicalize(&this).is_ok_and(|this| this == installed) {
            return Ok(());
        }
        if let Some(differs) =
            install::handing_over(env!("CARGO_PKG_VERSION"), &found.record.version)
        {
            eprintln!("{differs}");
        }
        let mut command = std::process::Command::new(installed);
        command.args(std::env::args_os().skip(1));
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt as _;
            Err(format!("cannot run {}: {}", installed.display(), command.exec()).into())
        }
        #[cfg(not(unix))]
        {
            let status = command.status()?;
            std::process::exit(status.code().unwrap_or(1));
        }
    }

    /// The label a development install carries in a run's header.
    pub fn development_header(target: &BrowserTarget) -> Option<String> {
        if !matches!(target.mode, BrowserMode::Extension { .. }) {
            return None;
        }
        install::run_header(installed().ok()?.record.kind)
    }

    /// Where the channel is on this system.
    fn place() -> Result<Place, Error> {
        #[cfg(windows)]
        {
            use jev_pilot::browser::host::channel::pipe_name;
            Ok(Place::Pipe(pipe_name(&jev_pilot::peer::own_sid()?)))
        }
        #[cfg(not(windows))]
        {
            use jev_pilot::browser::host::channel::{check_path, socket_dir, socket_path};
            let os = Os::current();
            let path = socket_path(&socket_dir(os, &home()?, &env));
            check_path(os, &path)?;
            Ok(Place::Socket(path))
        }
    }

    /// What the other end must be: the installed copy, in this session.
    #[cfg_attr(
        not(windows),
        allow(clippy::unnecessary_wraps, reason = "Windows can fail")
    )]
    fn expected(found: &Found) -> Result<Expected, Error> {
        let expected = found.record.expected.clone();
        #[cfg(windows)]
        let expected = expected.with_session(Some(jev_pilot::peer::own_session()?));
        Ok(expected)
    }

    /// A run's way onto the bridge: check the host, take the bridge, then
    /// give up the install's group.
    pub fn connect(target: &BrowserTarget, goal: &str) -> Result<super::web::AnyWire, Error> {
        let found = installed()?;
        let sites: Vec<String> = target.urls.iter().map(ToString::to_string).collect();
        let wire = connect_run(
            &place()?,
            &expected(&found)?,
            &Native,
            goal,
            &sites,
            &jev_pilot::peer::parent(),
        )?;
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        jev_pilot::peer::drop_install_group()?;
        Ok(super::web::AnyWire::Extension(wire))
    }

    /// Host mode: Chrome started this program for the extension.
    ///
    /// The host is the genuine installed copy whoever starts it, so it first
    /// checks that the program that started it is a browser, audits what it
    /// found, and serves nothing otherwise: a program running as the owner
    /// could otherwise start it on pipes of its own and pose as the
    /// extension.
    pub fn host() -> Result<(), Error> {
        use jev_pilot::browser::host::{
            AUDIT_LIMIT, Audit, append_audit, refused_start_lately, utc,
        };
        let found = installed()?;
        let parent = jev_pilot::peer::browser::browser_parent();
        let started = Audit::HostStarted {
            parent_pid: parent.pid,
            parent_exe: parent.exe.as_ref().map(|e| e.display().to_string()),
            refused: parent.verdict.clone().err(),
        };
        // A refused start within a minute of the last is not written again,
        // so a loop of them cannot rotate the log away.
        let now = std::time::SystemTime::now();
        let tail = log_tail(&found.layout.audit_log.join("audit.log"));
        if parent.verdict.is_ok() || !refused_start_lately(&tail, now) {
            append_audit(
                &found.layout.audit_log,
                &started.line(&utc(now)),
                AUDIT_LIMIT,
            )?;
        }
        if let Err(why) = parent.verdict {
            return Err(format!(
                "the host serves only a browser, and what started it ({}) is not one: {why}",
                parent
                    .exe
                    .map_or_else(|| "unknown".to_owned(), |e| e.display().to_string())
            )
            .into());
        }
        let place = place()?;
        #[cfg(unix)]
        if let Place::Socket(path) = &place {
            prepare_socket_dir(path.parent().unwrap_or(path))?;
        }
        let diagnostics = diagnostics_file(&place)
            .map(|file| jev_pilot::browser::host::Diagnostics::new(file, DIAGNOSTICS_LIMIT));
        let layout = found.layout.clone();
        let install_json = layout.install_json.clone();
        let config = Serve {
            place,
            expected: expected(&found)?,
            check: std::sync::Arc::new(Native),
            audit_dir: layout.audit_log.clone(),
            version: env!("CARGO_PKG_VERSION").into(),
            extension_on_disk: extension_version(&layout.extension),
            installed_version: Box::new(move || {
                let text = std::fs::read_to_string(&install_json).ok()?;
                let record: install::InstallRecord = serde_json::from_str(&text).ok()?;
                Some(record.version)
            }),
            diagnostics,
        };
        serve(config, std::io::stdin(), std::io::stdout().lock())?;
        Ok(())
    }

    /// The last few lines of a log, for what was written lately.
    fn log_tail(log: &Path) -> String {
        use std::io::{Read as _, Seek as _, SeekFrom};
        let Ok(mut file) = std::fs::File::open(log) else {
            return String::new();
        };
        let length = file.metadata().map_or(0, |m| m.len());
        let _ = file.seek(SeekFrom::Start(length.saturating_sub(8 << 10)));
        let mut tail = String::new();
        let _ = file.read_to_string(&mut tail);
        tail
    }

    /// How large the host's diagnostics may grow before they start again.
    const DIAGNOSTICS_LIMIT: u64 = 256 << 10;

    /// Where the host notes what went wrong: beside the socket, or on
    /// Windows, where the pipe has no folder, in the owner's local data.
    fn diagnostics_file(place: &Place) -> Option<PathBuf> {
        match place {
            Place::Socket(path) => Some(path.with_file_name("host.log")),
            Place::Pipe(_) => {
                let dir = PathBuf::from(env("LOCALAPPDATA")?).join("jev-pilot");
                std::fs::create_dir_all(&dir).ok()?;
                Some(dir.join("host.log"))
            }
        }
    }

    /// The socket's directory: made if missing, the owner's and closed to
    /// everyone else, or refused.
    #[cfg(unix)]
    fn prepare_socket_dir(dir: &Path) -> Result<(), Error> {
        use jev_pilot::browser::host::channel::{DirFacts, check_dir};
        use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _};
        if !dir.exists() {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)?;
        }
        let meta = std::fs::symlink_metadata(dir)?;
        let facts = DirFacts {
            is_dir: meta.is_dir(),
            owner: meta.uid(),
            mode: meta.mode(),
        };
        check_dir(dir, facts, rustix::process::getuid().as_raw())?;
        Ok(())
    }

    /// The version in the extension's manifest on disk.
    fn extension_version(folder: &Path) -> Option<String> {
        let text = std::fs::read_to_string(folder.join("manifest.json")).ok()?;
        let manifest: serde_json::Value = serde_json::from_str(&text).ok()?;
        manifest["version"].as_str().map(str::to_owned)
    }

    /// `jev-pilot extension …`.
    pub fn command(command: ExtensionCommand) -> Result<(), Error> {
        match command {
            ExtensionCommand::Install { dev } => install_bridge(dev),
            ExtensionCommand::Status => status(),
            ExtensionCommand::Uninstall => uninstall(),
            ExtensionCommand::Log => log(),
            ExtensionCommand::Sites { add, origin } => {
                edit_policy(|policy| install::with_site(policy, &origin, add))
            }
            ExtensionCommand::OwnerTabs(rule) => {
                edit_policy(|policy| install::with_owner_tabs(policy, rule))
            }
        }
    }

    /// Whether this process has an administrator's rights.
    #[cfg_attr(
        not(windows),
        allow(clippy::unnecessary_wraps, reason = "Windows can fail")
    )]
    fn is_admin() -> Result<bool, Error> {
        #[cfg(unix)]
        return Ok(rustix::process::geteuid().is_root());
        #[cfg(windows)]
        return Ok(jev_pilot::peer::elevated()?);
    }

    const NEEDS_ADMIN: &str = "this changes what only an administrator may: run it with sudo \
                               (an elevated prompt on Windows)";

    /// Who the install is for, and where their home is.
    fn installer(dev: bool) -> Result<(Installer, PathBuf), Error> {
        #[cfg(unix)]
        {
            if dev {
                return Ok((Installer::Uid(rustix::process::getuid().as_raw()), home()?));
            }
            // Run with sudo: the owner is who ran sudo.
            let uid: u32 = env("SUDO_UID")
                .and_then(|u| u.parse().ok())
                .ok_or("run it with sudo from your own account, so the bridge is pinned to you")?;
            let user = nix::unistd::User::from_uid(nix::unistd::Uid::from_raw(uid))?
                .ok_or("cannot find the account that ran sudo")?;
            Ok((Installer::Uid(uid), user.dir))
        }
        #[cfg(windows)]
        {
            if !dev {
                install::check_elevated_account()?;
            }
            Ok((Installer::Sid(jev_pilot::peer::own_sid()?), home()?))
        }
    }

    fn install_bridge(dev: bool) -> Result<(), Error> {
        let os = Os::current();
        if !dev {
            println!("{}\n", install::MANAGED_WARNING);
        }
        if !dev && !is_admin()? {
            return Err(format!(
                "{NEEDS_ADMIN}, or `jev-pilot extension install --dev` for a development \
                 install"
            )
            .into());
        }
        let (installer, owner_home) = installer(dev)?;
        let (kind, layout) = if dev {
            (
                InstallKind::Development,
                Layout::development(os, &owner_home, &env),
            )
        } else {
            (InstallKind::Protected, Layout::protected(os, &folders()?))
        };
        let hosts = user_hosts(os, kind, &owner_home)?;
        // What an earlier install moved stays recorded, so uninstalling
        // still removes it.
        let moved_hosts = std::fs::read_to_string(&layout.install_json)
            .ok()
            .and_then(|text| serde_json::from_str::<install::InstallRecord>(&text).ok())
            .map(|record| record.moved_hosts)
            .unwrap_or_default();
        let existing = install::Existing {
            moved_hosts,
            user_host_dirs: user_host_dirs(os, &owner_home),
        };
        let steps = install::plan(
            os,
            kind,
            &layout,
            &std::env::current_exe()?,
            &hosts,
            &existing,
        );
        for line in install::apply(&steps, &installer, kind)? {
            println!("{line}");
        }
        println!(
            "installed jev-pilot {} at {}",
            env!("CARGO_PKG_VERSION"),
            layout.binary.display()
        );
        if dev {
            println!("{UNPROTECTED}");
        }
        println!(
            "\nNext, in Chrome: open chrome://extensions, turn on Developer mode (Chrome \
             reminds you of it when it starts), choose \"Load unpacked\", and pick\n  {}\n\
             Then `jev-pilot extension status` says whether the bridge answers.",
            layout.extension.display()
        );
        Ok(())
    }

    /// The owner's user-level host folders that exist, not followed
    /// through a link.
    fn user_host_dirs(os: Os, home: &Path) -> Vec<PathBuf> {
        install::user_host_dirs(os, home)
            .into_iter()
            .map(|(user, _)| user)
            .filter(|dir| std::fs::symlink_metadata(dir).is_ok_and(|m| m.is_dir()))
            .collect()
    }

    /// The owner's user-level hosts, as the install deals with them.
    fn user_hosts(os: Os, kind: InstallKind, home: &Path) -> Result<UserHosts, Error> {
        if kind == InstallKind::Development {
            // Registered for each browser the owner has.
            let dirs = install::user_host_dirs(os, home)
                .into_iter()
                .map(|(user, _)| user)
                .filter(|dir| dir.parent().is_some_and(Path::exists))
                .collect();
            return Ok(UserHosts::Register(dirs));
        }
        if os == Os::Windows {
            let reg = install::tool_here(install::Tool::Reg)?;
            let mut found = Vec::new();
            for key in install::windows_host_keys() {
                let output = std::process::Command::new(&reg)
                    .args(["query", &format!(r"HKCU\{key}"), "/s"])
                    .output()?;
                for (name, manifest) in
                    install::reg_hosts_in(&String::from_utf8_lossy(&output.stdout))
                {
                    found.push(install::RegistryHost {
                        key: key.to_owned(),
                        name,
                        manifest,
                    });
                }
            }
            // One the machine already has under that name is a vendor's.
            let at_machine_level = |host: &install::RegistryHost| {
                std::process::Command::new(&reg)
                    .args(["query", &format!(r"HKLM\{}\{}", host.key, host.name)])
                    .output()
                    .is_ok_and(|o| o.status.success())
            };
            return Ok(UserHosts::Registry(install::registry_hosts_to_copy(
                found,
                &at_machine_level,
            )));
        }
        // Only the owner's own folders and files, never through a link: the
        // install copies them as root.
        let mut found = Vec::new();
        for (user, system) in install::user_host_dirs(os, home) {
            if !std::fs::symlink_metadata(&user).is_ok_and(|m| m.is_dir()) {
                continue;
            }
            let Ok(entries) = std::fs::read_dir(&user) else {
                continue;
            };
            for entry in entries.flatten() {
                if entry.file_type().is_ok_and(|t| t.is_file()) {
                    found.push((entry.path(), system.clone()));
                }
            }
        }
        Ok(UserHosts::Files(install::host_copies(&found, &|p| {
            p.exists()
        })))
    }

    fn status() -> Result<(), Error> {
        let os = Os::current();
        let found = install::find_installed(os, &folders()?, &home()?, &env, &read_install)?;
        let host_answers = match &found {
            Some(found) => host_answers(&place()?, &expected(found)?, &Native),
            None => false,
        };
        let policy = found
            .as_ref()
            .and_then(|f| read(&f.layout.extension.join("policy.json")))
            .and_then(|text| serde_json::from_str(&text).ok());
        let status = Status {
            os,
            found,
            this_version: env!("CARGO_PKG_VERSION").into(),
            policy_in_place: policy_in_place(os),
            host_answers,
            policy,
        };
        for line in install::status_lines(&status)? {
            println!("{line}");
        }
        Ok(())
    }

    /// Whether Chrome's policy turning user-level hosts off is in place,
    /// for every browser: read for its value, not its presence.
    fn policy_in_place(os: Os) -> bool {
        match os {
            Os::MacOs => install::managed_preferences().iter().all(|plist| {
                install::tool_here(install::Tool::Plutil).is_ok_and(|plutil| {
                    std::process::Command::new(plutil)
                        .args([
                            "-extract",
                            "NativeMessagingUserLevelHosts",
                            "raw",
                            "-o",
                            "-",
                        ])
                        .arg(plist)
                        .output()
                        .is_ok_and(|o| o.status.success() && o.stdout.trim_ascii() == b"false")
                })
            }),
            Os::Linux => install::linux_policy_files()
                .iter()
                .all(|file| read(file).is_some_and(|text| install::linux_policy_holds(&text))),
            Os::Windows => install::tool_here(install::Tool::Reg).is_ok_and(|reg| {
                install::windows_policy_keys().iter().all(|key| {
                    std::process::Command::new(&reg)
                        .args([
                            "query",
                            &format!(r"HKLM\{key}"),
                            "/v",
                            "NativeMessagingUserLevelHosts",
                        ])
                        .output()
                        .is_ok_and(|o| {
                            install::reg_dword_in(
                                &String::from_utf8_lossy(&o.stdout),
                                "NativeMessagingUserLevelHosts",
                            ) == Some(0)
                        })
                })
            }),
        }
    }

    fn uninstall() -> Result<(), Error> {
        let found = installed()?;
        if found.record.kind == InstallKind::Protected && !is_admin()? {
            return Err(NEEDS_ADMIN.into());
        }
        let (installer, owner_home) = installer(found.record.kind == InstallKind::Development)?;
        let steps = install::uninstall_plan(
            Os::current(),
            &found.layout,
            &found.record,
            &user_host_dirs(Os::current(), &owner_home),
        );
        for line in install::apply(&steps, &installer, found.record.kind)? {
            println!("{line}");
        }
        println!(
            "removed jev-pilot's bridge; the audit log stays in {}",
            found.layout.audit_log.display()
        );
        Ok(())
    }

    fn log() -> Result<(), Error> {
        let found = installed()?;
        match std::fs::read_to_string(found.layout.audit_log.join("audit.log")) {
            Ok(text) => print!("{text}"),
            Err(_) => println!("the audit log is empty"),
        }
        Ok(())
    }

    fn edit_policy(change: impl Fn(&serde_json::Value) -> serde_json::Value) -> Result<(), Error> {
        let found = installed()?;
        if found.record.kind == InstallKind::Protected && !is_admin()? {
            return Err(NEEDS_ADMIN.into());
        }
        let path = found.layout.extension.join("policy.json");
        let policy: serde_json::Value = serde_json::from_str(
            &read(&path).ok_or("the extension's policy.json is missing: run the install again")?,
        )?;
        let changed = change(&policy);
        std::fs::write(&path, serde_json::to_string_pretty(&changed)?)?;
        println!("{}", serde_json::to_string_pretty(&changed)?);
        Ok(())
    }
}

/// Without the `extension` feature, the bridge says how to get it.
#[cfg(not(feature = "extension"))]
mod bridge {
    use jev_pilot::cli::{ExtensionCommand, Invocation};

    type Error = Box<dyn core::error::Error>;

    const REBUILD: &str = "this build has no bridge to Chrome's extension; rebuild with \
                           `cargo install --features extension`";

    pub fn give_up_group(_: &Invocation) -> Result<(), Error> {
        Ok(())
    }

    pub fn command(_: ExtensionCommand) -> Result<(), Error> {
        Err(REBUILD.into())
    }

    pub fn host() -> Result<(), Error> {
        Err(REBUILD.into())
    }

    #[cfg(feature = "browser")]
    pub fn connect(
        _: &jev_pilot::cli::BrowserTarget,
        _: &str,
    ) -> Result<super::web::AnyWire, Error> {
        Err(REBUILD.into())
    }

    #[cfg(feature = "browser")]
    pub fn development_header(_: &jev_pilot::cli::BrowserTarget) -> Option<String> {
        None
    }
}
