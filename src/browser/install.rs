#![cfg(feature = "extension")]
//! Installing the extension, its policy and the host's manifest where Chrome
//! looks for them on each system.
//!
//! Everything here is a value computed from a given home and environment:
//! where each file goes, and what is written there. The install is the
//! owner's own: it needs no administrator, and writes only in the owner's
//! folders (and, on Windows, under HKCU), as Chrome's other user-level
//! native hosts do. The bridge it sets up is kept from other users and
//! other machines; a program running as the owner could pose as jev-pilot,
//! which the README says.

use std::fs::OpenOptions;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::browser::bridge::{EXTENSION_ID, HOST_NAME};
use crate::browser::discover::Os;
use crate::cli::OwnerTabs;
use crate::peer::Expected;

/// `parts` joined onto `base` with `os`'s separator, whatever this program
/// was built for, so every system's layout can be computed anywhere.
fn join(os: Os, base: &str, parts: &[&str]) -> PathBuf {
    let separator = if os == Os::Windows { '\\' } else { '/' };
    let mut path = base.trim_end_matches(separator).to_owned();
    for part in parts {
        path.push(separator);
        path.push_str(part);
    }
    PathBuf::from(path)
}

/// The name of the host's manifest file.
fn manifest_name() -> String {
    format!("{HOST_NAME}.json")
}

/// The binary's file name on `os`.
fn binary_name(os: Os) -> &'static str {
    if os == Os::Windows {
        "jev-pilot.exe"
    } else {
        "jev-pilot"
    }
}

/// Where an install puts each of its files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    /// The installed binary: the only one the bridge serves.
    pub binary: PathBuf,
    /// What both ends of the bridge expect of the other.
    pub install_json: PathBuf,
    /// The extension's folder, `policy.json` included.
    pub extension: PathBuf,
    /// The audit log's directory.
    pub audit_log: PathBuf,
    /// The host manifest, where the Windows registry points; on macOS and
    /// Linux a copy also goes in each Chrome channel's folder.
    pub host_manifest: PathBuf,
}

impl Layout {
    /// The install's layout for the owner whose home is `home`: everything
    /// in one folder of the owner's.
    #[must_use]
    pub fn new(os: Os, home: &Path, env: &dyn Fn(&str) -> Option<String>) -> Self {
        let home = home.to_string_lossy();
        let root = match os {
            Os::MacOs => join(os, &home, &["Library", "Application Support", "jev-pilot"]),
            Os::Linux => join(os, &home, &[".local", "share", "jev-pilot"]),
            Os::Windows => {
                let local = env("LOCALAPPDATA").unwrap_or_else(|| {
                    join(os, &home, &["AppData", "Local"])
                        .to_string_lossy()
                        .into()
                });
                join(os, &local, &["jev-pilot"])
            }
        };
        let root = root.to_string_lossy();
        Self {
            binary: join(os, &root, &[binary_name(os)]),
            install_json: join(os, &root, &["install.json"]),
            extension: join(os, &root, &["extension"]),
            audit_log: join(os, &root, &["log"]),
            host_manifest: join(os, &root, &[&manifest_name()]),
        }
    }
}

/// The host manifest: Chrome starts `binary` for jev-pilot's extension, and
/// for no other.
#[must_use]
pub fn host_manifest(binary: &Path) -> String {
    let manifest = json!({
        "name": HOST_NAME,
        "description": "jev-pilot's bridge to its Chrome extension",
        "path": binary,
        "type": "stdio",
        "allowed_origins": [format!("chrome-extension://{EXTENSION_ID}/")],
    });
    serde_json::to_string_pretty(&manifest).unwrap_or_default()
}

/// `install.json`: what both ends of the bridge expect of the other.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RecordFile", into = "RecordFile")]
pub struct InstallRecord {
    /// The installed jev-pilot's version.
    pub version: String,
    /// What each end of the bridge expects of the other.
    pub expected: Expected,
}

/// `install.json` as written: with the extension ID, which must be
/// jev-pilot's. Fields an earlier install wrote are passed over.
#[derive(Serialize, Deserialize)]
struct RecordFile {
    version: String,
    extension_id: String,
    expected: Expected,
}

impl From<InstallRecord> for RecordFile {
    fn from(record: InstallRecord) -> Self {
        Self {
            version: record.version,
            extension_id: EXTENSION_ID.to_owned(),
            expected: record.expected,
        }
    }
}

