//! Which program is at the other end of the bridge's socket or pipe.
//!
//! `decide` is pure, so every OS's rules are tested here on every OS, from
//! plain fact values. Fact gathering is tested against this test process
//! itself, over an in-process socket or pipe.

#![cfg(feature = "extension")]

use std::path::PathBuf;

use jev_pilot::peer::{
    Check, Code, Expected, Facts, LinuxPin, Reason, Sha256Digest, Verdict, WindowsPin, decide,
    recheck,
};

const UID: u32 = 501;
const GROUP: u32 = 977;
const USER_GROUP: u32 = 20;
const PID: u32 = 4242;
const SID: &str = "S-1-5-21-1004336348-1177238915-682003330-1001";
const SESSION: u32 = 1;
const CDHASH_REQUIREMENT: &str = r#"cdhash H"32e719c23c33eb097271b746f2a09f3127b57e5a""#;
const TEAM_REQUIREMENT: &str = r#"anchor apple generic and identifier "jev-pilot" and certificate leaf[subject.OU] = "ABCDE12345""#;
const SIGNER: &str = "CN=Oliver Rhyme, O=Oliver Rhyme, C=PH";

fn digest(byte: u8) -> Sha256Digest {
    Sha256Digest::from_bytes([byte; 32])
}

fn refused(check: Check, reason: Reason) -> Verdict {
    Verdict::refuse(check, reason)
}

// ---- macOS ----

fn mac_path() -> PathBuf {
    PathBuf::from("/usr/local/libexec/jev-pilot/jev-pilot")
}

fn mac_expected(requirement: &str) -> Expected {
    Expected::Macos {
        uid: UID,
        path: mac_path(),
        requirement: requirement.to_owned(),
    }
}

fn mac_facts(requirement: &str, satisfied: bool) -> Facts {
    Facts {
        uid: Some(UID),
        pid: Some(PID),
        path: Some(mac_path()),
        token: Some([0, UID, USER_GROUP, UID, USER_GROUP, PID, 100_003, 7]),
        code: Some(Code::Requirement {
            requirement: requirement.to_owned(),
            satisfied,
        }),
        ..Facts::default()
    }
}

#[test]
fn macos_accepts_a_peer_that_satisfies_the_pinned_cdhash() {
    let verdict = decide(
        &mac_facts(CDHASH_REQUIREMENT, true),
        &mac_expected(CDHASH_REQUIREMENT),
    );
    assert_eq!(verdict, Verdict::Accept);
}

#[test]
fn macos_accepts_a_peer_that_satisfies_the_pinned_developer_id() {
    let verdict = decide(
        &mac_facts(TEAM_REQUIREMENT, true),
        &mac_expected(TEAM_REQUIREMENT),
    );
    assert_eq!(verdict, Verdict::Accept);
}

#[test]
fn macos_refuses_a_peer_that_does_not_satisfy_the_requirement() {
    let verdict = decide(
        &mac_facts(CDHASH_REQUIREMENT, false),
        &mac_expected(CDHASH_REQUIREMENT),
    );
    assert_eq!(verdict, refused(Check::Requirement, Reason::Mismatch));
}

#[test]
fn macos_refuses_a_satisfied_requirement_that_is_not_the_pinned_one() {
    let verdict = decide(
        &mac_facts(
            r#"cdhash H"0000000000000000000000000000000000000000""#,
            true,
        ),
        &mac_expected(CDHASH_REQUIREMENT),
    );
    assert_eq!(verdict, refused(Check::Requirement, Reason::Mismatch));
}

#[test]
fn macos_refuses_another_uid() {
    let facts = Facts {
        uid: Some(UID + 1),
        ..mac_facts(CDHASH_REQUIREMENT, true)
    };
    let verdict = decide(&facts, &mac_expected(CDHASH_REQUIREMENT));
    assert_eq!(verdict, refused(Check::Uid, Reason::Mismatch));
}

#[test]
fn macos_refuses_a_copy_outside_the_installed_path() {
    let facts = Facts {
        path: Some(PathBuf::from("/Users/someone/Downloads/jev-pilot")),
        ..mac_facts(CDHASH_REQUIREMENT, true)
    };
    let verdict = decide(&facts, &mac_expected(CDHASH_REQUIREMENT));
    assert_eq!(verdict, refused(Check::Path, Reason::Mismatch));
}

#[test]
fn macos_recheck_accepts_the_same_token_after_the_first_frame() {
    let first = mac_facts(CDHASH_REQUIREMENT, true);
    let again = first.clone();
    let verdict = recheck(&first, &again, &mac_expected(CDHASH_REQUIREMENT));
    assert_eq!(verdict, Verdict::Accept);
}

