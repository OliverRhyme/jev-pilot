#![cfg(feature = "extension")]
//! Installing the extension, its policy and the host's manifest where Chrome
//! looks for them on each system.
//!
//! Everything here is a value computed from a given home and environment:
//! where each file goes, and what is written there. The bridge's checks are
//! only as good as what they check against, so a protected install puts the
//! binary, `install.json`, the extension and its policy, and the host
//! manifest where only an administrator can write. A development install
//! puts them in the owner's home, and says everywhere that it is not
//! protected against other programs running as the owner.

use std::fmt::Write as _;
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

/// Where Windows keeps Program Files and ProgramData for the whole
/// machine. On other systems they are not used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemFolders {
    /// Program Files.
    pub program_files: String,
    /// ProgramData.
    pub program_data: String,
}

impl SystemFolders {
    /// Where the system says they are on Windows: from the known folders,
    /// never from the environment, which a program running as the owner
    /// can change for Chrome and so for the host it starts.
    ///
    /// Program Files is a literal path in the registry. ProgramData is
    /// `%SystemDrive%\ProgramData` there, and Windows expands it from the
    /// caller's environment: only the audit log is under it, which on
    /// Windows the owner can write anyway, while everything the bridge's
    /// checks rest on (install.json, the binary, the extension) is under
    /// Program Files. A protected install refuses any of its folders that
    /// is there already and is not owned by the system.
    ///
    /// # Errors
    /// When Windows cannot say.
    #[cfg(windows)]
    pub fn of_this_system() -> io::Result<Self> {
        use jev_pilot_os::{KnownFolder, known_folder};
        let text = |f| known_folder(f).map(|p| p.to_string_lossy().into_owned());
        Ok(Self {
            program_files: text(KnownFolder::ProgramFiles)?,
            program_data: text(KnownFolder::ProgramData)?,
        })
    }

    /// Not used on this system.
    #[cfg(not(windows))]
    #[must_use]
    pub fn of_this_system() -> Self {
        Self {
            program_files: String::new(),
            program_data: String::new(),
        }
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
    /// The host manifest Chrome reads, where the Windows registry points;
    /// on macOS and Linux a copy also goes in each browser's folder.
    pub host_manifest: PathBuf,
}

impl Layout {
    /// A protected install's layout: every file where only an administrator
    /// can write.
    #[must_use]
    pub fn protected(os: Os, folders: &SystemFolders) -> Self {
        let manifest = manifest_name();
        match os {
            Os::MacOs => {
                let support = "/Library/Application Support/jev-pilot";
                Self {
                    binary: join(os, "/usr/local/libexec/jev-pilot", &["jev-pilot"]),
                    install_json: join(os, support, &["install.json"]),
                    extension: join(os, support, &["extension"]),
                    audit_log: join(os, "/Library/Logs/jev-pilot", &[]),
                    host_manifest: join(os, support, &[&manifest]),
                }
            }
            Os::Linux => Self {
                binary: join(os, "/usr/local/libexec/jev-pilot", &["jev-pilot"]),
                install_json: join(os, "/etc/jev-pilot", &["install.json"]),
                extension: join(os, "/usr/local/share/jev-pilot", &["extension"]),
                audit_log: join(os, "/var/log/jev-pilot", &[]),
                host_manifest: join(os, "/etc/jev-pilot", &[&manifest]),
            },
            Os::Windows => {
                let files = &folders.program_files;
                let data = &folders.program_data;
                Self {
                    binary: join(os, files, &["jev-pilot", binary_name(os)]),
                    install_json: join(os, files, &["jev-pilot", "install.json"]),
                    extension: join(os, files, &["jev-pilot", "extension"]),
                    audit_log: join(os, data, &["jev-pilot", "audit"]),
                    host_manifest: join(os, files, &["jev-pilot", &manifest]),
                }
            }
        }
    }

