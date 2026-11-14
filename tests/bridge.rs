//! The bridge between a run and Chrome's jev-pilot extension: its frames, its
//! messages, its handshake, and the translator that lets the DevTools engine
//! drive tabs through it. Pure: nothing here opens a socket or starts Chrome.

#![cfg(feature = "browser")]

use jev_pilot::browser::bridge::{self, ByteOrder};

// --- framing ---

/// On the local channel a frame is its length as four little-endian bytes,
/// then the message, so the host can pass frames on without reading them.
#[test]
fn a_local_frame_is_a_little_endian_length_then_the_message() {
    let framed = bridge::encode(br#"{"type":"release"}"#, ByteOrder::Little);
    assert_eq!(&framed[..4], &[18, 0, 0, 0]);
    assert_eq!(&framed[4..], br#"{"type":"release"}"#);

    let mut reader = &framed[..];
    assert_eq!(
        bridge::decode(&mut reader, ByteOrder::Little).expect("reads"),
        Some(br#"{"type":"release"}"#.to_vec())
    );
    assert_eq!(
        bridge::decode(&mut reader, ByteOrder::Little).expect("a clean end"),
        None,
        "the stream ended between frames"
    );
}

/// Between the host and the extension, Chrome's native messaging writes the
/// length in the machine's own byte order.
#[test]
fn a_native_frame_has_the_machines_byte_order() {
    let framed = bridge::encode(b"{}", ByteOrder::Native);
    assert_eq!(&framed[..4], &2u32.to_ne_bytes());
    let mut reader = &framed[..];
    assert_eq!(
        bridge::decode(&mut reader, ByteOrder::Native).expect("reads"),
        Some(b"{}".to_vec())
    );
}

/// Chrome closes the channel on a message over 1 MiB headed for it, so such a
/// command is refused before it is sent.
#[test]
fn a_frame_for_chrome_over_one_mebibyte_is_refused() {
    let most = vec![b' '; bridge::TO_CHROME_LIMIT];
    assert!(bridge::encode_for_chrome(&most, ByteOrder::Little).is_ok());

    let over = vec![b' '; bridge::TO_CHROME_LIMIT + 1];
    let refused = bridge::encode_for_chrome(&over, ByteOrder::Little).expect_err("too large");
    assert_eq!(
        refused.to_string(),
        "that command is too large to pass to Chrome (over 1 MiB)"
    );
}

/// A frame claiming more than 64 MiB is refused from its length alone, before
/// anything is allocated for it.
#[test]
fn a_frame_over_sixty_four_mebibytes_is_not_read() {
    let claimed = u32::try_from(bridge::READ_LIMIT + 1).expect("fits");
    let mut reader = &claimed.to_le_bytes()[..];
    let refused = bridge::decode(&mut reader, ByteOrder::Little).expect_err("too large");
    assert_eq!(
        refused.to_string(),
        "the page is too large to read through the extension"
    );

    let mut framed = u32::try_from(bridge::READ_LIMIT)
        .expect("fits")
        .to_le_bytes()
        .to_vec();
    framed.extend(vec![b' '; bridge::READ_LIMIT]);
    let mut reader = &framed[..];
    assert_eq!(
        bridge::decode(&mut reader, ByteOrder::Little)
            .expect("64 MiB is allowed")
            .map(|m| m.len()),
        Some(bridge::READ_LIMIT)
    );
}

// --- messages ---

use bridge::{BridgeError, ErrorCode, Hello, Message, Op, Outcome, TabInfo};
use serde_json::json;

/// Every message reads from the JSON both ends write, and writes back the
/// same JSON: the shapes here are the protocol.
fn round_trip(wire: &serde_json::Value, message: &Message) {
    let read: Message = serde_json::from_value(wire.clone()).expect("reads");
    assert_eq!(&read, message, "{wire}");
    assert_eq!(&serde_json::to_value(message).expect("writes"), wire);
}

fn hello() -> Hello {
    Hello {
        protocol: 1,
        extension_version: "0.2.0".into(),
        product: "Chrome/158.0.7400.12".into(),
        paused: false,
        policy: json!({"sites": [], "owner_tabs": "group"}),
    }
}

#[test]
fn the_extension_says_hello_and_the_host_answers_a_run() {
    let fields = json!({
        "protocol": 1, "extension_version": "0.2.0", "product": "Chrome/158.0.7400.12",
        "paused": false, "policy": {"sites": [], "owner_tabs": "group"},
    });
    let mut wire = fields.clone();
    wire["type"] = "hello".into();
    round_trip(&wire, &Message::Hello(hello()));
    wire["type"] = "status".into();
    round_trip(&wire, &Message::Status(hello()));
    let mut ready = fields;
    ready["type"] = "ready".into();
    ready["host_version"] = "0.2.0".into();
    round_trip(
        &ready,
        &Message::Ready {
            hello: hello(),
            host_version: "0.2.0".into(),
        },
    );
    round_trip(
        &json!({"type": "busy", "pid": 4242, "goal": "Star the repository"}),
        &Message::Busy {
            pid: 4242,
            goal: "Star the repository".into(),
        },
    );
    round_trip(&json!({"type": "absent"}), &Message::Absent);
    round_trip(
        &json!({"type": "refused", "check": "the code signature"}),
        &Message::Refused {
            check: "the code signature".into(),
        },
    );
}

#[test]
fn a_run_asks_with_an_id_and_an_operation() {
    let request = |id, op| Message::Request { id, op };
    round_trip(
        &json!({"type": "request", "id": 1, "op": "tabs"}),
        &request(1, Op::Tabs),
    );
    round_trip(
        &json!({"type": "request", "id": 2, "op": "create", "url": "https://example.test/"}),
        &request(
            2,
            Op::Create {
                url: "https://example.test/".into(),
            },
        ),
    );
    round_trip(
        &json!({"type": "request", "id": 3, "op": "attach", "tab": 7}),
        &request(3, Op::Attach { tab: 7 }),
    );
    round_trip(
        &json!({"type": "request", "id": 4, "op": "detach", "tab": 7}),
        &request(4, Op::Detach { tab: 7 }),
    );
    round_trip(
        &json!({"type": "request", "id": 5, "op": "activate", "tab": 7}),
        &request(5, Op::Activate { tab: 7 }),
    );
    round_trip(
        &json!({"type": "request", "id": 6, "op": "send", "tab": 7, "method": "Page.getFrameTree", "params": {}}),
        &request(
            6,
            Op::Send {
                tab: 7,
                session: None,
                method: "Page.getFrameTree".into(),
                params: json!({}),
            },
        ),
    );
    round_trip(
        &json!({"type": "request", "id": 7, "op": "send", "tab": 7, "session": "C1", "method": "DOM.getDocument", "params": {"depth": 1}}),
        &request(
            7,
            Op::Send {
                tab: 7,
                session: Some("C1".into()),
                method: "DOM.getDocument".into(),
                params: json!({"depth": 1}),
            },
        ),
    );
}

#[test]
fn the_extension_replies_with_a_result_or_a_coded_error() {
    round_trip(
        &json!({"type": "reply", "id": 1, "result": {"tabs": [
            {"tab": 7, "target": "T7", "url": "https://example.test/", "title": "Example"},
            {"tab": 8, "target": "T8", "url": "https://example.test/next", "title": "Next", "opener": 7},
        ]}}),
        &Message::Reply {
            id: 1,
            outcome: Outcome::Result(json!({"tabs": [
                {"tab": 7, "target": "T7", "url": "https://example.test/", "title": "Example"},
                {"tab": 8, "target": "T8", "url": "https://example.test/next", "title": "Next", "opener": 7},
            ]})),
        },
    );
    for (code, wire) in [
        (ErrorCode::RefusedMethod, "refused_method"),
        (ErrorCode::RefusedScript, "refused_script"),
        (ErrorCode::RefusedSite, "refused_site"),
        (ErrorCode::RefusedPage, "refused_page"),
        (ErrorCode::RefusedTab, "refused_tab"),
        (ErrorCode::Paused, "paused"),
        (ErrorCode::NoTab, "no_tab"),
        (ErrorCode::AttachFailed, "attach_failed"),
        (ErrorCode::Chrome, "chrome"),
    ] {
        round_trip(
            &json!({"type": "reply", "id": 9, "error": {"code": wire, "message": "why"}}),
            &Message::Reply {
                id: 9,
                outcome: Outcome::Error(BridgeError {
                    code,
                    message: "why".into(),
                }),
            },
        );
    }
    let tab: TabInfo = serde_json::from_value(
        json!({"tab": 8, "target": "T8", "url": "u", "title": "t", "opener": 7}),
    )
    .expect("reads");
    assert_eq!(tab.opener, Some(7));
}

#[test]
fn the_extension_reports_page_events_and_tabs_coming_and_going() {
    round_trip(
        &json!({"type": "cdp", "tab": 7, "method": "Page.loadEventFired", "params": {"timestamp": 1.5}}),
        &Message::Cdp {
            tab: 7,
            session: None,
            method: "Page.loadEventFired".into(),
            params: json!({"timestamp": 1.5}),
        },
    );
    round_trip(
        &json!({"type": "tab_created", "tab": 8, "target": "T8", "opener": 7, "url": "https://example.test/next", "title": "Next"}),
        &Message::TabCreated(TabInfo {
            tab: 8,
            target: "T8".into(),
            url: "https://example.test/next".into(),
            title: "Next".into(),
            opener: Some(7),
        }),
    );
    round_trip(
        &json!({"type": "tab_removed", "tab": 8}),
        &Message::TabRemoved { tab: 8 },
    );
    round_trip(
        &json!({"type": "detached", "tab": 7, "reason": "canceled_by_user"}),
        &Message::Detached {
            tab: 7,
            reason: "canceled_by_user".into(),
            url: None,
        },
    );
    // A tab let go of for leaving the allowed sites says where it went.
    round_trip(
        &json!({"type": "detached", "tab": 7, "reason": "left_allowed_sites", "url": "https://elsewhere.test/"}),
        &Message::Detached {
            tab: 7,
            reason: "left_allowed_sites".into(),
            url: Some("https://elsewhere.test/".into()),
        },
    );
}

#[test]
fn the_host_tells_the_extension_a_run_started_or_went_or_files_changed() {
    round_trip(
        &json!({"type": "run_started", "pid": 4242, "goal": "Star the repository", "sites": ["https://github.com"]}),
        &Message::RunStarted {
            pid: 4242,
            goal: "Star the repository".into(),
            sites: vec!["https://github.com".into()],
        },
    );
    round_trip(&json!({"type": "release"}), &Message::Release);
    round_trip(&json!({"type": "reload"}), &Message::Reload);
}

// --- constants ---

/// Chrome names an unpacked extension by the first 16 bytes of the SHA-256
/// of its public key, in DER, each hex digit written as a letter from `a`
/// (0) to `p` (15). The manifest's `key` pins it, so the host manifest can
/// name it in `allowed_origins`.
#[test]
fn the_pinned_extension_id_is_derived_from_its_key() {
    let der = base64_decode(bridge::EXTENSION_KEY);
    let digest = sha256(&der);
    let id: String = digest[..16]
        .iter()
        .flat_map(|byte| [byte >> 4, byte & 0xf])
        .map(|nibble| char::from(b'a' + nibble))
        .collect();
    assert_eq!(id, bridge::EXTENSION_ID);
    assert_eq!(bridge::EXTENSION_ID.len(), 32);
}

#[test]
fn the_host_is_named_as_chrome_looks_it_up() {
    assert_eq!(bridge::HOST_NAME, "dev.jevpilot.bridge");
    assert_eq!(bridge::PROTOCOL, 1);
}

/// The checks below are written out here rather than taken from a crate, so
/// that the test does not lean on the code it checks; they are checked
/// against known answers first.
#[test]
fn the_checks_own_hash_and_decoder_give_known_answers() {
    assert_eq!(
        hex(&sha256(b"abc")),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        hex(&sha256(&[b'a'; 200])),
        "c2a908d98f5df987ade41b5fce213067efbcc21ef2240212a41e54b5e7c28ae5"
    );
    assert_eq!(base64_decode("aGVsbG8gd29ybGQ="), b"hello world");
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn base64_decode(text: &str) -> Vec<u8> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut bits = 0u32;
    let mut held = 0;
    let mut out = Vec::new();
    for c in text.bytes().filter(|c| *c != b'=') {
        let value = ALPHABET.iter().position(|a| *a == c).expect("base64") as u32;
        bits = (bits << 6) | value;
        held += 6;
        if held >= 8 {
            held -= 8;
            out.push((bits >> held) as u8);
            bits &= (1 << held) - 1;
        }
    }
    out
}

fn sha256(message: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut data = message.to_vec();
    let length_bits = (message.len() as u64) * 8;
    data.push(0x80);
    while data.len() % 64 != 56 {
        data.push(0);
    }
    data.extend_from_slice(&length_bits.to_be_bytes());
    for block in data.chunks(64) {
        let mut w = [0u32; 64];
        for (i, word) in block.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (slot, value) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *slot = slot.wrapping_add(value);
        }
    }
    let mut out = [0u8; 32];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

// --- the handshake ---

use bridge::Nonce;

/// The host opens the local channel with a fresh 32-byte nonce, and the run
/// must echo it in its first frame. A nonce is new every time.
#[test]
fn the_host_sends_a_fresh_nonce_and_the_run_echoes_it() {
    let nonce = Nonce::fresh();
    let Message::Nonce { nonce: sent } = nonce.message() else {
        panic!("a nonce message");
    };
    assert_eq!(sent.len(), 64, "32 bytes as hex: {sent}");
    assert!(sent.bytes().all(|b| b.is_ascii_hexdigit()));
    assert_ne!(Nonce::fresh().message(), nonce.message());

    let echo = Nonce::echoing(&nonce.message()).expect("a nonce is echoed");
    assert_eq!(
        echo,
        Message::Echo {
            nonce: sent.clone()
        }
    );
    assert!(nonce.is_echoed_by(&echo));
}

/// Anything but the same nonce, echoed, fails the handshake.
#[test]
fn a_wrong_or_missing_echo_fails_the_handshake() {
    let nonce = Nonce::fresh();
    let other = Nonce::fresh();
    assert!(!nonce.is_echoed_by(&Nonce::echoing(&other.message()).expect("echoes")));
    let Message::Nonce { nonce: sent } = nonce.message() else {
        panic!("a nonce message");
    };
    assert!(!nonce.is_echoed_by(&Message::Nonce { nonce: sent }));
    assert!(!nonce.is_echoed_by(&Message::Absent));
    assert_eq!(
        Nonce::echoing(&Message::Absent),
        None,
        "only a nonce is echoed"
    );
}