#[test]
fn macos_recheck_refuses_a_token_that_changed_after_the_first_frame() {
    // A forked child, or an exec in the same pid, writes the frame: the pid
    // version in the token moves even though the code still passes.
    let first = mac_facts(CDHASH_REQUIREMENT, true);
    let again = Facts {
        token: Some([0, UID, USER_GROUP, UID, USER_GROUP, PID, 100_003, 8]),
        ..first.clone()
    };
    let verdict = recheck(&first, &again, &mac_expected(CDHASH_REQUIREMENT));
    assert_eq!(verdict, refused(Check::Token, Reason::Mismatch));
}

#[test]
fn macos_recheck_refuses_when_either_token_is_missing() {
    let first = mac_facts(CDHASH_REQUIREMENT, true);
    let again = Facts {
        token: None,
        ..first.clone()
    };
    let expected = mac_expected(CDHASH_REQUIREMENT);
    assert_eq!(
        recheck(&first, &again, &expected),
        refused(Check::Token, Reason::Missing)
    );
    assert_eq!(
        recheck(&again, &first, &expected),
        refused(Check::Token, Reason::Missing)
    );
}

#[test]
fn macos_recheck_still_decides_on_the_new_facts() {
    let first = mac_facts(CDHASH_REQUIREMENT, true);
    let again = mac_facts(CDHASH_REQUIREMENT, false);
    let verdict = recheck(&first, &again, &mac_expected(CDHASH_REQUIREMENT));
    assert_eq!(verdict, refused(Check::Requirement, Reason::Mismatch));
}

// ---- Linux ----

fn linux_path() -> PathBuf {
    PathBuf::from("/usr/local/libexec/jev-pilot/jev-pilot")
}

fn linux_expected() -> Expected {
    Expected::Linux {
        uid: UID,
        path: linux_path(),
        pin: LinuxPin::Group(GROUP),
    }
}

fn linux_facts() -> Facts {
    // A setgid peer is non-dumpable, so its executable cannot be read and
    // there is no path among the facts.
    Facts {
        uid: Some(UID),
        pid: Some(PID),
        egid: Some(GROUP),
        ..Facts::default()
    }
}

fn linux_dev_expected() -> Expected {
    Expected::Linux {
        uid: UID,
        path: PathBuf::from("/home/oliver/.jev-pilot/bin/jev-pilot"),
        pin: LinuxPin::DevelopmentSha256(digest(7)),
    }
}

fn linux_dev_facts(hash: Sha256Digest) -> Facts {
    Facts {
        uid: Some(UID),
        pid: Some(PID),
        egid: Some(USER_GROUP),
        path: Some(PathBuf::from("/home/oliver/.jev-pilot/bin/jev-pilot")),
        code: Some(Code::ImageSha256(hash)),
        ..Facts::default()
    }
}

#[test]
fn linux_accepts_the_right_uid_with_the_jev_pilot_egid() {
    assert_eq!(decide(&linux_facts(), &linux_expected()), Verdict::Accept);
}

#[test]
fn linux_refuses_the_right_uid_with_the_users_own_gid() {
    let facts = Facts {
        egid: Some(USER_GROUP),
        ..linux_facts()
    };
    assert_eq!(
        decide(&facts, &linux_expected()),
        refused(Check::Group, Reason::Mismatch)
    );
}

#[test]
fn linux_refuses_another_uid() {
    let facts = Facts {
        uid: Some(0),
        ..linux_facts()
    };
    assert_eq!(
        decide(&facts, &linux_expected()),
        refused(Check::Uid, Reason::Mismatch)
    );
}

#[test]
fn linux_accepts_a_development_install_whose_hash_matches() {
    assert_eq!(
        decide(&linux_dev_facts(digest(7)), &linux_dev_expected()),
        Verdict::Accept
    );
}

#[test]
fn linux_refuses_a_development_install_whose_hash_differs() {
    assert_eq!(
        decide(&linux_dev_facts(digest(8)), &linux_dev_expected()),
        refused(Check::ImageSha256, Reason::Mismatch)
    );
}

#[test]
fn linux_refuses_a_development_install_from_another_path() {
    let facts = Facts {
        path: Some(PathBuf::from("/tmp/jev-pilot")),
        ..linux_dev_facts(digest(7))
    };
    assert_eq!(
        decide(&facts, &linux_dev_expected()),
        refused(Check::Path, Reason::Mismatch)
    );
}

