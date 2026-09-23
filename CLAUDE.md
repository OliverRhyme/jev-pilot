# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

A Rust crate plus two binaries that drive a mobile UI with TypeSafe Jev (a System One model) as the
decision provider. **Code lists every action the current screen allows, and the model only selects
one by option id.** It never generates coordinates or actions. `README.md` gives the reasoning;
`FIELD-NOTES.md` records what broke on real hardware and is the place to add new observations.

## Commands

CI (`.github/workflows/ci.yml`) runs these, and a change should pass all of them:

```sh
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
RUSTDOCFLAGS=-D warnings cargo doc --no-deps --all-features

# every feature must build on its own
cargo check --lib --no-default-features
cargo check --lib --no-default-features --features android   # likewise ios, http
```

- One test file: `cargo test --all-features --test pilot`. One test: add its name as a filter,
  e.g. `cargo test --all-features --test adb text_with_shell`.
- `tests/mcp.rs` is gated by `#![cfg(feature = "mcp")]`, so a plain `cargo test` skips it
  without saying so. Use `--all-features`.
- `.cargo/config.toml` sets `build.warnings = "deny"`: any warning fails the local build too.
  `missing_docs = "deny"` means every public item needs a doc comment. Clippy `pedantic` is on.
- The MSRV is 1.98 (edition 2024), and CI checks it. A change that raises it counts as a minor
  version change.
- The tests are pure and need no device or API key. The loop runs against a scripted `Judge`, `adb`
  invocations are tested as argument vectors, and hierarchy parsing uses the XML dumps in
  `tests/fixtures/`.
- Running against hardware: `cargo run -- "<goal>"` (the `jev-pilot` binary; `--help` shows the
  flags), plus `cargo run -- observe`, `devices`, `helper [install]`. You need `TYPESAFE_API_KEY`
  or `TYPESAFE_API_KEY_FILE` (see `.env.example`) and an attached Android device.
- MCP server: `cargo run --features mcp -- mcp` (the `mcp` subcommand of the one binary).

## Architecture

One step: `observe → Snapshot → Catalog → StepQuestions → Jev → StepAnswers → resolve → Act →
Command → device`.

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
  deliberately *not* `#[non_exhaustive]`, so a new gesture must be handled by every adapter.
  `adb.rs` shells out to `adb`, and its argument building (remote shell quoting, the ASCII limit of
  `input text`) is pure. `helper.rs` handles the on-device accessibility helper (about 50ms per
  read, against about 2.5s for `uiautomator dump`). The reader is chosen once per run, because a
  `uiautomator dump` unbinds the helper's accessibility service.
- **`desk`**: where an impasse question goes. The terminal (stdin) and `<desk>/answer.json` are
  both live, and whichever answers first wins. A blank stdin line means "check the desk now".
- **`mcp`** (feature `mcp`, subcommand `jev-pilot mcp`, built on `rmcp`): every tool runs the
  `jev-pilot` binary instead of driving the loop in-process, so the MCP server and the CLI cannot
  drift apart. A run is keyed by its device serial, which allows one run per device. `start_run` and
  `answer_run` block for up to about 45s and return the run's question, its ending, or "still
  working". The server does not use sampling (see the README for why).
- **`cli`**: a hand-written argument parser (no clap). Parsing is kept separate from execution so
  it can be tested.

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