impl TryFrom<RecordFile> for InstallRecord {
    type Error = String;

    fn try_from(file: RecordFile) -> Result<Self, Self::Error> {
        if file.extension_id != EXTENSION_ID {
            return Err(format!(
                "install.json is for extension {}, not jev-pilot's",
                file.extension_id
            ));
        }
        Ok(Self {
            version: file.version,
            expected: file.expected,
        })
    }
}

/// `policy.json` for an install: the existing policy's sites and tab rule
/// kept, or the defaults (every site, tabs in the group).
#[must_use]
pub fn policy_for_install(existing: Option<Value>) -> Value {
    match existing {
        Some(Value::Object(mut policy)) => {
            // An earlier install said what kind it was; there is one kind.
            policy.remove("install");
            Value::Object(policy)
        }
        _ => json!({"sites": [], "owner_tabs": OwnerTabs::Group}),
    }
}

/// `policy` with `origin` added to its sites once, or removed.
#[must_use]
pub fn with_site(policy: &Value, origin: &str, add: bool) -> Value {
    let mut policy = policy.clone();
    let mut sites: Vec<Value> = policy
        .get("sites")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    sites.retain(|site| site.as_str() != Some(origin));
    if add {
        sites.push(Value::from(origin));
    }
    policy["sites"] = Value::Array(sites);
    policy
}

/// `policy` with its rule for the owner's tabs set.
#[must_use]
pub fn with_owner_tabs(policy: &Value, rule: OwnerTabs) -> Value {
    let mut policy = policy.clone();
    policy["owner_tabs"] = serde_json::to_value(rule).unwrap_or_default();
    policy
}

/// Google Chrome's profile folders under `~/Library/Application Support`,
/// one per channel. The host serves Google Chrome alone: its check of the
/// browser that started it accepts nothing else.
const MACOS_CHROME: &[&str] = &[
    "Google/Chrome",
    "Google/Chrome Beta",
    "Google/Chrome Dev",
    "Google/Chrome Canary",
];

/// Google Chrome's profile folders under `~/.config`, one per channel.
const LINUX_CHROME: &[&str] = &[
    "google-chrome",
    "google-chrome-beta",
    "google-chrome-unstable",
    "google-chrome-canary",
];

/// Where Chrome looks for the owner's hosts on Windows, under HKCU.
pub const WINDOWS_HOST_KEY: &str = r"SOFTWARE\Google\Chrome\NativeMessagingHosts";

/// The folder each Chrome channel reads the owner's hosts from, under
/// `home`, on macOS or Linux. Windows uses the registry.
#[must_use]
pub fn chrome_host_dirs(os: Os, home: &Path) -> Vec<PathBuf> {
    let home = home.to_string_lossy();
    let (base, channels) = match os {
        Os::MacOs => (
            join(os, &home, &["Library", "Application Support"]),
            MACOS_CHROME,
        ),
        Os::Linux => (join(os, &home, &[".config"]), LINUX_CHROME),
        Os::Windows => return Vec::new(),
    };
    let base = base.to_string_lossy();
    channels
        .iter()
        .map(|channel| join(os, &base, &[channel, "NativeMessagingHosts"]))
        .collect()
}

/// `reg.exe` arguments registering the host under HKCU, its default value
/// naming `manifest`.
#[must_use]
pub fn reg_add_host(manifest: &Path) -> Vec<String> {
    vec![
        "add".into(),
        format!(r"HKCU\{WINDOWS_HOST_KEY}\{HOST_NAME}"),
        "/ve".into(),
        "/t".into(),
        "REG_SZ".into(),
        "/d".into(),
        manifest.to_string_lossy().into_owned(),
        "/f".into(),
    ]
}

/// `reg.exe` arguments removing the host's registration.
#[must_use]
pub fn reg_delete_host() -> Vec<String> {
    vec![
        "delete".into(),
        format!(r"HKCU\{WINDOWS_HOST_KEY}\{HOST_NAME}"),
        "/f".into(),
    ]
}

