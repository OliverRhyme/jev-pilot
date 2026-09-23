//! The desk: where a question goes, and where an answer may come from.
//!
//! One question, two channels, live at once. A person at the terminal types an
//! answer; anything else — a reasoning model, another agent, an MCP server —
//! writes `answer.json` beside it. Whichever answers first resolves the
//! impasse, and neither is configured in advance, because a run cannot know
//! which of them will be at the keyboard.
//!
//! Its own module because it is a seam, not a detail of the command line: it
//! is the only place a run hands a decision to something outside itself, and
//! the only place an outside decision comes back in. That is worth testing
//! directly.

use core::fmt::Write as _;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;

use crate::act::Operation;
use crate::judgment::Confidence;
use crate::pilot::{Impasse, Resolution, Writing};

/// Where a question goes, and where an answer may come from.
///
/// Both channels are live for every question. The terminal is read on its own
/// thread, because a blocking read there would stop the file from being
/// noticed, and the whole point is that either may answer.
pub struct Desk {
    dir: PathBuf,
    typed: Receiver<String>,
    /// How long the file goes unlooked-at when nothing nudges the desk.
    look_again_after: Duration,
    /// Text the caller supplied up front, by the field it belongs in.
    ///
    /// Consulted before anyone is asked. A scripted run knows the words it
    /// means to type — they are in the goal it was given — and stopping to ask
    /// for each one is what keeps such a run from finishing unattended.
    texts: Vec<(Box<str>, Box<str>)>,
    /// Words that came with the last answer, for the field it chose.
    ///
    /// An answer that says to type, and what, has said everything; asking for
    /// the words again is a second round trip for nothing.
    given: std::cell::RefCell<Option<Box<str>>>,
}

