//! Installing the bridge, as values: where everything goes on each system,
//! and what is written there. Nothing here touches the real system: every
//! path is computed from a given home and environment, on every OS's rules.

#![cfg(feature = "extension")]

use jev_pilot::browser::discover::Os;
use jev_pilot::browser::install::{
    InstallKind, InstallRecord, Layout, SystemFolders, host_manifest,
};

/// Where Windows keeps Program Files and ProgramData on a usual machine.
fn folders() -> SystemFolders {
    SystemFolders {
        program_files: r"C:\Program Files".into(),
        program_data: r"C:\ProgramData".into(),
    }
}
use jev_pilot::cli::OwnerTabs;
use jev_pilot::peer::{Expected, LinuxPin, MacosPin, Sha256Digest, WindowsPin};
use serde_json::json;
use std::path::{Path, PathBuf};

fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
    move |name| {
        vars.iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| (*v).to_owned())
    }
}

const WINDOWS: &[(&str, &str)] = &[
    ("ProgramFiles", r"C:\Program Files"),
    ("ProgramData", r"C:\ProgramData"),
    ("LOCALAPPDATA", r"C:\Users\oliver\AppData\Local"),
];

/// Everything a same-user program could rewrite to pass the checks lives
/// where only an administrator can write.
#[test]
fn a_protected_install_puts_everything_where_only_an_administrator_writes() {
    assert_eq!(
        Layout::protected(Os::MacOs, &folders()),
        Layout {
            binary: "/usr/local/libexec/jev-pilot/jev-pilot".into(),
            install_json: "/Library/Application Support/jev-pilot/install.json".into(),
            extension: "/Library/Application Support/jev-pilot/extension".into(),
            audit_log: "/Library/Logs/jev-pilot".into(),
            host_manifest: "/Library/Application Support/jev-pilot/dev.jevpilot.bridge.json".into(),
        }
    );
    assert_eq!(
        Layout::protected(Os::Linux, &folders()),
        Layout {
            binary: "/usr/local/libexec/jev-pilot/jev-pilot".into(),
            install_json: "/etc/jev-pilot/install.json".into(),
            extension: "/usr/local/share/jev-pilot/extension".into(),
            audit_log: "/var/log/jev-pilot".into(),
            host_manifest: "/etc/jev-pilot/dev.jevpilot.bridge.json".into(),
        }
    );
    assert_eq!(
        Layout::protected(Os::Windows, &folders()),
        Layout {
            binary: r"C:\Program Files\jev-pilot\jev-pilot.exe".into(),
            install_json: r"C:\Program Files\jev-pilot\install.json".into(),
            extension: r"C:\Program Files\jev-pilot\extension".into(),
            audit_log: r"C:\ProgramData\jev-pilot\audit".into(),
            host_manifest: r"C:\Program Files\jev-pilot\dev.jevpilot.bridge.json".into(),
        }
    );
    // Where Windows keeps them, as the system says: never from the
    // environment, which a program running as the owner can change.
    let moved = SystemFolders {
        program_files: r"D:\Apps".into(),
        program_data: r"D:\Data".into(),
    };
    assert_eq!(
        Layout::protected(Os::Windows, &moved).binary,
        PathBuf::from(r"D:\Apps\jev-pilot\jev-pilot.exe")
    );
    assert_eq!(
        Layout::protected(Os::Windows, &moved).audit_log,
        PathBuf::from(r"D:\Data\jev-pilot\audit")
    );
}

/// A development install is the owner's own, in the owner's home.
#[test]
fn a_development_install_lives_in_the_owners_home() {
    let mac = Layout::development(Os::MacOs, Path::new("/Users/oliver"), &env(&[]));
    assert_eq!(
        mac.binary,
        PathBuf::from("/Users/oliver/Library/Application Support/jev-pilot/dev/jev-pilot")
    );
    assert_eq!(
        mac.install_json,
        PathBuf::from("/Users/oliver/Library/Application Support/jev-pilot/dev/install.json")
    );
    assert_eq!(
        mac.audit_log,
        PathBuf::from("/Users/oliver/Library/Application Support/jev-pilot/dev/log")
    );
    let linux = Layout::development(Os::Linux, Path::new("/home/oliver"), &env(&[]));
    assert_eq!(
        linux.extension,
        PathBuf::from("/home/oliver/.local/share/jev-pilot/dev/extension")
    );
    let windows = Layout::development(Os::Windows, Path::new(r"C:\Users\oliver"), &env(WINDOWS));
    assert_eq!(
        windows.binary,
        PathBuf::from(r"C:\Users\oliver\AppData\Local\jev-pilot\dev\jev-pilot.exe")
    );
    assert_eq!(
        windows.host_manifest,
        PathBuf::from(r"C:\Users\oliver\AppData\Local\jev-pilot\dev\dev.jevpilot.bridge.json")
    );
}

/// Chrome starts `path` for the one origin it allows.
#[test]
fn the_host_manifest_names_the_installed_binary_and_only_jev_pilots_extension() {
    let manifest = host_manifest(Path::new("/usr/local/libexec/jev-pilot/jev-pilot"));
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&manifest).expect("JSON"),
        json!({
            "name": "dev.jevpilot.bridge",
            "description": "jev-pilot's bridge to its Chrome extension",
            "path": "/usr/local/libexec/jev-pilot/jev-pilot",
            "type": "stdio",
            "allowed_origins": ["chrome-extension://eckgjcpdiehapkphgcflmcikippoegcp/"],
        })
    );
}

fn mac_expected() -> Expected {
    Expected::Macos {
        uid: 501,
        path: "/usr/local/libexec/jev-pilot/jev-pilot".into(),
        pin: MacosPin::Cdhash([0x32; 20]),
    }
}

/// `install.json` says what both ends of the bridge expect of the other, the
/// extension's ID, the version, and what the install changed that uninstall
/// puts back.
#[test]
fn install_json_records_what_to_expect_and_what_to_put_back() {
    let record = InstallRecord {
        version: "0.4.0".into(),
        kind: InstallKind::Protected,
        expected: mac_expected(),
        moved_hosts: vec!["/Users/oliver/Library/Application Support/Google/Chrome/NativeMessagingHosts/com.anthropic.claude_browser_extension.json".into()],
    };
    let json = serde_json::to_value(&record).expect("JSON");
    assert_eq!(
        json,
        json!({
            "version": "0.4.0",
            "install": "protected",
            "extension_id": "eckgjcpdiehapkphgcflmcikippoegcp",
            "expected": {
                "os": "macos",
                "uid": 501,
                "path": "/usr/local/libexec/jev-pilot/jev-pilot",
                "pin": {"cdhash": "32".repeat(20)},
            },
            "moved_hosts": ["/Users/oliver/Library/Application Support/Google/Chrome/NativeMessagingHosts/com.anthropic.claude_browser_extension.json"],
        })
    );
    assert_eq!(
        serde_json::from_value::<InstallRecord>(json).expect("read back"),
        record
    );

    for expected in [
        Expected::Linux {
            uid: 1000,
            path: "/usr/local/libexec/jev-pilot/jev-pilot".into(),
            pin: LinuxPin::Group(977),
        },
        Expected::Windows {
            sid: "S-1-5-21-1-2-3-1001".into(),
            path: r"\\?\C:\Program Files\jev-pilot\jev-pilot.exe".into(),
            pin: WindowsPin::Sha256(Sha256Digest::from_bytes([7; 32])),
            session: None,
        },
    ] {
        let record = InstallRecord {
            version: "0.4.0".into(),
            kind: InstallKind::Development,
            expected,
            moved_hosts: vec![],
        };
        let text = serde_json::to_string(&record).expect("JSON");
        assert_eq!(
            serde_json::from_str::<InstallRecord>(&text).expect("read back"),
            record
        );
    }
}