/// One thing an install does.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Step {
    /// Create a directory, with its parents.
    CreateDir {
        /// The directory.
        path: PathBuf,
        /// Its permission bits.
        mode: u32,
    },
    /// Copy the running binary to where it is installed.
    CopyBinary {
        /// The running binary.
        from: PathBuf,
        /// The installed one.
        to: PathBuf,
    },
    /// Re-sign a binary ad hoc with the hardened runtime (macOS).
    Sign {
        /// The binary.
        path: PathBuf,
    },
    /// Give a file its permission bits.
    SetMode {
        /// The file.
        path: PathBuf,
        /// Its permission bits.
        mode: u32,
    },
    /// Write a file.
    WriteFile {
        /// The file.
        path: PathBuf,
        /// What it holds.
        contents: Vec<u8>,
        /// Its permission bits.
        mode: u32,
    },
    /// Write `policy.json`, keeping an existing one's sites and tab rule.
    WritePolicy {
        /// The file.
        path: PathBuf,
    },
    /// Run `reg.exe` with these arguments (Windows).
    Reg(Vec<String>),
    /// Remove a file, or a directory and all it holds.
    Remove {
        /// What.
        path: PathBuf,
    },
    /// Tell the owner something.
    Tell(String),
    /// Pin the installed binary and write `install.json`.
    Record {
        /// `install.json`.
        path: PathBuf,
        /// The installed binary, to pin.
        binary: PathBuf,
    },
}

/// `rel`, a path with `/` between its parts, under `base`, with `os`'s
/// separator.
fn under(os: Os, base: &Path, rel: &str) -> PathBuf {
    let parts: Vec<&str> = rel.split('/').collect();
    join(os, &base.to_string_lossy(), &parts)
}

/// The folder `path` is in, split with `os`'s separator, whatever this
/// program was built for.
fn parent(os: Os, path: &Path) -> PathBuf {
    let separator = if os == Os::Windows { '\\' } else { '/' };
    let text = path.to_string_lossy();
    text.rsplit_once(separator)
        .map_or_else(PathBuf::new, |(folder, _)| PathBuf::from(folder))
}

/// Everything an install does on `os`, in order, copying the running
/// binary `exe` into `layout` and registering the host in `chrome_dirs`,
/// the folders of the Chrome channels found in the owner's home (on
/// Windows, under HKCU instead).
#[must_use]
pub fn plan(os: Os, layout: &Layout, exe: &Path, chrome_dirs: &[PathBuf]) -> Vec<Step> {
    let mut steps = Vec::new();
    let dir = |steps: &mut Vec<Step>, path: &Path| {
        steps.push(Step::CreateDir {
            path: path.to_owned(),
            mode: 0o755,
        });
    };
    let file = |steps: &mut Vec<Step>, path: PathBuf, contents: &[u8]| {
        steps.push(Step::WriteFile {
            path,
            contents: contents.to_vec(),
            mode: 0o644,
        });
    };

    // The binary: copied closed to everyone, signed on macOS, then opened to
    // run.
    dir(&mut steps, &parent(os, &layout.binary));
    steps.push(Step::CopyBinary {
        from: exe.to_owned(),
        to: layout.binary.clone(),
    });
    if os == Os::MacOs {
        steps.push(Step::Sign {
            path: layout.binary.clone(),
        });
    }
    steps.push(Step::SetMode {
        path: layout.binary.clone(),
        mode: 0o755,
    });

    // The extension's files, and its policy.
    dir(&mut steps, &layout.extension);
    let mut made: Vec<PathBuf> = Vec::new();
    for (name, contents) in crate::browser::packaged::FILES {
        let path = under(os, &layout.extension, name);
        let folder = parent(os, &path);
        if folder != layout.extension && !made.contains(&folder) {
            dir(&mut steps, &folder);
            made.push(folder);
        }
        if *name != "policy.json" {
            file(&mut steps, path, contents);
        }
    }
    steps.push(Step::WritePolicy {
        path: under(os, &layout.extension, "policy.json"),
    });

    // The host manifest, and where Chrome finds it.
    let manifest = host_manifest(&layout.binary);
    file(
        &mut steps,
        layout.host_manifest.clone(),
        manifest.as_bytes(),
    );
    if os == Os::Windows {
        steps.push(Step::Reg(reg_add_host(&layout.host_manifest)));
    } else if chrome_dirs.is_empty() {
        steps.push(Step::Tell(
            "no Google Chrome profile was found in your home, so the host is registered with \
             none: run the install again once Chrome has started"
                .into(),
        ));
    }
    for folder in chrome_dirs {
        dir(&mut steps, folder);
        file(
            &mut steps,
            folder.join(manifest_name()),
            manifest.as_bytes(),
        );
    }

    steps.push(Step::CreateDir {
        path: layout.audit_log.clone(),
        mode: 0o700,
    });
    steps.push(Step::Record {
        path: layout.install_json.clone(),
        binary: layout.binary.clone(),
    });
    steps
}

