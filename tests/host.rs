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

// --- the run's side of the channel ---

mod run_side {
    use interprocess::local_socket::{ListenerOptions, Stream, prelude::*};
    use jev_pilot::browser::bridge::{self, ByteOrder, Message, Parent};
    use jev_pilot::browser::host::channel::{Place, RunStream, connect_run};
    use jev_pilot::peer::{Code, Expected, Facts, MacosPin, PeerCheck};
    use serde_json::json;
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{Duration, Instant};

    /// A place of this test's own on the local channel.
    pub(crate) fn place() -> Place {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let unique = format!(
            "jev-pilot-host-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        if cfg!(windows) {
            Place::Pipe(format!(r"\\.\pipe\{unique}"))
        } else {
            Place::Socket(std::env::temp_dir().join(format!("{unique}.sock")))
        }
    }

    fn listen(place: &Place) -> interprocess::local_socket::Listener {
        ListenerOptions::new()
            .name(place.name().expect("a name"))
            .create_sync()
            .expect("listens")
    }

    fn frame(message: &serde_json::Value) -> Vec<u8> {
        bridge::encode(message.to_string().as_bytes(), ByteOrder::Little)
    }

    /// A frame as Chrome writes one to the host.
    pub(crate) fn frame_native(message: &serde_json::Value) -> Vec<u8> {
        bridge::encode(message.to_string().as_bytes(), ByteOrder::Native)
    }

