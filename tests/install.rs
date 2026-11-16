//! Installing the bridge, as values: where everything goes on each system,
//! and what is written there. Nothing here touches the real system: every
//! path is computed from a given home and environment, on every OS's rules.
//!
//! The install is the owner's own: it needs no administrator, and writes
//! only in the owner's folders (and, on Windows, under HKCU).

#![cfg(feature = "extension")]

use jev_pilot::browser::discover::Os;
use jev_pilot::browser::install::{InstallRecord, Layout, Step, host_manifest, plan};
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

const WINDOWS: &[(&str, &str)] = &[("LOCALAPPDATA", r"C:\Users\owner\AppData\Local")];

const EXE: &str = "/Users/owner/Downloads/jev-pilot";

fn mac() -> Layout {
    Layout::new(Os::MacOs, Path::new("/Users/owner"), &env(&[]))
}

fn linux() -> Layout {
    Layout::new(Os::Linux, Path::new("/home/owner"), &env(&[]))
}

fn windows() -> Layout {
    Layout::new(Os::Windows, Path::new(r"C:\Users\owner"), &env(WINDOWS))
}

/// Everything the install writes is in one folder of the owner's.
#[test]
fn the_install_lives_in_the_owners_own_folder() {
    assert_eq!(
        mac(),
        Layout {
            binary: "/Users/owner/Library/Application Support/jev-pilot/jev-pilot".into(),
            install_json: "/Users/owner/Library/Application Support/jev-pilot/install.json".into(),
            extension: "/Users/owner/Library/Application Support/jev-pilot/extension".into(),
            audit_log: "/Users/owner/Library/Application Support/jev-pilot/log".into(),
            host_manifest:
                "/Users/owner/Library/Application Support/jev-pilot/dev.jevpilot.bridge.json".into(),
        }
    );
    assert_eq!(
        linux(),
        Layout {
            binary: "/home/owner/.local/share/jev-pilot/jev-pilot".into(),
            install_json: "/home/owner/.local/share/jev-pilot/install.json".into(),
            extension: "/home/owner/.local/share/jev-pilot/extension".into(),
            audit_log: "/home/owner/.local/share/jev-pilot/log".into(),
            host_manifest: "/home/owner/.local/share/jev-pilot/dev.jevpilot.bridge.json".into(),
        }
    );
    assert_eq!(
        windows(),
        Layout {
            binary: r"C:\Users\owner\AppData\Local\jev-pilot\jev-pilot.exe".into(),
            install_json: r"C:\Users\owner\AppData\Local\jev-pilot\install.json".into(),
            extension: r"C:\Users\owner\AppData\Local\jev-pilot\extension".into(),
            audit_log: r"C:\Users\owner\AppData\Local\jev-pilot\log".into(),
            host_manifest: r"C:\Users\owner\AppData\Local\jev-pilot\dev.jevpilot.bridge.json"
                .into(),
        }
    );
    // Without LOCALAPPDATA, the usual place under the home.
    assert_eq!(
        Layout::new(Os::Windows, Path::new(r"C:\Users\owner"), &env(&[])).binary,
        PathBuf::from(r"C:\Users\owner\AppData\Local\jev-pilot\jev-pilot.exe")
    );
}

/// Chrome starts `path` for the one origin it allows.
#[test]
fn the_host_manifest_names_the_installed_binary_and_only_jev_pilots_extension() {
    let manifest = host_manifest(Path::new("/home/owner/.local/share/jev-pilot/jev-pilot"));
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&manifest).expect("JSON"),
        json!({
            "name": "dev.jevpilot.bridge",
            "description": "jev-pilot's bridge to its Chrome extension",
            "path": "/home/owner/.local/share/jev-pilot/jev-pilot",
            "type": "stdio",
            "allowed_origins": ["chrome-extension://eckgjcpdiehapkphgcflmcikippoegcp/"],
        })
    );
}