/// The cdhash in `codesign -d -vvv`'s report, as a pin.
#[must_use]
pub fn cdhash_in(report: &str) -> Option<crate::peer::MacosPin> {
    report
        .lines()
        .find_map(|line| line.strip_prefix("CDHash="))
        .and_then(|hex| crate::peer::MacosPin::from_cdhash_hex(hex.trim()).ok())
}

/// Who the install is for: the owner, by uid or SID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Installer {
    /// macOS or Linux.
    Uid(u32),
    /// Windows.
    Sid(String),
}

/// Carry out `steps` on this system, for `installer`, saying what each
/// changed.
///
/// # Errors
/// The first step that fails, naming it.
pub fn apply(steps: &[Step], installer: &Installer) -> io::Result<Vec<String>> {
    let mut said = Vec::new();
    for step in steps {
        apply_one(step, installer, &mut said)
            .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", describe(step))))?;
    }
    Ok(said)
}

/// Copy `from` into `to`: into a new file beside it, closed to everyone
/// else, then renamed over `to`, so a running copy is replaced, not
/// rewritten.
///
/// # Errors
/// When the new file exists already, or the copy fails.
pub fn copy_into_place(from: &Path, to: &Path) -> io::Result<()> {
    let fresh = to.with_extension("new");
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(&fresh)?;
    io::copy(&mut std::fs::File::open(from)?, &mut file)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&fresh, to)
}

/// A step in words.
fn describe(step: &Step) -> String {
    match step {
        Step::CopyBinary { to, .. } => format!("copying jev-pilot to {}", to.display()),
        Step::Sign { path } => format!("signing {}", path.display()),
        Step::CreateDir { path, .. }
        | Step::Remove { path }
        | Step::SetMode { path, .. }
        | Step::WriteFile { path, .. }
        | Step::WritePolicy { path }
        | Step::Record { path, .. } => path.display().to_string(),
        Step::Reg(args) => format!("reg {}", args.join(" ")),
        Step::Tell(words) => words.clone(),
    }
}

fn apply_one(step: &Step, installer: &Installer, said: &mut Vec<String>) -> io::Result<()> {
    match step {
        Step::CreateDir { path, mode } => {
            std::fs::create_dir_all(path)?;
            set_mode(path, *mode)?;
        }
        Step::CopyBinary { from, to } => {
            if same_file(from, to) {
                said.push(format!("jev-pilot is already at {}", to.display()));
            } else {
                // A copy left by an install that failed part way.
                let _ = std::fs::remove_file(to.with_extension("new"));
                copy_into_place(from, to)?;
                said.push(format!("copied jev-pilot to {}", to.display()));
            }
        }
        Step::Sign { path } => {
            run(
                Tool::Codesign,
                &["--force", "-s", "-", "-o", "runtime"],
                Some(path),
            )?;
            said.push(format!(
                "signed {} with the hardened runtime",
                path.display()
            ));
        }
        Step::SetMode { path, mode } => set_mode(path, *mode)?,
        Step::WriteFile {
            path,
            contents,
            mode,
        } => {
            write_file(path, contents)?;
            set_mode(path, *mode)?;
        }
        Step::WritePolicy { path } => {
            let existing = std::fs::read(path)
                .ok()
                .and_then(|text| serde_json::from_slice(&text).ok());
            let kept = existing.is_some();
            let policy = policy_for_install(existing);
            write_file(path, serde_json::to_string_pretty(&policy)?.as_bytes())?;
            set_mode(path, 0o644)?;
            if kept {
                said.push(format!("kept the sites and tab rule in {}", path.display()));
            }
        }
        Step::Reg(args) => {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            run(Tool::Reg, &args, None)?;
        }
        Step::Record { path, binary } => {
            let record = InstallRecord {
                version: env!("CARGO_PKG_VERSION").into(),
                expected: pin(binary, installer)?,
            };
            write_file(path, serde_json::to_string_pretty(&record)?.as_bytes())?;
            set_mode(path, 0o644)?;
            said.push(format!("pinned {} in {}", binary.display(), path.display()));
        }
        Step::Remove { path } => {
            let gone = if path.is_dir() {
                std::fs::remove_dir_all(path)
            } else {
                std::fs::remove_file(path)
            };
            match gone {
                Ok(()) => said.push(format!("removed {}", path.display())),
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        Step::Tell(words) => said.push(words.clone()),
    }
    Ok(())
}

fn same_file(a: &Path, b: &Path) -> bool {
    matches!((std::fs::canonicalize(a), std::fs::canonicalize(b)), (Ok(a), Ok(b)) if a == b)
}

fn write_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    let fresh = path.with_extension("new");
    std::fs::write(&fresh, contents)?;
    std::fs::rename(&fresh, path)
}

/// A program an install runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Tool {
    /// macOS's code signing tool.
    Codesign,
    /// Windows's registry tool.
    Reg,
}