/// An install.json for another extension is not read as jev-pilot's.
#[test]
fn install_json_for_another_extension_id_is_refused() {
    let mut json = serde_json::to_value(InstallRecord {
        version: "0.4.0".into(),
        kind: InstallKind::Protected,
        expected: mac_expected(),
        moved_hosts: vec![],
    })
    .expect("JSON");
    json["extension_id"] = "abcdefghijklmnopabcdefghijklmnop".into();
    assert!(serde_json::from_value::<InstallRecord>(json).is_err());
}

// --- policy.json ---

use jev_pilot::browser::install::{policy_for_install, with_owner_tabs, with_site};

#[test]
fn a_new_install_writes_the_default_policy_and_says_what_kind_it_is() {
    assert_eq!(
        policy_for_install(None, InstallKind::Protected),
        json!({"sites": [], "owner_tabs": "group", "install": "protected"})
    );
    assert_eq!(
        policy_for_install(None, InstallKind::Development),
        json!({"sites": [], "owner_tabs": "group", "install": "development"})
    );
}

/// Re-running the install keeps the owner's sites and tab rule, and only
/// says again what kind of install it is.
#[test]
fn an_existing_policy_is_kept() {
    let existing =
        json!({"sites": ["https://github.com"], "owner_tabs": "never", "install": "development"});
    assert_eq!(
        policy_for_install(Some(existing), InstallKind::Protected),
        json!({"sites": ["https://github.com"], "owner_tabs": "never", "install": "protected"})
    );
    // One that is not an object is replaced by the default.
    assert_eq!(
        policy_for_install(Some(json!([1, 2])), InstallKind::Protected),
        json!({"sites": [], "owner_tabs": "group", "install": "protected"})
    );
}

#[test]
fn sites_are_added_once_and_removed() {
    let policy =
        json!({"sites": ["https://github.com"], "owner_tabs": "group", "install": "protected"});
    let added = with_site(&policy, "https://*.example.test", true);
    assert_eq!(
        added["sites"],
        json!(["https://github.com", "https://*.example.test"])
    );
    assert_eq!(with_site(&added, "https://*.example.test", true), added);
    assert_eq!(
        with_site(&added, "https://github.com", false)["sites"],
        json!(["https://*.example.test"])
    );
    assert_eq!(added["owner_tabs"], "group");
}

#[test]
fn the_owner_tabs_rule_is_set() {
    let policy = json!({"sites": [], "owner_tabs": "group", "install": "protected"});
    assert_eq!(
        with_owner_tabs(&policy, OwnerTabs::Never),
        json!({"sites": [], "owner_tabs": "never", "install": "protected"})
    );
}

// --- each Chromium-family browser ---

use jev_pilot::browser::install::{
    HostCopy, host_copies, linux_policy, linux_policy_files, mobileconfig, reg_add_host,
    reg_add_policy, reg_delete_host, reg_delete_policy, reg_hosts_in, system_host_dirs,
    user_host_dirs, windows_host_keys, windows_policy_keys,
};

const CHROME_MAC: &str = "/Library/Google/Chrome/NativeMessagingHosts";
const CHROME_LINUX: &str = "/etc/opt/chrome/native-messaging-hosts";

/// The host is registered only for the browsers it serves: on Linux each
/// known install, whose channels share a folder (and Brave reads Chrome's);
/// on macOS and Windows Chrome alone, which is all the host's check of its
/// parent accepts there.
#[test]
fn the_host_is_registered_for_the_browsers_it_serves() {
    assert_eq!(system_host_dirs(Os::MacOs), [PathBuf::from(CHROME_MAC)]);
    assert_eq!(
        system_host_dirs(Os::Linux),
        [
            PathBuf::from(CHROME_LINUX),
            "/etc/chromium/native-messaging-hosts".into(),
            "/etc/opt/edge/native-messaging-hosts".into(),
        ]
    );
    assert_eq!(system_host_dirs(Os::Windows), Vec::<PathBuf>::new());
}

/// Each browser's user-level folder, and the system folder the same browser
/// reads, where a host found there is copied before user-level hosts are
/// turned off.
#[test]
fn each_user_level_folder_maps_to_the_system_folder_its_browser_reads() {
    let mac = user_host_dirs(Os::MacOs, Path::new("/Users/owner"));
    let support = "/Users/owner/Library/Application Support";
    assert_eq!(mac.len(), 4, "Chrome's channels alone: {mac:?}");
    for (user, system) in [
        ("Google/Chrome", CHROME_MAC),
        ("Google/Chrome Beta", CHROME_MAC),
        ("Google/Chrome Dev", CHROME_MAC),
        ("Google/Chrome Canary", CHROME_MAC),
    ] {
        let user = PathBuf::from(format!("{support}/{user}/NativeMessagingHosts"));
        assert!(
            mac.contains(&(user.clone(), PathBuf::from(system))),
            "{} -> {system} in {mac:?}",
            user.display()
        );
    }
    let linux = user_host_dirs(Os::Linux, Path::new("/home/o"));
    for (user, system) in [
        ("google-chrome", CHROME_LINUX),
        ("google-chrome-beta", CHROME_LINUX),
        ("google-chrome-unstable", CHROME_LINUX),
        ("google-chrome-canary", CHROME_LINUX),
        ("chromium", "/etc/chromium/native-messaging-hosts"),
        ("microsoft-edge", "/etc/opt/edge/native-messaging-hosts"),
        ("BraveSoftware/Brave-Browser", CHROME_LINUX),
    ] {
        let user = PathBuf::from(format!("/home/o/.config/{user}/NativeMessagingHosts"));
        assert!(
            linux.contains(&(user.clone(), PathBuf::from(system))),
            "{} -> {system}",
            user.display()
        );
    }
    assert_eq!(user_host_dirs(Os::Windows, Path::new(r"C:\Users\o")), []);
}

