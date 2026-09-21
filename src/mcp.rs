//! An MCP server, so another agent can drive a device — and be the second
//! opinion a run escalates to.
//!
//! The interesting part is not the transport. This crate already has the seam
//! an MCP client wants to sit in: a run that cannot decide stops and asks, and
//! anything able to write an answer can answer it. That is the desk, and here
//! the party holding the conversation becomes what answers.
//!
//! The answering is client-driven, and deliberately. MCP has a way for a
//! server to call back into the client's model — `sampling/createMessage` —
//! and SEP-2577 deprecates it, tells new implementations not to adopt it, and
//! names it the most security-sensitive feature of the three it removes,
//! because it lets a server put text of its choosing in front of somebody
//! else's model.
//!
//! That warning is pointed here rather than general. The text this server
//! would be forwarding is whatever an app has drawn on the screen, and the
//! answer coming back moves money. A screen is not a trustworthy author.
//!
//! So the client asks, on its own turn: `run_status` says whether a run is
//! waiting and what it wants, and `answer_run` answers it. The model doing
//! the asking is the second opinion, and it reads the question itself rather
//! than having it pushed at it.
//!
//! Every tool is carried out by running the `jev-pilot` binary rather than by
//! driving the loop in this process. One behaviour, described once: a server
//! that reimplemented the run would drift from the command line it is
//! supposed to be a face for. It also keeps the loop synchronous, which is
//! what it wants to be — a step is observe, judge, act, settle, and each waits
//! on the one before, so there is nothing for a runtime to overlap.

// `#[tool_handler]` writes an async trait method that never awaits, and the
// lint is about code this module does not write.
#![allow(clippy::unused_async_trait_impl)]

use std::sync::{Arc, Mutex};

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig,
};
use rmcp::{ErrorData, ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

/// Which device a call is about, when more than one is attached.
#[derive(Debug, Deserialize, serde::Serialize, JsonSchema)]
pub struct Which {
    /// Which device, when more than one is attached.
    pub device: Option<String>,
}

/// What the helper tool takes.
#[derive(Debug, Deserialize, serde::Serialize, JsonSchema)]
pub struct HelperArgs {
    /// Which device, when more than one is attached.
    pub device: Option<String>,
    /// Install and enable it, rather than only reporting.
    pub install: Option<bool>,
}

/// What starting a run takes.
#[derive(Debug, Deserialize, serde::Serialize, JsonSchema)]
pub struct StartArgs {
    /// What to achieve, in plain words. Say what to achieve rather than how to
    /// find the app — pass `app` for that. Name the steps in the order the app
    /// asks for them, and for a keypad name the keys in order ("tap the digit
    /// keys 2, 4, 6, 8, 1, 0 in that order") rather than the number.
    pub goal: String,
    /// Which device, when more than one is attached.
    pub device: Option<String>,
    /// The app package the goal is about. It is brought to the front first,
    /// and the run knows when it has left it.
    pub app: Option<String>,
    /// Claims that must hold before success is believed. Write them about text
    /// that is visible when the run finishes: a claim about something further
    /// down the page can never be confirmed, and the run will walk off a
    /// finished screen still looking for it.
    pub accept: Option<Vec<String>>,
    /// Words to type, keyed by the field they belong in. The key is matched
    /// against whatever the screen calls the field — its hint, its label, its
    /// caption — by containment and ignoring case, so "password" finds
    /// "Password" and "account number" finds "RBGI Account Number". A field
    /// with nothing supplied stops the run to ask for the words.
    pub text: Option<std::collections::BTreeMap<String, String>>,
    /// How many steps before giving up.
    pub steps: Option<u32>,
    /// Below this confidence the run asks rather than acts. Lowering it
    /// loosens ordinary gestures only: ending the run, and leaving the app,
    /// keep their own floor whatever is passed here. Around 0.35 to 0.4 keeps
    /// most runs moving; the default of 0.6 asks often.
    pub floor: Option<f64>,
}

/// What answering a run takes.
#[derive(Debug, Deserialize, serde::Serialize, JsonSchema)]
pub struct AnswerArgs {
    /// Which device's run, when more than one is being driven.
    pub device: Option<String>,
    /// One of the operations the run listed, or `stop` to end it.
    pub operation: Option<String>,
    /// Which row, or which field when typing, counting from zero.
    pub target: Option<u32>,
    /// The words, when it asked for some.
    pub text: Option<String>,
}

/// The server.
#[derive(Clone)]
pub struct Pilot {
    tool_router: ToolRouter<Self>,
    sessions: Arc<Mutex<Sessions>>,
}

impl Default for Pilot {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Pilot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pilot").finish_non_exhaustive()
    }
}

