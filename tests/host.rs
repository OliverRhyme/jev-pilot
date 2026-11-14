//! The host: what Chrome starts for the jev-pilot extension, relaying one
//! verified run at a time.
//!
//! The relay is pure: each test feeds it what happened (a frame from the
//! extension, a run verified or refused, a frame from a run, a run gone,
//! stdin closed) and checks what it does in answer.

#![cfg(feature = "extension")]

use jev_pilot::browser::bridge::{Hello, Message, Op};
use jev_pilot::browser::host::{Audit, Input, Output, Relay, RunId, Started};
use serde_json::json;

const VERSION: &str = "0.4.0";

fn hello() -> Hello {
    Hello {
        protocol: 1,
        extension_version: VERSION.into(),
        product: "Chrome/158.0.7400.12".into(),
        paused: false,
        policy: json!({"sites": [], "owner_tabs": "group"}),
    }
}

fn relay() -> Relay {
    Relay::new(VERSION, Some(VERSION.into()))
}

/// A relay the extension has said hello to.
fn connected() -> Relay {
    let mut relay = relay();
    let out = relay.handle(Input::Extension(Message::Hello(hello())));
    assert_eq!(out, []);
    relay
}

fn started(pid: u32, goal: &str) -> Started {
    Started {
        pid,
        goal: goal.into(),
        sites: vec!["https://github.com".into()],
        parent_pid: Some(77),
        parent_exe: Some("/bin/zsh".into()),
    }
}

const FIRST: RunId = 1;
const SECOND: RunId = 2;

fn verified(relay: &mut Relay, run: RunId, pid: u32, goal: &str) -> Vec<Output> {
    relay.handle(Input::RunVerified {
        run,
        started: started(pid, goal),
    })
}

fn request(id: u64, op: Op) -> Message {
    Message::Request { id, op }
}

#[test]
fn a_verified_run_takes_the_bridge_and_the_extension_is_told() {
    let mut relay = connected();
    assert_eq!(
        verified(&mut relay, FIRST, 4242, "Star the repository"),
        [
            Output::ToRun {
                run: FIRST,
                message: Message::Ready {
                    hello: hello(),
                    host_version: VERSION.into(),
                },
            },
            Output::ToExtension(Message::RunStarted {
                pid: 4242,
                goal: "Star the repository".into(),
                sites: vec!["https://github.com".into()],
            }),
            Output::Audit(Audit::RunStarted(started(4242, "Star the repository"))),
        ]
    );
}

#[test]
fn a_run_before_the_extension_has_connected_is_told_it_is_absent() {
    let mut relay = relay();
    assert_eq!(
        verified(&mut relay, FIRST, 4242, "Star the repository"),
        [
            Output::ToRun {
                run: FIRST,
                message: Message::Absent,
            },
            Output::Close { run: FIRST },
        ]
    );
}

/// One run at a time: another is told who holds the bridge, and turned
/// away; the first carries on.
#[test]
fn a_second_run_is_told_the_bridge_is_busy() {
    let mut relay = connected();
    verified(&mut relay, FIRST, 4242, &"a".repeat(100));
    assert_eq!(
        verified(&mut relay, SECOND, 5151, "Something else"),
        [
            Output::ToRun {
                run: SECOND,
                message: Message::Busy {
                    pid: 4242,
                    goal: format!("{}…", "a".repeat(60)),
                },
            },
            Output::Close { run: SECOND },
        ]
    );
    // The turned-away run is not heard; the first one still is.
    assert_eq!(
        relay.handle(Input::Run {
            run: SECOND,
            message: request(1, Op::Tabs),
        }),
        []
    );
    assert_eq!(
        relay.handle(Input::Run {
            run: FIRST,
            message: request(1, Op::Tabs),
        }),
        [Output::ToExtension(request(1, Op::Tabs))]
    );
}

