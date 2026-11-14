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

---

## 2026-09-23 — Android, Chrome web search (Pixel 8 Pro)

Goal: search Google for "jev latest update" in Chrome and open a blog post
about it, through the MCP server, with `floor` 0.4. Three runs from Chrome on
`about:blank`, then three more after the fixes below.

| | endings | machine time / run | per step | read | rows |
|---|---|---|---|---|---|
| before | 2 achieved, 1 no-progress; 5 questions | 40–52s | 6.9s | 3.0s | 63 |
| after | 2 achieved, 1 no-progress; 2 questions | 10–11s | 1.3s | 0.08s | 17 |

### 1. The helper was enabled and reported switched off

The secure setting held the helper as `dev.jevpilot.helper/.PilotAccessibilityService`,
the short form Android writes for a class in its own package. The check looked
only for the full form, so every run read screens with `uiautomator dump`.
Accepting both spellings made reads about 40× faster and, because the helper
reports only what is on screen, cut a results page from about 120 rows to 20.

### 2. An answer about a covered row was remembered as the row

Jev chose a result below the fold, the tap was refused as covered, and the
answer was "scroll down". The run recorded the refused tap instead, so the next
step was told it had already tapped the result, and the first real tap on it
was treated as a repeat and waited out. The act carried out is now what the
run remembers.

### 3. A web page read whole does not move when scrolled

With `uiautomator dump`, Chrome reports every row on the page, on screen or
not. A scroll changed no row and no word, so the scrolled page counted as
another visit to the same place, and four visits ended the run. Which rows can
be reached is now part of a place.

### 4. A covered row was a question when it was only a scroll away

Jev picked the right result on its own, and the run still stopped to ask,
because the result lay under the toolbar. A covered row wholly past every
reachable row is now scrolled towards, for as long as scrolling moves the
screen; a row under a dialog is still asked about. The question now also says
which rows are covered.

### 5. Page furniture diluted the choice

About twenty "About this result" rows among 125, and the wanted result chosen
at 0.20–0.39 every time. Rows whose name repeats more than three times are no
longer offered to Jev; an escalation can still name them.

### 6. Still open

- A stale start: a run begun on the article the run before left open declared
  the goal achieved at its first step. The ending now says when a run achieved
  its goal without doing anything.
- The results page loads after `Submit` returns, and Jev leans `wait` at 0.34–0.41,
  just under the floor. Two of three runs asked for that.
- Jev tapped a sentence of Google's AI Overview, which is not a link, at 0.93,
  and chose it again on the unchanged screen until the run ended for going in
  circles. A second opinion could have picked a real result; the run ended
  instead of asking.
- Time spent answering a covered-row question is not counted in `waited_ms`.

---

## 2026-11-14 — Browser, fresh mode on GitHub (Chrome for Testing 157, headless)

Goal: logged out, search GitHub for the ratatui repository and open its issue
list, with a plan (open Search or jump to; type ratatui and submit; open
ratatui/ratatui; open its Issues tab), `--text search=ratatui`, the accept line
"An issue list for ratatui/ratatui is on screen", and `--floor 0.4`. Three runs;
the first two each found a fault, fixed before the next.

| run | ending | steps | per step | rows |
|---|---|---|---|---|
| 1 | refused by TypeSafe at step 5 (`max_tokens_exceeded`) | 4 | 0.9–3.2s | 16–100 |
| 2 | asked at step 6, unanswered, gave up after 300s | 5 | 0.7–2.9s | 16–176 |
| 3 | achieved | 7 | 0.5–2.8s | 16–176 |

Reads took 30–240ms and judgements 350–530ms; the rest of each step is
settling, which on GitHub is 0.3–2.4s.

### 1. Headless Chrome reads as a phone-sized page

Headless Chrome opens an 800 by 600 window, and GitHub served its narrow layout:
the search was folded into a collapsed "Toggle navigation" menu, so the plan's
first step named a row that did not exist. A headless fresh browser now starts
at 1280 by 800, and the same page offers "Search or jump to" directly.

### 2. A region that takes the clicks is named after everything in it

