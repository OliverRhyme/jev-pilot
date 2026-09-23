# jev-pilot

Drive a mobile UI with [TypeSafe Jev] as the decision provider.

Most mobile agents put a large model in the loop and ask it *what to do*. It
answers in prose or JSON you parse, containing coordinates it invented. Every
step is a generation call, and every step can hallucinate a tap.

`jev-pilot` inverts that. **Code enumerates every action the current screen
allows; the model only picks one.** Its entire vocabulary is a list of option
ids, so it cannot invent a coordinate, name an element that is not on screen, or
act on a screen that has since been replaced.

```text
observe ─► Snapshot ─► Catalog ─┬─► operation   ─┐
                                └─► tap_target  ─┤ one request
                                    goal_met    ─┤ evaluated in
                                    is_error    ─┘ parallel
                                          │
                                       resolve
                                          │
                                     Act ─► Command ─► device
```

The action space has two dimensions, asked as separate heads of one request.
`operation` says *what* to do; `tap_target` says *which row* to do it to. The
target head is answered speculatively — filled in whether or not the chosen
operation needs it — because questions in a map are evaluated in parallel, so
asking costs nothing and having the answer saves a round trip.

A flat list of every operation crossed with every row would grow as their
product and could express pairings that make no sense: a scroll aimed at a
button, a tap aimed at nothing.

## Install

One package, two programs, prebuilt for macOS (Apple silicon and Intel), Linux
(x86-64 and ARM64) and Windows (x86-64). No Rust toolchain needed.

| Program | What it is |
| --- | --- |
| `jev-pilot` | The command line. It observes, asks Jev, and drives the device. |
| `jev-pilot-mcp` | An MCP server over stdio. Every tool runs the `jev-pilot` installed beside it, so the two cannot drift apart. |

Because the server runs the command line rather than containing the loop, the two
are always installed together, into one directory.

### 1. Install jev-pilot

macOS and Linux:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/OliverRhyme/jev-pilot/releases/latest/download/jev-pilot-installer.sh | sh
```

Windows (PowerShell):

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/OliverRhyme/jev-pilot/releases/latest/download/jev-pilot-installer.ps1 | iex"
```