#[test]
fn the_runs_requests_and_the_extensions_answers_pass_unchanged() {
    let mut relay = connected();
    verified(&mut relay, FIRST, 4242, "Star the repository");
    let send = request(
        3,
        Op::Send {
            tab: 7,
            session: None,
            method: "DOM.getDocument".into(),
            params: json!({}),
        },
    );
    assert_eq!(
        relay.handle(Input::Run {
            run: FIRST,
            message: send.clone(),
        }),
        [Output::ToExtension(send)]
    );
    for message in [
        Message::Reply {
            id: 3,
            outcome: jev_pilot::browser::bridge::Outcome::Result(json!({"root": {}})),
        },
        Message::Cdp {
            tab: 7,
            session: None,
            method: "Page.loadEventFired".into(),
            params: json!({}),
        },
        Message::TabRemoved { tab: 9 },
    ] {
        assert_eq!(
            relay.handle(Input::Extension(message.clone())),
            [Output::ToRun {
                run: FIRST,
                message
            }]
        );
    }
}

/// Only requests reach the extension from a run: nothing a run sends can
/// speak for the host.
#[test]
fn a_run_cannot_send_the_extension_anything_but_requests() {
    let mut relay = connected();
    verified(&mut relay, FIRST, 4242, "Star the repository");
    for message in [
        Message::Release,
        Message::Reload,
        Message::RunStarted {
            pid: 1,
            goal: String::new(),
            sites: vec![],
        },
        Message::Elsewhere,
    ] {
        assert_eq!(
            relay.handle(Input::Run {
                run: FIRST,
                message
            }),
            []
        );
    }
}

/// Opening a tab, going somewhere and taking hold of a tab are each written
/// to the audit log as they pass.
#[test]
fn tabs_attached_and_sites_visited_are_audited() {
    let mut relay = connected();
    verified(&mut relay, FIRST, 4242, "Star the repository");
    let mut audited = |message: Message| -> Vec<Audit> {
        relay
            .handle(Input::Run {
                run: FIRST,
                message,
            })
            .into_iter()
            .filter_map(|o| match o {
                Output::Audit(a) => Some(a),
                _ => None,
            })
            .collect()
    };
    assert_eq!(
        audited(request(
            1,
            Op::Create {
                url: "https://github.com/x".into()
            }
        )),
        [Audit::SiteVisited {
            pid: 4242,
            url: "https://github.com/x".into()
        }]
    );
    assert_eq!(
        audited(request(2, Op::Attach { tab: 7 })),
        [Audit::TabAttached { pid: 4242, tab: 7 }]
    );
    assert_eq!(
        audited(request(
            3,
            Op::Send {
                tab: 7,
                session: None,
                method: "Page.navigate".into(),
                params: json!({"url": "https://github.com/y"}),
            }
        )),
        [Audit::SiteVisited {
            pid: 4242,
            url: "https://github.com/y".into()
        }]
    );
    assert_eq!(audited(request(4, Op::Tabs)), []);
}

#[test]
fn when_the_run_goes_the_extension_lets_go_and_the_bridge_is_free() {
    let mut relay = connected();
    verified(&mut relay, FIRST, 4242, "Star the repository");
    assert_eq!(
        relay.handle(Input::RunGone { run: FIRST }),
        [
            Output::ToExtension(Message::Release),
            Output::Audit(Audit::RunEnded {
                pid: 4242,
                how: "closed".into()
            }),
        ]
    );
    // A run turned away going is nothing to the extension.
    assert_eq!(relay.handle(Input::RunGone { run: SECOND }), []);
    assert!(matches!(
        &verified(&mut relay, SECOND, 5151, "Next")[0],
        Output::ToRun {
            message: Message::Ready { .. },
            ..
        }
    ));
}

/// Chrome closing the port closes stdin: the host ends the run and exits.
#[test]
fn the_host_exits_when_stdin_closes() {
    let mut relay = connected();
    assert_eq!(relay.handle(Input::StdinClosed), [Output::Exit]);

    let mut relay = connected();
    verified(&mut relay, FIRST, 4242, "Star the repository");
    assert_eq!(
        relay.handle(Input::StdinClosed),
        [
            Output::Close { run: FIRST },
            Output::Audit(Audit::RunEnded {
                pid: 4242,
                how: "chrome_closed".into()
            }),
            Output::Exit,
        ]
    );
}