impl Desk {
    /// A desk in `dir`, answered from this process's own terminal or from the
    /// file beside the question.
    ///
    /// # Errors
    /// Returns the error from making the directory the desk lives in.
    pub fn new(dir: PathBuf, texts: Vec<(Box<str>, Box<str>)>) -> std::io::Result<Self> {
        let (sender, typed) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in std::io::stdin().lines() {
                let Ok(line) = line else { return };
                if sender.send(line).is_err() {
                    return;
                }
            }
        });
        Self::at(dir, typed, texts, ASKED_AGAIN_AFTER)
    }

    /// A desk reading a channel somebody else supplies.
    ///
    /// Apart from [`Desk::new`] so that what answers is not fixed to this
    /// process's own terminal: a test supplies its own channel, and so could
    /// anything else that wants to answer without a keyboard.
    ///
    /// # Errors
    /// Returns the error from making the directory the desk lives in.
    pub fn at(
        dir: PathBuf,
        typed: Receiver<String>,
        texts: Vec<(Box<str>, Box<str>)>,
        look_again_after: Duration,
    ) -> std::io::Result<Self> {
        std::fs::create_dir_all(&dir)?;
        Ok(Self {
            dir,
            typed,
            look_again_after,
            texts,
            given: std::cell::RefCell::new(None),
        })
    }

    /// Text supplied for a field, matched by name.
    ///
    /// Contains rather than equals: a field is named by whatever the screen
    /// calls it, which is a hint, a label or a caption, and rarely the short
    /// name a caller would type. Case is ignored for the same reason.
    #[must_use]
    pub fn supplied(&self, field: &str) -> Option<&str> {
        let field = field.to_lowercase();
        self.texts
            .iter()
            .find(|(name, _)| field.contains(&name.to_lowercase()))
            .map(|(_, text)| &**text)
    }

    /// Put a question to both channels and wait for the first answer.
    ///
    /// # Errors
    /// Returns `TimedOut` when nobody answered, and any error from writing the
    /// question to the desk or the prompt to the terminal.
    pub fn ask(&self, question: &serde_json::Value, prompt: &str) -> std::io::Result<Answer> {
        let ask = self.dir.join("ask.json");
        let answer = self.dir.join("answer.json");
        let _ = std::fs::remove_file(&answer);
        std::fs::write(&ask, serde_json::to_string_pretty(question)?)?;

        print!("{prompt}");
        std::io::stdout().flush()?;

        // Drain anything typed before the question existed: it answered
        // something else.
        while self.typed.try_recv().is_ok() {}

        // A question nobody is there to answer must not hold a run open for
        // ever. A person at the terminal has as long as they like; a run with
        // nothing attached to its desk gives up and says so.
        let deadline = std::time::Instant::now() + Duration::from_secs(WAIT_SECONDS);
        while std::time::Instant::now() < deadline {
            if let Ok(raw) = std::fs::read_to_string(&answer)
                && let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&raw)
            {
                let _ = std::fs::remove_file(&ask);
                let _ = std::fs::remove_file(&answer);
                println!("[answered from {}]", answer.display());
                return Ok(Answer::File(parsed));
            }
            // Blocking, rather than a look and a sleep: an answer that
            // arrives on this channel wakes the run in the time a pipe takes,
            // and the interval is only how long the file goes unlooked-at
            // when nothing says otherwise.
            match self.typed.recv_timeout(self.look_again_after) {
                // An empty line is not an answer — it is whatever wrote the
                // file saying it has. Look now rather than at the next
                // interval, which is the whole of the saving.
                Ok(line) if line.trim().is_empty() => (),
                Ok(line) => {
                    let _ = std::fs::remove_file(&ask);
                    return Ok(Answer::Typed(line));
                }
                // Nothing typed, or no terminal at all to type at: either way
                // the file is still a way to answer, so keep looking at it.
                Err(RecvTimeoutError::Timeout) => (),
                Err(RecvTimeoutError::Disconnected) => {
                    std::thread::sleep(self.look_again_after);
                }
            }
        }
        let _ = std::fs::remove_file(&ask);
        Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            format!("nobody answered within {WAIT_SECONDS}s"),
        ))
    }

    /// Put an impasse to whoever is there, and resolve their answer.
    ///
    /// # Errors
    /// Returns the error from [`Desk::ask`] when nobody answered.
    pub fn choose(&self, impasse: &Impasse<'_>) -> Result<Resolution, std::io::Error> {
        let reply = self.ask(
            &serde_json::json!({
                "kind": "which_action",
                "step": impasse.step,
                "goal": impasse.goal,
                "why": impasse.because.to_string(),
                "leaning": impasse.leaning.key(),
                "operation_confidence": impasse.operation_confidence.get(),
                "row_confidence": impasse.target_confidence.map(Confidence::get),
                "torn_among": impasse.alternatives.iter().take(5)
                    .map(|(n, p)| serde_json::json!([n, p])).collect::<Vec<_>>(),
                "previous_action": impasse.previous,
                "recent_actions": impasse.lately,
                "operations": impasse.operations.iter().map(|o| o.key()).collect::<Vec<_>>(),
                "rows": impasse.rows,
                "covered": impasse.covered,
                "screen_says": impasse.says,
                "unavailable": impasse.unavailable,
                "keyboard_open": impasse.keyboard_open,
                "fields": impasse.fields,
            }),
            &describe(impasse),
        )?;
        let understood = match reply {
            Answer::Typed(line) => parse_typed(&line),
            Answer::File(value) => {
                // Only for a typing answer: words sent with any other would
                // otherwise wait here and land in the next field typed into.
                let typing = value.get("operation").and_then(serde_json::Value::as_str)
                    == Some(crate::act::Operation::TypeText.key());
                *self.given.borrow_mut() = value
                    .get("text")
                    .and_then(serde_json::Value::as_str)
                    .filter(|_| typing)
                    .map(Box::from);
                resolve_json(&value)
            }
        };
        // Saying so, rather than stopping. The alternative discards a run
        // over a misspelling, and says nothing about why.
        Ok(understood.unwrap_or_else(|| {
            eprintln!(
                "jev-pilot: that answer named no action this screen offers; \
                 the run is stopping. Offered here: {}",
                impasse
                    .operations
                    .iter()
                    .map(|operation| operation.key())
                    .collect::<Vec<_>>()
                    .join(" "),
            );
            Resolution::Stop
        }))
    }

    /// Ask what belongs in a field.
    ///
    /// # Errors
    /// Returns the error from [`Desk::ask`] when nobody answered.
    pub fn compose(&self, request: &Writing<'_>) -> Result<Box<str>, std::io::Error> {
        // Taken, not read: it answered the one question it came with.
        if let Some(text) = self.given.borrow_mut().take() {
            return Ok(text);
        }
        if let Some(text) = self.supplied(&request.field.describe()) {
            println!(
                "\n  \u{2500}\u{2500} step {}: typing the text given for {}",
                request.step,
                request.field.describe(),
            );
            return Ok(text.into());
        }
        let prompt = format!(
            "\n  \u{2500}\u{2500} step {}: what should go in {}?\n     goal: {}\n     text: ",
            request.step,
            request.field.describe(),
            request.goal,
        );
        let reply = self.ask(
            &serde_json::json!({
                "kind": "what_to_type",
                "step": request.step,
                "goal": request.goal,
                "field": request.field.describe(),
                "previous_action": request.previous,
                "rows": request.rows,
            }),
            &prompt,
        )?;
        let text = match reply {
            Answer::Typed(line) => line.trim().to_owned(),
            Answer::File(value) => value
                .get("text")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        };
        Ok(Box::<str>::from(text))
    }
}