fn mac_expected() -> Expected {
    Expected::Macos {
        uid: 501,
        path: "/Users/owner/Library/Application Support/jev-pilot/jev-pilot".into(),
        pin: MacosPin::Cdhash([0x32; 20]),
    }
}

fn record() -> InstallRecord {
    InstallRecord {
        version: "0.4.0".into(),
        expected: mac_expected(),
    }
}

/// `install.json` says what both ends of the bridge expect of the other,
/// the extension's ID, and the version.
#[test]
fn install_json_records_what_to_expect() {
    let json = serde_json::to_value(record()).expect("JSON");
    assert_eq!(
        json,
        json!({
            "version": "0.4.0",
            "extension_id": "eckgjcpdiehapkphgcflmcikippoegcp",
            "expected": {
                "os": "macos",
                "uid": 501,
                "path": "/Users/owner/Library/Application Support/jev-pilot/jev-pilot",
                "pin": {"cdhash": "32".repeat(20)},
            },
        })
    );
    assert_eq!(
        serde_json::from_value::<InstallRecord>(json).expect("read back"),
        record()
    );
    for expected in [
        Expected::Linux {
            uid: 1000,
            path: "/home/owner/.local/share/jev-pilot/jev-pilot".into(),
            pin: LinuxPin::Sha256(Sha256Digest::from_bytes([7; 32])),
        },
        Expected::Windows {
            sid: "S-1-5-21-1-2-3-1001".into(),
            path: r"\\?\C:\Users\owner\AppData\Local\jev-pilot\jev-pilot.exe".into(),
            pin: WindowsPin::Sha256(Sha256Digest::from_bytes([7; 32])),
            session: None,
        },
    ] {
        let record = InstallRecord {
            version: "0.4.0".into(),
            expected,
        };
        let text = serde_json::to_string(&record).expect("JSON");
        assert_eq!(
            serde_json::from_str::<InstallRecord>(&text).expect("read back"),
            record
        );
    }
}

/// An install.json written before the install was the owner's alone still
/// reads: its kind and moved hosts are passed over, and a Linux pin then
/// called a development one is the same hash.
#[test]
fn an_earlier_install_json_still_reads() {
    let earlier = json!({
        "version": "0.3.0",
        "install": "development",
        "extension_id": "eckgjcpdiehapkphgcflmcikippoegcp",
        "expected": {
            "os": "linux",
            "uid": 1000,
            "path": "/home/owner/.local/share/jev-pilot/dev/jev-pilot",
            "pin": {"development_sha256": "07".repeat(32)},
        },
        "moved_hosts": [],
    });
    assert_eq!(
        serde_json::from_value::<InstallRecord>(earlier).expect("read"),
        InstallRecord {
            version: "0.3.0".into(),
            expected: Expected::Linux {
                uid: 1000,
                path: "/home/owner/.local/share/jev-pilot/dev/jev-pilot".into(),
                pin: LinuxPin::Sha256(Sha256Digest::from_bytes([7; 32])),
            },
        }
    );
}

/// An install.json for another extension is not read as jev-pilot's.
#[test]
fn install_json_for_another_extension_id_is_refused() {
    let mut json = serde_json::to_value(record()).expect("JSON");
    json["extension_id"] = "abcdefghijklmnopabcdefghijklmnop".into();
    assert!(serde_json::from_value::<InstallRecord>(json).is_err());
}

// --- policy.json ---

use jev_pilot::browser::install::{policy_for_install, with_owner_tabs, with_site};

#[test]
fn a_new_install_writes_the_default_policy() {
    assert_eq!(
        policy_for_install(None),
        json!({"sites": [], "owner_tabs": "group"})
    );
}

/// Installing again keeps the owner's sites and tab rule, and drops what an
/// earlier install wrote of its kind.
#[test]
fn an_existing_policy_is_kept() {
    let existing =
        json!({"sites": ["https://github.com"], "owner_tabs": "never", "install": "protected"});
    assert_eq!(
        policy_for_install(Some(existing)),
        json!({"sites": ["https://github.com"], "owner_tabs": "never"})
    );
    // One that is not an object is replaced by the default.
    assert_eq!(
        policy_for_install(Some(json!([1, 2]))),
        json!({"sites": [], "owner_tabs": "group"})
    );
}

