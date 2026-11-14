//! Starting a fresh browser on a throwaway profile.
//!
//! Fresh mode needs no prompt and touches nothing of the owner's: the browser
//! gets a new user data directory, writes its `DevToolsActivePort` there, and
//! both are gone when the run ends. Chrome for Testing or Chromium is
//! preferred, because from version 136 Google-branded Chrome ignores
//! `--remote-debugging-port` on its default profile.
//!
//! The argument vector and the choice of binary are pure; [`Fresh`] is the
//! process and the folder.

use crate::browser::discover::{ActivePort, AttachError, Os};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// What a fresh profile's folder is called, before the process id and a
/// random part.
pub const PROFILE_PREFIX: &str = "jev-pilot-fresh-";

/// What a profile's lock file is called after the profile's own name.
const LOCK_SUFFIX: &str = ".lock";

/// The file a running run holds locked beside its profile folder.
///
/// The operating system lets go of the lock when the process ends however it
/// ends, so a folder whose lock can be taken belongs to a run that is gone,
/// such as one killed outright, and the next fresh run removes it. It sits
/// beside the folder rather than in it because Windows will not rename or
/// remove a folder holding a file that is open.
fn lock_of(dir: &Path) -> PathBuf {
    let mut path = dir.as_os_str().to_owned();
    path.push(LOCK_SUFFIX);
    PathBuf::from(path)
}

/// The profile folder a lock file belongs to.
fn profile_of(lock: &Path) -> Option<PathBuf> {
    let name = lock.file_name()?.to_str()?.strip_suffix(LOCK_SUFFIX)?;
    Some(lock.with_file_name(name))
}

/// Remove a folder, trying `tries` times a tenth of a second apart, since a
/// browser just ended can hold its files for a moment longer. `true` once it
/// is gone.
fn remove_patiently(dir: &Path, tries: u32) -> bool {
    for _ in 0..tries {
        if std::fs::remove_dir_all(dir).is_ok() || !dir.exists() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

/// Remove a profile's lock file, once its folder is gone; while the folder
/// is there, the lock is what lets the next sweep find it.
fn remove_lock(dir: &Path, removed: bool) {
    if removed {
        let _ = std::fs::remove_file(lock_of(dir));
    }
}

/// Where the browser's process id is kept, for removing a leftover browser
/// along with its folder.
const PID_FILE: &str = "jev-pilot.browser-pid";

/// The arguments a fresh browser starts with, opening one blank tab.
#[must_use]
pub fn launch_args(profile: &Path, headless: bool) -> Vec<String> {
    let mut args = vec![
        "--remote-debugging-port=0".to_owned(),
        format!("--user-data-dir={}", profile.display()),
        "--no-first-run".to_owned(),
        "--no-default-browser-check".to_owned(),
        "--enable-automation".to_owned(),
        "--disable-search-engine-choice-screen".to_owned(),
        "--password-store=basic".to_owned(),
        "--use-mock-keychain".to_owned(),
        "--disable-sync".to_owned(),
    ];
    if headless {
        args.push("--headless".to_owned());
    }
    args.push("about:blank".to_owned());
    args
}

/// Where a browser is usually installed, in the order they are tried.
#[must_use]
pub fn chrome_candidates(os: Os) -> Vec<PathBuf> {
    let paths: &[&str] = match os {
        Os::MacOs => &[
            "/Applications/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Google Chrome Beta.app/Contents/MacOS/Google Chrome Beta",
            "/Applications/Google Chrome Dev.app/Contents/MacOS/Google Chrome Dev",
            "/Applications/Google Chrome Canary.app/Contents/MacOS/Google Chrome Canary",
        ],
        Os::Linux => &[
            "/usr/bin/chromium",
            "/usr/bin/chromium-browser",
            "/snap/bin/chromium",
            "/opt/google/chrome/chrome",
            "/usr/bin/google-chrome",
            "/usr/bin/google-chrome-stable",
        ],
        Os::Windows => &[
            r"C:\Program Files\Chromium\Application\chrome.exe",
            r"C:\Program Files\Google\Chrome\Application\chrome.exe",
            r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
        ],
    };
    paths.iter().map(PathBuf::from).collect()
}

/// The browser to start: `explicit` (`--chrome`), else `env`
/// (`JEV_PILOT_CHROME`), else the first of the usual places that `exists`.
#[must_use]
pub fn find_chrome(
    explicit: Option<&Path>,
    env: Option<std::ffi::OsString>,
    os: Os,
    exists: &dyn Fn(&Path) -> bool,
) -> Option<PathBuf> {
    if let Some(path) = explicit {
        return Some(path.to_owned());
    }
    if let Some(path) = env.filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(path));
    }
    chrome_candidates(os).into_iter().find(|p| exists(p))
}