impl Pilot {
    /// A server with no runs going.
    #[must_use]
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
            sessions: Arc::new(Mutex::new(Sessions::default())),
        }
    }

    /// Which run a call is about, given what it said and what is in flight.
    fn resolve(&self, device: Option<&str>) -> Result<String, String> {
        let mut sessions = self.sessions.lock().map_err(|_| poisoned())?;
        let running = sessions.still_running();
        which_run(device, &running.iter().map(String::as_str).collect::<Vec<_>>())
    }
}

fn poisoned() -> String {
    "the run book is in an unknown state; start this server again".to_owned()
}

/// Run the command line and say what it said.
fn ran(args: &[String]) -> CallToolResult {
    match run_pilot(args) {
        Ok(said) => CallToolResult::success(vec![ContentBlock::text(said)]),
        Err(error) => refused(&format!("could not run jev-pilot: {error}")),
    }
}

/// A call that was understood and could not be carried out.
///
/// Inside the result rather than as a transport fault: the asking model can
/// read this and do something else, which is the whole point of telling it.
fn refused(said: &str) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(said.to_owned())])
}

#[tool_router(router = tool_router)]
impl Pilot {
    /// Read what is on the device's screen right now.
    #[tool(
        description = "Read what is on the device's screen right now: the rows that can be \
                       acted on, what the screen says, which controls it shows but will not \
                       let anything use, and which app is in front. Changes nothing. This is \
                       the cheapest way to find out where a device is.",
        annotations(title = "Look at the screen", read_only_hint = true)
    )]
    pub async fn observe(&self, Parameters(which): Parameters<Which>) -> CallToolResult {
        ran(&invocation("observe", &to_value(&which)).unwrap_or_default())
    }

    /// List the devices attached to this machine.
    #[tool(
        description = "List the devices attached to this machine, so a later call can name \
                       one. Changes nothing.",
        annotations(title = "List devices", read_only_hint = true)
    )]
    pub async fn devices(&self) -> CallToolResult {
        ran(&["devices".to_owned()])
    }

    /// Report on the on-device helper, and install it when asked.
    #[tool(
        description = "Report on the on-device helper that reads screens quickly, and install \
                       it when asked to. Without it every screen read takes about two seconds \
                       instead of about fifty milliseconds.",
        annotations(title = "The screen-reading helper", read_only_hint = false)
    )]
    pub async fn helper(&self, Parameters(args): Parameters<HelperArgs>) -> CallToolResult {
        ran(&invocation("helper", &to_value(&args)).unwrap_or_default())
    }

    /// Drive the device towards a goal.
    #[tool(
        description = "Drive the device towards a goal, written in plain words. THIS ACTS ON \
                       A REAL DEVICE: it taps, types and navigates, and on an app that moves \
                       money or sends messages it will do those things. Waits for the run to want \
                       something and returns its question, or says it is still working. One run per \
                       device — a device already being driven is refused rather than driven \
                       twice. Call observe first: a goal written for a screen nobody looked \
                       at is where most bad runs begin.",
        annotations(
            title = "Drive the device towards a goal",
            read_only_hint = false,
            destructive_hint = true,
            open_world_hint = true
        )
    )]
    pub async fn start_run(
        &self,
        Parameters(args): Parameters<StartArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let arguments = to_value(&args);
        let Some(line) = invocation("start_run", &arguments) else {
            return Ok(refused("a run needs a goal"));
        };

        let (name, desk) = match self.launch(line, args.device.as_deref()) {
            Ok(run) => run,
            Err(said) => return Ok(said),
        };
        Ok(waited(&name, until_it_wants_something(&desk, patience(), None).await))
    }

    /// Start a run and remember it, or say why not.
    ///
    /// Apart from the tool it serves so that the lock on the sessions is taken
    /// and given back here, with no waiting in between: a guard held across a
    /// wait would keep every other call out for as long as this one waits.
    fn launch(&self, mut line: Vec<String>, device: Option<&str>) -> Result<(String, std::path::PathBuf), CallToolResult> {
        let Ok(mut sessions) = self.sessions.lock() else {
            return Err(refused(&poisoned()));
        };
        let busy = sessions.still_running();
        if let Some(already) = busy
            .iter()
            .find(|running| device.is_none_or(|asked| *running == asked))
        {
            return Err(refused(&format!(
                "{already} is already being driven. Watch it with run_status, answer it with \
                 answer_run, or end it with stop_run."
            )));
        }

        let name = device.map_or_else(|| "the attached device".to_owned(), ToOwned::to_owned);
        let desk = std::env::temp_dir().join(format!(
            "jev-pilot-mcp-{}",
            name.replace(|c: char| !c.is_ascii_alphanumeric(), "-")
        ));
        if let Err(error) = std::fs::create_dir_all(&desk) {
            return Err(refused(&format!("could not make a desk for {name}: {error}")));
        }
        // The goal is the one positional and must stay last.
        let goal = line.pop().unwrap_or_default();
        line.push("--desk".to_owned());
        line.push(desk.display().to_string());
        line.push(goal);

        // Kept, because it is where the run says what it is doing and how it
        // ended. Thrown away, a finished run could not say how it went.
        let log = std::fs::File::create(desk.join("run.log"))
            .and_then(|log| log.try_clone().map(|complaints| (log, complaints)));
        let Ok((log, complaints)) = log else {
            return Err(refused(&format!("could not keep a log for {name}")));
        };

        match std::process::Command::new(binary())
            .args(&line)
            // Kept open, not because a run reads anything from it, but
            // because it is how an answer written to the desk is announced.
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::from(log))
            .stderr(std::process::Stdio::from(complaints))
            .spawn()
        {
            Ok(child) => {
                sessions.remember(name.clone(), desk.clone(), child);
                Ok((name, desk))
            }
            Err(error) => Err(refused(&format!("could not start jev-pilot: {error}"))),
        }
    }

    /// How a run is getting on.
    #[tool(
        description = "How a run is getting on: every step it has taken, whether it is waiting \
                       for an answer and what it is asking, and how it ended if it has. \
                       Changes nothing. Name a device only when more than one is being driven.",
        annotations(title = "How a run is getting on", read_only_hint = true)
    )]
    pub async fn run_status(&self, Parameters(which): Parameters<Which>) -> Result<CallToolResult, ErrorData> {
        let run = match self.resolve(which.device.as_deref()) {
            Ok(run) => run,
            Err(said) => return Ok(refused(&said)),
        };
        let Ok(sessions) = self.sessions.lock() else {
            return Ok(refused(&poisoned()));
        };
        match sessions.desk_of(&run) {
            Some(desk) => Ok(CallToolResult::success(vec![ContentBlock::text(status_of(desk))])),
            None => Ok(refused(&format!("no run on {run}"))),
        }
    }

    /// Answer a run that stopped to ask.
    #[tool(
        description = "Answer a run that stopped to ask. Name an operation it was offered, the \
                       row or field it applies to, or the words to type. This is how the \
                       caller becomes the second opinion a run escalates to, so it acts on \
                       the device. Waits for the next question the way start_run does.",
        annotations(
            title = "Answer a run",
            read_only_hint = false,
            destructive_hint = true,
            open_world_hint = true
        )
    )]
    pub async fn answer_run(&self, Parameters(args): Parameters<AnswerArgs>) -> Result<CallToolResult, ErrorData> {
        let run = match self.resolve(args.device.as_deref()) {
            Ok(run) => run,
            Err(said) => return Ok(refused(&said)),
        };
        let Some(answer) = answer_from(&to_value(&args)) else {
            return Ok(refused(
                "an answer needs an operation, a target or some text; this one said nothing",
            ));
        };
        // Read before the answer is written, so the question being replaced is
        // known and cannot be handed back as a new one.
        let (desk, asked) = match self.accept(&run, &answer) {
            Ok(both) => both,
            Err(said) => return Ok(said),
        };
        // Waited on for the same reason as starting one: the next question
        // arrives as the result of the answer that led to it.
        Ok(waited(
            &run,
            until_it_wants_something(&desk, patience(), asked.as_deref()).await,
        ))
    }

    /// Put an answer on a run's desk, and say which desk it went to.
    ///
    /// Separate from the tool for the same reason as `launch`: the lock is
    /// given back before anything waits.
    fn accept(
        &self,
        run: &str,
        answer: &serde_json::Value,
    ) -> Result<(std::path::PathBuf, Option<String>), CallToolResult> {
        let Ok(mut sessions) = self.sessions.lock() else {
            return Err(refused(&poisoned()));
        };
        let Some(desk) = sessions.desk_of(run) else {
            return Err(refused(&format!("no run on {run}")));
        };
        let desk = desk.to_path_buf();
        // Read before the answer is written: this is the question being
        // answered, and waiting must not hand it back as a new one.
        let asked = std::fs::read_to_string(desk.join("ask.json")).ok();
        if let Err(error) = std::fs::write(desk.join("answer.json"), answer.to_string()) {
            return Err(refused(&format!("could not answer {run}: {error}")));
        }
        // Written first, so the run finds the answer already there when it
        // looks. A nudge that arrived first would send it to an empty desk.
        sessions.nudge(run);
        Ok((desk, asked))
    }

    /// End a run that is still going.
    #[tool(
        description = "End a run that is still going, leaving the device wherever it got to. \
                       Nothing further is done to the device.",
        annotations(title = "Stop a run", read_only_hint = false)
    )]
    pub async fn stop_run(&self, Parameters(which): Parameters<Which>) -> Result<CallToolResult, ErrorData> {
        let run = match self.resolve(which.device.as_deref()) {
            Ok(run) => run,
            Err(said) => return Ok(refused(&said)),
        };
        let Ok(mut sessions) = self.sessions.lock() else {
            return Ok(refused(&poisoned()));
        };
        if sessions.forget(&run) {
            Ok(CallToolResult::success(vec![ContentBlock::text(format!(
                "the run on {run} is stopped; the device is left where it got to"
            ))]))
        } else {
            Ok(refused(&format!("no run on {run}")))
        }
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Pilot {
    fn get_info(&self) -> ServerConfig {
        let mut me = Implementation::default();
        "jev-pilot".clone_into(&mut me.name);
        env!("CARGO_PKG_VERSION").clone_into(&mut me.version);

        let mut info = ServerConfig::default();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.server_info = me;
        info.instructions = Some(GUIDANCE.to_owned());
        info
    }
}

/// What a client is told before it calls anything.
///
/// Worth spending words on. The tools are easy to call and easy to call
/// badly, and every rule below is one that has actually cost a run: a goal
/// that sent it hunting through a launcher, a claim about text that was below
/// the fold, a floor lowered in the belief it only made things quicker.
const GUIDANCE: &str = "\
Drives an Android or iOS device towards a goal. Code enumerates what the \
screen allows and a model picks one of those, so nothing here can invent a \
coordinate or name a row that is not on screen.

LOOK FIRST. `observe` is free and changes nothing, and it tells you the rows, \
what the screen says, the controls it shows but will not let anything use, \
which app is in front, and how long the screen has been still. Most bad runs \
start with a goal written for a screen nobody looked at.

WRITING A GOAL. Say what to achieve, not how to find the app: pass `app` with \
the package instead, which brings it to the front and lets the run notice when \
it has wandered off. Name the steps in the order the app asks for them, and \
name what to enter where. For a keypad, name the keys in order — 'tap the \
digit keys 2, 4, 6, 8, 1, 0 in that order' works, 'enter PIN 246810' leaves \
it counting.

WRITING `accept`. These are what must hold before success is believed, and \
they are checked against the screen as read. Write them about text that is \
actually visible when the run finishes — a claim about something further down \
the page can never be confirmed, and the run will walk off a finished screen \
looking for it. 'A transfer receipt is on screen' is checkable. 'A receipt \
showing a reference number' is not, if the reference is below the fold.

WRITING `text`. Keys are matched against whatever the screen calls the field — \
its hint, its label, its caption — by containment, ignoring case. So \
'password' finds 'Password', and 'account number' finds 'RBGI Account \
Number'. A field with nothing supplied stops the run to ask you for the words.

THE FLOOR. Below it the run asks instead of acting. Lowering it loosens \
ordinary gestures only: ending the run, and leaving the app, keep their own \
floor whatever you pass, because a run that gives up on a guess has answered \
wrongly rather than cheaply. Around 0.35 to 0.4 keeps most runs moving; the \
default of 0.6 asks often.

WHEN IT ASKS. A run that cannot decide stops and waits, and the question comes \
back as the result of the call that caused it: `start_run` and `answer_run` \
both hold on until the run wants something, is over, or has been working \
quietly for a while. So a question is not something to go and look for — it is \
handed to you, and you answer it with `answer_run`, naming an operation it \
listed and a row it showed you. An operation it did not offer, or a row it \
does not have, is refused and you are asked again. You are the second opinion \
here, so read what it is asking rather than guessing; it stopped precisely \
because the obvious answer was not obvious.

If a call comes back saying the run is still working, it has only handed your \
time back — the run is untouched. Call `run_status` to see where it has got \
to, or `answer_run` when it next asks. A run left unanswered waits five \
minutes and then gives up, so a run nobody comes back to is a run that dies \
of it.

ONE RUN PER DEVICE. A second run on the same phone would take turns at the \
same screen with the first. Stop or finish the one that is there.";

/// Say what came of waiting, in a way the caller can act on.
fn waited(run: &str, how: Waited) -> CallToolResult {
    let said = match how {
        Waited::Asking(question) => format!(
            "the run on {run} cannot decide, and is waiting for you.\n\n{question}\n\n\
             You are the second opinion. Answer with answer_run, naming an operation it \
             listed and a row it showed you — an operation it did not offer, or a row it \
             does not have, is refused and you are asked again. It waits five minutes."
        ),
        Waited::Ended(how) => format!("the run on {run} is over.\n\n{how}"),
        Waited::StillGoing => format!(
            "the run on {run} is working and has not asked for anything. Call run_status \
             to see where it has got to, or answer_run when it wants something."
        ),
    };
    CallToolResult::success(vec![ContentBlock::text(said)])
}

/// How long a call will wait before handing the caller its time back.
///
/// Shorter than a client's own timeout, and shorter than the five minutes a
/// run waits at an impasse, so a caller that keeps waiting is never the reason
/// a run gives up.
fn patience() -> std::time::Duration {
    std::time::Duration::from_secs(45)
}

/// What came of waiting on a run.
#[derive(Debug)]
pub enum Waited {
    /// It stopped to ask, and this is what it wants.
    Asking(String),
    /// It is over, and this is how it went.
    Ended(String),
    /// Neither yet. The caller was given its time back rather than held.
    StillGoing,
}

/// Wait until a run wants something, is over, or has had long enough.
///
/// There is no way for a server to call into a client's model — sampling did
/// that and is deprecated — so the next best thing is not to answer the call
/// until there is something worth saying. A question then arrives as the
/// result of the call that caused it, and the model that asked reads it in
/// the ordinary way.
///
/// Bounded, because a client is waiting on this: a run that is simply working
/// gets a "still going" and the caller decides whether to wait again. Holding
/// the call open until the run finished would trip a client's own timeout and
/// lose the run behind it.
///
/// `answered` is the question the caller has just replied to, when it has. A
/// run clears its desk in its own time, so for a moment after an answer is
/// written the question it answers is still lying there, and reporting it
/// would have the caller answer the same impasse for ever.
pub async fn until_it_wants_something(
    desk: &std::path::Path,
    patience: std::time::Duration,
    answered: Option<&str>,
) -> Waited {
    let deadline = std::time::Instant::now() + patience;
    loop {
        // `ask.json` is written when a run stops to ask and taken away once it
        // has been answered, so its being there is the question.
        if let Ok(question) = std::fs::read_to_string(desk.join("ask.json"))
            && answered != Some(question.as_str())
        {
            return Waited::Asking(question);
        }
        if let Some(how) = ended(desk) {
            return Waited::Ended(how);
        }
        if std::time::Instant::now() >= deadline {
            return Waited::StillGoing;
        }
        tokio::time::sleep(std::time::Duration::from_millis(LOOKED_AT_EVERY_MS)).await;
    }
}

/// How a run ended, if it has.
///
/// Read from what the run itself printed. The line is the last thing it says
/// and the only place the ending is written down.
fn ended(desk: &std::path::Path) -> Option<String> {
    let said = std::fs::read_to_string(desk.join("run.log")).ok()?;
    let from = said.find("\nending : ")?;
    Some(said[from..].trim().to_owned())
}

/// How often a waited-on run is looked at.
///
/// Short, because this sits on the critical path of an impasse: a run that has
/// stopped is doing nothing at all until the question reaches somebody, and
/// every interval here is dead time added to a round trip that already costs a
/// model call. Two stats on small files this often is nothing next to that, and
/// it only happens while a call is waiting.
const LOOKED_AT_EVERY_MS: u64 = 20;

/// Turn a typed argument struct back into the JSON the pure helpers read.
///
/// Those helpers are shared with the command line and tested without any of
/// this, which is worth a serialisation: the rules about what becomes which
/// flag live in one place and are checked there.
fn to_value<T: serde::Serialize>(args: &T) -> serde_json::Value {
    serde_json::to_value(args).unwrap_or(serde_json::Value::Null)
}
/// Which run a call is about.
///
/// A run is named by the device it drives, because that is what it is. Naming
/// them separately would make two runs on one phone look like an ordinary
/// thing to ask for, and it is not: they would take turns at the same screen,
/// each undoing what the other had just done.
///
/// # Errors
/// Returns what to say to the caller when there is no such run, or when there
/// is more than one and the call did not say which.
pub fn which_run(asked: Option<&str>, running: &[&str]) -> Result<String, String> {
    match (asked, running) {
        (Some(device), _) if running.contains(&device) => Ok(device.to_owned()),
        (Some(device), []) => Err(format!("nothing is being driven, so no run on {device}")),
        (Some(device), _) => Err(format!(
            "no run on {device}; these are being driven: {}",
            running.join(", ")
        )),
        (None, [only]) => Ok((*only).to_owned()),
        (None, []) => Err("no run is in flight; start_run begins one".to_owned()),
        (None, _) => Err(format!(
            "more than one run is in flight; name a device: {}",
            running.join(", ")
        )),
    }
}

/// The command line one tool call stands for.
///
/// `None` when the arguments do not make a call worth running — a run with no
/// goal is not a run.
///
/// Built here rather than in a shell string: the goal and the words to type
/// come from whoever is talking to this server, and a string handed to a shell
/// is a string that can carry a second command. These reach the process as
/// separate arguments and nothing parses them again.
#[must_use]
pub fn invocation(tool: &str, arguments: &serde_json::Value) -> Option<Vec<String>> {
    let text = |key: &str| {
        arguments
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned)
    };
    let mut args = Vec::new();
    if let Some(device) = text("device") {
        args.push("--device".to_owned());
        args.push(device);
    }
    match tool {
        "observe" => args.push("observe".to_owned()),
        "devices" => args.push("devices".to_owned()),
        "helper" => {
            args.push("helper".to_owned());
            if arguments.get("install").and_then(serde_json::Value::as_bool) == Some(true) {
                args.push("install".to_owned());
            }
        }
        "start_run" => {
            let goal = text("goal")?;
            if let Some(app) = text("app") {
                args.push("--app".to_owned());
                args.push(app);
            }
            for claim in arguments
                .get("accept")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_str)
            {
                args.push("--accept".to_owned());
                args.push(claim.to_owned());
            }
            if let Some(words) = arguments.get("text").and_then(serde_json::Value::as_object) {
                for (field, said) in words {
                    if let Some(said) = said.as_str() {
                        args.push("--text".to_owned());
                        args.push(format!("{field}={said}"));
                    }
                }
            }
            if let Some(steps) = arguments.get("steps").and_then(serde_json::Value::as_u64) {
                args.push("--steps".to_owned());
                args.push(steps.to_string());
            }
            if let Some(floor) = arguments.get("floor").and_then(serde_json::Value::as_f64) {
                args.push("--floor".to_owned());
                args.push(floor.to_string());
            }
            // Last, and after a separator is unnecessary because it is the
            // only positional the command line takes.
            args.push(goal);
        }
        _ => return None,
    }
    Some(args)
}