/// A request over 1 MiB would make Chrome close the channel, so it is
/// answered by the host and never sent.
#[test]
fn a_request_too_large_for_chrome_is_refused_by_the_host() {
    let mut relay = connected();
    verified(&mut relay, FIRST, 4242, "Star the repository");
    let out = relay.handle(Input::Run {
        run: FIRST,
        message: request(
            9,
            Op::Send {
                tab: 7,
                session: None,
                method: "Input.insertText".into(),
                params: json!({"text": "x".repeat(1 << 20)}),
            },
        ),
    });
    let [Output::ToRun { run, message }] = &out[..] else {
        panic!("one answer to the run: {out:?}");
    };
    assert_eq!(*run, FIRST);
    assert_eq!(
        serde_json::to_value(message).expect("JSON"),
        json!({"type": "reply", "id": 9, "error": {
            "code": "chrome",
            "message": "that command is too large to pass to Chrome (over 1 MiB)"
        }})
    );
}

/// A frame from Chrome over 64 MiB leaves stdin unreadable: the run is told
/// why, and the host exits.
#[test]
fn a_frame_from_chrome_too_large_to_read_ends_the_bridge() {
    let mut relay = connected();
    verified(&mut relay, FIRST, 4242, "Star the repository");
    assert_eq!(
        relay.handle(Input::ExtensionTooLarge),
        [
            Output::ToRun {
                run: FIRST,
                message: Message::Ended {
                    reason: "too_large".into()
                },
            },
            Output::Close { run: FIRST },
            Output::Audit(Audit::RunEnded {
                pid: 4242,
                how: "too_large".into()
            }),
            Output::Exit,
        ]
    );
}

#[test]
fn a_run_that_fails_the_hosts_check_is_refused_and_audited() {
    let mut relay = connected();
    assert_eq!(
        relay.handle(Input::RunRefused {
            run: FIRST,
            pid: Some(666),
            check: "image SHA-256: not the expected value".into(),
        }),
        [
            Output::ToRun {
                run: FIRST,
                message: Message::Refused {
                    check: "image SHA-256: not the expected value".into()
                },
            },
            Output::Close { run: FIRST },
            Output::Audit(Audit::Refused {
                pid: Some(666),
                check: "image SHA-256: not the expected value".into()
            }),
        ]
    );
}

/// The extension says what changed since its hello, and the next run is
/// told the latest.
#[test]
fn a_status_from_the_extension_is_what_the_next_run_hears() {
    let mut relay = connected();
    let paused = Hello {
        paused: true,
        ..hello()
    };
    assert_eq!(
        relay.handle(Input::Extension(Message::Status(paused.clone()))),
        []
    );
    assert_eq!(
        verified(&mut relay, FIRST, 4242, "x")[0],
        Output::ToRun {
            run: FIRST,
            message: Message::Ready {
                hello: paused,
                host_version: VERSION.into()
            }
        }
    );
}

/// Files on disk newer than the running extension: it is asked to reload.
#[test]
fn an_extension_older_than_its_files_is_asked_to_reload() {
    let mut relay = Relay::new(VERSION, Some("0.4.1".into()));
    assert_eq!(
        relay.handle(Input::Extension(Message::Hello(hello()))),
        [Output::ToExtension(Message::Reload)]
    );
    for (on_disk, running) in [("0.4.0", "0.4.0"), ("0.4.0", "0.10.0"), ("0.9.0", "0.10.0")] {
        let mut relay = Relay::new(VERSION, Some(on_disk.into()));
        let hello = Hello {
            extension_version: running.into(),
            ..hello()
        };
        assert_eq!(
            relay.handle(Input::Extension(Message::Hello(hello))),
            [],
            "{running} against {on_disk} on disk"
        );
    }
}