#[test]
fn sites_are_added_once_and_removed() {
    let policy = json!({"sites": ["https://github.com"], "owner_tabs": "group"});
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
    let policy = json!({"sites": [], "owner_tabs": "group"});
    assert_eq!(
        with_owner_tabs(&policy, OwnerTabs::Never),
        json!({"sites": [], "owner_tabs": "never"})
    );
}

// --- where Chrome looks for the owner's hosts ---

use jev_pilot::browser::install::{
    WINDOWS_HOST_KEY, chrome_host_dirs, reg_add_host, reg_delete_host,
};

/// Each of Google Chrome's channels reads hosts from a folder in its own
/// profile folder, in the owner's home; Windows uses HKCU.
#[test]
fn chrome_reads_the_owners_hosts_from_each_channels_folder() {
    assert_eq!(
        chrome_host_dirs(Os::MacOs, Path::new("/Users/owner")),
        [
            "/Users/owner/Library/Application Support/Google/Chrome/NativeMessagingHosts",
            "/Users/owner/Library/Application Support/Google/Chrome Beta/NativeMessagingHosts",
            "/Users/owner/Library/Application Support/Google/Chrome Dev/NativeMessagingHosts",
            "/Users/owner/Library/Application Support/Google/Chrome Canary/NativeMessagingHosts",
        ]
        .map(PathBuf::from)
    );
    assert_eq!(
        chrome_host_dirs(Os::Linux, Path::new("/home/owner")),
        [
            "/home/owner/.config/google-chrome/NativeMessagingHosts",
            "/home/owner/.config/google-chrome-beta/NativeMessagingHosts",
            "/home/owner/.config/google-chrome-unstable/NativeMessagingHosts",
            "/home/owner/.config/google-chrome-canary/NativeMessagingHosts",
        ]
        .map(PathBuf::from)
    );
    assert_eq!(
        chrome_host_dirs(Os::Windows, Path::new(r"C:\Users\owner")),
        Vec::<PathBuf>::new()
    );
    assert_eq!(
        WINDOWS_HOST_KEY,
        r"SOFTWARE\Google\Chrome\NativeMessagingHosts"
    );
}

/// The registration goes through `reg.exe` with fixed argument vectors,
/// under HKCU: the key's default value names the manifest.
#[test]
fn reg_exe_is_given_fixed_argument_vectors() {
    let manifest = Path::new(r"C:\Users\owner\AppData\Local\jev-pilot\dev.jevpilot.bridge.json");
    assert_eq!(
        reg_add_host(manifest),
        [
            "add",
            r"HKCU\SOFTWARE\Google\Chrome\NativeMessagingHosts\dev.jevpilot.bridge",
            "/ve",
            "/t",
            "REG_SZ",
            "/d",
            r"C:\Users\owner\AppData\Local\jev-pilot\dev.jevpilot.bridge.json",
            "/f",
        ]
    );
    assert_eq!(
        reg_delete_host(),
        [
            "delete",
            r"HKCU\SOFTWARE\Google\Chrome\NativeMessagingHosts\dev.jevpilot.bridge",
            "/f",
        ]
    );
}

// --- the install, as a plan of steps ---

fn files_written(steps: &[Step]) -> Vec<PathBuf> {
    steps
        .iter()
        .filter_map(|s| match s {
            Step::WriteFile { path, .. }
            | Step::WritePolicy { path }
            | Step::Record { path, .. }
            | Step::CopyBinary { to: path, .. } => Some(path.clone()),
            _ => None,
        })
        .collect()
}

fn dirs_made(steps: &[Step]) -> Vec<PathBuf> {
    steps
        .iter()
        .filter_map(|s| match s {
            Step::CreateDir { path, .. } => Some(path.clone()),
            _ => None,
        })
        .collect()
}

