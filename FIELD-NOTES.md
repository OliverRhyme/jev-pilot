# Field notes

What broke, stalled or surprised when driving a real app with `jev-pilot`, so it
can be fixed rather than rediscovered. Each entry says what was observed, on
what, and what would have made the run work.

---

## 2026-09-21 — Android, Flutter banking app (Pixel 8 Pro, helper Ready)

Driving a Flutter app's enrollment form on staging: fill an identification form,
submit it, and reach the screen a refusal should route to. Navigation and
tapping were reliable throughout; everything below is about forms and about
knowing what the screen currently is.

### 1. The soft keyboard floods the catalog and stalls the loop

With an IME open, the catalog was 55 rows, 45 of which were individual keys
(`q`, `w`, `e`, …, `Shift`, `Delete`, `Symbol keyboard`, `Space`). The app's own
rows were pushed to the end and the distribution went flat: `operation` 0.27,
`target` 0.31, against a floor of 0.6. Two consecutive steps stalled on it.

The keys are never a sensible target — they are the IME's rendering of a text
field the loop already knows it wants to type into. They belong out of the
catalog, not in it competing with the screen.

**Suggested:** drop nodes belonging to the current IME package from the catalog
(the input method's package name is available from
`settings get secure default_input_method`), or expose it as a `Platform`
concern the way system gestures already are. A `keyboard_open: bool` on the
snapshot would also let the judge reason about it explicitly rather than
inferring it from 45 single-letter rows.

### 2. Text fields are not targetable, so forms cannot be filled

The field under test — a password field with a visible label and an adjacent
"Show password" toggle — **never appeared as a row** in any catalog, at any
scroll position, keyboard open or closed. `Show password` appeared; the field
it belongs to did not.

**The rule turned out to be: a text field is surfaced only once it holds
text.** On a form whose five fields were empty, the catalog was
`['Pick it up', 'Not now', <hint>, 'Pick a date', <hint>, 'Continue']` — not one
field. After the same five were filled out of band, the very next catalog read
`['Pick it up', 'Not now', 'Test', 'Revise — Give it if the bank has one on
file…', 'Nieva', 'Pick a date', '(0939) 777 7777 — We will text you…',
'Continue']`. Same screen, same reader, same scroll position; the only thing
that changed is that the fields had values.

That is exactly backwards for driving an app. An empty field is the one you
need to act on, and it is the one the loop cannot see. `uiautomator dump` sees
these nodes throughout — they are `android.widget.EditText`, `clickable="true"`,
`focusable="true"`, with `text=""` — so the information is present in the
hierarchy and is being dropped on the way to the catalog.

`type_text` therefore had no valid target, and the loop had no way to express
"type into that field". Jev kept leaning `type_text` (0.31–0.36, its top choice
both times) while no row could receive it — it knew what to do and had nothing
to do it to.

The run was only completed by leaving `jev-pilot` and typing through
`adb shell input text`, then handing the tap back.

This looks like the same class of bug as the Settings-rows discovery in the
README: the reader prunes on a rule about labels that does not hold for the
widget in question. An editable node with no text of its own but a label
rendered as a sibling is invisible, exactly as a clickable container with no
text of its own was.

**Suggested:** always surface nodes reporting an editable/text-input trait
regardless of whether they carry text, and label them from their associated
label node (`labeled-by`, or the nearest preceding label sibling). A field that
cannot be named cannot be filled, and filling fields is most of what driving an
app is.

### 3. `back` through the desk did not dismiss the keyboard

Answering an impasse with `{"operation":"back"}` while the IME was open left the
next catalog identical — same 55 rows, keyboard still present. On Android, Back
with an IME up normally closes the IME. Either the gesture did not land or the
screen was re-read before the IME finished animating away.

Worth checking whether the post-action settle allows for IME dismissal, which is
slower than a typical screen transition.

### 4. A step that changes the screen's shape can still re-choose a stale row