/// Where `tool` is: a full path, never found through `PATH`, nor (on
/// Windows) beside the running binary, where Windows looks first.
/// `system32` is Windows's System32 folder, as the system says.
#[must_use]
pub fn tool_path(tool: Tool, system32: &str) -> PathBuf {
    match tool {
        Tool::Codesign => "/usr/bin/codesign".into(),
        Tool::Reg => join(Os::Windows, system32, &["reg.exe"]),
    }
}

/// Where `tool` is on this system.
///
/// # Errors
/// When Windows cannot say where System32 is.
pub fn tool_here(tool: Tool) -> io::Result<PathBuf> {
    #[cfg(windows)]
    let system32 = jev_pilot_os::known_folder(jev_pilot_os::KnownFolder::System)?
        .to_string_lossy()
        .into_owned();
    #[cfg(not(windows))]
    let system32 = String::new();
    Ok(tool_path(tool, &system32))
}

/// Run `tool` with `args` (and `path` last), failing unless it succeeds.
fn run(tool: Tool, args: &[&str], path: Option<&Path>) -> io::Result<()> {
    let program = tool_here(tool)?;
    let mut command = std::process::Command::new(&program);
    command.args(args);
    if let Some(path) = path {
        command.arg(path);
    }
    let output = command.output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "{} failed: {}",
            program.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
#[allow(clippy::unnecessary_wraps, reason = "the same signature as on Unix")]
fn set_mode(_: &Path, _: u32) -> io::Result<()> {
    // The owner's local app data gives its own access to what is made in it.
    Ok(())
}

/// What the installed `binary` is pinned by, for `installer`.
fn pin(binary: &Path, installer: &Installer) -> io::Result<Expected> {
    let path = std::fs::canonicalize(binary)?;
    let sha256 = |path: &Path| -> io::Result<crate::peer::Sha256Digest> {
        use sha2::{Digest as _, Sha256};
        let bytes: [u8; 32] = Sha256::digest(std::fs::read(path)?).into();
        Ok(crate::peer::Sha256Digest::from_bytes(bytes))
    };
    match (Os::current(), installer) {
        (Os::MacOs, Installer::Uid(uid)) => {
            let report = std::process::Command::new(tool_here(Tool::Codesign)?)
                .args(["-d", "-vvv"])
                .arg(&path)
                .output()?;
            let pin = cdhash_in(&String::from_utf8_lossy(&report.stderr))
                .ok_or_else(|| io::Error::other("codesign gave no cdhash"))?;
            Ok(Expected::Macos {
                uid: *uid,
                path,
                pin,
            })
        }
        (Os::Linux, Installer::Uid(uid)) => Ok(Expected::Linux {
            uid: *uid,
            pin: crate::peer::LinuxPin::Sha256(sha256(&path)?),
            path,
        }),
        (Os::Windows, Installer::Sid(sid)) => Ok(Expected::Windows {
            sid: sid.clone(),
            pin: crate::peer::WindowsPin::Sha256(sha256(&path)?),
            path,
            session: None,
        }),
        _ => Err(io::Error::other(
            "the installer's identity does not fit this system",
        )),
    }
}

/// Everything uninstalling does: remove the host's registrations and what
/// the install made, keeping the audit log.
#[must_use]
pub fn uninstall_plan(os: Os, layout: &Layout, chrome_dirs: &[PathBuf]) -> Vec<Step> {
    let mut steps = Vec::new();
    if os == Os::Windows {
        steps.push(Step::Reg(reg_delete_host()));
    }
    for folder in chrome_dirs {
        steps.push(Step::Remove {
            path: folder.join(manifest_name()),
        });
    }
    for path in [
        &layout.host_manifest,
        &layout.extension,
        &layout.binary,
        &layout.install_json,
    ] {
        steps.push(Step::Remove { path: path.clone() });
    }
    steps
}

/// What is said when nothing is installed.
pub const NOT_SET_UP: &str =
    "jev-pilot's Chrome extension is not set up: run `jev-pilot extension install`";

/// What a run started from another copy says as it hands over to the
/// installed one, when their versions differ.
#[must_use]
pub fn handing_over(this: &str, installed: &str) -> Option<String> {
    version_differs(this, installed)
        .map(|differs| format!("jev-pilot: {differs}; the run goes on with the installed copy"))
}

/// What is said when this binary and the installed one differ.
#[must_use]
pub fn version_differs(this: &str, installed: &str) -> Option<String> {
    (this != installed).then(|| {
        format!("this jev-pilot is {this}, the installed one is {installed}: run the install again")
    })
}

/// An install found on this system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// Where its files are.
    pub layout: Layout,
    /// Its `install.json`.
    pub record: InstallRecord,
}