/// On macOS: the binary copied, re-signed with the hardened runtime and
/// opened to run; the extension, its policy and the manifest written; the
/// host registered for each Chrome channel found; the log's folder made;
/// and install.json last, all in the owner's home.
#[test]
fn a_macos_install_plan() {
    let layout = mac();
    let chrome = PathBuf::from(
        "/Users/owner/Library/Application Support/Google/Chrome/NativeMessagingHosts",
    );
    let steps = plan(
        Os::MacOs,
        &layout,
        Path::new(EXE),
        std::slice::from_ref(&chrome),
    );
    let copy = steps
        .iter()
        .position(|s| matches!(s, Step::CopyBinary { .. }))
        .expect("copies the binary");
    assert_eq!(
        steps[copy],
        Step::CopyBinary {
            from: EXE.into(),
            to: layout.binary.clone(),
        }
    );
    assert_eq!(
        steps[copy + 1],
        Step::Sign {
            path: layout.binary.clone()
        }
    );
    assert_eq!(
        steps[copy + 2],
        Step::SetMode {
            path: layout.binary.clone(),
            mode: 0o755,
        }
    );
    let written = files_written(&steps);
    for path in [
        layout.extension.join("manifest.json"),
        layout.extension.join("policy.json"),
        layout.host_manifest.clone(),
        chrome.join("dev.jevpilot.bridge.json"),
    ] {
        assert!(written.contains(&path), "{} is written", path.display());
    }
    assert!(steps.contains(&Step::WriteFile {
        path: chrome.join("dev.jevpilot.bridge.json"),
        contents: host_manifest(&layout.binary).into_bytes(),
        mode: 0o644,
    }));
    assert!(steps.contains(&Step::WritePolicy {
        path: layout.extension.join("policy.json")
    }));
    assert!(steps.contains(&Step::CreateDir {
        path: layout.audit_log.clone(),
        mode: 0o700,
    }));
    assert_eq!(
        steps.last(),
        Some(&Step::Record {
            path: layout.install_json.clone(),
            binary: layout.binary.clone(),
        })
    );
    for path in written.iter().chain(&dirs_made(&steps)) {
        assert!(
            path.starts_with("/Users/owner"),
            "{} is the owner's",
            path.display()
        );
    }
    assert!(!steps.iter().any(|s| matches!(s, Step::Reg(_))));
}

/// On Linux the same, without signing.
#[test]
fn a_linux_install_plan() {
    let layout = linux();
    let chrome = PathBuf::from("/home/owner/.config/google-chrome/NativeMessagingHosts");
    let steps = plan(
        Os::Linux,
        &layout,
        Path::new(EXE),
        std::slice::from_ref(&chrome),
    );
    assert!(!steps.iter().any(|s| matches!(s, Step::Sign { .. })));
    assert!(steps.contains(&Step::SetMode {
        path: layout.binary.clone(),
        mode: 0o755,
    }));
    assert!(files_written(&steps).contains(&chrome.join("dev.jevpilot.bridge.json")));
    for path in files_written(&steps).iter().chain(&dirs_made(&steps)) {
        assert!(
            path.starts_with("/home/owner"),
            "{} is the owner's",
            path.display()
        );
    }
}

/// On Windows the host is registered under HKCU, and everything else goes
/// in the owner's local app data.
#[test]
fn a_windows_install_plan() {
    let layout = windows();
    let steps = plan(
        Os::Windows,
        &layout,
        Path::new(r"C:\Users\owner\Downloads\jev-pilot.exe"),
        &[],
    );
    assert!(steps.contains(&Step::Reg(reg_add_host(&layout.host_manifest))));
    assert!(!steps.iter().any(|s| matches!(s, Step::Sign { .. })));
    assert!(
        !steps.iter().any(|s| matches!(s, Step::Tell(_))),
        "no profile folder is needed"
    );
    for path in files_written(&steps).iter().chain(&dirs_made(&steps)) {
        // As text: a Windows path is one component elsewhere.
        let root = r"C:\Users\owner\AppData\Local\jev-pilot";
        let text = path.to_string_lossy();
        assert!(
            text == root || text.starts_with(&format!("{root}\\")),
            "{} is the owner's",
            path.display()
        );
    }
}

