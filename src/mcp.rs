//! Speaking the Model Context Protocol, so another agent can drive a device.
//!
//! The interesting part is not the transport. It is that this crate already
//! has the seam an MCP client wants to sit in: a run that cannot decide asks,
//! and anything able to write an answer can answer it. That is the desk, and
//! over MCP the asking party becomes whoever is holding the conversation.
//!
//! So a run is not one call that blocks until it is over. It is started, then
//! looked in on, then answered when it wants something — which is the same
//! shape a person at a terminal already uses.
//!
//! Every tool here is carried out by running the `jev-pilot` binary, rather
//! than by driving the loop in this process. One behaviour, described once: a
//! server that reimplemented the run would drift from the command line it is
//! supposed to be a face for.

/// The protocol version this server speaks.
pub const PROTOCOL: &str = "2025-06-18";

/// Versions this server will answer in, newest first.
///
/// A client that asked for an older one is answered in it: it asked because
/// that is what it understands, and naming the newest instead tells it
/// nothing it can use.
const SPOKEN: &[&str] = &[PROTOCOL, "2025-03-26", "2024-11-05"];

/// The runs this server has started, by the name it gave them.
///
/// Named here rather than by the caller, so one conversation cannot reach into
/// another's run by guessing a name it was never given.
#[derive(Debug, Default)]
pub struct Sessions {
    runs: std::collections::BTreeMap<String, Run>,
    started: u32,
}

