//! Whether the program that started the host is the browser.
//!
//! The host is the genuine installed jev-pilot whoever starts it, so a run's
//! check of the host cannot tell Chrome's host from one a program running as
//! the owner started on pipes of its own, posing as the extension. The host
//! therefore checks its own parent, with the same kind of OS mechanism the
//! bridge's checks use, and refuses to serve when it is not a browser:
//!
//! - macOS: the parent's running code satisfies Chrome's own designated
//!   requirement (Google's Developer ID), Chrome for Testing included;
//! - Linux: the parent's executable, through `/proc`, is one of the known
//!   browser installs, a root file in folders only root can change;
//! - Windows: Chrome starts the host through `cmd.exe /d /s /c`, so the
//!   parent is System32's `cmd.exe`, and its parent an image under Program
//!   Files signed by Google.

use std::path::{Path, PathBuf};

/// The code requirement a macOS browser's running code must satisfy: Google
/// Chrome's own designated requirement. Chrome for Testing is left out: it
/// loads any unpacked extension it is told to, so a fake with jev-pilot's ID
/// is easy there.
const MACOS_BROWSER: &str = r#"(identifier "com.google.Chrome" or identifier "com.google.Chrome.beta" or identifier "com.google.Chrome.dev" or identifier "com.google.Chrome.canary") and anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] exists and certificate leaf[field.1.2.840.113635.100.6.1.13] exists and certificate leaf[subject.OU] = "EQHXZ8M8AV""#;

/// The code requirement a macOS browser must satisfy.
#[must_use]
pub fn macos_browser_requirement() -> &'static str {
    MACOS_BROWSER
}

/// Google Chrome's installs on Linux, as its packages install them.
/// Chromium, Edge and Brave are left out: they load any unpacked extension
/// they are told to, so a fake with jev-pilot's ID is easy there.
const LINUX_BROWSERS: &[&str] = &[
    "/opt/google/chrome/chrome",
    "/opt/google/chrome-beta/chrome",
    "/opt/google/chrome-unstable/chrome",
    "/opt/google/chrome-canary/chrome",
];

/// What the OS says of a file or folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileFacts {
    /// Its owner's uid.
    pub uid: u32,
    /// Its permission bits.
    pub mode: u32,
    /// Whether it is a symbolic link.
    pub is_link: bool,
    /// The device it is on.
    pub dev: u64,
    /// Its inode there.
    pub ino: u64,
}

/// Which Google Chrome install the file the parent runs is, by its device
/// and inode (`exe`, from `stat` of `/proc/<pid>/exe`), when that install
/// and every folder above it are root's, not writable by group or others,
/// and not links. `facts` reports a path, without following a link, or
/// `None` when it is missing.
///
/// Comparing the file itself, not the link's text, needs no trust in a
/// name. A Chrome still running a file an upgrade replaced matches none,
/// and is refused until it restarts.
///
/// # Errors
/// Why it is not one.
pub fn linux_browser(
    exe: (u64, u64),
    facts: &dyn Fn(&Path) -> Option<FileFacts>,
) -> Result<PathBuf, String> {
    let known = LINUX_BROWSERS
        .iter()
        .map(Path::new)
        .find(|path| facts(path).is_some_and(|f| (f.dev, f.ino) == exe))
        .ok_or("it runs no Google Chrome install jev-pilot knows")?;
    for path in known.ancestors() {
        let found = facts(path).ok_or_else(|| format!("{} is missing", path.display()))?;
        if found.is_link {
            return Err(format!("{} is a link", path.display()));
        }
        if found.uid != 0 {
            return Err(format!("{} is not root's", path.display()));
        }
        if found.mode & 0o022 != 0 {
            return Err(format!("{} can be written by others", path.display()));
        }
    }
    Ok(known.to_owned())
}