/// A user-level host other than jev-pilot's is copied to its browser's
/// system folder, unless one of that name is already there, so turning off
/// user-level hosts does not turn it off.
#[test]
fn user_level_hosts_are_copied_to_system_level_once() {
    let user =
        PathBuf::from("/Users/o/Library/Application Support/Google/Chrome/NativeMessagingHosts");
    let found = [
        (
            user.join("com.anthropic.claude_browser_extension.json"),
            PathBuf::from(CHROME_MAC),
        ),
        (
            user.join("dev.jevpilot.bridge.json"),
            PathBuf::from(CHROME_MAC),
        ),
        (
            user.join("com.example.already.json"),
            PathBuf::from(CHROME_MAC),
        ),
        (user.join("notes.txt"), PathBuf::from(CHROME_MAC)),
    ];
    let exists = |path: &Path| path.ends_with("com.example.already.json");
    assert_eq!(
        host_copies(&found, &exists),
        [HostCopy {
            from: user.join("com.anthropic.claude_browser_extension.json"),
            to: PathBuf::from(CHROME_MAC).join("com.anthropic.claude_browser_extension.json"),
        }]
    );
}

/// On Windows the host serves Chrome alone: it is registered, and the
/// policy set, for Chrome.
#[test]
fn windows_registers_the_host_for_chrome() {
    assert_eq!(
        windows_host_keys(),
        [r"SOFTWARE\Google\Chrome\NativeMessagingHosts"]
    );
    assert_eq!(windows_policy_keys(), [r"SOFTWARE\Policies\Google\Chrome"]);
}

/// Registry writes go through `reg.exe` with fixed argument vectors: the
/// key's default value names the manifest, in the 64-bit view.
#[test]
fn reg_exe_is_given_fixed_argument_vectors() {
    let manifest = Path::new(r"C:\Program Files\jev-pilot\dev.jevpilot.bridge.json");
    let key = r"SOFTWARE\Google\Chrome\NativeMessagingHosts";
    assert_eq!(
        reg_add_host("HKLM", key, manifest),
        [
            "add",
            r"HKLM\SOFTWARE\Google\Chrome\NativeMessagingHosts\dev.jevpilot.bridge",
            "/ve",
            "/t",
            "REG_SZ",
            "/d",
            r"C:\Program Files\jev-pilot\dev.jevpilot.bridge.json",
            "/f",
            "/reg:64",
        ]
    );
    assert_eq!(
        reg_delete_host("HKCU", key),
        [
            "delete",
            r"HKCU\SOFTWARE\Google\Chrome\NativeMessagingHosts\dev.jevpilot.bridge",
            "/f",
            "/reg:64",
        ]
    );
    assert_eq!(
        reg_add_policy(r"SOFTWARE\Policies\Google\Chrome"),
        [
            "add",
            r"HKLM\SOFTWARE\Policies\Google\Chrome",
            "/v",
            "NativeMessagingUserLevelHosts",
            "/t",
            "REG_DWORD",
            "/d",
            "0",
            "/f",
            "/reg:64",
        ]
    );
    assert_eq!(
        reg_delete_policy(r"SOFTWARE\Policies\Google\Chrome"),
        [
            "delete",
            r"HKLM\SOFTWARE\Policies\Google\Chrome",
            "/v",
            "NativeMessagingUserLevelHosts",
            "/f",
            "/reg:64",
        ]
    );
}

/// `reg query` lists the user-level hosts under one key; each subkey's
/// default value is its manifest.
#[test]
fn the_user_level_hosts_in_the_registry_are_read_from_reg_query() {
    let output = "\r\n\
HKEY_CURRENT_USER\\SOFTWARE\\Google\\Chrome\\NativeMessagingHosts\\com.anthropic.claude_browser_extension\r\n\
    (Default)    REG_SZ    C:\\Users\\o\\AppData\\Roaming\\Claude\\host.json\r\n\
\r\n\
HKEY_CURRENT_USER\\SOFTWARE\\Google\\Chrome\\NativeMessagingHosts\\dev.jevpilot.bridge\r\n\
    (Default)    REG_SZ    C:\\x.json\r\n\
\r\n\
HKEY_CURRENT_USER\\SOFTWARE\\Google\\Chrome\\NativeMessagingHosts\\com.example.empty\r\n\
\r\n";
    assert_eq!(
        reg_hosts_in(output),
        [(
            "com.anthropic.claude_browser_extension".to_owned(),
            PathBuf::from(r"C:\Users\o\AppData\Roaming\Claude\host.json")
        )]
    );
}

#[test]
fn linux_turns_off_user_level_hosts_with_a_policy_file_per_family() {
    assert_eq!(
        linux_policy_files(),
        [
            PathBuf::from("/etc/opt/chrome/policies/managed/jev-pilot.json"),
            "/etc/chromium/policies/managed/jev-pilot.json".into(),
            "/etc/opt/edge/policies/managed/jev-pilot.json".into(),
            "/etc/brave/policies/managed/jev-pilot.json".into(),
        ]
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&linux_policy()).expect("JSON"),
        json!({"NativeMessagingUserLevelHosts": false})
    );
}

