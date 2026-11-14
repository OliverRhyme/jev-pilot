//! Which program is at the other end of the bridge's socket or pipe.
//!
//! `decide` is pure, so every OS's rules are tested here on every OS, from
//! plain fact values. Fact gathering is tested against this test process
//! itself, over an in-process socket or pipe.

#![cfg(feature = "extension")]

use std::path::PathBuf;

use jev_pilot::peer::{
    Check, Code, Expected, Facts, LinuxPin, MacosPin, Reason, Sha256Digest, Verdict, WindowsPin,
    decide, recheck,
};

const UID: u32 = 501;
const GROUP: u32 = 977;
const USER_GROUP: u32 = 20;
const PID: u32 = 4242;
const SID: &str = "S-1-5-21-1004336348-1177238915-682003330-1001";
const SESSION: u32 = 1;
const CDHASH: [u8; 20] = [
    0x32, 0xe7, 0x19, 0xc2, 0x3c, 0x33, 0xeb, 0x09, 0x72, 0x71, 0xb7, 0x46, 0xf2, 0xa0, 0x9f, 0x31,
    0x27, 0xb5, 0x7e, 0x5a,
];
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

fn cdhash_pin() -> MacosPin {
    MacosPin::Cdhash(CDHASH)
}

fn team_pin() -> MacosPin {
    MacosPin::developer_id("dev.jevpilot.jev-pilot", "ABCDE12345").unwrap()
}

fn mac_expected(pin: MacosPin) -> Expected {
    Expected::Macos {
        uid: UID,
        path: mac_path(),
        pin,
    }
}

fn mac_facts(pin: MacosPin, satisfied: bool) -> Facts {
    Facts {
        uid: Some(UID),
        pid: Some(PID),
        path: Some(mac_path()),
        token: Some([0, UID, USER_GROUP, UID, USER_GROUP, PID, 100_003, 7]),
        code: Some(Code::Requirement { pin, satisfied }),
        ..Facts::default()
    }
}

#[test]
fn macos_accepts_a_peer_that_satisfies_the_pinned_cdhash() {
    let verdict = decide(&mac_facts(cdhash_pin(), true), &mac_expected(cdhash_pin()));
    assert_eq!(verdict, Verdict::Accept);
}

#[test]
fn macos_accepts_a_peer_that_satisfies_the_pinned_developer_id() {
    let verdict = decide(&mac_facts(team_pin(), true), &mac_expected(team_pin()));
    assert_eq!(verdict, Verdict::Accept);
}

#[test]
fn macos_refuses_a_peer_that_does_not_satisfy_the_requirement() {
    let verdict = decide(&mac_facts(cdhash_pin(), false), &mac_expected(cdhash_pin()));
    assert_eq!(verdict, refused(Check::Requirement, Reason::Mismatch));
}

#[test]
fn macos_refuses_a_satisfied_requirement_that_is_not_the_pinned_one() {
    let verdict = decide(
        &mac_facts(MacosPin::Cdhash([0; 20]), true),
        &mac_expected(cdhash_pin()),
    );
    assert_eq!(verdict, refused(Check::Requirement, Reason::Mismatch));
}

#[test]
fn macos_refuses_another_uid() {
    let facts = Facts {
        uid: Some(UID + 1),
        ..mac_facts(cdhash_pin(), true)
    };
    let verdict = decide(&facts, &mac_expected(cdhash_pin()));
    assert_eq!(verdict, refused(Check::Uid, Reason::Mismatch));
}

#[test]
fn macos_refuses_a_copy_outside_the_installed_path() {
    let facts = Facts {
        path: Some(PathBuf::from("/Users/someone/Downloads/jev-pilot")),
        ..mac_facts(cdhash_pin(), true)
    };
    let verdict = decide(&facts, &mac_expected(cdhash_pin()));
    assert_eq!(verdict, refused(Check::Path, Reason::Mismatch));
}

#[test]
fn macos_recheck_accepts_the_same_token_after_the_first_frame() {
    let first = mac_facts(cdhash_pin(), true);
    let again = first.clone();
    let verdict = recheck(&first, &again, &mac_expected(cdhash_pin()));
    assert_eq!(verdict, Verdict::Accept);
}