/// An answer written as JSON, resolved through the same catalog a typed one is.
///
/// `None` is an answer that named nothing this crate knows. It is not a
/// refusal: a misspelled operation that quietly ends a run is a run thrown
/// away over a typo, and the name a caller reaches for is the one the catalog
/// just offered them.
#[must_use]
pub fn resolve_json(value: &serde_json::Value) -> Option<Resolution> {
    let name = value.get("operation").and_then(serde_json::Value::as_str)?;
    if name == "stop" {
        return Some(Resolution::Stop);
    }
    Some(Resolution::Choose {
        operation: Operation::from_key(name)?,
        target: value
            .get("target")
            .and_then(serde_json::Value::as_u64)
            .and_then(|n| usize::try_from(n).ok()),
    })
}

/// How long a question waits for an answer before the run gives up.
pub const WAIT_SECONDS: u64 = 300;

/// How often the desk is looked at while a question is outstanding.
///
/// A stopped run is doing nothing until its answer arrives, so this interval
/// is dead time on every impasse — and an impasse is already the slowest thing
/// a run does, because something has to read the question and reply. Checking
/// this often costs a `read` of a small file that is usually not there, and
/// only while the run is blocked anyway.
pub const ASKED_AGAIN_AFTER: Duration = Duration::from_millis(20);

/// Which channel an answer came back on.
pub enum Answer {
    /// Typed at a terminal, in the shorthand a person uses.
    Typed(String),
    /// Written to the desk as JSON, by whatever else is answering.
    File(serde_json::Value),
}

/// Read `tap 3`, `back`, `done` and the like, as a person would type them.
#[must_use]
pub fn parse_typed(line: &str) -> Option<Resolution> {
    let mut words = line.split_whitespace();
    let name = words.next()?;
    if name == "stop" {
        return Some(Resolution::Stop);
    }
    let operation = Operation::from_key(name)?;
    Some(Resolution::Choose {
        operation,
        target: words.next().and_then(|n| n.parse().ok()),
    })
}
/// The impasse, written out for whoever is reading the terminal.
#[must_use]
pub fn describe(impasse: &Impasse<'_>) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "\n  ── step {}: {}", impasse.step, impasse.because);
    let _ = writeln!(out, "     goal      : {}", impasse.goal);
    if let Some(previous) = impasse.previous {
        let _ = writeln!(out, "     last did  : {previous}");
    }
    let _ = write!(
        out,
        "     leaning   : {} at {:.2}",
        impasse.leaning,
        impasse.operation_confidence.get()
    );
    match impasse.target_confidence {
        Some(target) => {
            let _ = writeln!(out, ", row at {:.2}", target.get());
        }
        None => {
            let _ = writeln!(out);
        }
    }
    for (index, row) in impasse.rows.iter().enumerate() {
        let reach = if impasse.covered.contains(&index) {
            "  (covered)"
        } else {
            ""
        };
        let _ = writeln!(out, "     [{index}] {row}{reach}");
    }
    // Numbered separately from the rows, because typing is: on a form of
    // three editable rows among five, answering `type` with a row's number
    // types into the wrong field, or into nothing.
    for (index, field) in impasse.fields.iter().enumerate() {
        let _ = writeln!(out, "     type {index} -> {field}");
    }
    let _ = write!(
        out,
        "     tap <n> | type <n> | back | scroll_down | done | stop: "
    );
    out
}