#[test]
fn linux_recheck_refuses_another_pid() {
    let first = linux_facts();
    let again = Facts {
        pid: Some(PID + 1),
        ..linux_facts()
    };
    assert_eq!(
        recheck(&first, &again, &linux_expected()),
        refused(Check::Pid, Reason::Mismatch)
    );
    assert_eq!(recheck(&first, &first, &linux_expected()), Verdict::Accept);
}

// ---- Windows ----

fn windows_path() -> PathBuf {
    PathBuf::from(r"C:\Program Files\jev-pilot\jev-pilot.exe")
}

fn windows_expected(pin: WindowsPin) -> Expected {
    Expected::Windows {
        sid: SID.to_owned(),
        path: windows_path(),
        pin,
        session: Some(SESSION),
    }
}

fn windows_facts(code: Code) -> Facts {
    Facts {
        sid: Some(SID.to_owned()),
        pid: Some(PID),
        session: Some(SESSION),
        path: Some(windows_path()),
        code: Some(code),
        ..Facts::default()
    }
}

fn hash_pin() -> WindowsPin {
    WindowsPin::Sha256(digest(1))
}

fn signer_pin() -> WindowsPin {
    WindowsPin::Signer(SIGNER.to_owned())
}

#[test]
fn windows_accepts_a_matching_image_hash() {
    let verdict = decide(
        &windows_facts(Code::ImageSha256(digest(1))),
        &windows_expected(hash_pin()),
    );
    assert_eq!(verdict, Verdict::Accept);
}

#[test]
fn windows_accepts_a_matching_signer() {
    let verdict = decide(
        &windows_facts(Code::Signer {
            subject: SIGNER.to_owned(),
        }),
        &windows_expected(signer_pin()),
    );
    assert_eq!(verdict, Verdict::Accept);
}

#[test]
fn windows_refuses_another_session() {
    let facts = Facts {
        session: Some(SESSION + 1),
        ..windows_facts(Code::ImageSha256(digest(1)))
    };
    assert_eq!(
        decide(&facts, &windows_expected(hash_pin())),
        refused(Check::Session, Reason::Mismatch)
    );
}

#[test]
fn windows_refuses_a_path_outside_program_files() {
    let facts = Facts {
        path: Some(PathBuf::from(
            r"C:\Users\oliver\AppData\Local\Temp\jev-pilot.exe",
        )),
        ..windows_facts(Code::ImageSha256(digest(1)))
    };
    assert_eq!(
        decide(&facts, &windows_expected(hash_pin())),
        refused(Check::Path, Reason::Mismatch)
    );
}

#[test]
fn windows_refuses_a_hash_mismatch() {
    assert_eq!(
        decide(
            &windows_facts(Code::ImageSha256(digest(2))),
            &windows_expected(hash_pin())
        ),
        refused(Check::ImageSha256, Reason::Mismatch)
    );
}

#[test]
fn windows_refuses_a_signer_mismatch() {
    let facts = windows_facts(Code::Signer {
        subject: "CN=Someone Else".to_owned(),
    });
    assert_eq!(
        decide(&facts, &windows_expected(signer_pin())),
        refused(Check::Signer, Reason::Mismatch)
    );
}

#[test]
fn windows_refuses_a_hash_when_a_signer_is_pinned() {
    assert_eq!(
        decide(
            &windows_facts(Code::ImageSha256(digest(1))),
            &windows_expected(signer_pin())
        ),
        refused(Check::Signer, Reason::Missing)
    );
}

#[test]
fn windows_refuses_another_sid() {
    let facts = Facts {
        sid: Some("S-1-5-18".to_owned()),
        ..windows_facts(Code::ImageSha256(digest(1)))
    };
    assert_eq!(
        decide(&facts, &windows_expected(hash_pin())),
        refused(Check::Sid, Reason::Mismatch)
    );
}

#[test]
fn windows_refuses_when_its_own_session_was_never_filled_in() {
    let expected = Expected::Windows {
        sid: SID.to_owned(),
        path: windows_path(),
        pin: hash_pin(),
        session: None,
    };
    assert_eq!(
        decide(&windows_facts(Code::ImageSha256(digest(1))), &expected),
        refused(Check::Session, Reason::Missing)
    );
}

// ---- everywhere: a missing fact is refused, never assumed ----