    /// A development install's layout: everything in one folder of the
    /// owner's.
    #[must_use]
    pub fn development(os: Os, home: &Path, env: &dyn Fn(&str) -> Option<String>) -> Self {
        let home = home.to_string_lossy();
        let root = match os {
            Os::MacOs => join(
                os,
                &home,
                &["Library", "Application Support", "jev-pilot", "dev"],
            ),
            Os::Linux => join(os, &home, &[".local", "share", "jev-pilot", "dev"]),
            Os::Windows => {
                let local = env("LOCALAPPDATA").unwrap_or_else(|| {
                    join(os, &home, &["AppData", "Local"])
                        .to_string_lossy()
                        .into()
                });
                join(os, &local, &["jev-pilot", "dev"])
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

/// Whether an install is protected or a development one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallKind {
    /// Admin-owned and pinned.
    Protected,
    /// The owner's own, not protected against other programs running as
    /// the owner.
    Development,
}

/// `install.json`: what both ends of the bridge expect of the other, and
/// what the install changed that uninstalling puts back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RecordFile", into = "RecordFile")]
pub struct InstallRecord {
    /// The installed jev-pilot's version.
    pub version: String,
    /// Protected or development.
    pub kind: InstallKind,
    /// What each end of the bridge expects of the other.
    pub expected: Expected,
    /// The user-level host manifests copied to system level, to be removed
    /// again on uninstall.
    pub moved_hosts: Vec<PathBuf>,
}

/// `install.json` as written: with the extension ID, which must be
/// jev-pilot's.
#[derive(Serialize, Deserialize)]
struct RecordFile {
    version: String,
    install: InstallKind,
    extension_id: String,
    expected: Expected,
    #[serde(default)]
    moved_hosts: Vec<PathBuf>,
}

impl From<InstallRecord> for RecordFile {
    fn from(record: InstallRecord) -> Self {
        Self {
            version: record.version,
            install: record.kind,
            extension_id: EXTENSION_ID.to_owned(),
            expected: record.expected,
            moved_hosts: record.moved_hosts,
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
            kind: file.install,
            expected: file.expected,
            moved_hosts: file.moved_hosts,
        })
    }
}

/// `policy.json` for an install of `kind`: the existing policy's sites and
/// tab rule kept, or the defaults (every site, tabs in the group), with the
/// kind of install written for the extension's popup.
#[must_use]
pub fn policy_for_install(existing: Option<Value>, kind: InstallKind) -> Value {
    let mut policy = match existing {
        Some(Value::Object(policy)) => Value::Object(policy),
        _ => json!({"sites": [], "owner_tabs": OwnerTabs::Group}),
    };
    policy["install"] = serde_json::to_value(kind).unwrap_or_default();
    policy
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

/// One Chromium-family browser, by where it looks for hosts and policy.
///
/// From Chromium's `chrome_paths*.cc`, `launch_context_*.cc` and policy
/// loaders, Brave's `brave_main_delegate.cc`, and Microsoft's Edge
/// documentation. Chrome's Beta, Dev and Canary share Stable's system folder,
/// registry key and macOS preference domain; Brave reads Chrome's host
/// folders on macOS and Linux, and its own policy.
struct Family {
    /// Its system host folder on macOS.
    macos_hosts: &'static str,
    /// Its system host folder on Linux.
    linux_hosts: &'static str,
    /// Its user data folders under `~/Library/Application Support`, one per
    /// channel.
    macos_user: &'static [&'static str],
    /// Its user data folders under `~/.config`, one per channel.
    linux_user: &'static [&'static str],
    /// Its key under HKLM or HKCU for hosts, when it has its own.
    windows_hosts: Option<&'static str>,
    /// Its machine policy folder on Linux.
    linux_policy: &'static str,
    /// Its machine policy key on Windows.
    windows_policy: &'static str,
    /// The preference domain a macOS configuration profile sets its policy
    /// in.
    macos_domain: &'static str,
    /// Whether the host serves it: its check of the browser that started it
    /// accepts Google Chrome alone, on every system.
    served: bool,
}

/// The families the host serves, and so those the install registers for,
/// sets the policy for, and moves the owner's hosts of.
fn served() -> impl Iterator<Item = &'static Family> {
    FAMILIES.iter().filter(|family| family.served)
}

const CHROME_MAC_HOSTS: &str = "/Library/Google/Chrome/NativeMessagingHosts";
const CHROME_LINUX_HOSTS: &str = "/etc/opt/chrome/native-messaging-hosts";

static FAMILIES: &[Family] = &[
    Family {
        macos_hosts: CHROME_MAC_HOSTS,
        linux_hosts: CHROME_LINUX_HOSTS,
        macos_user: &[
            "Google/Chrome",
            "Google/Chrome Beta",
            "Google/Chrome Dev",
            "Google/Chrome Canary",
        ],
        linux_user: &[
            "google-chrome",
            "google-chrome-beta",
            "google-chrome-unstable",
            "google-chrome-canary",
        ],
        windows_hosts: Some(r"SOFTWARE\Google\Chrome\NativeMessagingHosts"),
        linux_policy: "/etc/opt/chrome/policies/managed",
        windows_policy: r"SOFTWARE\Policies\Google\Chrome",
        macos_domain: "com.google.Chrome",
        served: true,
    },
    Family {
        macos_hosts: "/Library/Application Support/Chromium/NativeMessagingHosts",
        linux_hosts: "/etc/chromium/native-messaging-hosts",
        macos_user: &["Chromium"],
        linux_user: &["chromium"],
        windows_hosts: Some(r"SOFTWARE\Chromium\NativeMessagingHosts"),
        linux_policy: "/etc/chromium/policies/managed",
        windows_policy: r"SOFTWARE\Policies\Chromium",
        macos_domain: "org.chromium.Chromium",
        served: false,
    },
    Family {
        macos_hosts: "/Library/Microsoft/Edge/NativeMessagingHosts",
        linux_hosts: "/etc/opt/edge/native-messaging-hosts",
        macos_user: &[
            "Microsoft Edge",
            "Microsoft Edge Beta",
            "Microsoft Edge Dev",
            "Microsoft Edge Canary",
        ],
        linux_user: &["microsoft-edge"],
        windows_hosts: Some(r"SOFTWARE\Microsoft\Edge\NativeMessagingHosts"),
        linux_policy: "/etc/opt/edge/policies/managed",
        windows_policy: r"SOFTWARE\Policies\Microsoft\Edge",
        macos_domain: "com.microsoft.Edge",
        served: false,
    },
    Family {
        macos_hosts: CHROME_MAC_HOSTS,
        linux_hosts: CHROME_LINUX_HOSTS,
        // On macOS Brave reads Chrome's user folder, listed above.
        macos_user: &[],
        linux_user: &["BraveSoftware/Brave-Browser"],
        // On Windows Brave reads Chromium's key, then Chrome's.
        windows_hosts: None,
        linux_policy: "/etc/brave/policies/managed",
        windows_policy: r"SOFTWARE\Policies\BraveSoftware\Brave",
        macos_domain: "com.brave.Browser",
        served: false,
    },
];

/// The system host folders on macOS or Linux, each once: where a protected
/// install puts a copy of the host manifest. Windows uses the registry.
#[must_use]
pub fn system_host_dirs(os: Os) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for family in served() {
        let dir = PathBuf::from(match os {
            Os::MacOs => family.macos_hosts,
            Os::Linux => family.linux_hosts,
            Os::Windows => return Vec::new(),
        });
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs
}

/// Each user-level host folder on macOS or Linux under `home`, with the
/// system folder the same browser reads: where a development install
/// registers, and where a protected install looks for hosts to copy before
/// turning user-level hosts off.
#[must_use]
pub fn user_host_dirs(os: Os, home: &Path) -> Vec<(PathBuf, PathBuf)> {
    let home = home.to_string_lossy();
    let mut dirs = Vec::new();
    for family in served() {
        let (base, users, system) = match os {
            Os::MacOs => (
                join(os, &home, &["Library", "Application Support"]),
                family.macos_user,
                family.macos_hosts,
            ),
            Os::Linux => (
                join(os, &home, &[".config"]),
                family.linux_user,
                family.linux_hosts,
            ),
            Os::Windows => return Vec::new(),
        };
        for user in users {
            let base = base.to_string_lossy();
            dirs.push((
                join(os, &base, &[user, "NativeMessagingHosts"]),
                PathBuf::from(system),
            ));
        }
    }
    dirs
}

/// A user-level host manifest copied to system level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostCopy {
    /// The user-level manifest.
    pub from: PathBuf,
    /// Where its copy goes.
    pub to: PathBuf,
}

/// Which of the user-level host manifests `found`, each with the system
/// folder its browser reads, to copy there: every JSON manifest but
/// jev-pilot's own, unless the system folder already has one of that name.
#[must_use]
pub fn host_copies(found: &[(PathBuf, PathBuf)], exists: &dyn Fn(&Path) -> bool) -> Vec<HostCopy> {
    let ours = manifest_name();
    found
        .iter()
        .filter_map(|(from, system)| {
            let name = from.file_name()?.to_str()?;
            // Chrome reads `<host name>.json`, exactly.
            if name.eq_ignore_ascii_case(&ours) || from.extension().is_none_or(|e| e != "json") {
                return None;
            }
            let to = system.join(name);
            (!exists(&to)).then(|| HostCopy {
                from: from.clone(),
                to,
            })
        })
        .collect()
}

/// Which of the owner's hosts registered under HKCU to register under HKLM
/// too: not jev-pilot's own, whatever its case, and not one the machine
/// already has under that name, which is a vendor's own.
#[must_use]
pub fn registry_hosts_to_copy(
    found: Vec<RegistryHost>,
    at_machine_level: &dyn Fn(&RegistryHost) -> bool,
) -> Vec<RegistryHost> {
    found
        .into_iter()
        .filter(|host| !host.name.eq_ignore_ascii_case(HOST_NAME) && !at_machine_level(host))
        .collect()
}

/// The largest host manifest an install copies.
#[cfg(unix)]
const HOST_MANIFEST_LIMIT: u64 = 64 << 10;

