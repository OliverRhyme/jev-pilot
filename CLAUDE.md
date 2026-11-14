# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

A Rust crate plus two binaries that drive a mobile UI, or a tab of a web browser, with TypeSafe Jev
(a System One model) as the decision provider. **Code lists every action the current screen
allows, and the model only selects one by option id.** It never generates coordinates or actions.
`README.md` gives the reasoning; `FIELD-NOTES.md` records what broke on real hardware and is the
place to add new observations.

## Commands

CI (`.github/workflows/ci.yml`) runs these, and a change should pass all of them:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings   # also on Windows
cargo test --all-features
RUSTDOCFLAGS=-D warnings cargo doc --no-deps --all-features

# every feature must build on its own
cargo check --lib --no-default-features
cargo check --lib --no-default-features --features android   # likewise ios, http
cargo check --lib --no-default-features --features browser
cargo check --lib --no-default-features --features extension # on macOS, Linux and Windows

# the Chrome extension's tests (Node 24, no packages); the glob is needed, a folder is not
# taken as one
node --test 'extension/test/*.test.js'
```

- One test file: `cargo test --all-features --test pilot`. One test: add its name as a filter,
  e.g. `cargo test --all-features --test adb text_with_shell`.
- `tests/mcp.rs` is gated by `#![cfg(feature = "mcp")]`; `tests/cdp.rs`, `tests/browser.rs`,
  `tests/session.rs`, `tests/page.rs`, `tests/bridge.rs` and `tests/packaged.rs` by
  `#![cfg(feature = "browser")]`; and `tests/host.rs`, `tests/install.rs` and `tests/peer.rs` by
  `#![cfg(feature = "extension")]`. A plain `cargo test` skips them without saying so. Use
  `--all-features`.
- `.cargo/config.toml` sets `build.warnings = "deny"`: any warning fails the local build too.
  `missing_docs = "deny"` means every public item needs a doc comment. Clippy `pedantic` is on.
- The MSRV is 1.98 (edition 2024), and CI checks it. A change that raises it counts as a minor
  version change. Fresh browser profiles rely on `std::fs::File::try_lock` (stable since 1.89),
  and the code uses let-chains (1.88).
- The tests are pure and need no device, browser or API key. The loop runs against a scripted
  `Judge`, `adb` invocations are tested as argument vectors, and hierarchy parsing uses the XML
  dumps in `tests/fixtures/`. DevTools traffic is checked as JSON messages over a scripted wire;
  `tests/fixtures/cdp/` holds a real Chrome's replies (how to record them again is at the top of
  `tests/cdp.rs`), and `tests/fixtures/browser/` holds page reads for the Browser platform.
  `tests/fixtures/extension/` holds recorded bridge conversations (its `README.md` gives the
  format); `tests/cdp.rs` replays them from the run's side and `extension/test/` from the
  extension's, and `JEV_PILOT_RECORD=1` records them again.
- Running against hardware: `cargo run -- "<goal>"` (the `jev-pilot` binary; `--help` shows the
  flags), plus `cargo run -- observe`, `devices`, `helper [install]`. You need `TYPESAFE_API_KEY`
  or `TYPESAFE_API_KEY_FILE` (see `.env.example`) and an attached Android device.
- Running against a browser: `cargo run --features browser -- observe --browser fresh --headless
  --open <url>` needs no key and no sign-in (Chrome for Testing, Chromium or Chrome; `--chrome`
  names one). `--browser extension` and `--browser attach` drive the owner's own Chrome; leave
  both to them. Extension mode needs `jev-pilot extension install` (as an administrator, or
  `--dev` for a user-owned install that is not protected), and attach mode asks them to click
  Allow on every connection.
- MCP server: `cargo run --features mcp -- mcp` (the `mcp` subcommand of the one binary).

## Architecture

One step: `observe → Snapshot → Catalog → StepQuestions → Jev → StepAnswers → resolve → Act →
Command → device`. A browser is one more device: its page read becomes a `Snapshot` like any
screen, and the loop does not know which it is driving.

- **`snapshot`**: each `Snapshot` gets a fresh `Generation`, and every `ElementRef` carries that
  stamp, so a reference from an old screen will not resolve against the new one. Pixel geometry
  lives only here. `ElementRef` has no bounds field, so the model never sees a coordinate.
- **`act`**: `Operation` (what to do) and the target row (which one) are two separate heads of one
  request. They are not one flat product list. `Act` is a sum type, so an invalid action does not
  compile. `Catalog::resolve` applies the confidence `Floors` and refuses when the distribution is
  flat.
- **`step` / `judgment`**: `StepQuestions` and `StepAnswers` mirror each other field for field, and
  their field names are the wire keys. Every question in a step (operation, tap_target, goal_met,
  is_error, acceptance claims) goes in a single request, and the server evaluates them in
  parallel. To add a question, add it to both structs.
