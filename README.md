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
`ureq`. No Appium, no WebDriverAgent client, no `mobile-use`, no MCP server. The
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