    /// A frame that arrives in two halves, the second after longer than the
    /// run waits for a frame to start, is still read whole: the wait applies
    /// only between frames.
    #[test]
    fn a_frame_is_never_torn_by_the_wait_between_frames() {
        let place = place();
        let listener = listen(&place);
        let writer = std::thread::spawn(move || {
            let mut host = listener.accept().expect("accepts");
            let whole = frame(&json!({"type": "absent"}));
            host.write_all(&whole[..6]).expect("writes");
            std::thread::sleep(Duration::from_millis(300));
            host.write_all(&whole[6..]).expect("writes");
            std::thread::sleep(Duration::from_millis(300));
        });
        let stream = Stream::connect(place.name().expect("a name")).expect("connects");
        let (mut run, pacer) = RunStream::new(stream);
        pacer.set(Duration::from_millis(50));
        let message = bridge::decode(&mut run, ByteOrder::Little)
            .expect("read whole")
            .expect("a frame");
        assert_eq!(message, br#"{"type":"absent"}"#);

        // Between frames, nothing within the wait is "nothing yet".
        let started = Instant::now();
        let mut byte = [0u8; 1];
        let error = run.read(&mut byte).expect_err("nothing yet");
        assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
        assert!(started.elapsed() < Duration::from_millis(250));
        writer.join().expect("the host thread");
    }

    /// Facts that pass against `expected`, whatever the stream.
    pub(crate) struct Vouched;

    impl PeerCheck for Vouched {
        fn facts(&self, _: &Stream, expected: &Expected) -> Facts {
            let Expected::Macos { uid, path, pin } = expected else {
                panic!("the tests pin macOS facts");
            };
            Facts {
                uid: Some(*uid),
                pid: Some(1),
                path: Some(path.clone()),
                token: Some([0; 8]),
                code: Some(Code::Requirement {
                    pin: pin.clone(),
                    satisfied: true,
                }),
                ..Facts::default()
            }
        }
    }

    /// Facts the OS could not give.
    pub(crate) struct Unknown;

    impl PeerCheck for Unknown {
        fn facts(&self, _: &Stream, _: &Expected) -> Facts {
            Facts::default()
        }
    }

    pub(crate) fn expected() -> Expected {
        Expected::Macos {
            uid: 501,
            path: "/usr/local/libexec/jev-pilot/jev-pilot".into(),
            pin: MacosPin::Cdhash([1; 20]),
        }
    }

    fn ready() -> serde_json::Value {
        json!({"type": "ready", "protocol": 1, "extension_version": "0.4.0",
               "product": "Chrome/158.0.7400.12", "paused": false,
               "policy": {"sites": []}, "host_version": "0.4.0"})
    }

    /// The run checks the host before anything is sent, then answers its
    /// nonce with the run's goal, sites and parent.
    #[test]
    fn a_run_connects_to_a_verified_host() {
        let place = place();
        let listener = listen(&place);
        let host = std::thread::spawn(move || {
            let mut run = listener.accept().expect("accepts");
            run.write_all(&frame(&json!({"type": "nonce", "nonce": "cd".repeat(32)})))
                .expect("writes");
            let echo = bridge::decode(&mut run, ByteOrder::Little)
                .expect("reads")
                .expect("an echo");
            run.write_all(&frame(&ready())).expect("writes");
            serde_json::from_slice::<Message>(&echo).expect("a message")
        });
        let parent = Parent {
            pid: Some(77),
            exe: Some("/bin/zsh".into()),
        };
        let wire = connect_run(
            &place,
            &expected(),
            &Vouched,
            "Star the repository",
            &["https://github.com".to_owned()],
            &parent,
        )
        .expect("connects");
        assert_eq!(wire.hello().product, "Chrome/158.0.7400.12");
        assert_eq!(
            host.join().expect("the host thread"),
            Message::Echo {
                nonce: "cd".repeat(32),
                goal: "Star the repository".into(),
                sites: vec!["https://github.com".into()],
                parent_pid: Some(77),
                parent_exe: Some("/bin/zsh".into()),
            }
        );
    }

    /// Something other than the installed host on the channel is told
    /// nothing, and the run says which check failed.
    #[test]
    fn a_run_sends_nothing_to_a_host_that_fails_its_check() {
        let place = place();
        let listener = listen(&place);
        let squatter = std::thread::spawn(move || {
            let mut run = listener.accept().expect("accepts");
            let mut heard = Vec::new();
            let _ = run.read_to_end(&mut heard);
            heard
        });
        let error = connect_run(&place, &expected(), &Unknown, "x", &[], &Parent::default())
            .expect_err("refused");
        assert_eq!(
            error.to_string(),
            "something other than the installed jev-pilot answered on the bridge (process id: \
             not reported by the OS); nothing was sent"
        );
        assert_eq!(
            squatter.join().expect("the squatter thread"),
            Vec::<u8>::new()
        );
    }

    /// Facts that pass, but name another process the second time they are
    /// gathered: on macOS, whoever used the socket last.
    pub(crate) struct Changing(pub(crate) std::sync::atomic::AtomicU32);

    impl PeerCheck for Changing {
        fn facts(&self, stream: &Stream, expected: &Expected) -> Facts {
            let seen = self.0.fetch_add(1, Ordering::SeqCst);
            let mut facts = Vouched.facts(stream, expected);
            facts.token = Some([seen; 8]);
            facts
        }
    }

    /// The run checks the host again after reading its nonce: a host that
    /// handed the socket to another process by then is sent nothing.
    #[test]
    fn a_run_checks_the_host_again_before_answering_its_nonce() {
        let place = place();
        let listener = listen(&place);
        let host = std::thread::spawn(move || {
            let mut run = listener.accept().expect("accepts");
            run.write_all(&frame(&json!({"type": "nonce", "nonce": "cd".repeat(32)})))
                .expect("writes");
            let mut heard = Vec::new();
            let _ = run.read_to_end(&mut heard);
            heard
        });
        let check = Changing(AtomicU32::new(0));
        let error = connect_run(&place, &expected(), &check, "x", &[], &Parent::default())
            .expect_err("refused");
        assert_eq!(
            error.to_string(),
            "something other than the installed jev-pilot answered on the bridge (audit token: \
             not the expected value); nothing was sent"
        );
        assert_eq!(host.join().expect("the host thread"), Vec::<u8>::new());
    }

    /// No host listening is the extension not running.
    #[test]
    fn no_host_is_chrome_not_running_with_the_extension() {
        let error = connect_run(
            &place(),
            &expected(),
            &Vouched,
            "x",
            &[],
            &Parent::default(),
        )
        .expect_err("no host");
        assert_eq!(
            error.to_string(),
            "Chrome is not running with the jev-pilot extension: open Chrome; if it is open, \
             check the extension is on in chrome://extensions with Developer mode on \
             (`jev-pilot extension status` says more)"
        );
    }
}

// --- the host, end to end over an in-process channel ---

mod serving {
    use super::run_side::{Unknown, Vouched, expected, frame_native, place};
    use interprocess::local_socket::{Stream, prelude::*};
    use jev_pilot::browser::bridge::{self, ByteOrder, Parent};
    use jev_pilot::browser::extension::ConnectError;
    use jev_pilot::browser::host::channel::{Place, RunError, connect_run};
    use jev_pilot::browser::host::{Serve, serve};
    use jev_pilot::peer::PeerCheck;
    use serde_json::json;
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    /// What the host wrote to Chrome.
    #[derive(Clone, Default)]
    struct Out(Arc<Mutex<Vec<u8>>>);

