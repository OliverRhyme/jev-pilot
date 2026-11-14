//! Installing the bridge, as values: where everything goes on each system,
//! and what is written there. Nothing here touches the real system: every
//! path is computed from a given home and environment, on every OS's rules.

#![cfg(feature = "extension")]

use jev_pilot::browser::discover::Os;
use jev_pilot::browser::install::{InstallKind, InstallRecord, Layout, host_manifest};
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
        Layout::protected(Os::MacOs, &env(&[])),
        Layout {
            binary: "/usr/local/libexec/jev-pilot/jev-pilot".into(),
            install_json: "/Library/Application Support/jev-pilot/install.json".into(),
            extension: "/Library/Application Support/jev-pilot/extension".into(),
            audit_log: "/Library/Logs/jev-pilot".into(),
            host_manifest: "/Library/Application Support/jev-pilot/dev.jevpilot.bridge.json".into(),
        }
    );
    assert_eq!(
        Layout::protected(Os::Linux, &env(&[])),
        Layout {
            binary: "/usr/local/libexec/jev-pilot/jev-pilot".into(),
            install_json: "/etc/jev-pilot/install.json".into(),
            extension: "/usr/local/share/jev-pilot/extension".into(),
            audit_log: "/var/log/jev-pilot".into(),
            host_manifest: "/etc/jev-pilot/dev.jevpilot.bridge.json".into(),
        }
    );
    assert_eq!(
        Layout::protected(Os::Windows, &env(WINDOWS)),
        Layout {
            binary: r"C:\Program Files\jev-pilot\jev-pilot.exe".into(),
            install_json: r"C:\Program Files\jev-pilot\install.json".into(),
            extension: r"C:\Program Files\jev-pilot\extension".into(),
            audit_log: r"C:\ProgramData\jev-pilot\audit".into(),
            host_manifest: r"C:\Program Files\jev-pilot\dev.jevpilot.bridge.json".into(),
        }
    );
    // Without the variables, Windows's own defaults.
    assert_eq!(
        Layout::protected(Os::Windows, &env(&[])).binary,
        PathBuf::from(r"C:\Program Files\jev-pilot\jev-pilot.exe")
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

/// Chrome's channels share one system folder, and Brave reads Chrome's.
#[test]
fn the_host_is_registered_in_each_browser_familys_system_folder() {
    assert_eq!(
        system_host_dirs(Os::MacOs),
        [
            PathBuf::from(CHROME_MAC),
            "/Library/Application Support/Chromium/NativeMessagingHosts".into(),
            "/Library/Microsoft/Edge/NativeMessagingHosts".into(),
        ]
    );
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
    let mac = user_host_dirs(Os::MacOs, Path::new("/Users/o"));
    let support = "/Users/o/Library/Application Support";
    for (user, system) in [
        ("Google/Chrome", CHROME_MAC),
        ("Google/Chrome Beta", CHROME_MAC),
        ("Google/Chrome Dev", CHROME_MAC),
        ("Google/Chrome Canary", CHROME_MAC),
        (
            "Chromium",
            "/Library/Application Support/Chromium/NativeMessagingHosts",
        ),
        (
            "Microsoft Edge",
            "/Library/Microsoft/Edge/NativeMessagingHosts",
        ),
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

#[test]
fn windows_registers_the_host_under_each_familys_key() {
    assert_eq!(
        windows_host_keys(),
        [
            r"SOFTWARE\Google\Chrome\NativeMessagingHosts",
            r"SOFTWARE\Chromium\NativeMessagingHosts",
            r"SOFTWARE\Microsoft\Edge\NativeMessagingHosts",
        ]
    );
    assert_eq!(
        windows_policy_keys(),
        [
            r"SOFTWARE\Policies\Google\Chrome",
            r"SOFTWARE\Policies\Chromium",
            r"SOFTWARE\Policies\Microsoft\Edge",
            r"SOFTWARE\Policies\BraveSoftware\Brave",
        ]
    );
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
/// Mac means a configuration profile: one payload per browser's preference
/// domain. Chrome's channels all read `com.google.Chrome`.
#[test]
fn macos_turns_off_user_level_hosts_with_a_configuration_profile() {
    let profile = mobileconfig();
    assert!(profile.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist"));
    for domain in [
        "com.google.Chrome",
        "org.chromium.Chromium",
        "com.microsoft.Edge",
        "com.brave.Browser",
    ] {
        assert!(
            profile.contains(&format!(
                "<key>PayloadType</key>\n\t\t\t<string>{domain}</string>"
            )),
            "a payload for {domain}"
        );
    }
    assert_eq!(
        profile
            .matches("<key>NativeMessagingUserLevelHosts</key>\n\t\t\t<false/>")
            .count(),
        4
    );
    assert!(profile.contains("<key>PayloadType</key>\n\t<string>Configuration</string>"));
    assert!(profile.contains("<string>dev.jevpilot.policy</string>"));
    // The same every time, so installing again changes nothing.
    assert_eq!(profile, mobileconfig());
}

// --- the install, as a plan of steps ---

use jev_pilot::browser::install::{Owner, Step, UserHosts, plan};

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
    let layout = Layout::protected(Os::Linux, &env(&[]));
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
    let layout = Layout::protected(Os::MacOs, &env(&[]));
    let steps = plan(
        Os::MacOs,
        InstallKind::Protected,
        &layout,
        Path::new(EXE),
        &UserHosts::Files(vec![]),
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
    let layout = Layout::protected(Os::Windows, &env(WINDOWS));
    let steps = plan(
        Os::Windows,
        InstallKind::Protected,
        &layout,
        Path::new(r"C:\Users\o\.jev-pilot\bin\jev-pilot.exe"),
        &UserHosts::Registry(vec![(
            r"SOFTWARE\Google\Chrome\NativeMessagingHosts".into(),
            "com.example.x".into(),
            PathBuf::from(r"C:\Users\o\x.json"),
        )]),
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
        r"C:\Users\o\x.json".into(),
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
    );
    assert!(!steps.contains(&Step::EnsureGroup));
    assert!(
        !steps
            .iter()
            .any(|s| matches!(s, Step::SetOwner { .. } | Step::Sign { .. }))
    );
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
    let layout = Layout::protected(Os::Linux, &env(&[]));
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
    let steps = uninstall_plan(Os::Linux, &layout, &record);
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
        &Layout::protected(Os::MacOs, &env(&[])),
        &InstallRecord {
            expected: mac_expected(),
            ..record.clone()
        },
    );
    assert!(
        mac.iter()
            .any(|s| matches!(s, Step::Tell(words) if words.contains("profile")))
    );

    let windows = uninstall_plan(
        Os::Windows,
        &Layout::protected(Os::Windows, &env(WINDOWS)),
        &InstallRecord {
            moved_hosts: vec![PathBuf::from(
                r"HKLM\SOFTWARE\Google\Chrome\NativeMessagingHosts\com.example.x",
            )],
            ..record
        },
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
    let layout = Layout::protected(Os::MacOs, &env(&[]));
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
    );
    assert!(
        steps.contains(&Step::Tell(
            "no Chrome, Chromium, Edge or Brave profile was found in your home, so the host is \
         registered with none: run the install again once one has started"
                .into()
        ))
    );
}

/// Installing again does the same, in place: the same steps, a policy that
/// keeps the owner's sites, a profile with the same UUIDs.
#[test]
fn installing_again_plans_the_same_install() {
    for os in [Os::MacOs, Os::Linux, Os::Windows] {
        let layout = Layout::protected(os, &env(WINDOWS));
        let hosts = UserHosts::Files(vec![]);
        assert_eq!(
            plan(os, InstallKind::Protected, &layout, Path::new(EXE), &hosts),
            plan(os, InstallKind::Protected, &layout, Path::new(EXE), &hosts),
            "{os:?}"
        );
    }
    let once = policy_for_install(None, InstallKind::Protected);
    let edited = with_site(&once, "https://github.com", true);
    assert_eq!(
        policy_for_install(Some(edited.clone()), InstallKind::Protected),
        edited
    );
}