fn without(facts: &Facts, check: Check) -> Facts {
    let mut facts = facts.clone();
    match check {
        Check::Uid => facts.uid = None,
        Check::Sid => facts.sid = None,
        Check::Pid => facts.pid = None,
        Check::Session => facts.session = None,
        Check::Group => facts.egid = None,
        Check::Path => facts.path = None,
        Check::Requirement | Check::ImageSha256 | Check::Signer => facts.code = None,
        _ => unreachable!("{check:?} is not a fact"),
    }
    facts
}

#[test]
fn every_missing_fact_is_refused_and_named() {
    let cases: Vec<(Facts, Expected, Vec<Check>)> = vec![
        (
            mac_facts(CDHASH_REQUIREMENT, true),
            mac_expected(CDHASH_REQUIREMENT),
            vec![Check::Uid, Check::Pid, Check::Path, Check::Requirement],
        ),
        (
            linux_facts(),
            linux_expected(),
            vec![Check::Uid, Check::Pid, Check::Group],
        ),
        (
            linux_dev_facts(digest(7)),
            linux_dev_expected(),
            vec![Check::Uid, Check::Pid, Check::Path, Check::ImageSha256],
        ),
        (
            windows_facts(Code::ImageSha256(digest(1))),
            windows_expected(hash_pin()),
            vec![
                Check::Sid,
                Check::Pid,
                Check::Session,
                Check::Path,
                Check::ImageSha256,
            ],
        ),
        (
            windows_facts(Code::Signer {
                subject: SIGNER.to_owned(),
            }),
            windows_expected(signer_pin()),
            vec![Check::Signer],
        ),
    ];
    for (facts, expected, checks) in cases {
        assert_eq!(decide(&facts, &expected), Verdict::Accept, "{expected:?}");
        for check in checks {
            assert_eq!(
                decide(&without(&facts, check), &expected),
                refused(check, Reason::Missing),
                "{check:?} missing against {expected:?}",
            );
        }
    }
}

#[test]
fn no_facts_at_all_is_refused_everywhere() {
    for expected in [
        mac_expected(CDHASH_REQUIREMENT),
        linux_expected(),
        linux_dev_expected(),
        windows_expected(hash_pin()),
    ] {
        assert!(
            matches!(decide(&Facts::default(), &expected), Verdict::Refuse(_)),
            "{expected:?}"
        );
    }
}

#[test]
fn a_refusal_names_the_failed_check() {
    let verdict = decide(
        &Facts {
            egid: Some(USER_GROUP),
            ..linux_facts()
        },
        &linux_expected(),
    );
    let Verdict::Refuse(refusal) = verdict else {
        panic!("accepted");
    };
    assert_eq!(refusal.check, Check::Group);
    assert_eq!(refusal.to_string(), "group: not the expected value");
    assert_eq!(
        Verdict::refuse(Check::Requirement, Reason::Missing).to_string(),
        "refused: code requirement: not reported by the OS"
    );
}

// ---- what install.json pins ----

#[test]
fn expected_round_trips_through_json() {
    let expected = mac_expected(CDHASH_REQUIREMENT);
    let json = serde_json::to_value(&expected).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "os": "macos",
            "uid": 501,
            "path": "/usr/local/libexec/jev-pilot/jev-pilot",
            "requirement": CDHASH_REQUIREMENT,
        })
    );
    assert_eq!(serde_json::from_value::<Expected>(json).unwrap(), expected);

    let dev = linux_dev_expected();
    let json = serde_json::to_value(&dev).unwrap();
    assert_eq!(
        json["pin"],
        serde_json::json!({ "development_sha256": "07".repeat(32) })
    );
    assert_eq!(serde_json::from_value::<Expected>(json).unwrap(), dev);
}

#[test]
fn a_windows_session_is_never_pinned() {
    // A session belongs to one logon, so it is filled in at run time.
    let json = serde_json::to_value(windows_expected(signer_pin())).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "os": "windows",
            "sid": SID,
            "path": r"C:\Program Files\jev-pilot\jev-pilot.exe",
            "pin": { "signer": SIGNER },
        })
    );
    let read: Expected = serde_json::from_value(json).unwrap();
    assert_eq!(read, windows_expected(signer_pin()).with_session(None));
}

#[test]
fn a_digest_reads_hex_in_either_case_and_writes_lowercase() {
    let upper = "AB".repeat(32);
    let parsed: Sha256Digest = upper.parse().unwrap();
    assert_eq!(parsed, Sha256Digest::from_bytes([0xab; 32]));
    assert_eq!(parsed.to_string(), "ab".repeat(32));
    assert!("ab".repeat(31).parse::<Sha256Digest>().is_err());
    assert!("zz".repeat(32).parse::<Sha256Digest>().is_err());
}
