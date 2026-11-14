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