/// An owner's host manifest, to be copied to system level: parsed as one,
/// named as its file (`stem`), with an absolute `path`, and written out
/// again with only a manifest's fields rather than copied byte for byte.
///
/// # Errors
/// When it is not such a manifest.
pub fn manifest_to_copy(text: &str, stem: &str) -> Result<String, String> {
    let manifest: Value =
        serde_json::from_str(text).map_err(|e| format!("not a host manifest: {e}"))?;
    let object = manifest.as_object().ok_or("not a host manifest")?;
    if object.get("name").and_then(Value::as_str) != Some(stem) {
        return Err(format!("its name is not {stem}"));
    }
    let path = object
        .get("path")
        .and_then(Value::as_str)
        .ok_or("it names no program")?;
    if !(Path::new(path).is_absolute() || path.starts_with('/')) {
        return Err(format!("its program {path} is not an absolute path"));
    }
    // Only what a host manifest has is written again.
    let kept: serde_json::Map<String, Value> = object
        .iter()
        .filter(|(key, _)| {
            matches!(
                key.as_str(),
                "name" | "description" | "path" | "type" | "allowed_origins"
            )
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    serde_json::to_string_pretty(&Value::Object(kept)).map_err(|e| e.to_string())
}

/// An owner's host file, read without following a link, when it is a
/// regular file of `uid`'s no larger than a manifest is.
///
/// # Errors
/// When it is a link, not a regular file, another user's, too large, or
/// cannot be read.
#[cfg(unix)]
pub fn read_owners_host(path: &Path, uid: u32) -> io::Result<String> {
    use std::io::Read as _;
    use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
    let mut file = OpenOptions::new()
        .read(true)
        // Without following a link, and without waiting on a pipe.
        .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.uid() != uid || meta.len() > HOST_MANIFEST_LIMIT {
        return Err(io::Error::other(format!(
            "{} is not a host manifest of yours",
            path.display()
        )));
    }
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    Ok(text)
}

/// The registry keys, under HKLM or HKCU, that hold host registrations on
/// Windows. Chrome reads its own; Chromium and Brave read Chromium's, then
/// Chrome's; Edge reads its own, then Chromium's, then Chrome's.
#[must_use]
pub fn windows_host_keys() -> Vec<&'static str> {
    served().filter_map(|f| f.windows_hosts).collect()
}

/// The machine policy keys on Windows, one per family.
#[must_use]
pub fn windows_policy_keys() -> Vec<&'static str> {
    served().map(|f| f.windows_policy).collect()
}

/// `reg.exe` arguments registering the host under `hive` (`HKLM`, or
/// `HKCU` for a development install) and `key`: the key's default value
/// names the manifest, in the 64-bit view.
#[must_use]
pub fn reg_add_host(hive: &str, key: &str, manifest: &Path) -> Vec<String> {
    vec![
        "add".into(),
        format!(r"{hive}\{key}\{HOST_NAME}"),
        "/ve".into(),
        "/t".into(),
        "REG_SZ".into(),
        "/d".into(),
        manifest.to_string_lossy().into_owned(),
        "/f".into(),
        "/reg:64".into(),
    ]
}

/// `reg.exe` arguments removing the host's registration.
#[must_use]
pub fn reg_delete_host(hive: &str, key: &str) -> Vec<String> {
    vec![
        "delete".into(),
        format!(r"{hive}\{key}\{HOST_NAME}"),
        "/f".into(),
        "/reg:64".into(),
    ]
}

/// The policy that turns user-level hosts off.
const USER_LEVEL_HOSTS: &str = "NativeMessagingUserLevelHosts";

/// `reg.exe` arguments setting the machine policy that turns user-level
/// hosts off.
#[must_use]
pub fn reg_add_policy(key: &str) -> Vec<String> {
    vec![
        "add".into(),
        format!(r"HKLM\{key}"),
        "/v".into(),
        USER_LEVEL_HOSTS.into(),
        "/t".into(),
        "REG_DWORD".into(),
        "/d".into(),
        "0".into(),
        "/f".into(),
        "/reg:64".into(),
    ]
}

/// `reg.exe` arguments removing that policy.
#[must_use]
pub fn reg_delete_policy(key: &str) -> Vec<String> {
    vec![
        "delete".into(),
        format!(r"HKLM\{key}"),
        "/v".into(),
        USER_LEVEL_HOSTS.into(),
        "/f".into(),
        "/reg:64".into(),
    ]
}

/// The hosts other than jev-pilot's, by name and manifest, in the output of
/// `reg query <key> /s`: each subkey's default value.
#[must_use]
pub fn reg_hosts_in(output: &str) -> Vec<(String, PathBuf)> {
    let mut hosts = Vec::new();
    let mut name: Option<String> = None;
    for line in output.lines() {
        let line = line.trim_end();
        if line.starts_with("HKEY_") {
            name = line.rsplit('\\').next().map(str::to_owned);
            continue;
        }
        let mut fields = line.split_whitespace();
        if fields.next() != Some("(Default)") || fields.next() != Some("REG_SZ") {
            continue;
        }
        let Some(value) = line.split("REG_SZ").nth(1).map(str::trim) else {
            continue;
        };
        if let Some(host) = name.take().filter(|n| !n.eq_ignore_ascii_case(HOST_NAME)) {
            hosts.push((host, PathBuf::from(value)));
        }
    }
    hosts
}

/// The policy files that turn user-level hosts off on Linux, one per
/// family.
#[must_use]
pub fn linux_policy_files() -> Vec<PathBuf> {
    served()
        .map(|f| join(Os::Linux, f.linux_policy, &["jev-pilot.json"]))
        .collect()
}

/// Their content.
#[must_use]
pub fn linux_policy() -> String {
    serde_json::to_string_pretty(&json!({ USER_LEVEL_HOSTS: false })).unwrap_or_default()
}

/// The value `name` has in the output of `reg query <key> /v <name>`, as a
/// DWORD.
#[must_use]
pub fn reg_dword_in(output: &str, name: &str) -> Option<u32> {
    output.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        if fields.next()? != name || fields.next()? != "REG_DWORD" {
            return None;
        }
        u32::from_str_radix(fields.next()?.trim_start_matches("0x"), 16).ok()
    })
}

/// Whether a Linux policy file turns user-level hosts off.
#[must_use]
pub fn linux_policy_holds(text: &str) -> bool {
    serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|policy| policy.get(USER_LEVEL_HOSTS).and_then(Value::as_bool))
        == Some(false)
}

/// Where macOS puts each browser's managed preferences once the owner has
/// approved the profile.
#[must_use]
pub fn managed_preferences() -> Vec<PathBuf> {
    served()
        .map(|f| {
            PathBuf::from(format!(
                "/Library/Managed Preferences/{}.plist",
                f.macos_domain
            ))
        })
        .collect()
}