Given a compound goal ("submit this form, then reach X"), the run tapped row
index 7 on twelve consecutive steps at `target` 0.90–0.97. The first tap
submitted the form and the server's refusal grew the layout by a whole section,
so row 7 stopped being the button — but the choice never moved off it.

Generation stamping worked correctly (refs ran `Generation(0)` through
`Generation(11)`, each resolving against its own screen), so nothing unsafe
happened. The gap is that nothing notices *twelve identical choices produced no
progress*. The README already identifies feeding the previous action into the
state as the fix for a related problem; this is the case where the previous
action is present but its **ineffectiveness** is not.

**Suggested:** carry a short run of recent (action, screen-hash) pairs, and when
the same action has repeated N times against a screen that keeps changing but
never toward the goal, treat it as an impasse and escalate rather than spending
the whole step budget.

Also worth noting: this was against a **live banking API**, and each of those
taps was a real request. A loop that cannot notice it is repeating itself is
more expensive than wasted wall-clock when the actions are not idempotent.

### 5. `--steps 1` reports `OutOfSteps` after a successful action

A deliberate one-shot (`--steps 1`, "tap Continue") executed correctly at
`target` 1.00 and then reported `ending : OutOfSteps { limit: 1 }`. Reading that
as a failure is wrong, but nothing in the output distinguishes "budget spent
having done the thing" from "budget spent getting nowhere". Minor, but it makes
scripted single actions awkward to check.

### 6. No way to see the catalog without provoking an impasse

To find out what `jev-pilot` believed was on screen, the only routes were to
force a stall (raise `--floor`, which also refuses to act) or to leave the tool
and run `uiautomator dump` — the slow reader the helper exists to replace.

**Suggested:** a read-only `jev-pilot observe [--device <serial>]` that prints
the catalog the next step would be offered — rows, operations, and whether the
helper or the CLI read it. It is the first thing wanted when a run behaves
unexpectedly, and it costs no model call.

### 7. A tap on a small icon row did nothing, twice, then worked by coordinate

The form's date-of-birth control is a calendar icon whose accessibility node is
`content-desc="Pick a date"`, `clickable="true"`, `bounds="[835,1086][957,1208]"`
— a 122x122 box at the right edge of a full-width field.

`jev-pilot` chose that row twice (`target` 0.88 then 0.68) and the screen did
not change either time; the catalog came back identical, and `previous_action`
read `Tapped Pick a date`. Tapping the centre of those same bounds directly
(`adb shell input tap 896 1147`) opened the picker immediately.

So the row was the right row and the choice was right — the gesture did not
reach the node. Worth checking what point a tap is dispatched at relative to the
`ElementRef`'s own bounds, particularly where a small clickable sits inside a
much larger parent: a centre computed from the parent would land in the text
field instead, which is exactly the observed behaviour.

### 8. A dialog transition is read before it has happened

Tapping `Switch to input` on the date picker scored 0.97/0.99 and the next
catalog was still the calendar, unchanged. Answering `wait` once produced the
new view. The same shape as entry 3: the post-action settle is tuned for a
screen transition and is too short for a dialog swapping its own content, or
for an IME appearing or leaving.

Because the loop cannot tell "my action did nothing" from "my action has not
rendered yet", it then re-chooses the same action against what looks like an
unchanged screen — which is how entry 4 happens. These three are probably one
fix: settle on a stable read rather than on a fixed delay.

---

### What worked well, for contrast

- The helper read every screen for the whole session; no CLI fallbacks.
- Navigation confidence was high and correct: `Tap` on a named button scored
  `target` 1.00, `scroll_down` to reveal the rest of a form was `Achieved` on
  the first try at 0.82.
- The desk seam is the right shape. Answering `ask.json` from another process,
  mid-run, with the run picking it up and carrying on, worked exactly as
  described — including handing back `stop` to end a run cleanly.
- Generation stamping never mis-resolved a reference across a screen change.
