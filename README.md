# jev-pilot

Drive a mobile UI, or a tab of a web browser, with [TypeSafe Jev] as the
decision provider.

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

One program, `jev-pilot`, prebuilt for macOS (Apple silicon and Intel), Linux
(x86-64 and ARM64) and Windows (x86-64). No Rust toolchain needed.

| Command | What it does |
| --- | --- |
| `jev-pilot "<goal>"` | Observes the screen, asks Jev, and drives the device. |
| `jev-pilot --browser extension\|fresh\|attach "<goal>"` | The same, in a Chrome tab. See [A web browser](#a-web-browser). |
| `sudo jev-pilot extension install` | Sets up the bridge to Chrome's jev-pilot extension, once per machine. See [The jev-pilot extension](#the-jev-pilot-extension). |
| `jev-pilot mcp` | Serves the Model Context Protocol over stdio, so an agent can start runs and answer their questions. Each tool call runs this same program, so the server and the command line cannot drift apart. |

### 1. Install jev-pilot

macOS and Linux:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/OliverRhyme/jev-pilot/releases/latest/download/jev-pilot-installer.sh | sh
```

Windows (PowerShell):

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/OliverRhyme/jev-pilot/releases/latest/download/jev-pilot-installer.ps1 | iex"
```

Both put `jev-pilot` in `~/.jev-pilot/bin` (`%USERPROFILE%\.jev-pilot\bin` on
Windows) and add it to your `PATH`; open a new terminal afterwards. To install by
hand instead, download the archive for your platform from the
[releases page](https://github.com/OliverRhyme/jev-pilot/releases), unpack it, and
put `jev-pilot` anywhere on your `PATH`.

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
$ claude mcp add jev-pilot -s user -e TYPESAFE_API_KEY_FILE=/path/to/api-key -- jev-pilot mcp
```

Any other client, wherever it keeps its servers:

```jsonc
{
  "mcpServers": {
    "jev-pilot": {
      "command": "jev-pilot",
      "args": ["mcp"],
      "env": { "TYPESAFE_API_KEY_FILE": "/path/to/api-key" }
    }
  }
}
```

A client that does not start servers with your `PATH` needs the full path:
`~/.jev-pilot/bin/jev-pilot`, or on Windows
`C:\Users\<you>\.jev-pilot\bin\jev-pilot.exe`.

### Updating

```console
$ jev-pilot --version        # which version this is
$ jev-pilot update           # update to the latest release, if there is a newer one
```

`update` asks GitHub for the latest release and does nothing when you already
have it. Otherwise it runs the same installer you installed with, which replaces
the program in place — on every platform, including Windows, where a running
program cannot simply be overwritten. A copy built from source is not replaced by
a download; `update` says to rebuild it instead. Running the installer again by
hand does the same thing.

A server already running keeps its old tool list until it is restarted, so
reconnect it in the client afterwards — in Claude Code, `/mcp` and then reconnect
`jev-pilot`. A reconnect does not always replace the process; if the tools still
lack a new argument, end the old server and reconnect:

```sh
pkill -f "jev-pilot mcp"                 # macOS, Linux
```

```powershell
Get-CimInstance Win32_Process -Filter "CommandLine LIKE '%jev-pilot%mcp%'" | Invoke-CimMethod -MethodName Terminate   # Windows
```

### From source

With Rust 1.98 or newer, from a clone:

```console
$ cargo install --path . --features mcp,browser,extension --locked
```

This installs into `~/.cargo/bin` instead. Leave off `mcp` for a build without
the server; it brings an async runtime nothing else needs. Leave off `browser`
(and `extension`, which needs it) for a build that drives phones only, or
`extension` alone for a browser build without the bridge to Chrome's
extension. The prebuilt releases have all three.

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

`jev-pilot mcp` speaks the Model Context Protocol over stdin and stdout, so a
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
| `stop_run` | end it, leaving the device or tab where it got to |

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

**A browser over MCP.** `observe`, `devices` and `start_run` take a `browser`
argument in place of `device` (`mode`, `tab`, `open`, `channel`, `endpoint`,
`chrome`, `headless`), and `start_run` takes `urls` in place of `app`; each
becomes the command line's flag, and a call naming both a device and a browser
is refused. A browser run is named `browser:extension`,
`browser:attach:<channel>` or `browser:fresh`, beside device serials, so the
rule of one run per target still holds and a browser run never blocks a phone
run.

`mode: "extension"` is the one to use for the owner's own Chrome. It becomes
`--browser extension --sites-only`, and nothing waits for a click: the owner
set the extension up once, and its policy decides which sites and tabs a run
may have. The server refuses a [development install](#a-development-install)
unless it was started with `JEV_PILOT_ALLOW_DEV_BRIDGE=1`, and says so in the
refusal, because another agent would read straight past the warning a run
prints for one. Its instructions tell the client what to pass on to the owner
when the extension is not set up, is paused, refuses a site, or is asked for
a tab outside the "jev-pilot" group, and never to retry a refused identity
check.

Attach mode is kept below it as the fallback. Every tool call is its own
connection, so each one needs its own click on Allow; a look waits forty
seconds for it, because a client gives up long before the command line's two
minutes. The server's instructions tell the client never to retry a refused
attach. Stopping a fresh run closes its browser and removes its profile.

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

When every criterion holds, the run ends there, achieved, whatever the model
chose next, except that a run given a plan (`following`, or `--then`) must first
have done something, because criteria that held before any of the plan's work
was done (a star button reading "Star" before a star-then-unstar) say nothing
about it.

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
| Browser | Accessibility tree + layout, over the DevTools Protocol | Back | Reads and acts on Chrome for Testing 157 in fresh mode, and on an everyday Chrome in attach mode, where a click on Allow for every connection led to the extension; extension mode is tested against recorded conversations and awaits its first runs |

### iOS caveat

The Android reader was built from a dump taken off a real handset, and doing so
immediately exposed a structural assumption that was wrong: Settings rows are
clickable containers with no text of their own, so labelling the leaf text
yields elements that read well and cannot be tapped. The iOS reader has had no
such contact with reality. Treat it as a starting point to correct against a
real hierarchy dump, not as a finished counterpart.

## A web browser

`--browser` drives a tab of Chrome (or, in fresh and attach mode, Chrome for
Testing or Chromium) with the same loop, the same desk and the same rules: code
lists what the page allows, and Jev picks one. It needs a build with the
`browser` feature, and extension mode the `extension` feature; the prebuilt
releases have both.

The engine speaks the Chrome DevTools Protocol itself. Each step reads the
tab's accessibility tree and its layout (boxes, paint order, which elements
have click listeners) and joins them into a page read; the Browser platform
turns that into rows, just as the Android reader turns a UIAutomator dump into
rows. The same messages go either over one websocket to 127.0.0.1 or through
the jev-pilot extension's `chrome.debugger`, and nothing above the transport
knows which.

### Three ways to reach a browser

| Mode | What it drives | What it asks of you |
| --- | --- | --- |
| `--browser extension` | Your own Chrome, signed in as you, through the jev-pilot extension | Nothing per run, after a one-time install as an administrator |
| `--browser fresh` | A browser started for the run, on a throwaway profile | Nothing |
| `--browser attach` | Your own Chrome, over remote debugging | A click on Allow for every connection |

Use the extension for your own Chrome. Attach mode is the fallback for a
Chrome without it.

### The jev-pilot extension

**`--browser extension`** drives the Chrome you already have open through a
small extension that ships inside jev-pilot. A run speaks to it over a local
socket (a named pipe on Windows) to a native messaging host that Chrome
starts, which is the installed jev-pilot itself. Nobody clicks anything during
a run: what a run may do is set once, by an administrator, in the extension's
policy.

You see every run:

- a notification as it starts: "jev-pilot is driving <site>: <goal>";
- Chrome's "started debugging this browser" bar. Pressing Cancel on it lets go
  of every tab at once, and the run ends saying so;
- the run's tabs, in a tab group named "jev-pilot";
- the extension's toolbar button. Its badge shows "!" when the bridge is
  missing, and its popup shows the bridge's state, the browser, whether the
  install is protected or a development one, the current run, the policy, and
  a pause switch.

The pause switch refuses every new tab, attach and command until you turn it
off, and a run that meets it stops with "jev-pilot is paused in Chrome: resume
it from the extension's toolbar button". It is a convenience, not a security
control. A run never closes one of your tabs, and when it ends, however it
ends, the extension lets go of every tab it held.

#### Installing it

Once per machine, as an administrator, after installing jev-pilot itself:

```console
$ sudo jev-pilot extension install           # macOS and Linux
```

```powershell
jev-pilot extension install                  # Windows, from a prompt run as administrator
```

The install copies the running jev-pilot where only an administrator can
write, pins its identity, writes the extension's files and its `policy.json`
(keeping an existing one), registers the host with Google Chrome, turns off
Chrome's user-level native messaging hosts, creates the audit log's
folder, and prints what it changed. Native messaging hosts you already had at
user level (Claude in Chrome's, for example) are copied to system level first,
so they keep working, and `uninstall` puts them back.

**Google Chrome only.** The host serves branded Google Chrome, stable, Beta,
Dev or Canary, and nothing else: not Chrome for Testing, Chromium, Edge or
Brave, which load any unpacked extension they are told to on the command line.
On Linux that means Chrome installed under `/opt/google/`; a Snap or Flatpak
browser is not served.

| | macOS | Linux | Windows |
| --- | --- | --- | --- |
| jev-pilot (also the host) | `/usr/local/libexec/jev-pilot/jev-pilot`, setgid `jev-pilot`, re-signed with the hardened runtime | `/usr/local/libexec/jev-pilot/jev-pilot`, setgid `jev-pilot` | `%ProgramFiles%\jev-pilot\jev-pilot.exe` |
| What each end expects of the other (`install.json`) | `/Library/Application Support/jev-pilot/` | `/etc/jev-pilot/` | `%ProgramFiles%\jev-pilot\` |
| The extension and its `policy.json` | `/Library/Application Support/jev-pilot/extension/` | `/usr/local/share/jev-pilot/extension/` | `%ProgramFiles%\jev-pilot\extension\` |
| User-level hosts turned off by | a configuration profile you approve once | `/etc/opt/chrome/policies/managed/jev-pilot.json` and each other browser's | `HKLM\SOFTWARE\Policies\Google\Chrome` and each other browser's |
| Audit log | `/Library/Logs/jev-pilot/` | `/var/log/jev-pilot/` | `%ProgramData%\jev-pilot\audit\` |

**On macOS**, Chrome honours the setting that turns user-level hosts off only
as a mandatory policy, which on a Mac nobody manages means a configuration
profile. The install writes one and opens it: approve it in System Settings ›
General › Device Management. Until you do, `jev-pilot extension status` says
"Chrome still accepts user-level native hosts", and a program running as you
could register a host of its own under jev-pilot's name. With the profile in
place, Chrome says it is
"managed by your organization"; that is the profile, and it sets nothing else.

**Then, in Chrome**, once:

1. Open `chrome://extensions` and turn on Developer mode.
2. Choose "Load unpacked" and pick the extension folder the install printed
   (the table above).
3. Run `jev-pilot extension status`. It says what is installed, how well it is
   protected, whether Chrome's policy is in place, the allowed sites, and
   whether the bridge answers.

**Developer mode must stay on.** Since Chrome 134 an unpacked extension runs
only with Developer mode on; switch it off and the extension is disabled at
the next start, and runs say "Chrome is not running with the jev-pilot
extension". Chrome reminds you that Developer mode is on when it starts. An
unpacked extension's files are not checked by Chrome against anything, which
is why the folder they are loaded from is one only an administrator can
write.

`sudo jev-pilot extension uninstall` (or elevated on Windows) removes what the
install made and restores the hosts it moved; on macOS it tells you to remove
the profile in System Settings.

#### Upgrading

`jev-pilot update` replaces the copy on your `PATH`, but the bridge serves only
the installed copy, so an upgrade needs the admin command again:

```console
$ jev-pilot update
$ sudo jev-pilot extension install
```

Installing again updates everything in place and re-pins the new binary's
identity. Chrome picks up the new extension by itself: the host sees the newer
files and the extension reloads, at most once per version, so there is nothing
to press in `chrome://extensions`. Until you install again, a run says "this
jev-pilot is <v>, the installed one is <w>: run the install again" and goes on
with the installed copy, which is the one any `--browser extension` run hands
itself to.

#### What a run may do: the policy

`policy.json` lives in the admin-owned extension folder, and the extension
reads it for every request it checks. Nothing a run sends, and nothing in
Chrome's storage, can change it. The popup shows it, read-only.

```console
$ sudo jev-pilot extension policy sites add https://github.com
$ sudo jev-pilot extension policy sites add "https://*.example.com"
$ sudo jev-pilot extension policy sites remove https://github.com
$ sudo jev-pilot extension policy owner-tabs group    # or never
```

- **Sites.** A run may open, attach to and navigate to only the listed
  origins; `https://*.example.com` covers the subdomains of example.com. An
  empty list, the default, allows any `http(s)` site. A tab the run holds that
  goes anywhere else is let go of at once, and the run ends with "the tab went
  to <origin>, which is not on the allowed sites".
- **Tabs.** A run may take the tabs it opened and tabs those opened. With
  `owner-tabs group`, the default, `--tab` may also take a tab of yours that you
  have put in the "jev-pilot" tab group yourself; anything else is refused with
  "put that tab in the 'jev-pilot' tab group first, so the run may use it".
  With `owner-tabs never`, a run uses only tabs it opened.
- **Always:** only the DevTools methods the engine uses (no Network, Storage,
  Fetch, or `Runtime.evaluate`), page script only when it is byte for byte one
  of the engine's own fixed scripts, and only `http(s)` pages and
  `about:blank`: never `chrome:`, `chrome-extension:`, `file:`, `devtools:`,
  `view-source:` or the Web Store.

#### How the bridge is protected

A run hands a page's text to Jev and acts on your signed-in sessions, so the
bridge is built so that only the installed jev-pilot can use it. Five layers:

1. **The local channel.** A Unix socket of mode 0600, in a folder of mode 0700
   that is yours (`~/Library/Application Support/jev-pilot/run/` on macOS;
   `$XDG_RUNTIME_DIR/jev-pilot/`, else `~/.local/state/jev-pilot/run/`, on
   Linux), or on Windows a named pipe whose access list admits only you and
   the system, and refuses remote clients. Each run is served on a new
   instance of the pipe, so you may create instances; a program running as
   you could therefore add one of its own and wait for a run, which is why
   the run checks who serves it before it sends anything (the next layer).
   Other users, other machines and web pages cannot connect. Any program
   running as you still could, which is what the next layers are for.
2. **Which program is at the other end.** Each end asks the OS about the
   other. On macOS, the peer's audit token and its code signature, checked
   against the requirement pinned at install. On Linux, the peer's kernel
   credentials: only a process started from the root-owned setgid binary has
   the `jev-pilot` group, and that also makes it impossible for a program
   running as you to trace it or preload code into it. On Windows, the pipe's
   client or server process, its image path under Program Files, and that
   image's SHA-256 (or Authenticode signer, for a signed build). Every run also
   echoes a nonce the host hands it, and on macOS, where the kernel names the
   peer when asked rather than when it connected, the host checks again once
   the first frame is read. A refusal names the check that failed, and the
   host logs it.
3. **An admin-owned install.** The binary, the extension's files, its policy
   and the host registration are where only an administrator can write, so a
   program running as you cannot rewrite what the checks compare against. The
   policy turning off user-level hosts stops it registering its own host under
   jev-pilot's name, which Chrome would otherwise prefer.
4. **Per-run limits.** The policy above, enforced by the extension.
5. **The audit log, and what you see.** Every run is written down, and every
   run raises a notification, Chrome's debugging bar and the tab group.

The host also checks the program that started it, and serves nothing unless
that is Google Chrome. On macOS the check is Chrome's code signature; on Linux,
the path of the program, which must be a root-owned regular file at one of
Chrome's own paths under `/opt/google/` (or that path marked " (deleted)", for a
Chrome still running after an upgrade replaced its file); on Windows, System32's
cmd.exe started by a Google-signed chrome.exe under Program Files. Each start is
written to the audit log, a refused one at most about once a minute, with a
count of those refused since. This narrows what a program running as you can
pose as; it cannot stop one that drives Chrome itself (see
[What it cannot protect against](#what-it-cannot-protect-against)).

The two refusals mean different things. "something other than the installed
jev-pilot answered on the bridge (…); nothing was sent" is the run refusing the
host: something is squatting on the channel. "the bridge only serves the
installed jev-pilot at … (…)" is the host refusing the run. Neither is worth
retrying.

#### What it cannot protect against

**The front door, on every OS.** A program running as you can start the
genuine, installed jev-pilot with a goal and sites of its own choosing, and
every check passes, because it *is* jev-pilot. Telling your agent from malware
would need you present, which running with no click rules out. What bounds it:
such a run gets only what any run gets (Jev choosing among offered rows,
`navigate` only to the addresses it names, the allowed sites, the tab rules,
and no cookies, storage, network or script of its own), it raises a
notification, and on macOS and Linux it is written to a log it cannot erase.

**Posing as the extension, on every OS.** The host's check of the program that
started it is made of facts a program running as you can still arrange, by
driving Chrome itself:

- On Linux, a program can start the host, keep its pipes in a process of its
  own, and then replace itself with Chrome by exec, so the host's parent is
  Chrome's own file when it is checked. On macOS the host also checks that its
  parent has not exec'd since it started.
- A program running as you can start Chrome with a modified copy of the
  extension that keeps its key, and so its id: Google Chrome 137 to 141 still
  loads one from `--load-extension` when also given
  `--disable-features=DisableLoadExtensionCommandLineSwitch`, and Google Chrome
  started with `--remote-debugging-pipe --enable-unsafe-extension-debugging`
  accepts DevTools' `Extensions.loadUnpacked`. It can also write to Chrome's
  profile, which Chrome protects only weakly, to the same end.
- On Linux, Chrome started with `LD_PRELOAD` runs the program's code inside
  Chrome.
- On Windows, a process may name any process it can open as the parent of one
  it creates (`PROC_THREAD_ATTRIBUTE_PARENT_PROCESS`), so the host's parent
  can be made to be Chrome's.

Each of these is more work than speaking the protocol directly, and each start
of the host is in the audit log with its parent; refused starts are logged at
most about once a minute, with a count.

**macOS** gives the most. Without a prompt, a program running as you cannot
drive the screen, read it, script Chrome or read Chrome's cookie key, and the
layers above keep the bridge from being a way round those. It can still write
to Chrome's profile folder, which Chrome protects only weakly; that threat
exists with or without jev-pilot.

**Linux.** Under X11 any client can already send input to Chrome and read the
screen, so the bridge adds little to what such a program has; Wayland needs a
portal prompt. Without the setgid install, a distribution whose Yama
`ptrace_scope` defaults to 0 (Debian, and Fedora 44 and earlier) lets a
program running as you trace jev-pilot. No Linux secret store hides a secret
from code running as you, which is why the bridge holds none.

**Windows.** A process running as you at the same integrity level can open
another of your processes, inject code into it, and send input to its windows;
Google says as much of Chrome itself. So on Windows the checks stop a program
that speaks the protocol directly, or a copied or patched jev-pilot, but not
one that injects into the genuine jev-pilot or into Chrome. Windows has no
setgid, so the audit log is one you, and anything running as you, can edit.

#### The audit log

The host appends one JSON line for each run started (the time, the run's pid,
its parent's pid and program, the goal, the addresses, and the identity
check's result), each tab attached and site visited, each run ended and how,
and each connection refused and which check failed it. It rotates the log at
4 MiB and keeps three old ones. `jev-pilot extension log` shows it.

On macOS and Linux the log's folder belongs to root and the `jev-pilot` group,
so only the setgid jev-pilot can add to it: you can read it, and nothing
running as you can alter it.

#### A development install

```console
$ jev-pilot extension install --dev
```

For working on jev-pilot itself: no administrator, everything in a folder of
yours (`~/Library/Application Support/jev-pilot/dev/` on macOS,
`~/.local/share/jev-pilot/dev/` on Linux, `%LOCALAPPDATA%\jev-pilot\dev\` on
Windows), the binary's hash pinned in a file you can write, no setgid group,
and the host registered at user level. Load the extension from that folder in
the same way. It is not protected against other programs running as you, and
it says so everywhere it shows: "development install: not protected against
other programs running as you", in `status`, in every run's header, and in the
popup. The MCP server refuses one unless it was started with
`JEV_PILOT_ALLOW_DEV_BRIDGE=1`.

### A fresh browser

**`--browser fresh`** starts a browser for the run on a new, empty profile, and
closes it and deletes the profile when the run ends. Nobody is asked anything,
and nothing of yours is touched. It looks for the binary in this order:
`--chrome <path>`, `JEV_PILOT_CHROME`, then Chrome for Testing, Chromium and
Chrome in their usual places. `npx @puppeteer/browsers install chrome@stable`
installs Chrome for Testing and prints its path. `--headless` runs it without a
window.

Ctrl-C closes a fresh run's browser and deletes its profile before the run
exits; in extension and attach mode it only lets go of Chrome and closes
nothing. If a fresh run is killed outright, its profile is removed the next
time a fresh run starts. When an MCP client stops a run, the server removes it
at once, along with the browser.

### Attach mode, the fallback

**`--browser attach`** drives the Chrome you already have open, signed in as
you, without the extension. It was the first way to reach an everyday Chrome,
and running it is what led to the extension: Chrome asks for every
connection, and a click per tool call is too many for an agent. Turn on remote
debugging first, at `chrome://inspect/#remote-debugging` (Chrome 144 or
later). Then:

- **Chrome asks every time.** Each connection raises "Allow remote debugging?",
  and nothing remembers the answer. Cancel is the focused button, so pressing
  Enter refuses. A run connects once, at its start, and says "Click Allow in
  Chrome (Enter chooses Cancel)" while it waits, for up to two minutes
  (`--allow-wait <seconds>` to change that).
- **It never retries.** Each retry would stack another dialog. A refusal says
  what to do: Cancel was chosen, or Chrome had no window open (it then refuses
  without asking), or remote debugging is off, or its `DevToolsActivePort` file
  is stale.
- **On macOS**, if the terminal may not read Chrome's folder, grant it access
  under System Settings > Privacy & Security > Files & Folders.
- `--channel beta|dev|canary` attaches to another channel's Chrome, and
  `--endpoint ws://…` skips finding it.
- `--sites-only` lists your tabs by id and site alone, with no titles or
  paths. The MCP server passes it on every extension and attach call.

### Choosing the tab, and where the run may go

| Flag | Meaning |
| --- | --- |
| `--tab <id\|text>` | Drive an existing tab: by its id, or the one tab whose address contains the text. `jev-pilot devices --browser extension` (or `attach`) lists the tabs and their ids. Without it, a browser with a single tab drives that tab. Through the extension, a tab of yours must be in the "jev-pilot" tab group first. |
| `--open <url>` | Open a new tab at the address and drive it. In fresh mode it loads in the browser's first tab; through the extension it opens in the "jev-pilot" group of your last focused window. Not with `--tab`. |
| `--url <url>` | Another address `navigate` may go to; repeatable. The `--open` address is always one. This is what `--app` is to a phone, and `--app` and `--device` are refused alongside `--browser`. |

`observe` and `devices` take the same flags. `observe`'s list of operations
never includes `navigate`, because the addresses are given only to a run; a
run with `--url` offers it whenever a supplied address is not the current one.

### What a run sees and does

- **Rows** are the controls a person could use, in paint order, so a sticky
  header or a modal covers what is under it. Rows up to one viewport above or
  below the screen are offered too, marked "off screen"; anything further is
  reached by scrolling. A password field says "contents not shown", and its
  value is never read.
- **Clicks are checked when they are made.** Before clicking, the engine
  checks that the page has not been replaced since it was read, scrolls an
  off-screen row into view, and hit-tests the point. If something else is
  drawn there (a cookie banner, say), it does not click, and tells the run
  what is in the way.
- **Each action reports what it did**: "navigated to …", "opened a new tab:
  …", "a dialog opened: …", "the field now holds …", "the tap was not made:
  …", or "nothing changed". The line reaches the next step's judgment and the
  transcript.
- **JavaScript dialogs.** An alert is accepted at once and its words shown on
  the next step. A confirm, prompt or "leave this page?" waits for
  `accept_dialog` or `dismiss_dialog`; accepting is held to the floor for
  destructive actions. The page cannot be read or touched while one is open,
  and Chrome does not report a dialog that was already open before the run
  attached, so a page that will not answer is reported as possibly blocked by
  one.
- **Tabs.** Tabs the run opens, and tabs those open, belong to the run, and
  `switch_tab` can move to them. Other tabs are never described. In extension
  and attach mode no tab the run did not open is closed.

### Privacy

In extension and attach mode the page text of the tab being driven goes to
TypeSafe with each step, exactly as screen text does on a phone, and that tab
may be signed in to your accounts. Only the tabs the run started on or opened
are described; your other tabs never are. Over MCP, `devices` in extension and
attach mode and the error for a tab that could not be chosen name each tab by
its id and its site alone (github.com), with no title and no path; the command
line, run by hand in a terminal, lists them in full. Password values are never
read. Words you supply are typed only after the endpoint has shown it is Chrome,
and page script runs only for the engine's own fixed jobs (choosing a `<select>`
option, watching for the page to go quiet), never with text from the model.

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
nothing else uses them. The `browser` feature adds `tungstenite`, with no TLS and
no async runtime: every browser endpoint is a local websocket, and the DevTools
messages are written by hand rather than generated for the whole protocol. It
also adds `ctrlc`, so that Ctrl-C closes a fresh browser and removes its
profile. The `extension` feature adds `interprocess` for the local socket or
named pipe, `sha2` for pinned hashes, and per OS what asks it about the other
end of the channel: `nix` and `rustix` on Unix, `security-framework` and
`core-foundation` on macOS, and `widestring` on Windows. On macOS and Windows
it also adds `jev-pilot-os`, a small crate of this workspace and the only
place `unsafe` is allowed, around the few OS calls no safe crate offers
(which image another process runs and who signed it, on Windows; `setregid`,
on macOS). No Appium, no WebDriverAgent client, no `mobile-use`. The Android
adapter shells out to `adb` directly.

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