/// With no Chrome profile in the home, the host is registered nowhere, and
/// the install says so.
#[test]
fn an_install_with_no_chrome_profile_says_so() {
    let steps = plan(Os::MacOs, &mac(), Path::new(EXE), &[]);
    assert!(steps.contains(&Step::Tell(
        "no Google Chrome profile was found in your home, so the host is registered with none: \
         run the install again once Chrome has started"
            .into()
    )));
}

/// The cdhash a macOS pin is made from, as `codesign -d -vvv` prints it.
#[test]
fn the_cdhash_is_read_from_codesigns_report() {
    use jev_pilot::browser::install::cdhash_in;
    let report = "Executable=/Users/owner/Library/Application Support/jev-pilot/jev-pilot\n\
                  Identifier=jev_pilot-90cd7c3a4126267f\n\
                  CodeDirectory v=20500 size=50000 flags=0x10002(adhoc,runtime) hashes=1557+2 location=embedded\n\
                  CDHash=32e719c23c33eb097271b746f2a09f3127b57e5a\n";
    assert_eq!(
        cdhash_in(report),
        Some(MacosPin::from_cdhash_hex("32e719c23c33eb097271b746f2a09f3127b57e5a").unwrap())
    );
    assert_eq!(cdhash_in("code object is not signed at all\n"), None);
}

/// Uninstalling removes the registrations and what the install made, and
/// keeps the audit log.
#[test]
fn uninstalling_undoes_the_install() {
    use jev_pilot::browser::install::uninstall_plan;
    let layout = linux();
    let chrome = PathBuf::from("/home/owner/.config/google-chrome/NativeMessagingHosts");
    let steps = uninstall_plan(Os::Linux, &layout, std::slice::from_ref(&chrome));
    let removed: Vec<&Path> = steps
        .iter()
        .filter_map(|s| match s {
            Step::Remove { path } => Some(path.as_path()),
            _ => None,
        })
        .collect();
    let registration = chrome.join("dev.jevpilot.bridge.json");
    for path in [
        registration.as_path(),
        &layout.host_manifest,
        &layout.extension,
        &layout.binary,
        &layout.install_json,
    ] {
        assert!(removed.contains(&path), "{} is removed", path.display());
    }
    assert!(
        !removed.contains(&layout.audit_log.as_path()),
        "the audit log stays"
    );

    let windows = uninstall_plan(Os::Windows, &windows(), &[]);
    assert!(windows.contains(&Step::Reg(reg_delete_host())));
}

// --- status, and what is said when something is wrong ---

use jev_pilot::browser::install::{Found, Status, status_lines};

fn found() -> Found {
    Found {
        record: record(),
        layout: mac(),
    }
}

fn status(found: Option<Found>) -> Status {
    Status {
        found,
        this_version: "0.4.0".into(),
        host_answers: true,
        policy: Some(json!({"sites": ["https://github.com"], "owner_tabs": "group"})),
    }
}

#[test]
fn status_says_when_nothing_is_set_up_and_how_to_fix_it() {
    assert_eq!(
        status_lines(&status(None)).expect_err("not set up"),
        "jev-pilot's Chrome extension is not set up: run `jev-pilot extension install`"
    );
}

#[test]
fn status_reports_a_working_install() {
    let lines = status_lines(&status(Some(found()))).expect("set up");
    assert_eq!(
        lines,
        [
            "install    : version 0.4.0, in your own folders",
            "binary     : /Users/owner/Library/Application Support/jev-pilot/jev-pilot",
            "extension  : /Users/owner/Library/Application Support/jev-pilot/extension (loaded \
             unpacked in chrome://extensions, with Developer mode on)",
            "sites      : https://github.com",
            "owner tabs : group",
            "bridge     : the host answers; Chrome is running with the extension",
        ]
    );
}

