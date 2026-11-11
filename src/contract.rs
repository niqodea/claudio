//! The checks of what claudio assumes of Claude Code beyond the SDK's types, one for each
//! assumption, which `claudio check-claude-code` runs against the Claude Code that claudio runs.
//! A check's doc says what it assumes and how it was learned. The code that relies on it is the
//! smallest item that does, a function, type or field of its own, so a link to the check stays
//! true for as long as the item lives. Its doc links to the check with "Undocumented:", the only
//! place claudio says "Undocumented", so a check that goes away breaks the link, and `lint`
//! fails. The unit tests take what Claude Code writes from a check's
//! fixture, which the check compares with what Claude Code writes, so they never drift apart.
//!
//! A free check makes no call to the API. A paid one makes a few, with the cheapest model, and
//! runs only with `--paid`.
//! Ref: <https://martinfowler.com/bliki/ContractTest.html>, on checking a test double against
//! the real service

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tempfile::TempDir;

use crate::claude_code;
use crate::protocol::{self, Event, read_events};

/// A check, which fails with what Claude Code did instead.
pub struct Check {
    pub name: &'static str,
    pub is_paid: bool,
    pub run: fn(&Path) -> Result<(), String>,
}

pub const CHECKS: [Check; 8] = [
    Check {
        name: "session state comes with its variable",
        is_paid: false,
        run: session_state_comes_with_its_variable,
    },
    Check {
        name: "send now is announced",
        is_paid: false,
        run: send_now_is_announced,
    },
    Check {
        name: "the paying key is named by its source",
        is_paid: false,
        run: the_paying_key_is_named_by_its_source,
    },
    Check {
        name: "a clear resets the session",
        is_paid: false,
        run: a_clear_resets_the_session,
    },
    Check {
        name: "a rate limit event has every window",
        is_paid: true,
        run: a_rate_limit_event_has_every_window,
    },
    Check {
        name: "a permission after the input closes fails",
        is_paid: true,
        run: a_permission_after_the_input_closes_fails,
    },
    Check {
        name: "SIGINT stops Claude Code after its input closes",
        is_paid: true,
        run: sigint_stops_claude_code_after_its_input_closes,
    },
    Check {
        name: "the output closes as Claude Code exits",
        is_paid: true,
        run: the_output_closes_as_claude_code_exits,
    },
];

/// The model of the paid checks, the cheapest.
pub const MODEL: &str = "haiku";

/// How long a free check waits for Claude Code, which answers at once.
const FREE_TIMEOUT: Duration = Duration::from_secs(60);

/// How long a paid check waits for Claude Code, which calls the API.
const PAID_TIMEOUT: Duration = Duration::from_secs(180);

/// Read in Claude Code 2.1.316 and seen with it: with `CLAUDE_CODE_SDK_READS_SESSION_STATE`
/// set, Claude Code sends `session_state_changed` as a turn starts and ends, and without it,
/// never. The SDK's types type the message, but name no variable.
pub(crate) fn session_state_comes_with_its_variable(program: &Path) -> Result<(), String> {
    let with = Run::start(program, &[], &[], &[])?.clear_once()?;
    compare(&["state"], &with, &session_states_of_a_turn())?;
    let without =
        Run::start(program, &[], &[], &["CLAUDE_CODE_SDK_READS_SESSION_STATE"])?.clear_once()?;
    compare(&["state"], &without, &[])
}

/// The session states of a turn, as Claude Code sends them.
#[must_use]
pub fn session_states_of_a_turn() -> Vec<String> {
    ["running", "idle"].map(session_state).to_vec()
}

/// A change of the session's state to `state`, like `running`.
#[must_use]
pub fn session_state(state: &str) -> String {
    json!({"type": "system", "subtype": "session_state_changed", "state": state}).to_string()
}