Both put the two programs in `~/.jev-pilot/bin` (`%USERPROFILE%\.jev-pilot\bin` on
Windows) and add it to your `PATH`; open a new terminal afterwards. To install by
hand instead, download the archive for your platform from the
[releases page](https://github.com/OliverRhyme/jev-pilot/releases), unpack it, and
keep both programs in the same directory.

While the repository is private, the one-line installers cannot fetch anything
for someone who is not signed in. With access and the GitHub CLI, download the
archive directly — for example, on an Apple silicon Mac:

```sh
gh release download -R OliverRhyme/jev-pilot -p 'jev-pilot-aarch64-apple-darwin.tar.xz'
```

The archives are named `jev-pilot-<target>`: `aarch64-apple-darwin`,
`x86_64-apple-darwin`, `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`
(`.tar.xz`), and `x86_64-pc-windows-msvc` (`.zip`).

### 2. Install adb

jev-pilot drives the phone through Android's `adb`:

| | |
| --- | --- |
| macOS | `brew install --cask android-platform-tools` |
| Windows | `winget install Google.PlatformTools` |
| Debian, Ubuntu | `sudo apt install adb` |
| Anything else | Android's [platform tools], unpacked and on your `PATH` |

[platform tools]: https://developer.android.com/tools/releases/platform-tools

Then turn on USB or wireless debugging on the phone, connect it, and accept the
prompt to trust this computer. `adb devices` should list it as `device`, not
`unauthorized`.

### 3. Give it your TypeSafe key

Read at run time, never compiled in. Either variable works; the file wins when
both are set, and is the better choice because the key stays out of the
environment of every process you start.

macOS and Linux, in your shell's profile:

```sh
export TYPESAFE_API_KEY_FILE="$HOME/.config/typesafe/api-key"   # a file holding the key
# or: export TYPESAFE_API_KEY=ts-live-...
```

Windows, once (applies to new terminals):

```powershell
setx TYPESAFE_API_KEY_FILE "$env:USERPROFILE\.config\typesafe\api-key"
```

The programs do not read a `.env` file themselves.

### 4. Check the device

```console
$ jev-pilot devices           # the serials adb can see
$ jev-pilot helper            # whether the on-device helper is installed and on
$ jev-pilot helper install    # install it and switch its accessibility service on
$ jev-pilot observe           # what the next step would be offered, changing nothing
```

The helper is optional and strongly recommended: it reads a screen in about 60ms
where `uiautomator dump` takes about 2.5s, and it reports only what is actually on
screen. It is an accessibility service, so it can read every screen on that
device; it answers only on loopback, to a caller holding a per-run token, and is
built from the source in `helper/`. Nothing installs it except `helper install`.

### 5. Run a goal

```console
$ jev-pilot --app com.google.android.youtube --then "Tap Search" --then "Type opus 5.5 into the search box and submit" --then "Tap a video about Opus 5.5" --text "search=opus 5.5" --accept "A video is open in the player" --floor 0.4 "Find a video about Opus 5.5 and play it"
```

`jev-pilot --help` lists every flag. A run that cannot decide asks at the terminal.

### 6. Register the MCP server (optional)

The server needs the key in its own environment: the client starts it, and it
starts the command line, which inherits it. With Claude Code:

```console
$ claude mcp add jev-pilot -s user -e TYPESAFE_API_KEY_FILE=/path/to/api-key -- jev-pilot-mcp
```

Any other client, wherever it keeps its servers:

```jsonc
{
  "mcpServers": {
    "jev-pilot": {
      "command": "jev-pilot-mcp",
      "env": { "TYPESAFE_API_KEY_FILE": "/path/to/api-key" }
    }
  }
}
```

A client that does not start servers with your `PATH` needs the full path:
`~/.jev-pilot/bin/jev-pilot-mcp`, or on Windows
`C:\Users\<you>\.jev-pilot\bin\jev-pilot-mcp.exe`.

### Updating

Run the installer again; it replaces both programs. A server already running
keeps its old tool list until it is restarted, so reconnect it in the client
afterwards — in Claude Code, `/mcp` and then reconnect `jev-pilot`.

A reconnect does not always replace the process. If the tools still lack a new
argument, end the old server and reconnect:

```sh
pkill jev-pilot-mcp                      # macOS, Linux
```

```powershell
Stop-Process -Name jev-pilot-mcp         # Windows
```

### From source

With Rust 1.98 or newer, from a clone:

```console
$ cargo install --path . --features mcp --locked
```

This installs into `~/.cargo/bin` instead. Leave off `--features mcp` for the
command line alone; the server brings an async runtime the command line does not
need.

### Releasing

Releases are built by [`dist`](https://opensource.axo.dev/cargo-dist/), configured
in `dist-workspace.toml`. Pushing a version tag builds every platform on its own
runner and publishes the archives and installers:

```console
$ git tag v0.1.0 && git push origin v0.1.0
```

## Why it is fast

Two properties, both structural:

- **Select, not generate.** An answer is an option id and a probability
  distribution. Nothing to parse, repair, or retry.
- **One round trip per step.** The endpoint evaluates a whole question map in
  parallel against one state, so a step asking *"which action?"*, *"is the goal
  met?"* and *"is this an error screen?"* costs what asking one costs.

## What the types guarantee

- **References cannot outlive their screen.** Every `Snapshot` is stamped with a
  fresh generation; an `ElementRef` carries that stamp. A reference taken before
  a tap will not resolve against the screen after it — even if the screen looks
  identical, because it may have rebound every row underneath.
- **Geometry never reaches the model.** `ElementRef` has no `bounds` field. Tap
  points exist only inside `Snapshot`, reachable solely by handing back a
  reference it issued.
- **Illegal actions do not compile.** `Act` is a sum type: a tap without a
  target, or typed text with no field to receive it, is a type error.
- **Uncertainty stops the loop.** `Catalog::resolve` takes a confidence floor and
  refuses rather than acting on a flat distribution.

## Operations

`tap`, `double_tap`, `long_press`, `swipe_left`, `swipe_right`, `peek`,
`scroll_up`, `scroll_down`, `back`, `home`, `app_switcher`, `wait`, `done`,
`blocked` — each platform offers the subset it actually supports.

`scroll_down` is not a convenience. A virtualised Android list only materialises
the rows it is showing, so anything past the fold is genuinely absent from the
screen as observed and unreachable without it.

## When Jev is not sure

A flat distribution stops the run by default — acting on one is how a loop
wanders into unrelated application state. From there the caller chooses.

**Re-asking Jev the same question is pointless.** It returns a calibrated
distribution over the state it was given, so an unchanged state yields the same
answer and the same refusal. Progress needs either a changed screen (`wait`,
`scroll_down`) or a different judge.

That different judge is the `Escalate` trait — where a System Two model, or a
person, belongs:

```rust
let mut pilot = Pilot::new(device, judge, &Android)
    .requiring(Confidence::new(0.6).unwrap())
    .escalating_to(|impasse: &Impasse<'_>| {
        // impasse.rows, impasse.operations, impasse.because, impasse.previous
        Ok::<_, Infallible>(Resolution::Choose {
            operation: Operation::Tap,
            target: Some(1),
        })
    });
```

A second opinion answers with an **index into the same catalog Jev was offered**,
never an action it built itself. So escalating widens *who decides* without
widening *what may happen*: a reasoning model cannot name an operation the
platform lacks, a row that is not on screen, or a coordinate — the same
constraints Jev works under. An out-of-range row is refused, not resolved into a
tap somewhere arbitrary.

A real run: Jev opened Network & Internet at confidence 1.00, stalled at 0.43
choosing among its sub-rows, handed over to a person who picked `Internet`, and
then recognised the goal met at 0.87 on its own.

## Driving from another agent

`jev-pilot-mcp` speaks the Model Context Protocol over stdin and stdout, so a
model holding a conversation can drive a device — and, more to the point, can
*be* the second opinion above. A run that cannot decide stops and asks; the
caller answers it and the run carries on.

Installing and registering it is under [Install](#install).

Built on [`rmcp`], the official SDK, so the protocol surface tracks the spec
rather than this repo. The server is async because `rmcp` is; the loop it drives
is not, and should not be — a step is observe, judge, act, settle, and each
waits on the one before, so there is nothing for a runtime to overlap. Several
devices at once is one process each, which also means a wedged phone cannot take
the others with it.

[`rmcp`]: https://crates.io/crates/rmcp

Seven tools. `observe`, `devices` and `run_status` only look. `start_run` acts
on a real device and says so in its description and its annotations, because a
client is going to put that in front of a person, and reading a screen and
moving money through one are not the same permission.

The server tells the client how to prompt it — how to write a goal, what makes
an `accept` claim checkable, how `text` keys are matched, what lowering the
floor does and does not loosen. Every one of those rules is one that has cost a
real run, so they are worth the words.

A run is four calls rather than one that blocks until it is over:

| | |
| --- | --- |
| `start_run` | begin, and come back with the first thing it wants |
| `run_status` | every step so far, what it is asking, or how it ended |
| `answer_run` | answer it — an operation and row it was offered, or the words to type — and come back with the next thing it wants |
| `stop_run` | end it, leaving the device where it got to |

Latency matters here more than anywhere else in the loop, because a run at an
impasse is doing *nothing* — not observing, not judging, not tapping — until an
answer reaches it. So neither direction waits on a timer if it can help it. The
question is noticed within 20ms of the run writing it. The answer does not wait
at all: the server keeps the run's own input open and writes an empty line to it
once the answer is on the desk, and the run treats a blank line as "look now".
That is the same channel a person types answers on, so the push path is the one
that was already there.

`start_run` and `answer_run` do not return the instant the run is under way.
They wait, and come back with one of three things: the question the run stopped
on, how it ended, or — after about forty-five seconds — that it is still
working and the caller's time is its own again. That last one is why the wait is
bounded: held open until a run finished, the call would trip a client's own
timeout and lose the run behind it.

Waiting is what makes the question reach a model at all. A run that asks and is
never answered waits five minutes and then gives up, so a design where the
caller has to think to come back and look is a design where good runs die of
inattention. Returning the question *as the result of the call that caused it*
puts it in front of the model in the ordinary way, with no callback and no
protocol feature behind it — the client is already waiting for a tool result.

A run is named by the device it drives, so there is no run id to carry: with one
phone attached, none of these needs an argument at all. That is not only
convenience — it makes *one run per device* structural. Two runs on one screen
take turns at it, each undoing what the other has just done, and an identifier of
their own would make that look like an ordinary thing to ask for. A device
already being driven is refused, and told to watch, answer or stop the run it has.

Runs end when the conversation does. Left going, one carries on tapping at
somebody's phone with nothing watching it and nothing able to answer it.

**When a run cannot decide, the client answers it** — on its own turn, through
the result of the call it is already waiting on. The model holding the
conversation *is* the second opinion, and its answer is still an index into the
catalog the run offered, never an action it invented.

There is a way for a server to call back into the client's model instead —
`sampling/createMessage` — and this does not use it. [SEP-2577] deprecates
sampling, tells new implementations not to adopt it, and names it the most
security-sensitive of the three features it removes, because it lets a server
put text of its choosing in front of somebody else's model. That is a pointed
objection here rather than a general one: the text would be whatever an app has
drawn on the screen, and the answer coming back moves money. A screen is not a
trustworthy author.

Elicitation, which asks the *person* rather than the model, is not deprecated
and would be the way to put a question in front of a user directly. It is not
wired up yet.

[SEP-2577]: https://modelcontextprotocol.io/seps/2577-deprecate-roots-sampling-and-logging

Every tool runs the `jev-pilot` binary rather than driving the loop in-process,
so the server cannot drift from the command line it is a face for, and an
answer written over MCP lands in the same desk a person at a terminal writes to.

## Reading the screen quickly

`uiautomator dump` costs about 2.4s per observation: it spawns a JVM and then
waits a hardcoded second for the accessibility event stream to fall quiet, with
no flag to relax either. An accessibility helper already holding that session
open answers in about 50ms over a tunnelled port, and emits the same document.

The two were compared on a scrolled search-results list: **23 of 23 labelled
elements matched in both directions**, no negative or off-screen bounds either
side. Their total node counts differ, because the helper prunes unlabelled
structural wrappers, but only labelled elements are ever acted on.

That comparison is worth repeating on a busy screen whenever either side
changes. A reader that silently drops a row is worse than a slow one: the loop
reads the absence as the screen genuinely not offering it, and reasons
confidently from a false premise.

## Confirming what was reached

A verdict of success can be refused by acceptance criteria — specific claims
that must hold before the run is allowed to report success:

```rust
let mut pilot = Pilot::new(device, judge, &Android)
    .confirming([
        "A video is currently playing",
        "The thing playing is a full-length video rather than a Short",
        "The video is about Jev or TypeSafe AI",
    ]);
```

Each becomes its own judgment in the same parallel request, so confirming is
free, and a rejected verdict is fed back into the next step rather than ending
the run: *"Declared the goal done, but that was rejected: … was not true."*

**Write one claim per criterion.** Bundling them reads naturally and fails.
Against a real player screen, `"A full-length video is playing, not a Short and
not a search results page"` scored **0.31**, while the same three claims asked
separately scored **0.86 / 0.86 / 0.18** — all correct. A judgment asked about
three things at once has no coherent yes.

## Platforms

`Platform` abstracts the only two things that actually differ: how a hierarchy
is spelled, and which system gestures exist. Android has a system Back button;
iOS does not, so an iOS catalog never offers one.

| | Hierarchy | System gestures | Status |
|---|---|---|---|
| Android | UIAutomator dump | Back, Home, App switcher | Drives a physical Pixel 8 Pro end to end |
| iOS | XCUITest | Home, App switcher | **Unverified against hardware** — see below |

### iOS caveat

The Android reader was built from a dump taken off a real handset, and doing so
immediately exposed a structural assumption that was wrong: Settings rows are
clickable containers with no text of their own, so labelling the leaf text
yields elements that read well and cannot be tapped. The iOS reader has had no
such contact with reality. Treat it as a starting point to correct against a
real hierarchy dump, not as a finished counterpart.

## Credentials

The key is read at runtime and never compiled in:

```sh
export TYPESAFE_API_KEY=ts-live-...
# or, preferred for services, a mounted secret file (takes precedence):
export TYPESAFE_API_KEY_FILE=/run/secrets/typesafe-api-key
```

`ApiKey` redacts itself in `Debug`, has no `Display` and no `Serialize`, and
requires an explicit `.expose()` to read — so a struct holding one cannot leak
it through a derived `Debug` in a panic message or error report.

## Dependencies

`serde`, `serde_json`, `roxmltree`, and — only with the default `http` feature —
`ureq`. The `mcp` feature adds `rmcp`, `tokio` and `schemars` for the server, and
nothing else uses them. No Appium, no WebDriverAgent client, no `mobile-use`. The
Android adapter shells out to `adb` directly.

Turn `http` off and the crate builds requests for you to send with whatever
client you already have; `Judge` is the seam, and the loop is tested against a
scripted one rather than against a model.

**iOS cannot be driven this way.** There is no `adb` equivalent: `simctl` cannot
tap or read a hierarchy, and `idb`'s UI commands are simulator-only. Anything
that reads the accessibility tree or injects touches on a physical device must
link `XCTest.framework` and run as a code-signed test bundle. That is Apple's
model, not a WebDriverAgent tax — writing our own runner would not avoid it.

## Status

The Android loop runs end to end against real hardware: it observes a live
screen, asks Jev, taps, re-observes, and stops. A run from the Settings home
screen toward "open the Storage screen" navigates there and reports
`Finished(Achieved)`.

There is no official Rust SDK for TypeSafe, so the wire types are hand-rolled
against the documented `/v1/systemone` contract.

Navigation is reliable: the first step lands on the right screen at confidence
1.00 across every goal tried. Recognising *completion* is weaker, and two bugs
found by running against hardware account for much of it.

**The target head was asked blind.** Its question named no goal, so it reduced
to "which row looks important?" and the answer spread across every plausible
row. Giving it the goal moved target confidence on one run from 0.52 to 1.00.

**Steps had no memory.** Each judgment saw only the current rows, so "is the
goal met?" was answered by something that could not tell a screen it had just
reached from one it had been stuck on, nor whether its last action changed
anything. Feeding the previous action into the state moved a stalled run from
`Uncertain` to `Finished(Achieved)`.

What remains is genuine model uncertainty on ambiguous sub-navigation rather
than a defect: asked to "open Wi-Fi settings", a run reaches Network & Internet,
correctly judges the goal unmet, and then declines to guess between sub-rows.
The floor refusing is the designed behaviour.

**Thresholds here are invented, not measured.** `CERTAINTY` (0.8) and the
confidence floor are defaults. The TypeSafe documentation is explicit that
thresholds must be evaluated against your own data and the cost of a wrong
action; a handful of runs is not that.

**A `Done` verdict is taken at face value.** Browser Use's Jev agent verifies
its own `DONE` independently, and that is the right shape. Not done here.

The iOS device layer is not implemented at all — see below.

## Scope

A personal project, not a release. `publish = false` in the manifest; it is not
on crates.io and is not headed there.

## Versioning

The conventions below are for keeping my own call sites from breaking, not for
anyone else's. Public enums that are expected to grow — operations, endings, failure reasons —
are `#[non_exhaustive]`, so a new gesture or a new way of stopping is a minor
release rather than a breaking one. `Device::Command` is deliberately *not*:
adding a gesture there should force every device adapter to acknowledge it at
compile time rather than silently ignore it.

Raising the minimum supported Rust version is treated as a minor-version change.

## License

MIT OR Apache-2.0.

[TypeSafe Jev]: https://typesafe.ai/