#[test]
fn status_names_each_problem_with_its_fix() {
    let mut s = status(Some(found()));
    s.host_answers = false;
    s.this_version = "0.5.0".into();
    s.policy = Some(json!({"sites": [], "owner_tabs": "never"}));
    let lines = status_lines(&s).expect("set up");
    for line in [
        "this jev-pilot is 0.5.0, the installed one is 0.4.0: run the install again",
        "sites      : every site",
        "owner tabs : never",
        "Chrome is not running with the jev-pilot extension: open Chrome; if it is open, check \
         the extension is on in chrome://extensions with Developer mode on (`jev-pilot \
         extension status` says more)",
    ] {
        assert!(lines.iter().any(|l| l == line), "{line} in {lines:#?}");
    }
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

/// A run started from a copy other than the installed one says when the
/// versions differ, and goes on with the installed copy.
#[test]
fn a_run_handed_to_an_installed_copy_of_another_version_says_so() {
    use jev_pilot::browser::install::handing_over;
    assert_eq!(
        handing_over("0.5.0", "0.4.0").as_deref(),
        Some(
            "jev-pilot: this jev-pilot is 0.5.0, the installed one is 0.4.0: run the install \
             again; the run goes on with the installed copy"
        )
    );
    assert_eq!(handing_over("0.4.0", "0.4.0"), None);
}

/// The install is found where it is made; a missing install.json is no
/// install, and one that cannot be read or parsed says why.
#[test]
fn the_install_is_found_in_the_owners_folder() {
    use jev_pilot::browser::install::find_installed;
    let home = Path::new("/home/owner");
    let layout = linux();
    let text = serde_json::to_string(&InstallRecord {
        version: "0.4.0".into(),
        expected: Expected::Linux {
            uid: 1000,
            path: layout.binary.clone(),
            pin: LinuxPin::Sha256(Sha256Digest::from_bytes([7; 32])),
        },
    })
    .unwrap();
    let at = layout.install_json.clone();
    let there = |path: &Path| {
        if path == at {
            Ok(text.clone())
        } else {
            Err(std::io::ErrorKind::NotFound.into())
        }
    };
    let found = find_installed(Os::Linux, home, &env(&[]), &there)
        .expect("read")
        .expect("found");
    assert_eq!(found.layout, layout);
    assert_eq!(found.record.version, "0.4.0");

    let missing =
        |_: &Path| -> std::io::Result<String> { Err(std::io::ErrorKind::NotFound.into()) };
    assert_eq!(
        find_installed(Os::Linux, home, &env(&[]), &missing),
        Ok(None)
    );

    let unreadable =
        |_: &Path| -> std::io::Result<String> { Err(std::io::ErrorKind::PermissionDenied.into()) };
    let error = find_installed(Os::Linux, home, &env(&[]), &unreadable).expect_err("unreadable");
    assert!(
        error.starts_with(&format!("{} cannot be read", layout.install_json.display())),
        "{error}"
    );

    let garbled = |_: &Path| -> std::io::Result<String> { Ok("{".into()) };
    assert!(find_installed(Os::Linux, home, &env(&[]), &garbled).is_err());
}

/// The binary is copied into a new file that must not exist yet, closed to
/// everyone else, then put in place: a running copy is replaced, not
/// rewritten.
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
    assert_eq!(mode & 0o077, 0, "closed to others until its mode is set");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The programs an install runs are named by their full paths, never found
/// through PATH (or, on Windows, beside the binary).
#[test]
fn the_programs_an_install_runs_are_named_by_their_full_paths() {
    use jev_pilot::browser::install::{Tool, tool_path};
    let system32 = r"C:\Windows\System32";
    for (tool, path) in [
        (Tool::Codesign, "/usr/bin/codesign"),
        (Tool::Reg, r"C:\Windows\System32\reg.exe"),
    ] {
        assert_eq!(tool_path(tool, system32), PathBuf::from(path), "{tool:?}");
    }
}