/// The installed copy, if any, for the owner whose home is `home`. `read`
/// reads a file.
///
/// # Errors
/// An `install.json` that cannot be read as one.
pub fn find_installed(
    os: Os,
    home: &Path,
    env: &dyn Fn(&str) -> Option<String>,
    read: &dyn Fn(&Path) -> io::Result<String>,
) -> Result<Option<Found>, String> {
    let layout = Layout::new(os, home, env);
    let text = match read(&layout.install_json) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(format!(
                "{} cannot be read: {e}",
                layout.install_json.display()
            ));
        }
    };
    let record = serde_json::from_str(&text)
        .map_err(|e| format!("{} cannot be read: {e}", layout.install_json.display()))?;
    Ok(Some(Found { layout, record }))
}

/// The installed copy on this system, for the owner whose home is `home`:
/// [`find_installed`] with the system's own environment and files.
///
/// # Errors
/// When an `install.json` cannot be read.
pub fn installed_on_this_system(home: &Path) -> Result<Option<Found>, String> {
    let env = |name: &str| std::env::var(name).ok();
    find_installed(Os::current(), home, &env, &|path| {
        std::fs::read_to_string(path)
    })
}

/// What `jev-pilot extension status` looked at.
#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    /// The install, if any.
    pub found: Option<Found>,
    /// This binary's version.
    pub this_version: String,
    /// Whether a host that passes the check answers on the channel.
    pub host_answers: bool,
    /// The installed `policy.json`.
    pub policy: Option<Value>,
}

/// What `status` says, one line each, or why there is nothing to say.
///
/// # Errors
/// [`NOT_SET_UP`] when nothing is installed.
pub fn status_lines(status: &Status) -> Result<Vec<String>, String> {
    let found = status.found.as_ref().ok_or(NOT_SET_UP)?;
    let record = &found.record;
    let mut lines = vec![format!(
        "install    : version {}, in your own folders",
        record.version
    )];
    lines.extend(version_differs(&status.this_version, &record.version));
    lines.push(format!("binary     : {}", found.layout.binary.display()));
    lines.push(format!(
        "extension  : {} (loaded unpacked in chrome://extensions, with Developer mode on)",
        found.layout.extension.display()
    ));
    if let Some(policy) = &status.policy {
        let sites: Vec<&str> = policy
            .get("sites")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        lines.push(if sites.is_empty() {
            "sites      : every site".to_owned()
        } else {
            format!("sites      : {}", sites.join(", "))
        });
        if let Some(rule) = policy.get("owner_tabs").and_then(Value::as_str) {
            lines.push(format!("owner tabs : {rule}"));
        }
    }
    lines.push(if status.host_answers {
        "bridge     : the host answers; Chrome is running with the extension".to_owned()
    } else {
        crate::browser::extension::ConnectError::Absent.to_string()
    });
    Ok(lines)
}