/// From Claude Code's changelog and seen with 2.1.322: the fields of a send-now interrupt are
/// Claude Code's own, absent from the SDK's types, and Claude Code announces them as
/// `interrupt_send_now_v1` in the capabilities of its init. One without them takes the request
/// as a plain interrupt.
/// Ref: <https://code.claude.com/docs/en/changelog>, on send now moving running tools to the
/// background
pub(crate) fn send_now_is_announced(program: &Path) -> Result<(), String> {
    // A clear starts a session, whose init comes without a call
    let lines = Run::start(program, &[], &[], &[])?.clear_once()?;
    let announces = lines.iter().any(|line| {
        let line: Value = serde_json::from_str(line).unwrap_or_default();
        line["subtype"] == "init"
            && line["capabilities"]
                .as_array()
                .is_some_and(|capabilities| capabilities.contains(&json!("interrupt_send_now_v1")))
    });
    if announces {
        Ok(())
    } else {
        Err("no init has the interrupt_send_now_v1 capability".to_string())
    }
}

/// Seen with Claude Code 2.1.319 and 2.1.322: `ANTHROPIC_API_KEY` fills the account's
/// `apiKeySource` with its name, `ANTHROPIC_AUTH_TOKEN` fills `tokenSource` with its name, and
/// the subscription's login leaves `apiKeySource` out and `tokenSource` out or `claude.ai`. The
/// check needs a login to the subscription, and no settings file that sets either variable.
pub(crate) fn the_paying_key_is_named_by_its_source(program: &Path) -> Result<(), String> {
    const KEYS: [&str; 2] = ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN"];
    let cases = [
        (None, subscription_account()),
        (Some("ANTHROPIC_API_KEY"), api_key_account()),
        (Some("ANTHROPIC_AUTH_TOKEN"), auth_token_account()),
    ];
    for (key, account) in cases {
        let env: Vec<(&str, &str)> = key.map(|key| (key, "claudio-check")).into_iter().collect();
        let removed: Vec<&str> = KEYS
            .into_iter()
            .filter(|other| Some(*other) != key)
            .collect();
        let mut run = Run::start(program, &[], &env, &removed)?;
        run.send(&protocol::Command::Initialize)?;
        run.close_input();
        let lines = run.read_to_end(FREE_TIMEOUT)?;
        let real = texts(&lines);
        compare(&["paying"], &real, &[initialize_response(&account)])
            .map_err(|error| format!("with {}: {error}", key.unwrap_or("the subscription")))?;
    }
    Ok(())
}

#[must_use]
pub fn subscription_account() -> Value {
    json!({"subscriptionType": "Claude Team", "apiProvider": "firstParty"})
}

#[must_use]
pub fn api_key_account() -> Value {
    json!({"tokenSource": "claude.ai", "apiKeySource": "ANTHROPIC_API_KEY", "apiProvider": "firstParty"})
}

#[must_use]
pub fn auth_token_account() -> Value {
    json!({"tokenSource": "ANTHROPIC_AUTH_TOKEN", "apiProvider": "firstParty"})
}

/// The answer to `initialize` for `account`.
#[must_use]
pub fn initialize_response(account: &Value) -> String {
    let line = json!({
        "type": "control_response",
        "response": {"subtype": "success", "request_id": protocol::INITIALIZE_REQUEST_ID, "response": {
            "commands": [],
            "account": account,
        }},
    });
    line.to_string()
}

/// Seen with Claude Code 2.1.322: `/clear` sends `conversation_reset`, whose `session_id` is
/// the cleared session's, then the new session's init, then a result that ends a turn of no
/// calls.
pub(crate) fn a_clear_resets_the_session(program: &Path) -> Result<(), String> {
    let mut run = Run::start(program, &[], &[], &[])?;
    run.send(&protocol::Command::Clear)?;
    run.send(&protocol::Command::Clear)?;
    run.close_input();
    let lines = run.read_to_end(FREE_TIMEOUT)?;
    let real = texts(&lines);
    compare(&["reset", "session", "turn end"], &real, &two_clears())
}

