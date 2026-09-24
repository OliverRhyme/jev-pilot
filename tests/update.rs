//! Updating an installed copy. Pure: nothing here downloads or runs anything.

use jev_pilot::update::{Plan, is_newer, plan};
use std::path::Path;

/// A copy the installer put in place is updated by running the installer.
#[test]
fn an_installed_copy_is_updated_by_its_installer() {
    let plan = plan(
        Path::new("/home/ana/.jev-pilot/bin/jev-pilot"),
        Path::new("/home/ana"),
        false,
    );

    let Plan::Installer { program, args } = plan else {
        panic!("expected the installer, got {plan:?}");
    };
    assert_eq!(program, "sh");
    assert!(
        args.join(" ").contains("jev-pilot-installer.sh"),
        "{args:?}"
    );
}

/// On Windows the installer is the PowerShell one.
#[test]
fn a_windows_copy_is_updated_by_the_powershell_installer() {
    let plan = plan(
        Path::new("C:/Users/ana/.jev-pilot/bin/jev-pilot.exe"),
        Path::new("C:/Users/ana"),
        true,
    );

    let Plan::Installer { program, args } = plan else {
        panic!("expected the installer, got {plan:?}");
    };
    assert_eq!(program, "powershell");
    assert!(
        args.join(" ").contains("jev-pilot-installer.ps1"),
        "{args:?}"
    );
}

/// A copy built from source is not replaced by a download beside it: that
/// would leave two on the path, and whichever comes first would win.
#[test]
fn a_copy_built_from_source_is_pointed_at_cargo() {
    let plan = plan(
        Path::new("/home/ana/.cargo/bin/jev-pilot"),
        Path::new("/home/ana"),
        false,
    );

    let Plan::FromSource { hint } = plan else {
        panic!("expected a hint, got {plan:?}");
    };
    assert!(hint.contains("cargo install"), "{hint}");
}

/// A release tag is newer when its version is higher, with or without its `v`.
#[test]
fn a_release_is_newer_only_when_its_version_is_higher() {
    assert!(is_newer("0.1.2", "v0.1.3"));
    assert!(is_newer("0.1.9", "0.2.0"));
    assert!(is_newer("0.9.0", "v0.10.0"));
    assert!(!is_newer("0.1.2", "v0.1.2"));
    assert!(!is_newer("0.2.0", "v0.1.9"));
    assert!(!is_newer("0.1.2", "not a version"));
}