#[test]
fn macos_recheck_refuses_a_token_that_changed_after_the_first_frame() {
    // A forked child, or an exec in the same pid, writes the frame: the pid
    // version in the token moves even though the code still passes.
    let first = mac_facts(cdhash_pin(), true);
    let again = Facts {
        token: Some([0, UID, USER_GROUP, UID, USER_GROUP, PID, 100_003, 8]),
        ..first.clone()
    };
    let verdict = recheck(&first, &again, &mac_expected(cdhash_pin()));
    assert_eq!(verdict, refused(Check::Token, Reason::Mismatch));
}

#[test]
fn macos_recheck_refuses_when_either_token_is_missing() {
    let first = mac_facts(cdhash_pin(), true);
    let again = Facts {
        token: None,
        ..first.clone()
    };
    let expected = mac_expected(cdhash_pin());
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
fn recheck_refuses_first_facts_that_never_passed() {
    // The same process both times, but the first read never passed: it
    // must not be let through by a second read that does.
    let first = mac_facts(cdhash_pin(), false);
    let again = Facts {
        code: mac_facts(cdhash_pin(), true).code,
        ..first.clone()
    };
    let verdict = recheck(&first, &again, &mac_expected(cdhash_pin()));
    assert_eq!(verdict, refused(Check::Requirement, Reason::Mismatch));
}

#[test]
fn macos_recheck_still_decides_on_the_new_facts() {
    let first = mac_facts(cdhash_pin(), true);
    let again = mac_facts(cdhash_pin(), false);
    let verdict = recheck(&first, &again, &mac_expected(cdhash_pin()));
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
            mac_facts(cdhash_pin(), true),
            mac_expected(cdhash_pin()),
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
        mac_expected(cdhash_pin()),
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
fn a_macos_pin_is_rendered_as_a_code_requirement() {
    assert_eq!(
        cdhash_pin().requirement(),
        r#"cdhash H"32e719c23c33eb097271b746f2a09f3127b57e5a""#
    );
    assert_eq!(
        team_pin().requirement(),
        r#"anchor apple generic and identifier "dev.jevpilot.jev-pilot" and certificate leaf[subject.OU] = "ABCDE12345""#
    );
    assert_eq!(
        MacosPin::from_cdhash_hex("32E719C23C33EB097271B746F2A09F3127B57E5A").unwrap(),
        cdhash_pin()
    );
}

#[test]
fn a_developer_id_pin_takes_only_a_team_id_and_a_reverse_dns_identifier() {
    for (identifier, team) in [
        ("dev.jevpilot.jev-pilot", "abcde12345"),
        ("dev.jevpilot.jev-pilot", "ABCDE1234"),
        ("dev.jevpilot.jev-pilot", "ABCDE123456"),
        ("dev.jevpilot.jev-pilot", r#"ABCDE1234""#),
        ("", "ABCDE12345"),
        (r#"x" or anchor apple"#, "ABCDE12345"),
        ("dev jevpilot", "ABCDE12345"),
        ("dev_jevpilot", "ABCDE12345"),
    ] {
        assert!(
            MacosPin::developer_id(identifier, team).is_err(),
            "{identifier:?} {team:?}"
        );
    }
    assert!(MacosPin::from_cdhash_hex(&"3".repeat(39)).is_err());
    assert!(MacosPin::from_cdhash_hex(&"g".repeat(40)).is_err());
}

#[test]
fn install_json_with_an_invalid_macos_pin_is_not_read() {
    let mut json = serde_json::to_value(mac_expected(team_pin())).unwrap();
    assert_eq!(
        json["pin"],
        serde_json::json!({
            "developer_id": { "identifier": "dev.jevpilot.jev-pilot", "team": "ABCDE12345" }
        })
    );
    assert_eq!(
        serde_json::from_value::<Expected>(json.clone()).unwrap(),
        mac_expected(team_pin())
    );
    json["pin"]["developer_id"]["team"] = serde_json::json!(r#"X" or anchor apple"#);
    assert!(serde_json::from_value::<Expected>(json).is_err());
    let json = serde_json::json!({
        "os": "macos", "uid": 501, "path": "/x", "pin": { "cdhash": "00" }
    });
    assert!(serde_json::from_value::<Expected>(json).is_err());
}

#[test]
fn expected_round_trips_through_json() {
    let expected = mac_expected(cdhash_pin());
    let json = serde_json::to_value(&expected).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "os": "macos",
            "uid": 501,
            "path": "/usr/local/libexec/jev-pilot/jev-pilot",
            "pin": { "cdhash": "32e719c23c33eb097271b746f2a09f3127b57e5a" },
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

// ---- gathering facts about this test process, over a channel to itself ----

mod gather {
    use std::sync::atomic::{AtomicU32, Ordering};

    use interprocess::local_socket::{ListenerOptions, Stream, prelude::*};
    use jev_pilot::peer::{Expected, Native, PeerCheck, Verdict, decide, recheck};

    /// A connected pair of this process's own: (the listener's end, the
    /// connecting end).
    fn pair() -> (Stream, Stream) {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let unique = format!("jev-pilot-peer-{}-{n}", std::process::id());

        #[cfg(unix)]
        let path = std::env::temp_dir().join(format!("{unique}.sock"));
        #[cfg(unix)]
        let name = || {
            use interprocess::local_socket::GenericFilePath;
            path.as_path().to_fs_name::<GenericFilePath>().unwrap()
        };
        #[cfg(windows)]
        let name = || {
            use interprocess::local_socket::GenericNamespaced;
            unique.as_str().to_ns_name::<GenericNamespaced>().unwrap()
        };

        let listener = ListenerOptions::new().name(name()).create_sync().unwrap();
        let client = Stream::connect(name()).unwrap();
        let server = listener.accept().unwrap();
        (server, client)
    }

    fn own_pid() -> u32 {
        std::process::id()
    }

    fn own_exe() -> std::path::PathBuf {
        std::env::current_exe().unwrap().canonicalize().unwrap()
    }

    #[cfg(target_os = "macos")]
    fn own_cdhash() -> String {
        let out = std::process::Command::new("codesign")
            .args(["-d", "-vvv"])
            .arg(own_exe())
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stderr);
        text.lines()
            .find_map(|line| line.strip_prefix("CDHash="))
            .unwrap_or_else(|| panic!("no CDHash in: {text}"))
            .to_owned()
    }

    #[cfg(target_os = "macos")]
    use jev_pilot::peer::MacosPin;

    #[cfg(target_os = "macos")]
    fn mac_expected(pin: MacosPin) -> Expected {
        Expected::Macos {
            uid: rustix::process::geteuid().as_raw(),
            path: own_exe(),
            pin,
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_gathers_this_process_from_either_end() {
        let (server, client) = pair();
        let expected = mac_expected(MacosPin::from_cdhash_hex(&own_cdhash()).unwrap());
        for end in [&server, &client] {
            let facts = Native.facts(end, &expected);
            assert_eq!(facts.uid, Some(rustix::process::geteuid().as_raw()));
            assert_eq!(facts.pid, Some(own_pid()));
            assert!(facts.token.is_some());
            assert_eq!(decide(&facts, &expected), Verdict::Accept, "{facts:?}");
            let again = Native.facts(end, &expected);
            assert_eq!(recheck(&facts, &again, &expected), Verdict::Accept);
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_refuses_this_process_against_another_cdhash() {
        use jev_pilot::peer::{Check, Reason};
        let (server, _client) = pair();
        let expected = mac_expected(MacosPin::Cdhash([0; 20]));
        let facts = Native.facts(&server, &expected);
        assert_eq!(
            decide(&facts, &expected),
            Verdict::refuse(Check::Requirement, Reason::Mismatch)
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_gathers_this_process_from_either_end() {
        use jev_pilot::peer::LinuxPin;
        let (server, client) = pair();
        let uid = rustix::process::geteuid().as_raw();
        let egid = rustix::process::getegid().as_raw();
        let expected = Expected::Linux {
            uid,
            path: own_exe(),
            pin: LinuxPin::Group(egid),
        };
        for end in [&server, &client] {
            let facts = Native.facts(end, &expected);
            assert_eq!(facts.uid, Some(uid));
            assert_eq!(facts.egid, Some(egid));
            assert_eq!(facts.pid, Some(own_pid()));
            assert_eq!(decide(&facts, &expected), Verdict::Accept, "{facts:?}");
            let again = Native.facts(end, &expected);
            assert_eq!(recheck(&facts, &again, &expected), Verdict::Accept);
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_hashes_a_development_peers_executable() {
        use jev_pilot::peer::{LinuxPin, Sha256Digest};
        use sha2::{Digest, Sha256};
        let (server, _client) = pair();
        let hash: [u8; 32] = Sha256::digest(std::fs::read(own_exe()).unwrap()).into();
        let expected = Expected::Linux {
            uid: rustix::process::geteuid().as_raw(),
            path: own_exe(),
            pin: LinuxPin::DevelopmentSha256(Sha256Digest::from_bytes(hash)),
        };
        let facts = Native.facts(&server, &expected);
        assert_eq!(facts.path, Some(own_exe()));
        assert_eq!(decide(&facts, &expected), Verdict::Accept, "{facts:?}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_development_process_can_make_itself_not_dumpable() {
        use rustix::process::{DumpableBehavior, dumpable_behavior};
        const CHILD: &str = "JEV_PILOT_PEER_TEST_NOT_DUMPABLE";
        // It changes the whole process, so it runs in a child of its own.
        if std::env::var_os(CHILD).is_some() {
            jev_pilot::peer::set_not_dumpable().unwrap();
            assert_eq!(dumpable_behavior().unwrap(), DumpableBehavior::NotDumpable);
            return;
        }
        assert_eq!(dumpable_behavior().unwrap(), DumpableBehavior::Dumpable);
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "gather::a_development_process_can_make_itself_not_dumpable",
            ])
            .env(CHILD, "1")
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_process_can_give_up_the_install_group_for_good() {
        const CHILD: &str = "JEV_PILOT_PEER_TEST_DROP_GROUP";
        // It changes the whole process, so it runs in a child of its own.
        if std::env::var_os(CHILD).is_some() {
            jev_pilot::peer::drop_install_group().unwrap();
            let ids = nix::unistd::getresgid().unwrap();
            let real = nix::unistd::getgid();
            assert_eq!((ids.real, ids.effective, ids.saved), (real, real, real));
            return;
        }
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "gather::a_process_can_give_up_the_install_group_for_good",
            ])
            .env(CHILD, "1")
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[cfg(windows)]
    #[test]
    fn windows_gathers_this_process_from_either_end() {
        use jev_pilot::peer::{Sha256Digest, WindowsPin, own_session, own_sid};
        use sha2::{Digest, Sha256};
        let (server, client) = pair();
        let hash: [u8; 32] = Sha256::digest(std::fs::read(own_exe()).unwrap()).into();
        let expected = Expected::Windows {
            sid: own_sid().unwrap(),
            // What an install pins: the installed path, canonicalized.
            path: own_exe(),
            pin: WindowsPin::Sha256(Sha256Digest::from_bytes(hash)),
            session: None,
        }
        .with_session(Some(own_session().unwrap()));
        for end in [&server, &client] {
            let facts = Native.facts(end, &expected);
            assert_eq!(facts.pid, Some(own_pid()));
            assert_eq!(decide(&facts, &expected), Verdict::Accept, "{facts:?}");
            let again = Native.facts(end, &expected);
            assert_eq!(recheck(&facts, &again, &expected), Verdict::Accept);
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_finds_no_signer_on_an_unsigned_image() {
        use jev_pilot::peer::{Check, Reason, WindowsPin, own_session, own_sid};
        let (server, _client) = pair();
        let expected = Expected::Windows {
            sid: own_sid().unwrap(),
            // What an install pins: the installed path, canonicalized.
            path: own_exe(),
            pin: WindowsPin::Signer("CN=Anyone".to_owned()),
            session: None,
        }
        .with_session(Some(own_session().unwrap()));
        let facts = Native.facts(&server, &expected);
        assert_eq!(facts.code, None);
        assert_eq!(
            decide(&facts, &expected),
            Verdict::refuse(Check::Signer, Reason::Missing)
        );
    }
}