/// What Claude Code writes for two `/clear`s in a row, of which the second one clears the
/// session that the first one started.
#[must_use]
pub fn two_clears() -> Vec<String> {
    clear("first", "second")
        .into_iter()
        .chain(clear("second", "third"))
        .collect()
}

/// What Claude Code writes for a `/clear` that clears the session `cleared` and starts `new`.
#[must_use]
pub fn clear(cleared: &str, new: &str) -> [String; 3] {
    [
        json!({"type": "conversation_reset", "trigger": "clear", "session_id": cleared})
            .to_string(),
        init(new),
        json!({"type": "result", "subtype": "success"}).to_string(),
    ]
}

/// The init of the session `session`, which Claude Code sends as the session starts, with the
/// interrupts it takes.
#[must_use]
pub fn init(session: &str) -> String {
    let line = json!({
        "type": "system",
        "subtype": "init",
        "session_id": session,
        "claude_code_version": claude_code::VERSION,
        "capabilities": ["interrupt_send_now_v1"],
    });
    line.to_string()
}

/// Read in the schema of Claude Code 2.1.316, which marks it internal: only `unifiedWindows`
/// holds every limit of the plan, where the documented fields describe only the limit that
/// applies now. The check needs a login to the subscription, whose plan has the limits.
pub(crate) fn a_rate_limit_event_has_every_window(program: &Path) -> Result<(), String> {
    let mut run = Run::start(program, &["--model", MODEL], &[], &[])?;
    run.send_message("Reply with the single word ok.")?;
    run.close_input();
    let lines = run.read_to_end(PAID_TIMEOUT)?;
    let mut real = summary(&["limits"], &events(&texts(&lines)));
    // A turn can tell the limits more than once, each time whole
    real.dedup();
    same(&real, &summary(&["limits"], &events(&[rate_limit_event()])))
}

/// A rate limit event, as Claude Code 2.1.316 sent it.
#[must_use]
pub fn rate_limit_event() -> String {
    let line = json!({
        "type": "rate_limit_event",
        "rate_limit_info": {
            "status": "allowed",
            "resetsAt": 1_794_088_800,
            "rateLimitType": "five_hour",
            "overageStatus": "rejected",
            "overageDisabledReason": "out_of_credits",
            "isUsingOverage": false,
            "unifiedWindows": {
                "five_hour": {"utilization": 0.39, "resetsAt": 1_794_088_800, "status": "allowed"},
                "seven_day": {"utilization": 1.0, "resetsAt": 1_794_492_000, "status": "rejected"},
            },
        },
        "uuid": "a",
        "session_id": "b",
    });
    line.to_string()
}

/// Seen with Claude Code 2.1.316: once its input closed, Claude Code fails each permission it
/// would ask for, with "Stream closed" in the tool's result.
pub(crate) fn a_permission_after_the_input_closes_fails(program: &Path) -> Result<(), String> {
    let mut run = Run::start(
        program,
        &["--model", MODEL, "--permission-mode", "default"],
        &[],
        &[],
    )?;
    run.send_message(
        "Use the Bash tool to run this command, and nothing else: touch claudio-check",
    )?;
    run.close_input();
    let lines = run.read_to_end(PAID_TIMEOUT)?;
    let results: Vec<String> = events(&texts(&lines))
        .into_iter()
        .filter_map(|event| match event {
            Event::ToolResult(result) => Some(result.text.unwrap_or_default()),
            _ => None,
        })
        .collect();
    if results.iter().any(|text| text.contains("Stream closed")) {
        Ok(())
    } else {
        Err(format!("no tool's result says Stream closed: {results:?}"))
    }
}