/// macOS honours the policy only as a mandatory one, which on an unmanaged
/// Mac means a configuration profile: one payload, for the preference domain
/// all of Chrome's channels read, `com.google.Chrome`.
#[test]
fn macos_turns_off_user_level_hosts_with_a_configuration_profile() {
    let profile = mobileconfig();
    assert!(profile.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist"));
    assert!(profile.contains("<key>PayloadType</key>\n\t\t\t<string>com.google.Chrome</string>"));
    assert_eq!(
        profile
            .matches("<key>NativeMessagingUserLevelHosts</key>\n\t\t\t<false/>")
            .count(),
        1
    );
    assert!(profile.contains("<key>PayloadType</key>\n\t<string>Configuration</string>"));
    assert!(profile.contains("<string>dev.jevpilot.policy</string>"));
    // The same every time, so installing again changes nothing.
    assert_eq!(profile, mobileconfig());
}

// --- the install, as a plan of steps ---

use jev_pilot::browser::install::{Existing, Owner, RegistryHost, Step, UserHosts, plan};

const EXE: &str = "/Users/o/.jev-pilot/bin/jev-pilot";

fn files_written(steps: &[Step]) -> Vec<PathBuf> {
    steps
        .iter()
        .filter_map(|s| match s {
            Step::WriteFile { path, .. } => Some(path.clone()),
            _ => None,
        })
        .collect()
}

/// Linux: the binary is root's with the jev-pilot group and setgid, so a
/// process has the group only by executing it; everything it trusts is
/// root's; user-level hosts are copied, then turned off by policy.
#[test]
fn a_protected_linux_install_plan() {
    let layout = Layout::protected(Os::Linux, &folders());
    let user =
        PathBuf::from("/home/o/.config/google-chrome/NativeMessagingHosts/com.example.x.json");
    let steps = plan(
        Os::Linux,
        InstallKind::Protected,
        &layout,
        Path::new("/home/o/.jev-pilot/bin/jev-pilot"),
        &UserHosts::Files(vec![HostCopy {
            from: user.clone(),
            to: PathBuf::from(CHROME_LINUX).join("com.example.x.json"),
        }]),
        &Existing::default(),
    );
    let binary = PathBuf::from("/usr/local/libexec/jev-pilot/jev-pilot");
    assert_eq!(steps[0], Step::EnsureGroup);
    assert!(steps.contains(&Step::CreateDir {
        path: "/usr/local/libexec/jev-pilot".into(),
        mode: 0o755,
        owner: Owner::Root,
    }));
    let copy = steps
        .iter()
        .position(|s| {
            *s == Step::CopyBinary {
                from: "/home/o/.jev-pilot/bin/jev-pilot".into(),
                to: binary.clone(),
            }
        })
        .expect("the binary is copied");
    let owned = steps
        .iter()
        .position(|s| {
            *s == Step::SetOwner {
                path: binary.clone(),
                owner: Owner::RootGroup,
                mode: 0o2755,
            }
        })
        .expect("and made setgid");
    assert!(copy < owned);
    assert!(
        !steps.iter().any(|s| matches!(s, Step::Sign { .. })),
        "no signing on Linux"
    );

    let written = files_written(&steps);
    for file in [
        "/usr/local/share/jev-pilot/extension/manifest.json",
        "/etc/opt/chrome/native-messaging-hosts/dev.jevpilot.bridge.json",
        "/etc/chromium/native-messaging-hosts/dev.jevpilot.bridge.json",
        "/etc/opt/edge/native-messaging-hosts/dev.jevpilot.bridge.json",
        "/etc/opt/chrome/policies/managed/jev-pilot.json",
        "/etc/brave/policies/managed/jev-pilot.json",
    ] {
        assert!(written.contains(&PathBuf::from(file)), "{file} is written");
    }
    assert!(steps.contains(&Step::WritePolicy {
        path: "/usr/local/share/jev-pilot/extension/policy.json".into(),
        kind: InstallKind::Protected,
    }));
    for step in &steps {
        if let Step::WriteFile {
            owner, mode, path, ..
        } = step
        {
            assert_eq!((*owner, *mode), (Owner::Root, 0o644), "{}", path.display());
        }
    }
    let host_copy = steps
        .iter()
        .position(|s| {
            *s == Step::CopyHost(HostCopy {
                from: user.clone(),
                to: PathBuf::from(CHROME_LINUX).join("com.example.x.json"),
            })
        })
        .expect("the user-level host is copied");
    let policy = steps
        .iter()
        .position(|s| matches!(s, Step::WriteFile { path, .. } if path.ends_with("managed/jev-pilot.json")))
        .expect("the policy is written");
    assert!(
        host_copy < policy,
        "copied before user-level hosts are turned off"
    );
    assert!(steps.contains(&Step::CreateDir {
        path: "/var/log/jev-pilot".into(),
        mode: 0o775,
        owner: Owner::RootGroup,
    }));
    assert_eq!(
        steps.last(),
        Some(&Step::Record {
            path: "/etc/jev-pilot/install.json".into(),
            kind: InstallKind::Protected,
            binary,
            moved_hosts: vec![PathBuf::from(CHROME_LINUX).join("com.example.x.json")],
        })
    );
}

/// macOS: the copy is re-signed ad hoc with the hardened runtime before it
/// is made setgid, and the profile is opened for the owner to approve.
#[test]
fn a_protected_macos_install_plan() {
    let layout = Layout::protected(Os::MacOs, &folders());
    let steps = plan(
        Os::MacOs,
        InstallKind::Protected,
        &layout,
        Path::new(EXE),
        &UserHosts::Files(vec![]),
        &Existing::default(),
    );
    let binary = PathBuf::from("/usr/local/libexec/jev-pilot/jev-pilot");
    let sign = steps
        .iter()
        .position(|s| {
            *s == Step::Sign {
                path: binary.clone(),
            }
        })
        .expect("signed");
    let owned = steps
        .iter()
        .position(|s| {
            *s == Step::SetOwner {
                path: binary.clone(),
                owner: Owner::RootGroup,
                mode: 0o2755,
            }
        })
        .expect("setgid");
    assert!(sign < owned, "signing would clear the setgid bit");
    let profile = PathBuf::from("/Library/Application Support/jev-pilot/jev-pilot.mobileconfig");
    assert!(files_written(&steps).contains(&profile));
    assert!(steps.contains(&Step::OpenProfile { path: profile }));
    assert!(files_written(&steps).contains(&PathBuf::from(
        "/Library/Google/Chrome/NativeMessagingHosts/dev.jevpilot.bridge.json"
    )));
    assert!(steps.contains(&Step::CreateDir {
        path: "/Library/Logs/jev-pilot".into(),
        mode: 0o775,
        owner: Owner::RootGroup,
    }));
}

/// Windows: no group and no setgid; the host and the policy are registry
/// entries under HKLM, and user-level hosts found under HKCU are registered
/// under HKLM too.
#[test]
fn a_protected_windows_install_plan() {
    let layout = Layout::protected(Os::Windows, &folders());
    let steps = plan(
        Os::Windows,
        InstallKind::Protected,
        &layout,
        Path::new(r"C:\Users\o\.jev-pilot\bin\jev-pilot.exe"),
        &UserHosts::Registry(vec![RegistryHost {
            key: r"SOFTWARE\Google\Chrome\NativeMessagingHosts".into(),
            name: "com.example.x".into(),
            manifest: PathBuf::from(r"C:\Users\owner\x.json"),
        }]),
        &Existing::default(),
    );
    assert!(!steps.contains(&Step::EnsureGroup));
    let manifest = PathBuf::from(r"C:\Program Files\jev-pilot\dev.jevpilot.bridge.json");
    assert!(files_written(&steps).contains(&manifest));
    let regs: Vec<&Vec<String>> = steps
        .iter()
        .filter_map(|s| match s {
            Step::Reg(args) => Some(args),
            _ => None,
        })
        .collect();
    for key in jev_pilot::browser::install::windows_host_keys() {
        assert!(
            regs.contains(&&reg_add_host("HKLM", key, &manifest)),
            "{key}"
        );
    }
    for key in jev_pilot::browser::install::windows_policy_keys() {
        assert!(regs.contains(&&reg_add_policy(key)), "{key}");
    }
    let moved = vec![
        "add".to_owned(),
        r"HKLM\SOFTWARE\Google\Chrome\NativeMessagingHosts\com.example.x".to_owned(),
        "/ve".into(),
        "/t".into(),
        "REG_SZ".into(),
        "/d".into(),
        r"C:\Users\owner\x.json".into(),
        "/f".into(),
        "/reg:64".into(),
    ];
    assert!(
        regs.contains(&&moved),
        "the HKCU host is registered under HKLM"
    );
    let first_policy = regs
        .iter()
        .position(|a| a.contains(&"NativeMessagingUserLevelHosts".to_owned()))
        .unwrap();
    let copied = regs.iter().position(|a| **a == moved).unwrap();
    assert!(copied < first_policy);
}

/// A development install is the owner's: no group, no setgid, no policy,
/// registered at user level for the owner's browsers.
#[test]
fn a_development_install_plan() {
    let layout = Layout::development(Os::Linux, Path::new("/home/o"), &env(&[]));
    let user_dirs = vec![PathBuf::from(
        "/home/o/.config/google-chrome/NativeMessagingHosts",
    )];
    let steps = plan(
        Os::Linux,
        InstallKind::Development,
        &layout,
        Path::new("/home/o/.jev-pilot/bin/jev-pilot"),
        &UserHosts::Register(user_dirs),
        &Existing::default(),
    );
    assert!(!steps.contains(&Step::EnsureGroup));
    assert!(!steps.iter().any(|s| matches!(s, Step::Sign { .. })));
    // The copy is made closed to everyone, then opened to run.
    assert!(steps.contains(&Step::SetOwner {
        path: layout.binary.clone(),
        owner: Owner::User,
        mode: 0o755,
    }));
    for step in &steps {
        match step {
            Step::CreateDir { owner, .. } | Step::WriteFile { owner, .. } => {
                assert_eq!(*owner, Owner::User);
            }
            _ => {}
        }
    }
    assert!(files_written(&steps).contains(&PathBuf::from(
        "/home/o/.config/google-chrome/NativeMessagingHosts/dev.jevpilot.bridge.json"
    )));
    assert!(!files_written(&steps).iter().any(|p| p.starts_with("/etc")));
    assert!(steps.contains(&Step::WritePolicy {
        path: "/home/o/.local/share/jev-pilot/dev/extension/policy.json".into(),
        kind: InstallKind::Development,
    }));
}

/// The cdhash a macOS pin is made from, as `codesign -d -vvv` prints it.
#[test]
fn the_cdhash_is_read_from_codesigns_report() {
    use jev_pilot::browser::install::cdhash_in;
    let report = "Executable=/usr/local/libexec/jev-pilot/jev-pilot\n\
                  Identifier=jev_pilot-90cd7c3a4126267f\n\
                  CodeDirectory v=20500 size=50000 flags=0x10002(adhoc,runtime) hashes=1557+2 location=embedded\n\
                  CDHash=32e719c23c33eb097271b746f2a09f3127b57e5a\n";
    assert_eq!(
        cdhash_in(report),
        Some(MacosPin::from_cdhash_hex("32e719c23c33eb097271b746f2a09f3127b57e5a").unwrap())
    );
    assert_eq!(cdhash_in("code object is not signed at all\n"), None);
}

/// Uninstalling removes what the install made and the system copies of the
/// owner's hosts, and turns user-level hosts back on; on macOS the owner
/// removes the profile.
#[test]
fn uninstalling_undoes_the_install() {
    use jev_pilot::browser::install::uninstall_plan;
    let layout = Layout::protected(Os::Linux, &folders());
    let moved = PathBuf::from(CHROME_LINUX).join("com.example.x.json");
    let record = InstallRecord {
        version: "0.4.0".into(),
        kind: InstallKind::Protected,
        expected: Expected::Linux {
            uid: 1000,
            path: layout.binary.clone(),
            pin: LinuxPin::Group(977),
        },
        moved_hosts: vec![moved.clone()],
    };
    let steps = uninstall_plan(Os::Linux, &layout, &record, &[]);
    let removed: Vec<&Path> = steps
        .iter()
        .filter_map(|s| match s {
            Step::Remove { path } => Some(path.as_path()),
            _ => None,
        })
        .collect();
    for path in [
        Path::new("/etc/opt/chrome/native-messaging-hosts/dev.jevpilot.bridge.json"),
        &moved,
        Path::new("/etc/opt/chrome/policies/managed/jev-pilot.json"),
        Path::new("/usr/local/share/jev-pilot/extension"),
        Path::new("/usr/local/libexec/jev-pilot/jev-pilot"),
        Path::new("/etc/jev-pilot/install.json"),
    ] {
        assert!(removed.contains(&path), "{} is removed", path.display());
    }
    assert!(
        !removed.contains(&Path::new("/var/log/jev-pilot")),
        "the audit log stays"
    );

    let mac = uninstall_plan(
        Os::MacOs,
        &Layout::protected(Os::MacOs, &folders()),
        &InstallRecord {
            expected: mac_expected(),
            ..record.clone()
        },
        &[],
    );
    assert!(
        mac.iter()
            .any(|s| matches!(s, Step::Tell(words) if words.contains("profile")))
    );

    let windows = uninstall_plan(
        Os::Windows,
        &Layout::protected(Os::Windows, &folders()),
        &InstallRecord {
            moved_hosts: vec![PathBuf::from(
                r"HKLM\SOFTWARE\Google\Chrome\NativeMessagingHosts\com.example.x",
            )],
            ..record
        },
        &[],
    );
    assert!(windows.contains(&Step::Reg(reg_delete_policy(
        r"SOFTWARE\Policies\Google\Chrome"
    ))));
    assert!(windows.contains(&Step::Reg(vec![
        "delete".into(),
        r"HKLM\SOFTWARE\Google\Chrome\NativeMessagingHosts\com.example.x".into(),
        "/f".into(),
        "/reg:64".into(),
    ])));
}

// --- status, and what is said when something is wrong ---

use jev_pilot::browser::install::{Found, Status, status_lines};

fn found(kind: InstallKind) -> Found {
    let layout = Layout::protected(Os::MacOs, &folders());
    Found {
        record: InstallRecord {
            version: "0.4.0".into(),
            kind,
            expected: mac_expected(),
            moved_hosts: vec![],
        },
        layout,
    }
}

fn status(found: Option<Found>) -> Status {
    Status {
        os: Os::MacOs,
        found,
        this_version: "0.4.0".into(),
        policy_in_place: true,
        host_answers: true,
        policy: Some(json!({"sites": ["https://github.com"], "owner_tabs": "group"})),
    }
}

#[test]
fn status_says_when_nothing_is_set_up_and_how_to_fix_it() {
    assert_eq!(
        status_lines(&status(None)).expect_err("not set up"),
        "jev-pilot's Chrome extension is not set up: run `sudo jev-pilot extension install` \
         (an elevated prompt on Windows)"
    );
}

#[test]
fn status_reports_a_working_protected_install() {
    let lines = status_lines(&status(Some(found(InstallKind::Protected)))).expect("set up");
    assert_eq!(
        lines,
        [
            "install    : protected, version 0.4.0",
            "binary     : /usr/local/libexec/jev-pilot/jev-pilot",
            "extension  : /Library/Application Support/jev-pilot/extension (loaded unpacked in \
             chrome://extensions, with Developer mode on)",
            "policy     : Chrome starts only system-level native hosts",
            "sites      : https://github.com",
            "owner tabs : group",
            "bridge     : the host answers; Chrome is running with the extension",
        ]
    );
}

#[test]
fn status_names_each_problem_with_its_fix() {
    let mut s = status(Some(found(InstallKind::Protected)));
    s.policy_in_place = false;
    s.host_answers = false;
    s.this_version = "0.5.0".into();
    s.policy = Some(json!({"sites": [], "owner_tabs": "never"}));
    let lines = status_lines(&s).expect("set up");
    for line in [
        "this jev-pilot is 0.5.0, the installed one is 0.4.0: run the install again",
        "Chrome still accepts user-level native hosts: approve the jev-pilot profile in System \
         Settings › General › Device Management",
        "sites      : every site",
        "owner tabs : never",
        "Chrome is not running with the jev-pilot extension: open Chrome; if it is open, check \
         the extension is on in chrome://extensions with Developer mode on (`jev-pilot \
         extension status` says more)",
    ] {
        assert!(lines.iter().any(|l| l == line), "{line} in {lines:#?}");
    }
    s.os = Os::Linux;
    assert!(
        status_lines(&s)
            .expect("set up")
            .iter()
            .any(|l| l == "Chrome's policy for jev-pilot is missing: run the install again")
    );
}

#[test]
fn status_labels_a_development_install() {
    let lines = status_lines(&status(Some(found(InstallKind::Development)))).expect("set up");
    assert_eq!(
        lines[0],
        "install    : development install: not protected against other programs running as you"
    );
    assert!(
        !lines.iter().any(|l| l.starts_with("policy")),
        "no policy to report"
    );
}

#[test]
fn a_run_that_finds_another_version_installed_says_so() {
    use jev_pilot::browser::install::version_differs;
    assert_eq!(
        version_differs("0.5.0", "0.4.0"),
        Some(
            "this jev-pilot is 0.5.0, the installed one is 0.4.0: run the install again".to_owned()
        )
    );
    assert_eq!(version_differs("0.4.0", "0.4.0"), None);
}

/// A development install in a home with no browser profile registers
/// nowhere, and says so.
#[test]
fn a_development_install_with_no_browser_says_so() {
    let layout = Layout::development(Os::MacOs, Path::new("/Users/o"), &env(&[]));
    let steps = plan(
        Os::MacOs,
        InstallKind::Development,
        &layout,
        Path::new(EXE),
        &UserHosts::Register(vec![]),
        &Existing::default(),
    );
    assert!(
        steps.contains(&Step::Tell(
            "no Chrome, Chromium, Edge or Brave profile was found in your home, so the host is \
         registered with none: run the install again once one has started"
                .into()
        ))
    );
}

/// Installing again, over an install that had copied the owner's hosts,
/// keeps the earlier copies in install.json, so uninstalling can still
/// remove them; and keeps a policy.json the owner has edited.
#[test]
fn installing_again_keeps_what_the_earlier_install_moved() {
    let layout = Layout::protected(Os::Linux, &folders());
    let earlier = PathBuf::from(CHROME_LINUX).join("com.example.earlier.json");
    let now = HostCopy {
        from: "/home/owner/.config/google-chrome/NativeMessagingHosts/com.example.now.json".into(),
        to: PathBuf::from(CHROME_LINUX).join("com.example.now.json"),
    };
    let existing = Existing {
        moved_hosts: vec![earlier.clone(), now.to.clone()],
        user_host_dirs: vec![],
    };
    let steps = plan(
        Os::Linux,
        InstallKind::Protected,
        &layout,
        Path::new(EXE),
        &UserHosts::Files(vec![now.clone()]),
        &existing,
    );
    let Some(Step::Record { moved_hosts, .. }) = steps.last() else {
        panic!("ends with the record");
    };
    assert_eq!(moved_hosts, &vec![earlier, now.to]);

    let once = policy_for_install(None, InstallKind::Protected);
    let edited = with_site(&once, "https://github.com", true);
    assert_eq!(
        policy_for_install(Some(edited.clone()), InstallKind::Protected),
        edited
    );
}

/// The user-writable development install is read only when no protected
/// install exists: a protected install.json that cannot be read is an
/// error, not a reason to trust the owner's own.
#[test]
fn only_a_missing_protected_install_falls_back_to_a_development_one() {
    use jev_pilot::browser::install::find_installed;
    let home = Path::new("/home/owner");
    let protected = Layout::protected(Os::Linux, &folders()).install_json;
    let dev = Layout::development(Os::Linux, home, &env(&[]));
    let record = serde_json::to_string(&InstallRecord {
        version: "0.4.0".into(),
        kind: InstallKind::Development,
        expected: Expected::Linux {
            uid: 1000,
            path: dev.binary.clone(),
            pin: LinuxPin::DevelopmentSha256(Sha256Digest::from_bytes([7; 32])),
        },
        moved_hosts: vec![],
    })
    .unwrap();
    let reading = |protected_error: std::io::ErrorKind| {
        let protected = protected.clone();
        let dev = dev.install_json.clone();
        let record = record.clone();
        move |path: &Path| -> std::io::Result<String> {
            if path == protected {
                Err(protected_error.into())
            } else if path == dev {
                Ok(record.clone())
            } else {
                Err(std::io::ErrorKind::NotFound.into())
            }
        }
    };
    let found = find_installed(
        Os::Linux,
        &folders(),
        home,
        &env(&[]),
        &reading(std::io::ErrorKind::NotFound),
    )
    .expect("read")
    .expect("found");
    assert_eq!(found.record.kind, InstallKind::Development);
    assert!(
        find_installed(
            Os::Linux,
            &folders(),
            home,
            &env(&[]),
            &reading(std::io::ErrorKind::PermissionDenied)
        )
        .is_err()
    );
}

// --- the owner's own hosts, moved safely ---

/// jev-pilot's own name, in any case, is never taken for one of the
/// owner's hosts: macOS and Windows compare names without regard to case,
/// and a copy would overwrite jev-pilot's own manifest.
#[test]
fn a_host_named_like_jev_pilots_in_any_case_is_not_copied() {
    let user = PathBuf::from(
        "/Users/owner/Library/Application Support/Google/Chrome/NativeMessagingHosts",
    );
    let found = [
        (
            user.join("Dev.JevPilot.Bridge.json"),
            PathBuf::from(CHROME_MAC),
        ),
        (
            user.join("DEV.JEVPILOT.BRIDGE.JSON"),
            PathBuf::from(CHROME_MAC),
        ),
    ];
    assert_eq!(host_copies(&found, &|_| false), []);
    let output = "HKEY_CURRENT_USER\\SOFTWARE\\Google\\Chrome\\NativeMessagingHosts\\DEV.JEVPILOT.BRIDGE\r\n    (Default)    REG_SZ    C:\\x.json\r\n";
    assert_eq!(reg_hosts_in(output), []);
}

/// The owner's hosts are copied before jev-pilot's own manifest is
/// written, so jev-pilot's is the last word in every folder.
#[test]
fn the_owners_hosts_are_copied_before_jev_pilots_manifest_is_written() {
    let layout = Layout::protected(Os::MacOs, &folders());
    let copy = HostCopy {
        from: "/Users/owner/Library/Application Support/Google/Chrome/NativeMessagingHosts/x.json"
            .into(),
        to: PathBuf::from(CHROME_MAC).join("x.json"),
    };
    let steps = plan(
        Os::MacOs,
        InstallKind::Protected,
        &layout,
        Path::new(EXE),
        &UserHosts::Files(vec![copy.clone()]),
        &Existing::default(),
    );
    let copied = steps
        .iter()
        .position(|s| *s == Step::CopyHost(copy.clone()))
        .unwrap();
    let ours = steps
        .iter()
        .position(|s| matches!(s, Step::WriteFile { path, .. } if *path == PathBuf::from(CHROME_MAC).join("dev.jevpilot.bridge.json")))
        .unwrap();
    assert!(copied < ours);
}

/// What is copied of an owner's host is its manifest, read and written
/// again: one that is not a host manifest named as its file is refused.
#[test]
fn only_a_host_manifest_named_as_its_file_is_copied() {
    use jev_pilot::browser::install::manifest_to_copy;
    let good = r#"{"name": "com.example.x", "description": "x", "path": "/opt/x/host",
                   "type": "stdio", "allowed_origins": ["chrome-extension://abc/"]}"#;
    let copied = manifest_to_copy(good, "com.example.x").expect("a manifest");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&copied).unwrap(),
        serde_json::from_str::<serde_json::Value>(good).unwrap()
    );
    for (text, stem) in [
        (good, "com.example.other"),
        (
            r#"{"name": "com.example.x", "path": "relative/host", "type": "stdio"}"#,
            "com.example.x",
        ),
        (
            r#"{"name": "com.example.x", "type": "stdio"}"#,
            "com.example.x",
        ),
        ("root:x:0:0:root:/root:/bin/sh", "passwd"),
        ("[1, 2]", "x"),
    ] {
        assert!(manifest_to_copy(text, stem).is_err(), "{text}");
    }
}

