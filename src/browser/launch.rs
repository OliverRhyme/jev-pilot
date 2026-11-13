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

/// The file a running run holds locked in its profile folder.
///
/// The operating system lets go of the lock when the process ends however it
/// ends, so a folder whose lock can be taken belongs to a run that is gone,
/// such as one stopped by Ctrl-C, and the next fresh run removes it.
const LOCK_FILE: &str = "jev-pilot.lock";

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
    _lock: File,
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
        let dir = temp.join(format!(
            "{PROFILE_PREFIX}{}-{:08x}",
            std::process::id(),
            random_word()
        ));
        std::fs::create_dir_all(&dir).map_err(LaunchError::Profile)?;
        let lock = File::create(dir.join(LOCK_FILE)).map_err(LaunchError::Profile)?;
        lock.try_lock()
            .map_err(|e| LaunchError::Profile(std::io::Error::other(e.to_string())))?;
        let child = Command::new(binary)
            .args(launch_args(&dir, headless))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let child = match child {
            Ok(child) => child,
            Err(error) => {
                let _ = std::fs::remove_dir_all(&dir);
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
            _lock: lock,
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
        for _ in 0..20 {
            if std::fs::remove_dir_all(&self.dir).is_ok() || !self.dir.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
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
        if !name.to_string_lossy().starts_with(PROFILE_PREFIX) {
            continue;
        }
        let dir = entry.path();
        let Ok(lock) = File::open(dir.join(LOCK_FILE)) else {
            continue;
        };
        if lock.try_lock().is_err() {
            continue;
        }
        if let Ok(pid) = std::fs::read_to_string(dir.join(PID_FILE)) {
            end_leftover_browser(pid.trim(), &dir);
        }
        drop(lock);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// End a leftover browser by its process id, but only if that process is
/// still the browser started on `dir`: a process id is reused once its
/// process is gone, and ending whatever holds it now would be a stranger's.
///
/// Only where `ps` can say what a process was started with; elsewhere the
/// folder is removed and a leftover browser is left alone.
fn end_leftover_browser(pid: &str, dir: &Path) {
    if pid.is_empty() || !pid.bytes().all(|b| b.is_ascii_digit()) || cfg!(windows) {
        return;
    }
    let Ok(output) = Command::new("ps")
        .args(["-p", pid, "-o", "command="])
        .output()
    else {
        return;
    };
    let started_with = String::from_utf8_lossy(&output.stdout);
    if started_with.contains(&format!("--user-data-dir={}", dir.display())) {
        let _ = Command::new("kill").arg(pid).status();
    }
}

fn random_word() -> u32 {
    let mut bytes = [0u8; 4];
    let _ = getrandom::fill(&mut bytes);
    u32::from_le_bytes(bytes)
}