/// The macOS configuration profile that turns user-level hosts off: one
/// payload per family's preference domain. macOS honours the policy only as
/// a mandatory one, which on an unmanaged Mac a profile the owner approves
/// is. Its UUIDs are fixed, so installing again changes nothing.
#[must_use]
pub fn mobileconfig() -> String {
    const UUIDS: [&str; 4] = [
        "3F71D10A-ED98-4638-A5B4-139CF37FF09D",
        "24CF9B19-A734-46E4-8755-F37B275E934C",
        "0EB33AD2-C913-40BE-BE1A-AFED33441B47",
        "BBC10C2D-1632-481E-97F4-1614464FDF9A",
    ];
    let mut payloads = String::new();
    for (family, uuid) in served().zip(UUIDS) {
        let domain = family.macos_domain;
        let _ = write!(
            payloads,
            "\t\t<dict>\n\
             \t\t\t<key>{USER_LEVEL_HOSTS}</key>\n\
             \t\t\t<false/>\n\
             \t\t\t<key>PayloadDisplayName</key>\n\
             \t\t\t<string>{domain}</string>\n\
             \t\t\t<key>PayloadIdentifier</key>\n\
             \t\t\t<string>dev.jevpilot.policy.{domain}</string>\n\
             \t\t\t<key>PayloadType</key>\n\
             \t\t\t<string>{domain}</string>\n\
             \t\t\t<key>PayloadUUID</key>\n\
             \t\t\t<string>{uuid}</string>\n\
             \t\t\t<key>PayloadVersion</key>\n\
             \t\t\t<integer>1</integer>\n\
             \t\t</dict>\n"
        );
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \t<key>PayloadContent</key>\n\
         \t<array>\n\
         {payloads}\
         \t</array>\n\
         \t<key>PayloadDescription</key>\n\
         \t<string>Chrome, Chromium, Edge and Brave start only native messaging hosts an \
         administrator installed, so no program of yours can stand in for jev-pilot's.</string>\n\
         \t<key>PayloadDisplayName</key>\n\
         \t<string>jev-pilot: system-level native messaging hosts only</string>\n\
         \t<key>PayloadIdentifier</key>\n\
         \t<string>dev.jevpilot.policy</string>\n\
         \t<key>PayloadScope</key>\n\
         \t<string>System</string>\n\
         \t<key>PayloadType</key>\n\
         \t<string>Configuration</string>\n\
         \t<key>PayloadUUID</key>\n\
         \t<string>D1A38202-0F25-4399-A684-C8DADB5C05E6</string>\n\
         \t<key>PayloadVersion</key>\n\
         \t<integer>1</integer>\n\
         </dict>\n\
         </plist>\n"
    )
}

/// Who owns what an install writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    /// root (on Windows, the administrators who own Program Files).
    Root,
    /// root, with the `jev-pilot` group.
    RootGroup,
    /// The owner, for a development install.
    User,
}

/// One thing an install does.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Step {
    /// Create the `jev-pilot` group if it is missing.
    EnsureGroup,
    /// Create a directory, with its parents.
    CreateDir {
        /// The directory.
        path: PathBuf,
        /// Its permission bits.
        mode: u32,
        /// Its owner.
        owner: Owner,
    },
    /// Create a file if it is missing, keeping what an existing one holds,
    /// and give it its owner and permission bits.
    CreateFile {
        /// The file.
        path: PathBuf,
        /// Its permission bits.
        mode: u32,
        /// Its owner.
        owner: Owner,
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
    /// Give a file its owner and permission bits.
    SetOwner {
        /// The file.
        path: PathBuf,
        /// Its owner.
        owner: Owner,
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
        /// Its owner.
        owner: Owner,
    },
    /// Write `policy.json`, keeping an existing one's sites and tab rule.
    WritePolicy {
        /// The file.
        path: PathBuf,
        /// What kind of install says so.
        kind: InstallKind,
    },
    /// Copy a user-level host manifest to system level.
    CopyHost(HostCopy),
    /// Run `reg.exe` with these arguments (Windows).
    Reg(Vec<String>),
    /// Open the configuration profile for the owner to approve (macOS).
    OpenProfile {
        /// The profile.
        path: PathBuf,
    },
    /// Remove a file, or a directory and all it holds.
    Remove {
        /// What.
        path: PathBuf,
    },
    /// Tell the owner something to do by hand.
    Tell(String),
    /// Pin the installed binary and write `install.json`.
    Record {
        /// `install.json`.
        path: PathBuf,
        /// What kind of install.
        kind: InstallKind,
        /// The installed binary, to pin.
        binary: PathBuf,
        /// The user-level hosts copied to system level (or registered
        /// under HKLM), which uninstalling removes again.
        moved_hosts: Vec<PathBuf>,
    },
}

/// The owner's user-level hosts, as an install deals with them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserHosts {
    /// macOS and Linux, protected: the manifests to copy to system level.
    Files(Vec<HostCopy>),
    /// Windows, protected: the hosts registered under HKCU, to register
    /// under HKLM too.
    Registry(Vec<RegistryHost>),
    /// A development install: the user-level folders to register in.
    Register(Vec<PathBuf>),
}

/// A host registered in the Windows registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryHost {
    /// The key it is under, below the hive.
    pub key: String,
    /// Its name.
    pub name: String,
    /// Its manifest.
    pub manifest: PathBuf,
}

/// What an install finds already there.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Existing {
    /// The owner's hosts an earlier install copied to system level, from
    /// its install.json: kept in the new one, so uninstalling still removes
    /// them.
    pub moved_hosts: Vec<PathBuf>,
    /// The owner's user-level host folders, where an earlier development
    /// install may have registered.
    pub user_host_dirs: Vec<PathBuf>,
}

/// `rel`, a path with `/` between its parts, under `base`, with `os`'s
/// separator.
fn under(os: Os, base: &Path, rel: &str) -> PathBuf {
    let parts: Vec<&str> = rel.split('/').collect();
    join(os, &base.to_string_lossy(), &parts)
}

/// Everything an install of `kind` does on `os`, in order, copying the
/// running binary `exe` into `layout`.
#[must_use]
pub fn plan(
    os: Os,
    kind: InstallKind,
    layout: &Layout,
    exe: &Path,
    user_hosts: &UserHosts,
    existing: &Existing,
) -> Vec<Step> {
    let protected = kind == InstallKind::Protected;
    let mut planner = Planner {
        os,
        protected,
        owner: if protected { Owner::Root } else { Owner::User },
        steps: Vec::new(),
    };
    planner.binary(layout, exe);
    planner.extension(layout, kind);
    let mut moved_hosts = Vec::new();
    if protected {
        // Before jev-pilot's own manifest is written, so that it is the
        // last word in every folder; and every move kept, earlier ones too.
        planner.unregister_development(&existing.user_host_dirs);
        moved_hosts.clone_from(&existing.moved_hosts);
        for moved in planner.moved_hosts(user_hosts) {
            if !moved_hosts.contains(&moved) {
                moved_hosts.push(moved);
            }
        }
    }
    planner.registration(layout, user_hosts);
    if protected {
        planner.policy(layout);
    }
    let (mode, owner) = if protected {
        (0o775, Owner::RootGroup)
    } else {
        (0o700, Owner::User)
    };
    planner.steps.push(Step::CreateDir {
        path: layout.audit_log.clone(),
        mode,
        owner,
    });
    // The host appends to these and never makes them: made here, they are
    // root's, so the owner can neither make nor rewrite them.
    for name in ["audit.log", "audit.log.1", "audit.log.2", "audit.log.3"] {
        planner.steps.push(Step::CreateFile {
            path: under(os, &layout.audit_log, name),
            mode: if protected { 0o664 } else { 0o600 },
            owner,
        });
    }
    planner.steps.push(Step::Record {
        path: layout.install_json.clone(),
        kind,
        binary: layout.binary.clone(),
        moved_hosts,
    });
    planner.steps
}

