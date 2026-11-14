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
    /// The host manifest Chrome reads, where the Windows registry points;
    /// on macOS and Linux a copy also goes in each browser's folder.
    pub host_manifest: PathBuf,
}

impl Layout {
    /// A protected install's layout: every file where only an administrator
    /// can write.
    #[must_use]
    pub fn protected(os: Os, env: &dyn Fn(&str) -> Option<String>) -> Self {
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
                let files = env("ProgramFiles").unwrap_or_else(|| r"C:\Program Files".into());
                let data = env("ProgramData").unwrap_or_else(|| r"C:\ProgramData".into());
                Self {
                    binary: join(os, &files, &["jev-pilot", binary_name(os)]),
                    install_json: join(os, &files, &["jev-pilot", "install.json"]),
                    extension: join(os, &files, &["jev-pilot", "extension"]),
                    audit_log: join(os, &data, &["jev-pilot", "audit"]),
                    host_manifest: join(os, &files, &["jev-pilot", &manifest]),
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
    },
];

/// The system host folders on macOS or Linux, each once: where a protected
/// install puts a copy of the host manifest. Windows uses the registry.
#[must_use]
pub fn system_host_dirs(os: Os) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for family in FAMILIES {
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
    for family in FAMILIES {
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
            if name == ours || from.extension().is_none_or(|e| e != "json") {
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

/// The registry keys, under HKLM or HKCU, that hold host registrations on
/// Windows. Chrome reads its own; Chromium and Brave read Chromium's, then
/// Chrome's; Edge reads its own, then Chromium's, then Chrome's.
#[must_use]
pub fn windows_host_keys() -> Vec<&'static str> {
    FAMILIES.iter().filter_map(|f| f.windows_hosts).collect()
}

/// The machine policy keys on Windows, one per family.
#[must_use]
pub fn windows_policy_keys() -> Vec<&'static str> {
    FAMILIES.iter().map(|f| f.windows_policy).collect()
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
        if let Some(host) = name.take().filter(|n| n != HOST_NAME) {
            hosts.push((host, PathBuf::from(value)));
        }
    }
    hosts
}

/// The policy files that turn user-level hosts off on Linux, one per
/// family.
#[must_use]
pub fn linux_policy_files() -> Vec<PathBuf> {
    FAMILIES
        .iter()
        .map(|f| join(Os::Linux, f.linux_policy, &["jev-pilot.json"]))
        .collect()
}

/// Their content.
#[must_use]
pub fn linux_policy() -> String {
    serde_json::to_string_pretty(&json!({ USER_LEVEL_HOSTS: false })).unwrap_or_default()
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
    for (family, uuid) in FAMILIES.iter().zip(UUIDS) {
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