- **`client`**: builds the `/v1/systemone` request as a plain value. The blocking `ureq` transport
  sits behind the `http` feature. The wire types are written by hand because no Rust SDK exists.
- **`pilot`**: the synchronous loop. `Judge` is the seam that tests script. `Escalate` is where
  impasses go. An escalation answers with an index into the same catalog Jev saw and can never
  build an action of its own. Asking Jev again about an unchanged screen is pointless, because it
  gives the same distribution.
- **`platform`**: `Platform` covers only hierarchy parsing and the system gestures available. The
  Android reader is verified on a Pixel 8 Pro. The iOS reader has never run on hardware, and its
  fixture was written by hand.
- **`device`**: `Device` turns an `Act` into a `Command`. The `Device::Command` enum is
  deliberately *not* `#[non_exhaustive]`, so a new gesture must be handled by every adapter. The
  browser-only commands `Press`/`Fill` (a point plus the engine's opaque node key), `Navigate`,
  `SwitchTab` and `Dialog` are answered with an `Unsupported` error by the phone adapters.
  `Device::outcome()` (defaulted to `None`) says what the last command did; the pilot puts it in
  the next step's previous action and in the transcript. `Device::refused()` (defaulted to `false`)
  says a browser refused the command at click time: it was never made, so it is neither a repeat
  nor an action taken.
  `adb.rs` shells out to `adb`, and its argument building (remote shell quoting, the ASCII limit of
  `input text`) is pure. `helper.rs` handles the on-device accessibility helper (about 50ms per
  read, against about 2.5s for `uiautomator dump`). The reader is chosen once per run, because a
  `uiautomator dump` unbinds the helper's accessibility service.
- **`desk`**: where an impasse question goes. The terminal (stdin) and `<desk>/answer.json` are
  both live, and whichever answers first wins. A blank stdin line means "check the desk now".
- **`platform::page`** (feature `browser`): the `PageRead`, the engine-neutral contract between a
  browser engine and the Browser platform. Nodes are in paint order, with boxes in viewport CSS
  pixels; a same-origin frame's nodes follow their iframe, and a dialog's follow the dialog. A
  password field is `protected` and never carries its value.
- **`platform::browser`**: the Browser platform, page read to `Snapshot`. It picks rows (actionable
  roles, `isClickable`, named focusables), collapses a label into the field it names, offers rows
  within one viewport of the screen marked "off screen", keeps only a modal's rows under a modal,
  and gives two or three look-alikes their context.
- **`browser`** (feature `browser`): the engine seam and its Chrome DevTools Protocol engine,
  written by hand over one blocking `tungstenite` websocket (no CDP crate, no async runtime).
  The feature also brings `ctrlc`: on Ctrl-C, `main.rs` hands what to undo
  (`launch::Interrupted`) to a handler thread, because the run may be blocked on the browser or
  the model. A fresh run's browser is closed and its profile removed; an attach or extension run
  only lets go.
  - `engine`: the `BrowserEngine` trait (read, perform a `BrowserCommand`, outcome, tabs) and
    `CdpEngine`. Clicks are checked when made: same document as the read (node keys carry a
    document number), scrolled into view if off screen, and hit-tested with
    `DOM.getNodeForLocation`; a refused click is an outcome, not an error. Alerts are accepted and
    kept for the next read; other dialogs are held, and while one is held a read is the dialog
    alone, because Chrome answers nothing on that tab's session under it. Settling waits for a
    navigation, its load, and a quiet DOM watched from an isolated world.
  - `cdp` builds requests and parses replies as JSON values; `transport` is the `Wire` trait, the
    socket, and a `Connection` that matches replies by id and queues events; `read` turns raw
    replies into a `PageRead`; `input` holds the input events and the engine's fixed page scripts.
    All of it is pure but the socket.
  - `discover` finds a running Chrome through its `DevToolsActivePort` file and names each attach
    failure; `launch` starts a fresh browser on a throwaway profile that holds a file lock for as
    long as its run lives, so a profile whose lock is free belongs to a run that has gone;
    `device` is `BrowserDevice`, the `Device` over any engine.
  - `bridge` is the protocol between a run, the host and the extension (message types, framing,
    size limits, the nonce handshake, the pinned extension ID and its manifest `key`);
    `extension` translates CDP to bridge frames and back, and `ExtensionWire` is a `Wire` over any
    byte stream, so `CdpEngine` drives a tab through the extension with no change to the reader,
    the hit test or the catalog. Both are pure. `packaged::FILES` is every file of `extension/`,
    embedded with `include_bytes!`.
- **Extension mode** (feature `extension`, `--browser extension`): the owner's Chrome through the
  jev-pilot extension, with no click per run.
  - `browser::host` is the native messaging host Chrome starts (`jev-pilot` enters host mode when
    its first argument is the extension's origin): a pure `Relay` state machine, the local channel
    (a Unix socket, or a named pipe on Windows, through `interprocess`), and the audit log.
    `browser::install` plans the install as steps (tested as values, like `adb.rs`), carries them
    out, and reads `install.json`; a `--browser extension` run execs the installed copy, because
    only that copy passes the host's check.
  - `peer` gathers facts about the program at the other end of the channel from the OS (macOS
    audit token and code signature, Linux `SO_PEERCRED` and the setgid group, Windows pipe pid,
    image path and hash or Authenticode signer), and a pure `decide` compares them with what the
    install pinned.
  - `jev-pilot-os` (a workspace member) is the only crate that may use `unsafe`, for the few OS
    calls no safe crate offers (Windows `QueryFullProcessImageNameW`, `WinVerifyTrust` and
    friends; macOS `setregid`). Each such call sits in a function that allows `unsafe` for itself
    alone, every `unsafe` block carries a `// SAFETY:` comment (`undocumented_unsafe_blocks` is
    denied), and it exposes only safe functions. The main crate keeps `unsafe_code = "forbid"`.
    Its code is Windows- and macOS-only, which is why CI also runs clippy on Windows.
- **`mcp`** (feature `mcp`, subcommand `jev-pilot mcp`, built on `rmcp`): every tool runs the
  `jev-pilot` binary instead of driving the loop in-process, so the MCP server and the CLI cannot
  drift apart. A run is keyed by its device serial, or `browser:extension`,
  `browser:attach:<channel>` or `browser:fresh`, which allows one run per target. Extension mode
  maps to `--browser extension --sites-only`, and a development install is refused unless the
  server's environment has `JEV_PILOT_ALLOW_DEV_BRIDGE=1`. Stopping a run kills its process, so the
  server then removes the fresh profile and browser it left (`launch::reap_run`). `start_run` and
  `answer_run` block for up to about 45s and return the run's question, its ending, or "still
  working". The server does not use sampling (see the README for why).
- **`cli`**: a hand-written argument parser (no clap). Parsing is kept separate from execution so
  it can be tested. The browser flags parse into an optional `BrowserTarget` on `Run`, `Observe`
  and `Devices`; `main.rs` connects once (attach mode raises Chrome's prompt on every handshake,
  so it never probes or retries), checks the endpoint is Chrome, and runs the same loop. The
  `extension` subcommand (`install [--dev]`, `status`, `uninstall`, `log`, `policy sites
  add|remove <origin>`, `policy owner-tabs group|never`) and host mode parse into their own
  `Invocation`s.

## The Chrome extension (`extension/`)

An MV3 extension in plain ES modules, with no build step and no npm packages: `manifest.json`
(its `key` pins the ID in `browser::bridge`, and its `version` is the crate's), the service worker
(the `chrome.*` glue), the pure `policy.js`, `protocol.js`, `bridge.js` and `connection.js`, the
popup, the icons, and the default `policy.json`. `scripts.json` is the page-script allow list and
must equal `input::PAGE_SCRIPTS`. The binary embeds every file but `test/` through
`browser::packaged`, and the install writes them to an admin-owned folder.

`extension/test/` runs under `node --test` against a fake `chrome` object; it covers every policy
rule, the protocol module, and every recorded conversation in `tests/fixtures/extension/`.
`tests/packaged.rs` fails if a file under `extension/` (other than `test/`) is missing from
`packaged::FILES`, or if `scripts.json` differs from the engine's scripts. Adding a file to the
extension means adding it to `src/browser/packaged.rs`.

## The Android helper (`helper/`)

This is a Gradle project with `service`, `reader` and `shared` modules. The crate embeds
`helper/JevPilotHelper.apk`, `JevPilotReader.apk` and their `*_manifest.json` with
`include_bytes!`/`include_str!`. After changing the helper, run `helper/build.sh` (it needs the
Android SDK and JDK 21), then commit both APKs and both manifests together. Installing the helper
must never happen silently.

## Conventions

- Public enums that are expected to grow (operations, endings, failure reasons) are
  `#[non_exhaustive]`. `Device::Command` and `cli::Invocation` deliberately are not.
- `ApiKey` has a redacting `Debug`, no `Display` and no `Serialize`. Read it with `.expose()` only
  at the point of use.
- Doc comments explain *why*, in prose. Product names (TypeSafe, UIAutomator, XCUITest) are not
  backticked, and `doc_markdown` is allowed for that reason.
- Acceptance claims should state one fact per criterion. Claims that bundle several facts score
  badly; the README has the numbers.
- `.python-reference/` holds the Python prototype this crate replaced. It is gitignored and not
  part of the project.
- `publish = false`: this is a personal project and is not published to crates.io.