/// An install's steps, as they are worked out.
struct Planner {
    os: Os,
    protected: bool,
    owner: Owner,
    steps: Vec<Step>,
}

fn parent(path: &Path) -> PathBuf {
    path.parent().map_or_else(PathBuf::new, Path::to_owned)
}

impl Planner {
    fn dir(&mut self, path: &Path) {
        self.steps.push(Step::CreateDir {
            path: path.to_owned(),
            mode: 0o755,
            owner: self.owner,
        });
    }

    fn file(&mut self, path: PathBuf, contents: &[u8]) {
        self.steps.push(Step::WriteFile {
            path,
            contents: contents.to_vec(),
            mode: 0o644,
            owner: self.owner,
        });
    }

    /// The binary, copied, and on macOS and Linux made setgid.
    fn binary(&mut self, layout: &Layout, exe: &Path) {
        let unix = self.os != Os::Windows;
        if self.protected && unix {
            self.steps.push(Step::EnsureGroup);
        }
        self.dir(&parent(&layout.binary));
        self.steps.push(Step::CopyBinary {
            from: exe.to_owned(),
            to: layout.binary.clone(),
        });
        if self.protected && self.os == Os::MacOs {
            // Before setgid: re-signing writes the file, which clears the
            // bit.
            self.steps.push(Step::Sign {
                path: layout.binary.clone(),
            });
        }
        // The copy is made closed to everyone; it is opened to run once it
        // is in place (and, protected, signed and given its group).
        let (owner, mode) = match (self.protected, unix) {
            (true, true) => (Owner::RootGroup, 0o2755),
            (true, false) => (Owner::Root, 0o755),
            (false, _) => (Owner::User, 0o755),
        };
        self.steps.push(Step::SetOwner {
            path: layout.binary.clone(),
            owner,
            mode,
        });
    }

    /// The extension's files, and its policy.
    fn extension(&mut self, layout: &Layout, kind: InstallKind) {
        self.dir(&parent(&layout.install_json));
        self.dir(&layout.extension);
        let mut made: Vec<PathBuf> = Vec::new();
        for (name, contents) in crate::browser::packaged::FILES {
            let path = under(self.os, &layout.extension, name);
            let folder = parent(&path);
            if folder != layout.extension && !made.contains(&folder) {
                self.dir(&folder);
                made.push(folder);
            }
            if *name != "policy.json" {
                self.file(path, contents);
            }
        }
        self.steps.push(Step::WritePolicy {
            path: under(self.os, &layout.extension, "policy.json"),
            kind,
        });
    }

    /// The host manifest, and where each browser finds it.
    fn registration(&mut self, layout: &Layout, user_hosts: &UserHosts) {
        let manifest = host_manifest(&layout.binary);
        self.dir(&parent(&layout.host_manifest));
        self.file(layout.host_manifest.clone(), manifest.as_bytes());
        let folders = match (self.os, user_hosts) {
            (Os::Windows, _) => {
                let hive = if self.protected { "HKLM" } else { "HKCU" };
                for key in windows_host_keys() {
                    self.steps
                        .push(Step::Reg(reg_add_host(hive, key, &layout.host_manifest)));
                }
                return;
            }
            (_, UserHosts::Register(dirs)) if !self.protected => {
                if dirs.is_empty() {
                    self.steps.push(Step::Tell(
                        "no Google Chrome profile was found in your home, so the host is \
                         registered with none: run the install again once Chrome has started"
                            .into(),
                    ));
                }
                dirs.clone()
            }
            _ => system_host_dirs(self.os),
        };
        for folder in folders {
            self.dir(&folder);
            self.file(folder.join(manifest_name()), manifest.as_bytes());
        }
    }

    /// The owner's user-level hosts, copied to system level so they keep
    /// working once user-level hosts are turned off; what was copied.
    fn moved_hosts(&mut self, user_hosts: &UserHosts) -> Vec<PathBuf> {
        let mut moved = Vec::new();
        match user_hosts {
            UserHosts::Files(copies) => {
                for copy in copies {
                    self.steps.push(Step::CopyHost(copy.clone()));
                    moved.push(copy.to.clone());
                }
            }
            UserHosts::Registry(hosts) => {
                for RegistryHost {
                    key,
                    name,
                    manifest,
                } in hosts
                {
                    let target = format!(r"HKLM\{key}\{name}");
                    self.steps.push(Step::Reg(vec![
                        "add".into(),
                        target.clone(),
                        "/ve".into(),
                        "/t".into(),
                        "REG_SZ".into(),
                        "/d".into(),
                        manifest.to_string_lossy().into_owned(),
                        "/f".into(),
                        "/reg:64".into(),
                    ]));
                    moved.push(PathBuf::from(target));
                }
            }
            UserHosts::Register(_) => {}
        }
        moved
    }

    /// A development install's user-level registrations, which Chrome
    /// would prefer to the system's until the policy is in place.
    fn unregister_development(&mut self, user_host_dirs: &[PathBuf]) {
        if self.os == Os::Windows {
            for key in windows_host_keys() {
                self.steps.push(Step::Reg(reg_delete_host("HKCU", key)));
            }
        }
        for folder in user_host_dirs {
            self.steps.push(Step::Remove {
                path: folder.join(manifest_name()),
            });
        }
    }

    /// Chrome's policy, turning user-level hosts off.
    fn policy(&mut self, layout: &Layout) {
        match self.os {
            Os::Linux => {
                for policy in linux_policy_files() {
                    self.dir(&parent(&policy));
                    self.file(policy, linux_policy().as_bytes());
                }
            }
            Os::Windows => {
                for key in windows_policy_keys() {
                    self.steps.push(Step::Reg(reg_add_policy(key)));
                }
            }
            Os::MacOs => {
                let profile = parent(&layout.install_json).join("jev-pilot.mobileconfig");
                self.file(profile.clone(), mobileconfig().as_bytes());
                self.steps.push(Step::OpenProfile { path: profile });
            }
        }
    }
}

/// The cdhash in `codesign -d -vvv`'s report, as a pin.
#[must_use]
pub fn cdhash_in(report: &str) -> Option<crate::peer::MacosPin> {
    report
        .lines()
        .find_map(|line| line.strip_prefix("CDHash="))
        .and_then(|hex| crate::peer::MacosPin::from_cdhash_hex(hex.trim()).ok())
}

/// Whether an elevated install runs as the account signed in, so that what
/// it pins and the hosts it finds are that account's, not another
/// administrator's who elevated it over the owner's shoulder.
///
/// # Errors
/// What to say when it does not.
pub fn elevated_as_signed_in(elevated: &str, signed_in: &str) -> Result<(), String> {
    if elevated.eq_ignore_ascii_case(signed_in) {
        Ok(())
    } else {
        Err(format!(
            "this runs elevated as {elevated}, not as {signed_in} who is signed in: sign in as an \
             administrator yourself and run the install from there, so the bridge is pinned to you"
        ))
    }
}