/// A host file is read without following a link, and only when it is the
/// owner's own regular file of a manifest's size.
#[cfg(unix)]
#[test]
fn a_linked_or_foreign_host_file_is_not_read() {
    use jev_pilot::browser::install::read_owners_host;
    let dir = std::env::temp_dir().join(format!("jev-pilot-hosts-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let me = rustix::process::getuid().as_raw();
    let real = dir.join("com.example.x.json");
    std::fs::write(
        &real,
        r#"{"name": "com.example.x", "path": "/opt/x/host", "type": "stdio"}"#,
    )
    .unwrap();
    assert!(read_owners_host(&real, me).is_ok());
    assert!(
        read_owners_host(&real, me + 1).is_err(),
        "another user's file"
    );
    let link = dir.join("com.example.y.json");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert!(read_owners_host(&link, me).is_err(), "a link");
    let big = dir.join("com.example.big.json");
    std::fs::write(&big, vec![b' '; 1 << 20]).unwrap();
    assert!(read_owners_host(&big, me).is_err(), "too large");
    let _ = std::fs::remove_dir_all(&dir);
}

/// On Windows, a host already registered for the machine under the same
/// name is a vendor's own, and is left alone.
#[test]
fn a_host_already_registered_for_the_machine_is_left_alone() {
    use jev_pilot::browser::install::registry_hosts_to_copy;
    let host = |name: &str| RegistryHost {
        key: r"SOFTWARE\Google\Chrome\NativeMessagingHosts".into(),
        name: name.into(),
        manifest: PathBuf::from(r"C:\Users\owner\x.json"),
    };
    let found = vec![
        host("com.vendor.x"),
        host("com.example.y"),
        host("Dev.JevPilot.Bridge"),
    ];
    assert_eq!(
        registry_hosts_to_copy(found, &|h: &RegistryHost| h.name == "com.vendor.x"),
        [host("com.example.y")]
    );
}

/// A protected install removes a development install's user-level
/// registrations, which Chrome would otherwise still prefer until the
/// policy is in place.
#[test]
fn a_protected_install_removes_an_earlier_development_registration() {
    let layout = Layout::protected(Os::MacOs, &folders());
    let dir = PathBuf::from(
        "/Users/owner/Library/Application Support/Google/Chrome/NativeMessagingHosts",
    );
    let steps = plan(
        Os::MacOs,
        InstallKind::Protected,
        &layout,
        Path::new(EXE),
        &UserHosts::Files(vec![]),
        &Existing {
            moved_hosts: vec![],
            user_host_dirs: vec![dir.clone()],
        },
    );
    assert!(steps.contains(&Step::Remove {
        path: dir.join("dev.jevpilot.bridge.json")
    }));
}

/// Uninstalling a development install removes the user-level
/// registrations it made.
#[test]
fn uninstalling_a_development_install_removes_its_registrations() {
    use jev_pilot::browser::install::uninstall_plan;
    let home = Path::new("/home/owner");
    let layout = Layout::development(Os::Linux, home, &env(&[]));
    let record = InstallRecord {
        version: "0.4.0".into(),
        kind: InstallKind::Development,
        expected: Expected::Linux {
            uid: 1000,
            path: layout.binary.clone(),
            pin: LinuxPin::DevelopmentSha256(Sha256Digest::from_bytes([7; 32])),
        },
        moved_hosts: vec![],
    };
    let dir = PathBuf::from("/home/owner/.config/google-chrome/NativeMessagingHosts");
    let steps = uninstall_plan(Os::Linux, &layout, &record, std::slice::from_ref(&dir));
    assert!(steps.contains(&Step::Remove {
        path: dir.join("dev.jevpilot.bridge.json")
    }));
    assert!(steps.contains(&Step::Remove {
        path: layout.binary.clone()
    }));
}

/// The install makes the audit log's files, root's with the group, which
/// the host appends to and never makes: so the owner can neither make nor
/// rewrite them.
#[test]
fn a_protected_install_makes_the_audit_logs_files() {
    let layout = Layout::protected(Os::Linux, &folders());
    let steps = plan(
        Os::Linux,
        InstallKind::Protected,
        &layout,
        Path::new(EXE),
        &UserHosts::Files(vec![]),
        &Existing::default(),
    );
    for name in ["audit.log", "audit.log.1", "audit.log.2", "audit.log.3"] {
        assert!(
            steps.contains(&Step::CreateFile {
                path: PathBuf::from("/var/log/jev-pilot").join(name),
                mode: 0o664,
                owner: Owner::RootGroup,
            }),
            "{name}"
        );
    }
}

// --- where a protected install writes ---

/// What a protected install writes is safe only if no folder above it can
/// be changed by the owner: every one that exists must be root's, closed to
/// group and others, and not a link.
#[test]
fn a_protected_install_writes_only_under_folders_only_root_can_change() {
    use jev_pilot::browser::install::{DirMeta, check_ancestors};
    let target = Path::new("/usr/local/libexec/jev-pilot/jev-pilot");
    let safe = DirMeta {
        uid: 0,
        mode: 0o755,
        is_link: false,
    };
    let lookup = |unsafe_dir: &'static str, meta: DirMeta| {
        move |dir: &Path| -> Option<DirMeta> {
            if dir == Path::new("/usr/local/libexec")
                || dir == Path::new("/usr/local/libexec/jev-pilot")
            {
                None
            } else if dir == Path::new(unsafe_dir) {
                Some(meta)
            } else {
                Some(safe)
            }
        }
    };
    assert_eq!(check_ancestors(target, &lookup("/nowhere", safe)), Ok(()));
    for (meta, why) in [
        (DirMeta { uid: 501, ..safe }, "is owned by uid 501"),
        (
            DirMeta {
                mode: 0o775,
                ..safe
            },
            "can be written by others (mode 775)",
        ),
        (
            DirMeta {
                mode: 0o757,
                ..safe
            },
            "can be written by others (mode 757)",
        ),
        (
            DirMeta {
                is_link: true,
                ..safe
            },
            "is a link",
        ),
    ] {
        assert_eq!(
            check_ancestors(target, &lookup("/usr/local", meta)),
            Err(format!(
                "/usr/local {why}, so a program running as you could change what jev-pilot \
                 installs under it"
            )),
        );
    }
}

/// The binary is copied into a new file that must not exist yet, made by
/// the installer and closed to everyone else, then put in place: never into
/// a file someone else could have made or still writes.
#[cfg(unix)]
#[test]
fn the_binary_is_copied_into_a_new_file_closed_to_others() {
    use jev_pilot::browser::install::copy_into_place;
    use std::os::unix::fs::PermissionsExt as _;
    let dir = std::env::temp_dir().join(format!("jev-pilot-copy-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let from = dir.join("from");
    let to = dir.join("jev-pilot");
    std::fs::write(&from, b"new binary").unwrap();
    std::fs::write(&to, b"old binary").unwrap();
    copy_into_place(&from, &to).expect("copied");
    assert_eq!(std::fs::read(&to).unwrap(), b"new binary");
    let mode = std::fs::metadata(&to).unwrap().permissions().mode();
    assert_eq!(
        mode & 0o077,
        0,
        "closed to others until its owner and mode are set"
    );
    // A file already where the copy is made is refused, not written into.
    std::fs::write(dir.join("jev-pilot.new"), b"planted").unwrap();
    assert!(copy_into_place(&from, &to).is_err());
    assert_eq!(
        std::fs::read(dir.join("jev-pilot.new")).unwrap(),
        b"planted"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The programs an elevated install runs are named by their full paths,
/// never found through PATH (or, on Windows, beside the binary).
#[test]
fn the_programs_an_install_runs_are_named_by_their_full_paths() {
    use jev_pilot::browser::install::{Tool, tool_path};
    let system32 = r"C:\Windows\System32";
    for (tool, path) in [
        (Tool::Codesign, "/usr/bin/codesign"),
        (Tool::Open, "/usr/bin/open"),
        (Tool::Dseditgroup, "/usr/sbin/dseditgroup"),
        (Tool::Groupadd, "/usr/sbin/groupadd"),
        (Tool::Plutil, "/usr/bin/plutil"),
        (Tool::Reg, r"C:\Windows\System32\reg.exe"),
    ] {
        assert_eq!(tool_path(tool, system32), PathBuf::from(path), "{tool:?}");
    }
}

/// The policy is in place only when it says user-level hosts are off: on
/// Windows, a DWORD of 0 under each browser's key; on Linux, false in each
/// browser's file; on macOS, false in each browser's managed preferences.
#[test]
fn the_policy_is_read_for_its_value_not_its_presence() {
    use jev_pilot::browser::install::{linux_policy_holds, managed_preferences, reg_dword_in};
    let on = "\r\nHKEY_LOCAL_MACHINE\\SOFTWARE\\Policies\\Google\\Chrome\r\n    \
              NativeMessagingUserLevelHosts    REG_DWORD    0x0\r\n\r\n";
    let off = on.replace("0x0", "0x1");
    assert_eq!(reg_dword_in(on, "NativeMessagingUserLevelHosts"), Some(0));
    assert_eq!(reg_dword_in(&off, "NativeMessagingUserLevelHosts"), Some(1));
    assert_eq!(reg_dword_in(on, "Other"), None);
    assert!(linux_policy_holds(
        r#"{"NativeMessagingUserLevelHosts": false}"#
    ));
    assert!(!linux_policy_holds(
        r#"{"NativeMessagingUserLevelHosts": true}"#
    ));
    assert!(!linux_policy_holds("{}"));
    assert_eq!(
        managed_preferences(),
        [PathBuf::from(
            "/Library/Managed Preferences/com.google.Chrome.plist"
        )]
    );
}

/// On Windows, an install elevated as another administrator than the one
/// signed in would pin that administrator, and find that administrator's
/// hosts: it is refused.
#[test]
fn an_install_elevated_as_another_account_is_refused() {
    use jev_pilot::browser::install::elevated_as_signed_in;
    assert_eq!(elevated_as_signed_in(r"PC\owner", r"pc\OWNER"), Ok(()));
    assert_eq!(
        elevated_as_signed_in(r"PC\admin", r"PC\owner"),
        Err(r"this runs elevated as PC\admin, not as PC\owner who is signed in: sign in as an administrator yourself and run the install from there, so the bridge is pinned to you".to_owned())
    );
}

/// Before it asks for an administrator's rights, a protected install says
/// what the policy will make Chrome say, and why.
#[test]
fn a_protected_install_says_chrome_will_be_managed_before_it_starts() {
    assert_eq!(
        jev_pilot::browser::install::MANAGED_WARNING,
        "This turns off user-level native messaging hosts with a Chrome policy, so Chrome will \
         say it is \"managed by your organization\" (on macOS, once you approve the profile). \
         Your own hosts are copied to system level first, so they keep working."
    );
}