/// Seen with Claude Code 2.1.316: SIGINT stops Claude's turn and its background tasks, as
/// Ctrl-C does a program's work in the terminal, and Claude Code then exits, since its input
/// closed.
pub(crate) fn sigint_stops_claude_code_after_its_input_closes(
    program: &Path,
) -> Result<(), String> {
    let mut run = Run::start(
        program,
        &["--model", MODEL, "--allowedTools", "Bash(sleep:*)"],
        &[],
        &[],
    )?;
    run.send_message(
        "Run the Bash command `sleep 60` with run_in_background set to true, then reply with the single word started.",
    )?;
    run.close_input();
    run.read_until(PAID_TIMEOUT, |line| {
        line["subtype"] == "background_tasks_changed"
            && line["tasks"]
                .as_array()
                .is_some_and(|tasks| !tasks.is_empty())
    })?;
    run.interrupt()?;
    run.wait(Duration::from_secs(15))
        .map_err(|error| format!("after SIGINT, {error}"))
}

/// Seen with Claude Code 2.1.316: the processes Claude Code starts write to files of their own,
/// so its output closes as it exits, even while a process it started still runs.
pub(crate) fn the_output_closes_as_claude_code_exits(program: &Path) -> Result<(), String> {
    let mut run = Run::start(
        program,
        &["--model", MODEL, "--allowedTools", "Bash(sleep:*)"],
        &[],
        &[],
    )?;
    run.send_message(
        "Run this exact Bash command, without run_in_background: sleep 60 & Then reply with the single word started.",
    )?;
    run.close_input();
    run.wait(PAID_TIMEOUT)?;
    // The output holds what Claude Code wrote before it exited, which a reader takes at once
    let lines = run
        .read_to_end(Duration::from_secs(2))
        .map_err(|error| format!("after Claude Code exited, {error}"))?;
    let ran_bash = events(&texts(&lines))
        .iter()
        .any(|event| matches!(event, Event::ToolUse(tool_use) if tool_use.name == "Bash"));
    if ran_bash {
        Ok(())
    } else {
        Err("Claude ran no Bash command, so nothing it started outlived it".to_string())
    }
}

/// Compares the events of the kinds in `kinds` that `real` and `fixture` tell, in order.
fn compare(kinds: &[&str], real: &[String], fixture: &[String]) -> Result<(), String> {
    same(
        &summary(kinds, &events(real)),
        &summary(kinds, &events(fixture)),
    )
}

fn same(real: &[String], fixture: &[String]) -> Result<(), String> {
    if real == fixture {
        Ok(())
    } else {
        Err(format!(
            "Claude Code told {real:?}, where claudio assumes {fixture:?}"
        ))
    }
}

/// The lines as Claude Code wrote them.
fn texts(lines: &[Value]) -> Vec<String> {
    lines.iter().map(Value::to_string).collect()
}

/// The events claudio reads in `lines`, of which a line it cannot read tells none.
fn events(lines: &[String]) -> Vec<Event> {
    lines
        .iter()
        .flat_map(|line| read_events(line).unwrap_or_default())
        .collect()
}

/// Each event of the kinds in `kinds`, as the kind and what claudio takes from it. Ids are
/// named by their order of appearance, so the fixture's ids and Claude Code's are equal when
/// each repeats in the same places.
fn summary(kinds: &[&str], events: &[Event]) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    let mut id = |value: &str| {
        let index = ids.iter().position(|id| id == value).unwrap_or_else(|| {
            ids.push(value.to_string());
            ids.len() - 1
        });
        format!("id {}", index + 1)
    };
    events
        .iter()
        .filter_map(|event| match event {
            Event::State(state) => Some(("state", format!("{state:?}"))),
            Event::PayingKey(key) => Some(("paying", key.clone())),
            Event::Reset { previous_session } => Some(("reset", id(previous_session))),
            Event::SessionId(session) => Some(("session", id(session))),
            Event::TurnEnd { failure } => Some(("turn end", format!("{failure:?}"))),
            Event::Limits { session, week } => Some((
                "limits",
                format!("session {} week {}", session.is_some(), week.is_some()),
            )),
            _ => None,
        })
        .filter(|(kind, _)| kinds.contains(kind))
        .map(|(kind, value)| format!("{kind}: {value}"))
        .collect()
}