/// Whether this elevated install runs as the account signed in to the
/// session (Windows).
///
/// # Errors
/// What to say when it does not, or why it cannot be told.
#[cfg(windows)]
pub fn check_elevated_account() -> Result<(), String> {
    let elevated = jev_pilot_os::current_account().map_err(|e| e.to_string())?;
    let signed_in = jev_pilot_os::session_account().map_err(|e| e.to_string())?;
    elevated_as_signed_in(&elevated, &signed_in)
}

/// The `jev-pilot` group's name.
pub const GROUP: &str = "jev-pilot";

/// The variable the MCP server requires before it serves a development
/// install: `JEV_PILOT_ALLOW_DEV_BRIDGE=1`.
pub const ALLOW_DEV_BRIDGE: &str = "JEV_PILOT_ALLOW_DEV_BRIDGE";

/// What a development install says wherever it is shown.
pub const UNPROTECTED: &str =
    "development install: not protected against other programs running as you";

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
pub fn apply(steps: &[Step], installer: &Installer, kind: InstallKind) -> io::Result<Vec<String>> {
    let mut said = Vec::new();
    let groups = if kind == InstallKind::Protected {
        groups_of(installer)?
    } else {
        Vec::new()
    };
    let member_of = |gid: u32| groups.contains(&gid);
    for step in steps {
        if kind == InstallKind::Protected
            && let Some(target) = written(step)
        {
            check_ancestors(target, &dir_meta, &member_of).map_err(io::Error::other)?;
        }
        #[cfg(windows)]
        if kind == InstallKind::Protected
            && let Step::CreateDir { path, .. } = step
        {
            check_windows_folder(path)?;
        }
        apply_one(step, installer, &mut said)
            .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", describe(step))))?;
    }
    Ok(said)
}

/// The groups the owner is in, as `id -G` reports them.
fn groups_of(installer: &Installer) -> io::Result<Vec<u32>> {
    let Installer::Uid(uid) = installer else {
        return Ok(Vec::new());
    };
    let output = std::process::Command::new(tool_here(Tool::Id)?)
        .arg("-G")
        .arg(uid.to_string())
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "cannot tell which groups uid {uid} is in"
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .filter_map(|g| g.parse().ok())
        .collect())
}

/// Whether a Windows folder's owner, by SID, is one only an administrator
/// acts as: Administrators, SYSTEM or TrustedInstaller.
#[must_use]
pub fn trusted_owner(sid: &str) -> bool {
    matches!(
        sid,
        "S-1-5-32-544"
            | "S-1-5-18"
            | "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464"
    )
}

/// On Windows, whether a folder of a protected install that is there
/// already is owned by the system: one the owner made beforehand, in
/// ProgramData for instance, would be the owner's to change.
#[cfg(windows)]
fn check_windows_folder(path: &Path) -> io::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let owner = jev_pilot_os::owner_sid(path)?;
    if trusted_owner(&owner) {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "{} is owned by {owner}, not an administrator, so a program running as you could \
             change what jev-pilot installs in it: remove it and run the install again",
            path.display()
        )))
    }
}

/// What a step writes, whose folders must be safe.
fn written(step: &Step) -> Option<&Path> {
    match step {
        Step::CreateDir { path, .. }
        | Step::CreateFile { path, .. }
        | Step::WriteFile { path, .. }
        | Step::WritePolicy { path, .. }
        | Step::Record { path, .. }
        | Step::CopyBinary { to: path, .. }
        | Step::CopyHost(HostCopy { to: path, .. }) => Some(path),
        _ => None,
    }
}

/// What the OS says of a folder, for [`check_ancestors`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirMeta {
    /// Its owner's uid.
    pub uid: u32,
    /// Its group's gid.
    pub gid: u32,
    /// Its permission bits.
    pub mode: u32,
    /// Whether it is a symbolic link.
    pub is_link: bool,
}