/// What an answer to an impasse writes to the desk.
///
/// `None` when it says nothing: an empty answer would let a run act on an
/// empty file, and the desk reads whatever is there.
///
/// The shape is the desk's own, because that is the seam a run has always
/// escalated through — a person at a terminal and a model over MCP write the
/// same thing, and neither is privileged over the other.
#[must_use]
pub fn answer_from(arguments: &serde_json::Value) -> Option<serde_json::Value> {
    let mut answer = serde_json::Map::new();
    if let Some(operation) = arguments.get("operation").and_then(serde_json::Value::as_str) {
        answer.insert("operation".to_owned(), operation.into());
    }
    if let Some(target) = arguments.get("target").and_then(serde_json::Value::as_u64) {
        answer.insert("target".to_owned(), target.into());
    }
    if let Some(text) = arguments.get("text").and_then(serde_json::Value::as_str) {
        answer.insert("text".to_owned(), text.into());
    }
    (!answer.is_empty()).then_some(serde_json::Value::Object(answer))
}

/// The `jev-pilot` binary to run.
///
/// Beside this one when that is where it is, so a server installed somewhere
/// unusual still finds the command line it belongs to, and falls back to the
/// path otherwise.
fn binary() -> std::path::PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|me| me.parent().map(|beside| beside.join("jev-pilot")))
        .filter(|beside| beside.exists())
        .unwrap_or_else(|| "jev-pilot".into())
}