/// A newer jev-pilot installed while this host runs: the host exits once no
/// run holds the bridge, and Chrome starts the new one.
#[test]
fn a_host_exits_for_a_newer_install_once_idle() {
    let mut relay = connected();
    assert_eq!(
        relay.handle(Input::Installed {
            version: "0.4.1".into()
        }),
        [Output::Exit]
    );
    assert_eq!(
        connected().handle(Input::Installed {
            version: VERSION.into()
        }),
        []
    );

    let mut relay = connected();
    verified(&mut relay, FIRST, 4242, "x");
    assert_eq!(
        relay.handle(Input::Installed {
            version: "0.4.1".into()
        }),
        []
    );
    let out = relay.handle(Input::RunGone { run: FIRST });
    assert_eq!(out.last(), Some(&Output::Exit));
    // Nothing more is served on the way out.
    assert!(matches!(
        &verified(&mut relay, SECOND, 5151, "y")[..],
        [
            Output::ToRun {
                message: Message::Absent,
                ..
            },
            Output::Close { .. }
        ]
    ));
}

// --- the audit log ---

#[test]
fn each_audit_event_is_one_line_of_json() {
    let at = "2026-11-15T09:30:00Z";
    let line = |audit: Audit| -> serde_json::Value {
        let text = audit.line(at);
        assert!(text.ends_with('\n') && !text[..text.len() - 1].contains('\n'));
        serde_json::from_str(&text).expect("JSON")
    };
    assert_eq!(
        line(Audit::RunStarted(started(4242, "Star the repository"))),
        json!({"time": at, "event": "run_started", "pid": 4242, "parent_pid": 77,
               "parent_exe": "/bin/zsh", "goal": "Star the repository",
               "sites": ["https://github.com"], "identity": "accepted"})
    );
    assert_eq!(
        line(Audit::TabAttached { pid: 4242, tab: 7 }),
        json!({"time": at, "event": "tab_attached", "pid": 4242, "tab": 7})
    );
    assert_eq!(
        line(Audit::SiteVisited {
            pid: 4242,
            url: "https://github.com/x".into()
        }),
        json!({"time": at, "event": "site_visited", "pid": 4242, "url": "https://github.com/x"})
    );
    assert_eq!(
        line(Audit::RunEnded {
            pid: 4242,
            how: "closed".into()
        }),
        json!({"time": at, "event": "run_ended", "pid": 4242, "how": "closed"})
    );
    assert_eq!(
        line(Audit::Refused {
            pid: None,
            check: "uid: not the expected value".into()
        }),
        json!({"time": at, "event": "refused", "pid": null,
               "check": "uid: not the expected value"})
    );
}

#[test]
fn audit_times_are_utc_to_the_second() {
    use jev_pilot::browser::host::utc;
    use std::time::{Duration, UNIX_EPOCH};
    assert_eq!(utc(UNIX_EPOCH), "1970-01-01T00:00:00Z");
    assert_eq!(
        utc(UNIX_EPOCH + Duration::from_mins(29_912_300)),
        "2026-11-15T10:20:00Z"
    );
    assert_eq!(
        utc(UNIX_EPOCH + Duration::from_secs(1_794_738_007)),
        "2026-11-15T10:20:07Z"
    );
    assert_eq!(
        utc(UNIX_EPOCH + Duration::from_hours(264_384)),
        "2000-02-29T00:00:00Z"
    );
}

// --- the local channel ---

mod channel {
    use jev_pilot::browser::discover::Os;
    use jev_pilot::browser::host::channel::{
        ChannelError, DirFacts, check_dir, check_path, pipe_name, pipe_sddl, socket_dir,
        socket_path,
    };
    use std::path::{Path, PathBuf};