/// Whether every folder above `path` that exists is one the owner cannot
/// change: root's, not a link, not writable by anyone, and writable by its
/// group only when that group is root's or one the owner is not in
/// (`member_of` says which groups the owner is in). Debian's `/usr/local`
/// (root:staff 2775), Ubuntu's `/var/log` (root:syslog 0775) and the
/// install's own audit folder (root:jev-pilot 0775) pass for an owner not
/// in those groups. Folders that do not exist yet are made by the install,
/// as root.
///
/// # Errors
/// The first folder that is not, and why.
pub fn check_ancestors(
    path: &Path,
    meta: &dyn Fn(&Path) -> Option<DirMeta>,
    member_of: &dyn Fn(u32) -> bool,
) -> Result<(), String> {
    let mut above: Vec<&Path> = path.ancestors().skip(1).collect();
    above.reverse();
    for dir in above {
        let Some(found) = meta(dir) else {
            continue;
        };
        let mode = found.mode & 0o777;
        let why = if found.is_link {
            "is a link".to_owned()
        } else if found.uid != 0 {
            format!("is owned by uid {}", found.uid)
        } else if found.mode & 0o002 != 0 {
            format!("can be written by anyone (mode {mode:o})")
        } else if found.mode & 0o020 != 0 && found.gid != 0 && member_of(found.gid) {
            format!(
                "can be written by group {}, which you are in (mode {mode:o})",
                found.gid
            )
        } else {
            continue;
        };
        return Err(format!(
            "{} {why}, so a program running as you could change what jev-pilot installs under it",
            dir.display()
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn dir_meta(dir: &Path) -> Option<DirMeta> {
    use std::os::unix::fs::MetadataExt as _;
    let meta = std::fs::symlink_metadata(dir).ok()?;
    Some(DirMeta {
        uid: meta.uid(),
        gid: meta.gid(),
        mode: meta.mode(),
        is_link: meta.file_type().is_symlink(),
    })
}

#[cfg(not(unix))]
#[allow(clippy::unnecessary_wraps, reason = "the same signature as on Unix")]
fn dir_meta(_: &Path) -> Option<DirMeta> {
    // Program Files and ProgramData give their own access to what is made
    // in them.
    None
}

/// Copy `from` into `to`: into a new file beside it that must not exist
/// yet, made by this process and closed to everyone else, then renamed over
/// `to`. A running copy is replaced, not rewritten, and nothing someone
/// else made or still has open is written into.
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
        Step::EnsureGroup => format!("the {GROUP} group"),
        Step::CopyBinary { to, .. } => format!("copying jev-pilot to {}", to.display()),
        Step::Sign { path } => format!("signing {}", path.display()),
        Step::CreateDir { path, .. }
        | Step::CreateFile { path, .. }
        | Step::Remove { path }
        | Step::SetOwner { path, .. }
        | Step::WriteFile { path, .. }
        | Step::WritePolicy { path, .. }
        | Step::Record { path, .. } => path.display().to_string(),
        Step::CopyHost(copy) => copy.to.display().to_string(),
        Step::Reg(args) => format!("reg {}", args.join(" ")),
        Step::OpenProfile { path } => format!("opening {}", path.display()),
        Step::Tell(words) => words.clone(),
    }
}

fn apply_one(step: &Step, installer: &Installer, said: &mut Vec<String>) -> io::Result<()> {
    match step {
        Step::EnsureGroup => ensure_group(said)?,
        Step::CreateDir { path, mode, owner } => {
            std::fs::create_dir_all(path)?;
            set_owner(path, *owner, *mode)?;
        }
        Step::CreateFile { path, mode, owner } => make_file(path, *mode, *owner)?,
        Step::CopyBinary { from, to } => {
            if same_file(from, to) {
                said.push(format!("jev-pilot is already at {}", to.display()));
            } else {
                // A copy left by an install that failed part way: in a
                // folder only root can change, so it is the installer's own.
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
        Step::SetOwner { path, owner, mode } => set_owner(path, *owner, *mode)?,
        Step::WriteFile {
            path,
            contents,
            mode,
            owner,
        } => {
            write_file(path, contents)?;
            set_owner(path, *owner, *mode)?;
        }
        Step::WritePolicy { path, kind } => {
            let existing = std::fs::read(path)
                .ok()
                .and_then(|text| serde_json::from_slice(&text).ok());
            let kept = existing.is_some();
            let policy = policy_for_install(existing, *kind);
            write_file(path, serde_json::to_string_pretty(&policy)?.as_bytes())?;
            set_owner(path, owner_of(*kind), 0o644)?;
            if kept {
                said.push(format!("kept the sites and tab rule in {}", path.display()));
            }
        }
        Step::CopyHost(copy) => {
            copy_host(copy, installer)?;
            said.push(format!(
                "copied your host {} to {}",
                copy.from.display(),
                copy.to.display()
            ));
        }
        Step::Reg(args) => {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            run(Tool::Reg, &args, None)?;
        }
        Step::OpenProfile { path } => {
            let _ = run(Tool::Open, &[], Some(path));
            said.push(format!(
                "opened {}: approve it in System Settings › General › Device Management",
                path.display()
            ));
        }
        Step::Record {
            path,
            kind,
            binary,
            moved_hosts,
        } => record(path, *kind, binary, moved_hosts, installer, said)?,
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

/// Make a file if it is missing, keeping what an existing one holds, and
/// give it its owner and mode: opened without following a link, checked to
/// be a regular file, and changed through what was opened, so nothing
/// planted in its place is changed instead.
///
/// # Errors
/// When `path` is a link or not a regular file, or cannot be opened or
/// changed.
pub fn make_file(path: &Path, mode: u32, owner: Owner) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(nix::libc::O_NOFOLLOW).mode(0o600);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other(format!(
            "{} is not a file",
            path.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        match owner {
            Owner::User => {}
            Owner::Root => std::os::unix::fs::fchown(&file, Some(0), Some(0))?,
            Owner::RootGroup => std::os::unix::fs::fchown(&file, Some(0), Some(group_id()?))?,
        }
        file.set_permissions(std::fs::Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    let _ = (mode, owner);
    Ok(())
}

/// Pin the installed `binary` and write `install.json` at `path`.
fn record(
    path: &Path,
    kind: InstallKind,
    binary: &Path,
    moved_hosts: &[PathBuf],
    installer: &Installer,
    said: &mut Vec<String>,
) -> io::Result<()> {
    let record = InstallRecord {
        version: env!("CARGO_PKG_VERSION").into(),
        kind,
        expected: pin(kind, binary, installer)?,
        moved_hosts: moved_hosts.to_vec(),
    };
    write_file(path, serde_json::to_string_pretty(&record)?.as_bytes())?;
    set_owner(path, owner_of(kind), 0o644)?;
    said.push(format!("pinned {} in {}", binary.display(), path.display()));
    Ok(())
}

/// Copy an owner's host to system level: read safely, parsed as a
/// manifest, written out again to a file that must not exist yet.
#[cfg(unix)]
fn copy_host(copy: &HostCopy, installer: &Installer) -> io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt as _;
    let Installer::Uid(uid) = installer else {
        return Err(io::Error::other("a host copy needs the owner's uid"));
    };
    let text = read_owners_host(&copy.from, *uid)?;
    let stem = copy
        .from
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| io::Error::other("a host file without a name"))?;
    let manifest = manifest_to_copy(&text, stem).map_err(io::Error::other)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&copy.to)?;
    std::io::Write::write_all(&mut file, manifest.as_bytes())?;
    set_owner(&copy.to, Owner::Root, 0o644)
}

#[cfg(not(unix))]
fn copy_host(_: &HostCopy, _: &Installer) -> io::Result<()> {
    Err(io::ErrorKind::Unsupported.into())
}

fn owner_of(kind: InstallKind) -> Owner {
    match kind {
        InstallKind::Protected => Owner::Root,
        InstallKind::Development => Owner::User,
    }
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
    /// macOS's `open`.
    Open,
    /// macOS's directory service group editor.
    Dseditgroup,
    /// Linux's `groupadd`.
    Groupadd,
    /// macOS's property list tool.
    Plutil,
    /// Windows's registry tool.
    Reg,
    /// The Unix tool that names a user's groups.
    Id,
}

/// Where `tool` is: a full path, never found through `PATH`, which the
/// owner sets, nor (on Windows) beside the running binary, where Windows
/// looks first. `system32` is Windows's System32 folder, as the system says.
#[must_use]
pub fn tool_path(tool: Tool, system32: &str) -> PathBuf {
    match tool {
        Tool::Codesign => "/usr/bin/codesign".into(),
        Tool::Open => "/usr/bin/open".into(),
        Tool::Dseditgroup => "/usr/sbin/dseditgroup".into(),
        Tool::Groupadd => "/usr/sbin/groupadd".into(),
        Tool::Plutil => "/usr/bin/plutil".into(),
        Tool::Id => "/usr/bin/id".into(),
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
fn set_owner(path: &Path, owner: Owner, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    match owner {
        Owner::User => {}
        Owner::Root => std::os::unix::fs::chown(path, Some(0), Some(0))?,
        Owner::RootGroup => std::os::unix::fs::chown(path, Some(0), Some(group_id()?))?,
    }
    // After the owner: changing the owner clears a setgid bit.
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
#[allow(clippy::unnecessary_wraps, reason = "the same signature as on Unix")]
fn set_owner(_: &Path, _: Owner, _: u32) -> io::Result<()> {
    // Program Files and ProgramData give their own access to what is
    // created in them.
    Ok(())
}

#[cfg(unix)]
fn group_id() -> io::Result<u32> {
    nix::unistd::Group::from_name(GROUP)
        .map_err(io::Error::from)?
        .map(|g| g.gid.as_raw())
        .ok_or_else(|| io::Error::other(format!("there is no {GROUP} group")))
}

#[cfg(unix)]
fn ensure_group(said: &mut Vec<String>) -> io::Result<()> {
    if group_id().is_ok() {
        return Ok(());
    }
    if cfg!(target_os = "macos") {
        run(
            Tool::Dseditgroup,
            &["-o", "create", "-r", GROUP, GROUP],
            None,
        )?;
    } else {
        run(Tool::Groupadd, &["--system", GROUP], None)?;
    }
    said.push(format!("created the {GROUP} group"));
    Ok(())
}

#[cfg(not(unix))]
#[allow(clippy::unnecessary_wraps, reason = "the same signature as on Unix")]
fn ensure_group(_: &mut Vec<String>) -> io::Result<()> {
    Ok(())
}

/// What the installed `binary` is pinned by, for `installer`.
fn pin(kind: InstallKind, binary: &Path, installer: &Installer) -> io::Result<Expected> {
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
        (Os::Linux, Installer::Uid(uid)) => {
            let pin = match kind {
                #[cfg(unix)]
                InstallKind::Protected => crate::peer::LinuxPin::Group(group_id()?),
                #[cfg(not(unix))]
                InstallKind::Protected => return Err(io::ErrorKind::Unsupported.into()),
                InstallKind::Development => {
                    crate::peer::LinuxPin::DevelopmentSha256(sha256(&path)?)
                }
            };
            Ok(Expected::Linux {
                uid: *uid,
                path,
                pin,
            })
        }
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

/// Everything uninstalling does: remove what the install made and the
/// system copies of the owner's hosts, and turn user-level hosts back on.
#[must_use]
pub fn uninstall_plan(
    os: Os,
    layout: &Layout,
    record: &InstallRecord,
    user_host_dirs: &[PathBuf],
) -> Vec<Step> {
    let protected = record.kind == InstallKind::Protected;
    let mut steps = Vec::new();
    let hive = if protected { "HKLM" } else { "HKCU" };
    if os == Os::Windows {
        for key in windows_host_keys() {
            steps.push(Step::Reg(reg_delete_host(hive, key)));
        }
        for moved in &record.moved_hosts {
            steps.push(Step::Reg(vec![
                "delete".into(),
                moved.to_string_lossy().into_owned(),
                "/f".into(),
                "/reg:64".into(),
            ]));
        }
        if protected {
            for key in windows_policy_keys() {
                steps.push(Step::Reg(reg_delete_policy(key)));
            }
        }
    } else {
        if protected {
            for folder in system_host_dirs(os) {
                steps.push(Step::Remove {
                    path: folder.join(manifest_name()),
                });
            }
            for moved in &record.moved_hosts {
                steps.push(Step::Remove {
                    path: moved.clone(),
                });
            }
        }
        match (os, protected) {
            (Os::Linux, true) => {
                for policy in linux_policy_files() {
                    steps.push(Step::Remove { path: policy });
                }
            }
            (Os::MacOs, true) => steps.push(Step::Tell(
                "remove the jev-pilot profile in System Settings › General › Device \
                     Management, so Chrome accepts your own hosts again"
                    .into(),
            )),
            _ => {}
        }
    }
    // A development install registered in these; a protected one may have
    // been preceded by one that did.
    for folder in user_host_dirs {
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

/// What a protected install says before it asks for an administrator's
/// rights: the policy it sets makes Chrome say it is managed.
pub const MANAGED_WARNING: &str = "This turns off user-level native messaging hosts with a \
                                   Chrome policy, so Chrome will say it is \"managed by your \
                                   organization\" (on macOS, once you approve the profile). Your \
                                   own hosts are copied to system level first, so they keep \
                                   working.";

/// What is said when nothing is installed.
pub const NOT_SET_UP: &str = "jev-pilot's Chrome extension is not set up: run `sudo jev-pilot \
                              extension install` (an elevated prompt on Windows)";

/// What is said when macOS has not yet been given the profile's approval.
pub const PROFILE_NOT_APPROVED: &str = "Chrome still accepts user-level native hosts: approve \
                                        the jev-pilot profile in System Settings › General › \
                                        Device Management";

/// What is said when Chrome's policy has gone, on Linux or Windows.
pub const POLICY_MISSING: &str = "Chrome's policy for jev-pilot is missing: run the install again";

/// The line a run's header carries for an install of `kind`: a
/// development install says it is not protected.
#[must_use]
pub fn run_header(kind: InstallKind) -> Option<String> {
    (kind == InstallKind::Development).then(|| format!("bridge : {UNPROTECTED}"))
}

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

/// The installed copy, if any: a protected install's, else a development
/// one's. `read` reads a file.
///
/// # Errors
/// An `install.json` that cannot be read as one.
pub fn find_installed(
    os: Os,
    folders: &SystemFolders,
    home: &Path,
    env: &dyn Fn(&str) -> Option<String>,
    read: &dyn Fn(&Path) -> io::Result<String>,
) -> Result<Option<Found>, String> {
    for layout in [
        Layout::protected(os, folders),
        Layout::development(os, home, env),
    ] {
        // Only a missing file moves on to the next: one that cannot be read
        // is not a reason to trust the owner's own development install.
        let text = match read(&layout.install_json) {
            Ok(text) => Some(text),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => {
                return Err(format!(
                    "{} cannot be read: {e}",
                    layout.install_json.display()
                ));
            }
        };
        if let Some(text) = text {
            let record = serde_json::from_str(&text)
                .map_err(|e| format!("{} cannot be read: {e}", layout.install_json.display()))?;
            return Ok(Some(Found { layout, record }));
        }
    }
    Ok(None)
}

/// The installed copy on this system, for the owner whose home is `home`:
/// [`find_installed`] with the system's own folders and files.
///
/// # Errors
/// When Windows cannot say where its folders are, or an `install.json`
/// cannot be read.
pub fn installed_on_this_system(home: &Path) -> Result<Option<Found>, String> {
    #[cfg(windows)]
    let folders = SystemFolders::of_this_system().map_err(|e| e.to_string())?;
    #[cfg(not(windows))]
    let folders = SystemFolders::of_this_system();
    let env = |name: &str| std::env::var(name).ok();
    find_installed(Os::current(), &folders, home, &env, &|path| {
        std::fs::read_to_string(path)
    })
}

/// What `jev-pilot extension status` looked at.
#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    /// This system.
    pub os: Os,
    /// The install, if any.
    pub found: Option<Found>,
    /// This binary's version.
    pub this_version: String,
    /// Whether Chrome's policy turning user-level hosts off is in place.
    pub policy_in_place: bool,
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
    let mut lines = vec![match record.kind {
        InstallKind::Protected => format!("install    : protected, version {}", record.version),
        InstallKind::Development => format!("install    : {UNPROTECTED}"),
    }];
    lines.extend(version_differs(&status.this_version, &record.version));
    lines.push(format!("binary     : {}", found.layout.binary.display()));
    lines.push(format!(
        "extension  : {} (loaded unpacked in chrome://extensions, with Developer mode on)",
        found.layout.extension.display()
    ));
    if record.kind == InstallKind::Protected {
        lines.push(if status.policy_in_place {
            "policy     : Chrome starts only system-level native hosts".to_owned()
        } else if status.os == Os::MacOs {
            PROFILE_NOT_APPROVED.to_owned()
        } else {
            POLICY_MISSING.to_owned()
        });
    }
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
