//! The jev-pilot Chrome extension as the binary carries it: every file of
//! `extension/` but its tests, unchanged, and agreeing with the engine and
//! the bridge on what they must agree on.

#![cfg(feature = "browser")]

use jev_pilot::browser::{bridge, input, packaged};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

fn folder() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("extension")
}

/// Every file under `dir`, by its path from `root` with `/` between parts.
fn files_under(root: &Path, dir: &Path, found: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).expect("readable") {
        let path = entry.expect("an entry").path();
        if path.is_dir() {
            files_under(root, &path, found);
        } else {
            let relative = path.strip_prefix(root).expect("under the root");
            found.push(
                relative
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/"),
            );
        }
    }
}

fn packaged(path: &str) -> &'static [u8] {
    packaged::FILES
        .iter()
        .find(|(name, _)| *name == path)
        .map_or_else(|| panic!("{path} is not packaged"), |(_, bytes)| *bytes)
}

/// The binary carries every file of the extension but its tests, exactly as
/// it is on disk, and nothing else.
#[test]
fn every_file_of_the_extension_but_its_tests_is_packaged() {
    let root = folder();
    let mut on_disk = Vec::new();
    files_under(&root, &root, &mut on_disk);
    let mut wanted: Vec<String> = on_disk
        .into_iter()
        .filter(|path| !path.starts_with("test/"))
        .collect();
    wanted.sort();
    let mut carried: Vec<String> = packaged::FILES
        .iter()
        .map(|(name, _)| (*name).to_owned())
        .collect();
    carried.sort();
    assert_eq!(carried, wanted);
    for name in &wanted {
        assert_eq!(
            packaged(name),
            std::fs::read(root.join(name)).expect("readable"),
            "{name} is packaged as it is on disk"
        );
    }
}

/// The extension lets a page script through only when it is byte for byte
/// one of the engine's, so its list must be the engine's list.
#[test]
fn the_extensions_scripts_are_the_engines() {
    let scripts: Vec<String> =
        serde_json::from_slice(packaged("scripts.json")).expect("a JSON list of scripts");
    assert_eq!(scripts, input::PAGE_SCRIPTS);
}

/// The manifest pins the ID the host allows, matches the crate's version, and
/// asks for only what the extension uses.
#[test]
fn the_manifest_pins_the_id_and_asks_for_only_what_is_used() {
    let manifest: Value = serde_json::from_slice(packaged("manifest.json")).expect("JSON");
    assert_eq!(manifest["manifest_version"], 3);
    assert_eq!(manifest["name"], "jev-pilot");
    assert_eq!(manifest["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(manifest["key"], bridge::EXTENSION_KEY);
    assert_eq!(
        manifest["permissions"],
        json!([
            "debugger",
            "nativeMessaging",
            "tabs",
            "tabGroups",
            "webNavigation",
            "storage",
            "alarms",
            "notifications"
        ])
    );
    assert!(manifest.get("host_permissions").is_none());
    assert!(manifest.get("optional_host_permissions").is_none());
    assert!(manifest.get("content_scripts").is_none());
}

/// Out of the box the policy allows any site, and an owner's tab only when
/// it is in the jev-pilot group.
#[test]
fn the_default_policy_allows_any_site_and_only_grouped_owner_tabs() {
    let policy: Value = serde_json::from_slice(packaged("policy.json")).expect("JSON");
    assert_eq!(policy, json!({"sites": [], "owner_tabs": "group"}));
}