/// Run the command line and hand back everything it said.
fn run_pilot(args: &[String]) -> std::io::Result<String> {
    let done = std::process::Command::new(binary()).args(args).output()?;
    let mut said = String::from_utf8_lossy(&done.stdout).into_owned();
    let complained = String::from_utf8_lossy(&done.stderr);
    if !complained.trim().is_empty() {
        said.push_str(&complained);
    }
    Ok(said)
}

/// How a run is getting on, as far as its desk can say.
#[must_use]
pub fn status_of(desk: &std::path::Path) -> String {
    let mut said = String::new();
    match std::fs::read_to_string(desk.join("steps.jsonl")) {
        Ok(steps) if !steps.trim().is_empty() => {
            said.push_str("steps so far:\n");
            said.push_str(&steps);
        }
        _ => said.push_str("no steps yet\n"),
    }
    // `ask.json` is written when a run stops to ask and removed when it is
    // answered, so its presence is the question.
    if let Ok(asking) = std::fs::read_to_string(desk.join("ask.json")) {
        said.push_str("\nit is waiting for an answer:\n");
        said.push_str(&asking);
    } else if let Some(how) = ended(desk) {
        said.push_str("\nit is over:\n");
        said.push_str(&how);
        said.push('\n');
    } else {
        said.push_str("\nit is working, and not waiting for anything\n");
    }
    said
}