impl Sessions {
    /// A name for a run nobody has used yet.
    pub fn name_a_run(&mut self) -> String {
        self.started = self.started.saturating_add(1);
        format!("run-{}", self.started)
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

/// One run, and where it keeps its desk.
#[derive(Debug)]
struct Run {
    desk: std::path::PathBuf,
    child: std::process::Child,
}

/// Hold a conversation: one JSON object per line in, one per line out.
///
/// Newline-delimited because that is what MCP over a pipe is, and because it
/// keeps this readable in a terminal when something has gone wrong.
///
/// A line that is not JSON is complained about rather than hung up on. One bad
/// line says nothing about the next.
///
/// # Errors
/// Returns the first error from reading or writing the pipe. A conversation
/// whose other end has gone is over.
pub fn serve(asked: impl std::io::Read, said: &mut impl std::io::Write) -> std::io::Result<()> {
    use std::io::BufRead as _;

    let mut sessions = Sessions::default();
    for line in std::io::BufReader::new(asked).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let answer = match serde_json::from_str::<serde_json::Value>(&line) {
            Ok(request) => handle(&request, &mut sessions),
            Err(error) => Some(fault(
                &serde_json::Value::Null,
                -32700,
                &format!("that line is not JSON: {error}"),
            )),
        };
        if let Some(answer) = answer {
            writeln!(said, "{answer}")?;
            said.flush()?;
        }
    }
    Ok(())
}

/// Answer one request, or nothing at all when it was a notification.
///
/// Notifications carry no id and want no reply; answering one is a protocol
/// error, and some clients close the connection over it.
#[must_use]
pub fn handle(
    request: &serde_json::Value,
    sessions: &mut Sessions,
) -> Option<serde_json::Value> {
    let id = request.get("id").cloned();
    let method = request.get("method").and_then(serde_json::Value::as_str)?;
    // No id means a notification. It is still dispatched, because some of them
    // matter; it is simply not answered.
    let id = id?;

    let params = request.get("params").cloned().unwrap_or(serde_json::json!({}));
    Some(match method {
        "initialize" => reply(&id, &initialize(&params)),
        "ping" => reply(&id, &serde_json::json!({})),
        "tools/list" => reply(&id, &serde_json::json!({ "tools": tools() })),
        "tools/call" => reply(&id, &call(&params, sessions)),
        _ => fault(&id, -32601, &format!("no such method: {method}")),
    })
}

/// The handshake.
fn initialize(params: &serde_json::Value) -> serde_json::Value {
    let asked = params
        .get("protocolVersion")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(PROTOCOL);
    let speaking = if SPOKEN.contains(&asked) { asked } else { PROTOCOL };
    serde_json::json!({
        "protocolVersion": speaking,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": "jev-pilot", "version": env!("CARGO_PKG_VERSION") },
    })
}

/// What this server can be asked to do.
///
/// Every entry says plainly whether it changes the device. A client is going
/// to put these in front of a person, and "read the screen" and "move money
/// through an app" are not the same kind of permission.
fn tools() -> Vec<serde_json::Value> {
    let tool = |name: &str, about: &str, schema: serde_json::Value, reads: bool, wrecks: bool| {
        serde_json::json!({
            "name": name,
            "description": about,
            "inputSchema": schema,
            "annotations": {
                "readOnlyHint": reads,
                "destructiveHint": wrecks,
                "openWorldHint": true,
            },
        })
    };
    let device = serde_json::json!({
        "type": "object",
        "properties": {
            "device": {
                "type": "string",
                "description": "Which device, when more than one is attached.",
            },
        },
    });
    let named = |extra: serde_json::Value| {
        let mut schema = device.clone();
        let (Some(into), Some(from)) = (
            schema["properties"].as_object_mut(),
            extra.as_object(),
        ) else {
            return schema;
        };
        for (key, value) in from {
            into.insert(key.clone(), value.clone());
        }
        schema
    };
    let run = serde_json::json!({
        "type": "object",
        "properties": {
            "run": { "type": "string", "description": "Which run, as start_run named it." },
        },
        "required": ["run"],
    });

    let mut offered = looking(&tool, &device, &named);
    offered.extend(acting(&tool, &named, &run));
    offered
}

/// The tools that only look.
fn looking(
    tool: &dyn Fn(&str, &str, serde_json::Value, bool, bool) -> serde_json::Value,
    device: &serde_json::Value,
    named: &dyn Fn(serde_json::Value) -> serde_json::Value,
) -> Vec<serde_json::Value> {
    vec![
        tool(
            "observe",
            "Read what is on the device's screen right now: the rows that can \
             be acted on, what the screen says, which controls it shows but \
             will not let anything use, and which app is in front. Changes \
             nothing. This is the cheapest way to find out where a device is.",
            device.clone(),
            true,
            false,
        ),
        tool(
            "devices",
            "List the devices attached to this machine, so a later call can \
             name one. Changes nothing.",
            serde_json::json!({ "type": "object", "properties": {} }),
            true,
            false,
        ),
        tool(
            "helper",
            "Report on the on-device helper that reads screens quickly, and \
             install it when asked to. Without it every screen read takes \
             about two seconds instead of about fifty milliseconds.",
            named(serde_json::json!({
                "install": {
                    "type": "boolean",
                    "description": "Install and enable it, rather than only reporting.",
                },
            })),
            false,
            false,
        ),
    ]
}

/// The tools that act on the device.
fn acting(
    tool: &dyn Fn(&str, &str, serde_json::Value, bool, bool) -> serde_json::Value,
    named: &dyn Fn(serde_json::Value) -> serde_json::Value,
    run: &serde_json::Value,
) -> Vec<serde_json::Value> {
    vec![
        tool(
            "start_run",
            "Drive the device towards a goal, written in plain words. THIS \
             ACTS ON A REAL DEVICE: it taps, types and navigates, and on an \
             app that moves money or sends messages it will do those things. \
             Returns at once with a name for the run; watch it with \
             run_status and answer it with answer_run.",
            named(serde_json::json!({
                "goal": {
                    "type": "string",
                    "description": "What to achieve, in plain words.",
                },
                "app": {
                    "type": "string",
                    "description": "The app package the goal is about. It is brought to the \
                                    front first, and the run knows when it has left it.",
                },
                "accept": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Claims that must hold on the final screen before success \
                                    is accepted. Write them about text the screen shows.",
                },
                "text": {
                    "type": "object",
                    "description": "Words to type, keyed by the field they belong in. A field \
                                    with nothing supplied stops the run to ask.",
                    "additionalProperties": { "type": "string" },
                },
                "steps": { "type": "integer", "description": "How many steps before giving up." },
                "floor": {
                    "type": "number",
                    "description": "Below this confidence the run asks rather than acts. \
                                    Lowering it applies to ordinary gestures only.",
                },
            })),
            false,
            true,
        ),
        tool(
            "run_status",
            "How a run is getting on: every step it has taken, whether it is \
             waiting for an answer and what it is asking, and how it ended if \
             it has. Changes nothing.",
            run.clone(),
            true,
            false,
        ),
        tool(
            "answer_run",
            "Answer a run that stopped to ask. Name an operation it was \
             offered, the row or field it applies to, or the words to type. \
             This is how the caller becomes the second opinion a run escalates \
             to, so it acts on the device.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "run": { "type": "string", "description": "Which run, as start_run named it." },
                    "operation": {
                        "type": "string",
                        "description": "One of the operations the run listed, or `stop` to end it.",
                    },
                    "target": {
                        "type": "integer",
                        "description": "Which row, or which field when typing, counting from zero.",
                    },
                    "text": { "type": "string", "description": "The words, when it asked for some." },
                },
                "required": ["run"],
            }),
            false,
            true,
        ),
        tool(
            "stop_run",
            "End a run that is still going, leaving the device wherever it \
             got to. Nothing further is done to the device.",
            run.clone(),
            false,
            false,
        ),
    ]
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