    impl Write for Out {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Out {
        fn messages(&self) -> Vec<serde_json::Value> {
            let bytes = self.0.lock().unwrap().clone();
            let mut reader = &bytes[..];
            let mut all = Vec::new();
            while let Some(m) = bridge::decode(&mut reader, ByteOrder::Native).unwrap() {
                all.push(serde_json::from_slice(&m).unwrap());
            }
            all
        }
    }

    fn hello() -> serde_json::Value {
        json!({"type": "hello", "protocol": 1, "extension_version": "0.4.0",
               "product": "Chrome/158.0.7400.12", "paused": false, "policy": {"sites": []}})
    }

    struct Host {
        stdin: Option<std::io::PipeWriter>,
        out: Out,
        audit: std::path::PathBuf,
        done: std::thread::JoinHandle<std::io::Result<()>>,
    }

    fn host_at(place: &Place, check: Arc<dyn PeerCheck + Send + Sync>) -> Host {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let (stdin, mut writer) = std::io::pipe().unwrap();
        writer.write_all(&frame_native(&hello())).unwrap();
        let out = Out::default();
        let audit = std::env::temp_dir().join(format!(
            "jev-pilot-audit-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&audit);
        let config = Serve {
            place: place.clone(),
            expected: expected(),
            check,
            audit_dir: audit.clone(),
            version: "0.4.0".into(),
            extension_on_disk: Some("0.4.0".into()),
            installed_version: Box::new(|| Some("0.4.0".into())),
        };
        let stdout = out.clone();
        let done = std::thread::spawn(move || serve(config, stdin, stdout));
        Host {
            stdin: Some(writer),
            out,
            audit,
            done,
        }
    }

    /// A run, once the host is listening and has heard the extension.
    fn run_at(place: &Place, goal: &str) -> Result<impl Sized, RunError> {
        let started = Instant::now();
        loop {
            match connect_run(place, &expected(), &Vouched, goal, &[], &Parent::default()) {
                Err(RunError::NoHost | RunError::Connect(ConnectError::Absent))
                    if started.elapsed() < Duration::from_secs(5) =>
                {
                    std::thread::sleep(Duration::from_millis(20));
                }
                other => return other,
            }
        }
    }

    fn wait_for(what: &str, done: impl Fn() -> bool) {
        let started = Instant::now();
        while !done() {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "waiting for {what}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn a_run_is_relayed_and_released_and_the_host_leaves_with_chrome() {
        let place = place();
        let mut host = host_at(&place, Arc::new(Vouched));
        let run = run_at(&place, "Star the repository").expect("the run connects");
        wait_for("run_started", || {
            host.out
                .messages()
                .iter()
                .any(|m| m["type"] == "run_started")
        });
        drop(run);
        wait_for("release", || {
            host.out.messages().iter().any(|m| m["type"] == "release")
        });
        drop(host.stdin.take());
        host.done.join().unwrap().expect("the host ends cleanly");

        let sent: Vec<_> = host
            .out
            .messages()
            .iter()
            .map(|m| m["type"].clone())
            .collect();
        assert_eq!(sent, [json!("run_started"), json!("release")]);
        if let Place::Socket(path) = &place {
            assert!(!path.exists(), "the socket is removed");
        }
        let log = std::fs::read_to_string(host.audit.join("audit.log")).expect("an audit log");
        let events: Vec<String> = log
            .lines()
            .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()["event"].to_string())
            .collect();
        assert_eq!(events, [r#""run_started""#, r#""run_ended""#]);
    }

    /// The host checks a run before reading anything from it; a run that
    /// fails is told which check, and the refusal is audited.
    #[test]
    fn a_run_that_fails_the_hosts_check_is_turned_away() {
        let place = place();
        let mut host = host_at(&place, Arc::new(Unknown));
        let error = run_at(&place, "x").err().expect("refused");
        assert_eq!(
            error.to_string(),
            "the bridge only serves the installed jev-pilot at \
             /usr/local/libexec/jev-pilot/jev-pilot (process id: not reported by the OS)"
        );
        drop(host.stdin.take());
        host.done.join().unwrap().expect("the host ends cleanly");
        let log = std::fs::read_to_string(host.audit.join("audit.log")).expect("an audit log");
        assert!(log.contains(r#""event":"refused""#), "{log}");
    }

    /// A second host finds the first serving, tells its own extension so,
    /// and exits; the first carries on.
    #[test]
    fn a_second_host_steps_aside_for_one_already_serving() {
        let place = place();
        let mut first = host_at(&place, Arc::new(Vouched));
        wait_for("the first host", || {
            Stream::connect(place.name().unwrap()).is_ok()
        });
        let second = host_at(&place, Arc::new(Vouched));
        second.done.join().unwrap().expect("steps aside cleanly");
        assert_eq!(second.out.messages(), [json!({"type": "elsewhere"})]);
        let run = run_at(&place, "x").expect("the first host still serves");
        drop(run);
        drop(first.stdin.take());
        first.done.join().unwrap().expect("ends cleanly");
    }

    /// `status` asks whether a verified host answers, without taking the
    /// bridge or telling the extension anything.
    #[test]
    fn status_can_ask_whether_the_host_answers() {
        use jev_pilot::browser::host::channel::host_answers;
        let place = place();
        assert!(!host_answers(&place, &expected(), &Vouched));
        let mut host = host_at(&place, Arc::new(Vouched));
        wait_for("the host", || host_answers(&place, &expected(), &Vouched));
        assert!(!host_answers(&place, &expected(), &Unknown));
        drop(host.stdin.take());
        host.done.join().unwrap().expect("ends cleanly");
        assert_eq!(host.out.messages(), Vec::<serde_json::Value>::new());
    }

    /// A socket left by a host that has gone is removed, and a new host
    /// listens in its place.
    #[cfg(unix)]
    #[test]
    fn a_stale_socket_is_replaced() {
        let place = place();
        let Place::Socket(path) = &place else {
            unreachable!()
        };
        drop(std::os::unix::net::UnixListener::bind(path).unwrap());
        assert!(path.exists(), "a stale socket file");
        let mut host = host_at(&place, Arc::new(Vouched));
        drop(run_at(&place, "x").expect("the new host serves"));
        drop(host.stdin.take());
        host.done.join().unwrap().expect("ends cleanly");
    }
}

/// The audit log is rotated by size, three old logs kept.
#[test]
fn the_audit_log_is_rotated_by_size() {
    use jev_pilot::browser::host::append_audit;
    let dir = std::env::temp_dir().join(format!("jev-pilot-rotate-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    // Each line is 7 bytes, so the log reaches the limit every two lines.
    for n in 0..10 {
        append_audit(&dir, &format!("line {n}\n"), 10).expect("appends");
    }
    let read = |name: &str| std::fs::read_to_string(dir.join(name)).ok();
    assert_eq!(read("audit.log").as_deref(), Some("line 8\nline 9\n"));
    assert_eq!(read("audit.log.1").as_deref(), Some("line 6\nline 7\n"));
    assert_eq!(read("audit.log.2").as_deref(), Some("line 4\nline 5\n"));
    assert_eq!(read("audit.log.3").as_deref(), Some("line 2\nline 3\n"));
    assert_eq!(read("audit.log.4"), None, "the oldest is dropped");
    let _ = std::fs::remove_dir_all(&dir);
}