A nameless clickable is named after the words inside it, and GitHub's page
regions (the navigation menu, the repository header) take clicks for everything
they hold. On a repository page the header was offered as one row named after
the whole README, and the step's request was refused as too long, which ended
the run. A row's name, its description and a field's value are now cut to 120
characters. The regions are still offered as rows ("ratatui ratatui Repository
navigation Code Issues (141) Pull requests…"); they are noise, but a card that
is clickable as a whole is the same shape and is kept on purpose.

### 3. A tab that fetches its page looked like a tap that did nothing

GitHub switches repository tabs with a script: the old page stays up, a
progress bar is drawn, and the new page arrives when its fetch returns. The
pilot's watch after a tap saw a screen quiet since just after the tap and
unchanged, and called the tap dead after 627ms. The next judgement was made
about the old page, `done` at 0.53 fell under its floor, and the question asked
waited five minutes for an answer. A step too uncertain to act on, right after
an action that seemed to change nothing, now watches once more before asking.
In run 3 the same judgement came back over the floor at 0.62 instead, the
accept line refused it, and the next step read the issue list.

### 4. Still open

- "navigated to https://github.com" is the outcome of every navigation within
  the site: the outcome names only the origin, so a search results page and a
  repository read the same to the next step. The step record keeps the full
  address.
- The results page offered 100 rows and the repository page 176; the right row
  was still chosen at 0.97 and 0.82–0.86.
- Settling never waits for a fetch, only for a navigation, a load and a quiet
  DOM; the pilot's patience is what covers a script-driven page change.

## 2026-11-14 — Browser, how far beyond the screen to offer rows

`examples/band.rs --send` sends the first step's request on a recorded page at
band 0 (only rows on screen) and band 1 (rows up to one viewport away, marked
"off screen"), eight goals over the search-results and repository fixtures,
each with one right row. Jev 1.14.0.

| | right row offered | right row chosen | its confidence, rows on screen |
|---|---|---|---|
| band 0 | 6 of 8 | 6 of 8 | 0.77–1.00 |
| band 1 | 8 of 8 | 8 of 8 | 0.63–1.00 |

Band 1 chose the right element more often. For a row below the fold ("Open the
seventh result", "Open the releases page") band 0 could only lean `scroll_down`,
at 0.57 and 0.68, one of them under the default floor; band 1 named the row at
0.84 and 0.88, though with `scroll_down` rather than `tap` as the operation,
at 0.99 and 0.91. On goals whose row was already on screen band 0 was a little
sharper: the fourth result at 0.86 against 0.69, Star at 0.77 against 0.63.
`BAND_VIEWPORTS` stays at one viewport.

## 2026-11-14 — Browser, attach mode on GitHub (the owner's Chrome, stable, signed in)

Goal: star the jev-pilot repository, then unstar it, with
`--open https://github.com/OliverRhyme/jev-pilot` so the run had a tab of its
own, and the accept line "The repository's star button reads Star, not
Starred". Two runs, each started by clicking Allow on Chrome's "Allow remote
debugging?" dialog.

| run | ending | steps | what went wrong |
|---|---|---|---|
| 1 | `Finished(Achieved)`, having done nothing | 2 | the first read had no address and no rows; the accept line already held on the unstarred page |
| 2 | `Uncertain` (no progress, 4 repeats) | 6 | Star was pressed; Unstar was refused at click time and never pressed |

### 1. A tab opened for the run was read before it had loaded

The tab was created at its address and attached at once, so the first read
came before the navigation committed: no address and no rows, and the run
asked what to do. A run's tab is now created blank and sent to its address
through the same settled navigation a `navigate` step uses.

### 2. The accept line held before the run had done anything

"The star button reads Star" is as true before starring as after unstarring,
and the run ended achieved at step 2 without touching the page. With a plan,
the accept lines now end a run only once it has acted.

### 3. A redrawn button read as covered

Once a star lands, GitHub redraws its star button and its count with new
nodes. The click-time check could no longer find the node it had read and
reported "something else covers it", so Unstar was refused. When the node read
is gone, the engine now clicks the one control at that point with the same
role and name, and otherwise refuses with "the row was replaced since the page
was read". Real Chrome's replies for this are in `tests/fixtures/cdp/replaced/`.

### 4. A refused tap armed the guard against repeating it

The repeat guard turned the next two choices of Unstar into waits, though no
tap had been made, and the run ran out of patience with the repository still
starred. A tap refused at click time no longer counts as an action taken.

### 5. A click on Allow for every connection

Every connection raised Chrome's dialog, with Cancel focused, and nothing
remembered the answer. A run connects once, but over MCP every tool call is a
connection of its own, so an agent looking at a page and then starting a run
needs the owner at the keyboard for each. That cost is what the jev-pilot
extension removes: it is installed once, as an administrator, and a run then
needs no click.

## 2026-11-15 — Browser, extension mode acceptance (to be run)

The runs that accept extension mode, each pass or fail, with its transcript
under `notes/acceptance/`:

0. Install on the Mac: `sudo jev-pilot extension install`, the profile
   approved, the extension loaded unpacked from the root-owned folder.
   `jev-pilot extension status` reports a protected install, the bridge
   connected and the policy in force; Claude in Chrome still works.
1. Star, then unstar, on GitHub with zero clicks.
2. The same pair after a Chrome restart, and after a reboot.
3. The same pair through MCP, with `browser: {mode: "extension"}`.
4. The checks hold: a copy run from `~/Downloads`, a script speaking the
   protocol, a user-level host manifest, an edit to the extension folder,
   pause, Cancel on the bar, a site off the list, `--tab` outside the group,
   and a second run.
5. Linux (Ubuntu) and Windows 11: the install, the pair, and the first two
   checks.

Not yet run.