/// Why a fresh browser could not be started.
#[derive(Debug)]
#[non_exhaustive]
pub enum LaunchError {
    /// No browser was found.
    NoBrowser,
    /// The browser could not be started.
    Spawn {
        /// The binary.
        binary: PathBuf,
        /// What went wrong.
        error: std::io::Error,
    },
    /// The profile folder could not be made.
    Profile(std::io::Error),
    /// The browser exited before it was ready.
    Exited(std::process::ExitStatus),
    /// The browser did not write its endpoint in time.
    NotReady,
    /// The endpoint file it wrote could not be read.
    Endpoint(AttachError),
    /// The browser started says it is not Chrome or Chromium.
    NotChrome {
        /// The binary that was started.
        binary: PathBuf,
        /// What it says it is.
        product: Box<str>,
    },
}

impl core::fmt::Display for LaunchError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NoBrowser => f.write_str(
                "no Chrome for Testing, Chromium or Chrome found; name one with --chrome or \
                 JEV_PILOT_CHROME (`npx @puppeteer/browsers install chrome@stable` prints a path)",
            ),
            Self::Spawn { binary, error } => {
                write!(f, "could not start {}: {error}", binary.display())
            }
            Self::Profile(e) => write!(f, "could not make a profile folder: {e}"),
            Self::Exited(status) => write!(f, "the browser exited before it was ready ({status})"),
            Self::NotReady => f.write_str("the browser did not open its debugging port in time"),
            Self::Endpoint(e) => e.fmt(f),
            Self::NotChrome { binary, product } => write!(
                f,
                "{} says it is {product}, not Chrome, Chromium or Edge, and nothing was typed; \
                 name another browser with --chrome",
                binary.display()
            ),
        }
    }
}

impl core::error::Error for LaunchError {}

/// A browser this run started, with its throwaway profile.
///
/// Dropping it reaps the browser and removes the folder: give the browser a
/// moment to finish closing (after `Browser.close`), then end it outright.
#[derive(Debug)]
pub struct Fresh {
    child: Child,
    dir: PathBuf,
    lock: Option<File>,
}

impl Fresh {
    /// Start `binary` on a new profile under the system's temporary folder,
    /// and wait up to `wait` for its endpoint.
    ///
    /// Leftover profiles of fresh runs that were stopped without cleaning up
    /// are removed first.
    ///
    /// # Errors
    /// Returns [`LaunchError`] when the browser cannot be started or does not
    /// become ready.
    pub fn start(
        binary: &Path,
        headless: bool,
        wait: Duration,
    ) -> Result<(Self, ActivePort), LaunchError> {
        let temp = std::env::temp_dir();
        sweep_leftovers(&temp);
        let (dir, lock) = make_profile(&temp).map_err(LaunchError::Profile)?;
        let child = Command::new(binary)
            .args(launch_args(&dir, headless))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let child = match child {
            Ok(child) => child,
            Err(error) => {
                let removed = std::fs::remove_dir_all(&dir).is_ok();
                drop(lock);
                remove_lock(&dir, removed);
                return Err(LaunchError::Spawn {
                    binary: binary.to_owned(),
                    error,
                });
            }
        };
        let _ = std::fs::write(dir.join(PID_FILE), child.id().to_string());
        let mut fresh = Self {
            child,
            dir,
            lock: Some(lock),
        };
        let port = fresh.wait_for_endpoint(wait)?;
        Ok((fresh, port))
    }

    fn wait_for_endpoint(&mut self, wait: Duration) -> Result<ActivePort, LaunchError> {
        let deadline = Instant::now() + wait;
        let file = self.dir.join("DevToolsActivePort");
        loop {
            // Chrome writes the file whole, but a read can still race it.
            if let Ok(text) = std::fs::read_to_string(&file)
                && let Ok(port) = ActivePort::parse(&text)
            {
                return Ok(port);
            }
            if let Ok(Some(status)) = self.child.try_wait() {
                return Err(LaunchError::Exited(status));
            }
            if Instant::now() >= deadline {
                return Err(LaunchError::NotReady);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// The profile folder.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// What Ctrl-C has to undo for this browser, reached at `port`.
    #[must_use]
    pub fn interrupted(&self, port: &ActivePort) -> Interrupted {
        Interrupted::Fresh {
            endpoint: port.endpoint(),
            pid: self.child.id(),
            dir: self.dir.clone(),
        }
    }
}

impl Drop for Fresh {
    fn drop(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        // The browser's helpers can hold files for a moment after it exits.
        let removed = remove_patiently(&self.dir, 20);
        drop(self.lock.take());
        remove_lock(&self.dir, removed);
    }
}

/// Make a fresh profile folder under `temp`, holding its lock.
///
/// The lock file beside it is made and locked first, so the folder never
/// appears unlocked: a fresh run starting at the same moment would otherwise
/// take it for a dead run's and remove it.
///
/// # Errors
/// Returns the I/O error when the lock or the folder cannot be made, or the
/// lock cannot be taken.
pub fn make_profile(temp: &Path) -> std::io::Result<(PathBuf, File)> {
    let name = format!("{}-{:08x}", std::process::id(), random_word());
    let dir = temp.join(format!("{PROFILE_PREFIX}{name}"));
    let lock_path = lock_of(&dir);
    let lock = File::create_new(&lock_path)?;
    let made = lock
        .try_lock()
        .map_err(|e| std::io::Error::other(e.to_string()))
        .and_then(|()| std::fs::create_dir(&dir));
    match made {
        Ok(()) => Ok((dir, lock)),
        Err(error) => {
            drop(lock);
            let _ = std::fs::remove_file(&lock_path);
            Err(error)
        }
    }
}

/// Remove the profiles of fresh runs that are no longer running, and their
/// browsers if those outlived them.
pub fn sweep_leftovers(temp: &Path) {
    let Ok(entries) = std::fs::read_dir(temp) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(PROFILE_PREFIX) && name.ends_with(LOCK_SUFFIX) {
            clear_if_abandoned(&entry.path());
        }
    }
}

/// Remove what the run with process id `run` left behind: its fresh
/// profiles, and their browsers if those outlived it. Returns how many
/// profiles were removed.
///
/// For whoever ended the run outright, as stopping a run over MCP does. The
/// run gets no chance to close its browser then, and a headless browser left
/// running is invisible until the next fresh start would have swept it.
#[must_use = "the count says whether anything was left behind"]
pub fn reap_run(temp: &Path, run: u32) -> usize {
    let mine = format!("{PROFILE_PREFIX}{run}-");
    let Ok(entries) = std::fs::read_dir(temp) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.starts_with(&mine) && name.ends_with(LOCK_SUFFIX)
        })
        .filter(|entry| clear_if_abandoned(&entry.path()))
        .count()
}