/// Claude Code, run as claudio runs it, in a directory of its own.
struct Run {
    child: Child,
    input: Option<ChildStdin>,
    lines: Receiver<Value>,
    _directory: TempDir,
}

impl Run {
    /// Starts `program` with `args` after claudio's own, `env` set and `removed` unset.
    fn start(
        program: &Path,
        args: &[&str],
        env: &[(&str, &str)],
        removed: &[&str],
    ) -> Result<Self, String> {
        let directory =
            tempfile::tempdir().map_err(|error| format!("Cannot create a directory: {error}"))?;
        let mut command = claude_code::command(program);
        command
            .args(args)
            .current_dir(directory.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for (name, value) in env {
            command.env(name, value);
        }
        for name in removed {
            command.env_remove(name);
        }
        let mut child = command
            .spawn()
            .map_err(|error| format!("Cannot start {}: {error}", program.display()))?;
        let input = child.stdin.take();
        let output = child.stdout.take().expect("stdout is piped");
        let (sender, lines) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(output).lines().map_while(Result::ok) {
                let line = serde_json::from_str(&line).unwrap_or(Value::String(line));
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        Ok(Run {
            child,
            input,
            lines,
            _directory: directory,
        })
    }

    fn send(&mut self, command: &protocol::Command) -> Result<(), String> {
        let input = self.input.as_mut().expect("the input is open");
        input
            .write_all(command.to_line().as_bytes())
            .and_then(|()| input.flush())
            .map_err(|error| format!("Cannot write to Claude Code: {error}"))
    }

    fn send_message(&mut self, text: &str) -> Result<(), String> {
        self.send(&protocol::Command::Message {
            id: "claudio-check",
            text,
        })
    }

    fn close_input(&mut self) {
        self.input = None;
    }

    /// Sends a `/clear`, and returns what Claude Code writes until it exits, its input closed.
    fn clear_once(mut self) -> Result<Vec<String>, String> {
        self.send(&protocol::Command::Clear)?;
        self.close_input();
        let lines = self.read_to_end(FREE_TIMEOUT)?;
        Ok(texts(&lines))
    }

    /// The lines Claude Code writes, up to the first one that `is_last` takes, within `timeout`.
    fn read_until(
        &mut self,
        timeout: Duration,
        is_last: impl Fn(&Value) -> bool,
    ) -> Result<Vec<Value>, String> {
        let deadline = Instant::now() + timeout;
        let mut lines = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(line) => {
                    let is_done = is_last(&line);
                    lines.push(line);
                    if is_done {
                        return Ok(lines);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    return Err(format!("Claude Code wrote no awaited line in {timeout:?}"));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err("Claude Code's output closed before the awaited line".to_string());
                }
            }
        }
    }

    /// The lines Claude Code writes until its output closes, within `timeout`.
    fn read_to_end(&mut self, timeout: Duration) -> Result<Vec<Value>, String> {
        let deadline = Instant::now() + timeout;
        let mut lines = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(line) => lines.push(line),
                Err(RecvTimeoutError::Disconnected) => return Ok(lines),
                Err(RecvTimeoutError::Timeout) => {
                    return Err(format!("Claude Code's output stayed open for {timeout:?}"));
                }
            }
        }
    }

    fn interrupt(&self) -> Result<(), String> {
        // SAFETY: kill only sends a signal, to a child that this run waits for only once it
        // ends, so its id still names it
        if unsafe { libc::kill(self.child.id().cast_signed(), libc::SIGINT) } == -1 {
            return Err(format!(
                "Cannot send SIGINT: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }

    /// Waits for Claude Code to exit, within `timeout`.
    fn wait(&mut self, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(Some(_)) => return Ok(()),
                Ok(None) => thread::sleep(Duration::from_millis(100)),
                Err(error) => return Err(format!("Cannot wait for Claude Code: {error}")),
            }
        }
        Err(format!("Claude Code still ran after {timeout:?}"))
    }
}

impl Drop for Run {
    /// Ends a Claude Code that a failed check left running.
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
