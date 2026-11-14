# Recorded bridge conversations

Each file is one conversation between a run, the jev-pilot host and Chrome's
jev-pilot extension, as the frames that crossed the bridge, in order. The Rust
tests in `tests/cdp.rs` replay them from the run's side, and the extension's
tests replay them from the extension's side.

A file is a JSON array. Each entry is one frame:

```json
{"from": "run", "to": "extension", "frame": {"type": "request", "id": 3, "op": "attach", "tab": 1}}
```

- `from` and `to` are `run`, `host` or `extension`. The host relays frames
  between the run and the extension unchanged; a frame from or to the host
  itself is one the host reads or writes (the nonce, the echo, `ready`,
  `run_started`, `release`).
- `frame` is the message exactly as written on the wire, without its length
  prefix. Its shapes are `jev_pilot::browser::bridge::Message`.

The conversations:

| File | What happens |
|---|---|
| `handshake.json` | The extension says hello, the host sends its nonce, the run echoes it with its goal and sites, the host tells the extension a run started and the run it is ready. |
| `attach-and-read.json` | The run lists the tabs, attaches to one by its tab id and reads it, using the replies a real Chrome gave for the recorded sign-up page (`tests/fixtures/cdp/signup/`); then it clicks a row, which opens a tab, and switches to that tab. |
| `open-and-navigate.json` | The run opens a blank tab, attaches to it and sends it to its address; the extension forwards the navigation and the load. |
| `link-opens-tab.json` | A key press in a held tab opens another; the extension reports it with the held tab as its opener. |
| `alert.json` | An alert opens; the run accepts it at once. |
| `confirm.json` | A confirm opens and is held; the run reads the tab, which is the dialog alone. |
| `refusals.json` | Every refusal code, each on the call it refuses. |
| `pause.json` | The extension is paused; the host says so in `ready` and the run sends nothing. |
| `cancel.json` | The owner presses Cancel on Chrome's debugging bar mid-run; the extension reports the tab let go of. |
| `release.json` | The run lets go of its tab and goes; the host tells the extension to release everything the run held. |

The Rust tests write a conversation that has no file yet, and rewrite them all
when run with `JEV_PILOT_RECORD=1`; otherwise a conversation that differs from
its file fails. The frames between the run and the extension are produced by
the engine itself; the extension's side is played by the test, from the
recorded sign-up page.