/// Carry out one tool call.
fn call(params: &serde_json::Value, sessions: &mut Sessions) -> serde_json::Value {
    let name = params
        .get("name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or(serde_json::json!({}));
    let named_run = || {
        arguments
            .get("run")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    match name {
        "observe" | "devices" | "helper" => match invocation(name, &arguments) {
            Some(args) => match run_pilot(&args) {
                Ok(said) => said_so(&said),
                Err(error) => failed(&format!("could not run jev-pilot: {error}")),
            },
            None => failed(&format!("{name} was given arguments it cannot use")),
        },
        "start_run" => start(&arguments, sessions),
        "run_status" => {
            let run = named_run();
            match sessions.desk_of(&run) {
                Some(desk) => said_so(&status_of(desk)),
                None => failed(&unknown(&run)),
            }
        }
        "answer_run" => {
            let run = named_run();
            let Some(desk) = sessions.desk_of(&run) else {
                return failed(&unknown(&run));
            };
            let Some(answer) = answer_from(&arguments) else {
                return failed(
                    "an answer needs an operation, a target or some text; \
                     this one said nothing",
                );
            };
            match std::fs::write(desk.join("answer.json"), answer.to_string()) {
                Ok(()) => said_so(&format!("answered {run} with {answer}")),
                Err(error) => failed(&format!("could not answer {run}: {error}")),
            }
        }
        "stop_run" => {
            let run = named_run();
            if sessions.forget(&run) {
                said_so(&format!("{run} stopped; the device is left where it got to"))
            } else {
                failed(&unknown(&run))
            }
        }
        // Reported inside the result rather than as a transport fault: the
        // call was understood and refused, which is a thing the asking model
        // can read and act on.
        _ => failed(&format!("no such tool: {name}")),
    }
}

fn unknown(run: &str) -> String {
    format!("no run called {run}; start_run names them")
}

/// Begin a run, and hand back the name to watch it by.
fn start(arguments: &serde_json::Value, sessions: &mut Sessions) -> serde_json::Value {
    let Some(mut args) = invocation("start_run", arguments) else {
        return failed("a run needs a goal");
    };
    let name = sessions.name_a_run();
    let desk = std::env::temp_dir().join(format!("jev-pilot-mcp-{name}"));
    if let Err(error) = std::fs::create_dir_all(&desk) {
        return failed(&format!("could not make a desk for {name}: {error}"));
    }
    // The desk goes first: the goal is the one positional and must stay last.
    let goal = args.pop().unwrap_or_default();
    args.push("--desk".to_owned());
    args.push(desk.display().to_string());
    args.push(goal);

    match std::process::Command::new(binary())
        .args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(child) => {
            sessions.remember(name.clone(), desk, child);
            said_so(&format!(
                "{name} started. It acts on the device from here. Watch it with \
                 run_status, and when it asks, answer it with answer_run."
            ))
        }
        Err(error) => failed(&format!("could not start jev-pilot: {error}")),
    }
}

/// How a run is getting on, as far as its desk can say.
fn status_of(desk: &std::path::Path) -> String {
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
    } else {
        said.push_str("\nit is not waiting for anything\n");
    }
    said
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

/// A tool call that worked.
fn said_so(said: &str) -> serde_json::Value {
    serde_json::json!({ "content": [{ "type": "text", "text": said }] })
}

/// A tool call that could not be carried out.
fn failed(said: &str) -> serde_json::Value {
    serde_json::json!({
        "isError": true,
        "content": [{ "type": "text", "text": said }],
    })
}

fn reply(id: &serde_json::Value, result: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn fault(id: &serde_json::Value, code: i32, message: &str) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    })
}