/// The runs this server has started, by the name it gave them.
///
/// Named here rather than by the caller, so one conversation cannot reach into
/// another's run by guessing a name it was never given.
#[derive(Debug, Default)]
pub struct Sessions {
    runs: std::collections::BTreeMap<String, Run>,
}

impl Sessions {
    /// The devices with a run still going on them.
    ///
    /// Asked rather than remembered, because a run ends on its own: the child
    /// is reaped here, so a finished run stops holding its device.
    pub fn still_running(&mut self) -> Vec<String> {
        self.runs.retain(|_, run| !run.finished());
        self.runs.keys().cloned().collect()
    }

    /// Where a run keeps its desk, if this server started it.
    #[must_use]
    pub fn desk_of(&self, run: &str) -> Option<&std::path::Path> {
        self.runs.get(run).map(|run| run.desk.as_path())
    }

    /// Remember a run this server started.
    pub fn remember(&mut self, name: String, desk: std::path::PathBuf, child: std::process::Child) {
        self.runs.insert(name, Run { desk, child });
    }

    /// Tell a run to look at its desk now.
    ///
    /// An empty line on the run's own input, which is the channel a person
    /// types answers on. The run treats a blank one as "look again", so the
    /// answer just written is picked up in the time a pipe takes rather than
    /// at the run's next look. Without it every impasse costs that interval,
    /// on the one path where a run is doing nothing at all.
    ///
    /// `false` when there is no such run, or its input has been closed.
    pub fn nudge(&mut self, run: &str) -> bool {
        use std::io::Write as _;

        let Some(run) = self.runs.get_mut(run) else {
            return false;
        };
        let Some(input) = run.child.stdin.as_mut() else {
            return false;
        };
        input.write_all(b"\n").and_then(|()| input.flush()).is_ok()
    }

    /// Forget a run, ending it if it is still going.
    pub fn forget(&mut self, run: &str) -> bool {
        match self.runs.remove(run) {
            Some(mut run) => {
                let _ = run.child.kill();
                let _ = run.child.wait();
                true
            }
            None => false,
        }
    }
}

impl Drop for Sessions {
    /// A conversation that ends takes its runs with it.
    ///
    /// Left going, a run carries on tapping at somebody's phone with nothing
    /// watching it and nothing able to answer it when it asks.
    fn drop(&mut self) {
        for (_, run) in std::mem::take(&mut self.runs) {
            let mut run = run;
            let _ = run.child.kill();
            let _ = run.child.wait();
        }
    }
}

/// One run, and where it keeps its desk.
#[derive(Debug)]
struct Run {
    desk: std::path::PathBuf,
    child: std::process::Child,
}

impl Run {
    /// Whether this run has ended on its own.
    fn finished(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)) | Err(_))
    }
}