/// Remove the fresh profile a lock file belongs to, the browser started on
/// it, and then the lock, if the run that made it has gone. `true` when it
/// was removed.
fn clear_if_abandoned(lock_path: &Path) -> bool {
    let Some(dir) = profile_of(lock_path) else {
        return false;
    };
    let Ok(lock) = File::open(lock_path) else {
        return false;
    };
    if lock.try_lock().is_err() {
        return false;
    }
    let ended = std::fs::read_to_string(dir.join(PID_FILE))
        .is_ok_and(|pid| end_leftover_browser(pid.trim(), &dir));
    let removed = remove_patiently(&dir, if ended { 30 } else { 1 });
    drop(lock);
    remove_lock(&dir, removed);
    removed
}

/// End a leftover browser by its process id, but only if that process is
/// still the browser started on `dir`: a process id is reused once its
/// process is gone, and ending whatever holds it now would be a stranger's.
///
/// Only where `ps` can say what a process was started with; elsewhere the
/// folder is removed and a leftover browser is left alone.
///
/// `true` when a browser was told to end.
fn end_leftover_browser(pid: &str, dir: &Path) -> bool {
    browser_running(pid, dir)
        && Command::new("kill")
            .arg(pid)
            .status()
            .is_ok_and(|status| status.success())
}

/// Whether process `pid` is running, and is the browser started on `dir`.
///
/// Only where `ps` can say what a process was started with; elsewhere this
/// says no, and the browser is never ended by its process id.
fn browser_running(pid: &str, dir: &Path) -> bool {
    if pid.is_empty() || !pid.bytes().all(|b| b.is_ascii_digit()) || cfg!(windows) {
        return false;
    }
    let Ok(output) = Command::new("ps")
        .args(["-p", pid, "-o", "command="])
        .output()
    else {
        return false;
    };
    String::from_utf8_lossy(&output.stdout).contains(&format!("--user-data-dir={}", dir.display()))
}

/// What an interrupted run has to undo.
///
/// Kept apart from the run so that a Ctrl-C handler, on a thread of its own,
/// can undo it while the run is blocked on the browser or on the model and
/// will never reach its own clean-up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Interrupted {
    /// Attached to the owner's Chrome. Ending the process closes the
    /// connection, which detaches; nothing of the owner's is closed.
    Attached,
    /// A browser this run started.
    Fresh {
        /// Its browser websocket, for asking it to close.
        endpoint: String,
        /// Its process id.
        pid: u32,
        /// Its throwaway profile.
        dir: PathBuf,
    },
}

impl Interrupted {
    /// Undo what the run leaves behind.
    ///
    /// For a fresh browser, `close` is handed its endpoint to send
    /// `Browser.close`; the browser is given a moment to go, ended if it is
    /// still running on its profile, and the profile is removed. An attached
    /// run has nothing to undo.
    pub fn clean_up(&self, close: impl FnOnce(&str)) {
        let Self::Fresh { endpoint, pid, dir } = self else {
            return;
        };
        close(endpoint);
        let pid = pid.to_string();
        let deadline = Instant::now() + Duration::from_secs(3);
        while browser_running(&pid, dir) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        // Where it cannot be ended (no `ps`, as on Windows), it can only be
        // waited for, so the folder is tried for as long as a run's own
        // clean-up tries it.
        let ended = end_leftover_browser(&pid, dir);
        let removed = remove_patiently(dir, if ended { 30 } else { 20 });
        remove_lock(dir, removed);
    }
}

fn random_word() -> u32 {
    let mut bytes = [0u8; 4];
    let _ = getrandom::fill(&mut bytes);
    u32::from_le_bytes(bytes)
}