/// A Windows path as text, with a `\\?\` prefix set aside.
fn plain(path: &Path) -> String {
    let text = path.to_string_lossy().into_owned();
    text.strip_prefix(r"\\?\")
        .map_or(text.clone(), str::to_owned)
}

/// Whether the host's parent and grandparent are how Chrome starts it on
/// Windows: `cmd.exe` from `system32`, started by an image under one of
/// `program_files` whose Authenticode leaf signer is Google.
///
/// # Errors
/// Why they are not.
pub fn windows_browser(
    parent: &Path,
    grandparent: &Path,
    signer: Option<&str>,
    system32: &Path,
    program_files: &[&Path],
) -> Result<(), String> {
    let cmd = format!(r"{}\cmd.exe", plain(system32));
    if !plain(parent).eq_ignore_ascii_case(&cmd) {
        return Err(format!("{} is not {cmd}", parent.display()));
    }
    let browser = plain(grandparent).to_ascii_lowercase();
    let installed = program_files.iter().any(|folder| {
        let folder = plain(folder).to_ascii_lowercase();
        browser.starts_with(&format!("{folder}\\"))
    });
    if !installed {
        return Err(format!(
            "{} is not under Program Files",
            grandparent.display()
        ));
    }
    if !browser.ends_with("\\chrome.exe") {
        return Err(format!("{} is not Chrome", grandparent.display()));
    }
    match signer {
        Some(subject) if subject.split(", ").any(|part| part == "O=Google LLC") => Ok(()),
        Some(subject) => Err(format!(
            "{} is signed by {subject}, not Google",
            grandparent.display()
        )),
        None => Err(format!("{} is not signed", grandparent.display())),
    }
}

/// What the host found about the program that started it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserParent {
    /// Its process id, when the OS gave one.
    pub pid: Option<u32>,
    /// Its executable, when the OS gave one.
    pub exe: Option<PathBuf>,
    /// Whether it is a browser, or why not.
    pub verdict: Result<(), String>,
}

/// What the OS says of the program that started this one, and whether it is
/// a browser.
#[must_use]
pub fn browser_parent() -> BrowserParent {
    #[cfg(target_os = "macos")]
    return macos();
    #[cfg(target_os = "linux")]
    return linux();
    #[cfg(windows)]
    return windows();
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    BrowserParent {
        pid: None,
        exe: None,
        verdict: Err("jev-pilot has no rule for a browser on this system".into()),
    }
}

/// A parent this host does not serve, and why.
fn refused(pid: Option<u32>, exe: Option<PathBuf>, why: impl Into<String>) -> BrowserParent {
    BrowserParent {
        pid,
        exe,
        verdict: Err(why.into()),
    }
}

#[cfg(target_os = "macos")]
fn macos() -> BrowserParent {
    use security_framework::os::macos::code_signing::{
        Flags, GuestAttributes, SecCode, SecRequirement,
    };
    let pid = std::os::unix::process::parent_id();
    // A process whose parent has gone is the init process's.
    if pid == 1 {
        return refused(
            Some(pid),
            None,
            "the program that started the host has gone",
        );
    }
    let mut attributes = GuestAttributes::new();
    let Ok(raw) = i32::try_from(pid) else {
        return refused(Some(pid), None, "the parent's process id is out of range");
    };
    attributes.set_pid(raw);
    let Ok(code) = SecCode::copy_guest_with_attribues(None, &attributes, Flags::NONE) else {
        return refused(Some(pid), None, "the parent's code cannot be read");
    };
    let exe = code.path(Flags::NONE).ok().and_then(|url| url.to_path());
    let Ok(requirement) = MACOS_BROWSER.parse::<SecRequirement>() else {
        return refused(Some(pid), exe, "the browser requirement cannot be read");
    };
    let verdict = code
        .check_validity(Flags::NONE, &requirement)
        .map_err(|_| "it is not Chrome signed by Google".to_owned());
    // The code checked is what the parent runs now. A parent that ran
    // another program in place since it started the host has a newer pid
    // version than the one the host recorded of it at its start.
    let unchanged = match (
        jev_pilot_os::pid_versions(std::process::id()),
        jev_pilot_os::pid_versions(pid),
    ) {
        (Ok(mine), Ok(parent)) => parent.version == mine.parent_at_start,
        _ => false,
    };
    if !unchanged {
        return refused(
            Some(pid),
            exe,
            "the program that started the host has since run another program",
        );
    }
    // Still the same parent: had it gone, the host would have a new one.
    if std::os::unix::process::parent_id() != pid {
        return refused(Some(pid), exe, "the program that started the host has gone");
    }
    BrowserParent {
        pid: Some(pid),
        exe,
        verdict,
    }
}