    const HOME: &str = "/home/oliver";

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            vars.iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    #[test]
    fn the_socket_lives_in_a_per_user_directory_never_tmp() {
        assert_eq!(
            socket_dir(
                Os::MacOs,
                Path::new("/Users/oliver"),
                &env(&[("TMPDIR", "/var/x")])
            ),
            PathBuf::from("/Users/oliver/Library/Application Support/jev-pilot/run")
        );
        assert_eq!(
            socket_dir(
                Os::Linux,
                Path::new(HOME),
                &env(&[("XDG_RUNTIME_DIR", "/run/user/1000")])
            ),
            PathBuf::from("/run/user/1000/jev-pilot")
        );
        assert_eq!(
            socket_dir(Os::Linux, Path::new(HOME), &env(&[])),
            PathBuf::from("/home/oliver/.local/state/jev-pilot/run")
        );
        // A relative runtime directory is not one to trust.
        assert_eq!(
            socket_dir(
                Os::Linux,
                Path::new(HOME),
                &env(&[("XDG_RUNTIME_DIR", "run")])
            ),
            PathBuf::from("/home/oliver/.local/state/jev-pilot/run")
        );
        assert_eq!(
            socket_path(Path::new("/run/user/1000/jev-pilot")),
            PathBuf::from("/run/user/1000/jev-pilot/bridge.sock")
        );
    }

    /// The directory must be the user's own and closed to everyone else.
    #[test]
    fn a_directory_not_the_users_or_open_to_others_is_refused() {
        let dir = Path::new("/run/user/1000/jev-pilot");
        let ok = DirFacts {
            is_dir: true,
            owner: 1000,
            mode: 0o700,
        };
        assert_eq!(check_dir(dir, ok, 1000), Ok(()));
        for (facts, why) in [
            (DirFacts { owner: 0, ..ok }, "is owned by uid 0, not you"),
            (
                DirFacts { mode: 0o750, ..ok },
                "is open to others (mode 750)",
            ),
            (
                DirFacts { mode: 0o701, ..ok },
                "is open to others (mode 701)",
            ),
            (
                DirFacts {
                    is_dir: false,
                    ..ok
                },
                "is not a directory",
            ),
        ] {
            let error = check_dir(dir, facts, 1000).expect_err("refused");
            assert_eq!(
                error.to_string(),
                format!("the bridge's directory {} {why}", dir.display()),
            );
        }
    }

    #[test]
    fn a_socket_path_over_the_system_limit_is_refused_naming_it() {
        let fits = PathBuf::from(format!("/{}", "a".repeat(102)));
        assert_eq!(check_path(Os::MacOs, &fits), Ok(()));
        let long = PathBuf::from(format!("/{}", "a".repeat(103)));
        let error = check_path(Os::MacOs, &long).expect_err("too long for macOS");
        assert_eq!(error, ChannelError::PathTooLong { path: long.clone() });
        assert_eq!(
            error.to_string(),
            format!(
                "the bridge's socket path {} is too long for this system",
                long.display()
            )
        );
        assert_eq!(check_path(Os::Linux, &long), Ok(()));
        let longer = PathBuf::from(format!("/{}", "a".repeat(107)));
        assert!(check_path(Os::Linux, &longer).is_err());
    }

    const SID: &str = "S-1-5-21-1004336348-1177238915-682003330-1001";

    #[test]
    fn the_pipe_is_named_for_the_user_and_closed_to_everyone_else() {
        assert_eq!(pipe_name(SID), format!(r"\\.\pipe\jev-pilot-{SID}"));
        // Deny network and anonymous logons first (an access check stops at
        // the first ACE that decides), then all to SYSTEM, and to the user
        // read and write without FILE_CREATE_PIPE_INSTANCE (0x4), so no
        // program of the user's can add an instance of its own.
        assert_eq!(
            pipe_sddl(SID),
            format!("D:P(D;;GA;;;NU)(D;;GA;;;AN)(A;;GA;;;SY)(A;;0x12019b;;;{SID})")
        );
        let mask = 0x0012_019b_u32;
        assert_eq!(mask & 0x4, 0, "no FILE_CREATE_PIPE_INSTANCE");
        assert_eq!(mask & 0x0012_0089, 0x0012_0089, "FILE_GENERIC_READ");
        assert_eq!(mask & 0x2, 0x2, "FILE_WRITE_DATA");
    }
}