#[cfg(target_os = "linux")]
fn linux() -> BrowserParent {
    use std::os::unix::fs::MetadataExt as _;
    let pid = std::os::unix::process::parent_id();
    if pid == 1 {
        return refused(
            Some(pid),
            None,
            "the program that started the host has gone",
        );
    }
    // The kernel lets one process look at another's executable only when
    // its filesystem ids match the other's, and the setgid host's gid is
    // the install's group: it looks as the owner's own gid, then goes back.
    let held = nix::unistd::setfsgid(nix::unistd::getgid());
    let link = format!("/proc/{pid}/exe");
    let file = std::fs::metadata(&link).map(|meta| (meta.dev(), meta.ino()));
    let named = std::fs::read_link(&link).ok();
    nix::unistd::setfsgid(held);
    let Ok(file) = file else {
        return refused(Some(pid), named, "the parent's executable cannot be read");
    };
    if std::os::unix::process::parent_id() != pid {
        return refused(
            Some(pid),
            named,
            "the program that started the host has gone",
        );
    }
    let facts = |path: &Path| {
        std::fs::symlink_metadata(path).ok().map(|meta| FileFacts {
            uid: meta.uid(),
            mode: meta.mode(),
            is_link: meta.file_type().is_symlink(),
            dev: meta.dev(),
            ino: meta.ino(),
        })
    };
    match linux_browser(file, &facts) {
        Ok(chrome) => BrowserParent {
            pid: Some(pid),
            exe: Some(chrome),
            verdict: Ok(()),
        },
        Err(why) => refused(Some(pid), named, why),
    }
}

#[cfg(windows)]
fn windows() -> BrowserParent {
    use jev_pilot_os::{
        KnownFolder, Process, authenticode_signer, final_path, known_folder, parent_process_id,
        process_image,
    };
    /// A process and when it began, held open so its id stays its own.
    fn open(pid: u32) -> Option<(Process, std::time::SystemTime)> {
        let process = Process::open(pid).ok()?;
        let created = process.created().ok()?;
        Some((process, created))
    }
    let me = std::process::id();
    let Some((_, mine)) = open(me) else {
        return refused(None, None, "this process cannot be read");
    };
    // Each parent must have begun before its child: a later one only
    // reused the id.
    let Some((parent, parent_began)) = parent_process_id(me)
        .ok()
        .and_then(open)
        .filter(|(_, began)| *began <= mine)
    else {
        return refused(None, None, "the program that started the host has gone");
    };
    let Ok(cmd) = process_image(&parent) else {
        return refused(None, None, "the parent's image cannot be read");
    };
    let Ok(browser_pid) = parent_process_id(me).and_then(parent_process_id) else {
        return refused(None, Some(cmd), "the parent's parent has gone");
    };
    let Some((browser, _)) = open(browser_pid).filter(|(_, began)| *began <= parent_began) else {
        return refused(Some(browser_pid), Some(cmd), "the parent's parent has gone");
    };
    let Ok(started) = process_image(&browser) else {
        return refused(
            Some(browser_pid),
            None,
            "the browser's image cannot be read",
        );
    };
    let Some(image) = std::fs::File::open(&started)
        .ok()
        .and_then(|file| final_path(&file).ok())
    else {
        return refused(
            Some(browser_pid),
            Some(started),
            "the browser's image cannot be opened",
        );
    };
    // Started from the path it resolves to, so no junction stands in.
    if !plain(&started).eq_ignore_ascii_case(&plain(&image)) {
        return refused(
            Some(browser_pid),
            Some(started),
            "the browser was started through a link",
        );
    }
    let folders = (
        known_folder(KnownFolder::System),
        known_folder(KnownFolder::ProgramFiles),
        known_folder(KnownFolder::ProgramFilesX86),
    );
    let (Ok(system32), Ok(programs), Ok(programs_x86)) = folders else {
        return refused(
            Some(browser_pid),
            Some(image),
            "Windows's folders cannot be read",
        );
    };
    let signer = authenticode_signer(&image);
    let verdict = windows_browser(
        &cmd,
        &image,
        signer.as_ref().map(|s| s.subject.as_str()),
        &system32,
        &[&programs, &programs_x86],
    );
    drop(parent);
    BrowserParent {
        pid: Some(browser_pid),
        exe: Some(image),
        verdict,
    }
}
