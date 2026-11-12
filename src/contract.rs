//! The checks of what claudio assumes of Claude Code beyond the SDK's types, one for each
//! assumption, which `claudio check-claude-code` runs against the Claude Code that claudio runs.
//! A check's doc says what it assumes and how it was learned. The code that relies on it is the
//! smallest item that does, a function, type or field of its own, so a link to the check stays
//! true for as long as the item lives. Its doc links to the check with "Undocumented:", the only
//! place claudio says "Undocumented", so a check that goes away breaks the link, and `lint`
//! fails. The unit tests take what Claude Code writes from a check's
//! fixture, which the check compares with what Claude Code writes, so they never drift apart.
//!
//! The README's "What claudio relies on" tells where the checks fit: what claudio shows is
//! either promised by the SDK's types or observed by a check.
//!
//! A free check makes no call to the API, or calls the fake of `fake_api` instead. A paid one
//! makes a few, with the cheapest model, and runs only with `--paid`.
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
use crate::fake_api::FakeApi;
use crate::protocol::{self, Event, read_events};

/// A check, which fails with what Claude Code did instead.
pub struct Check {
    pub name: &'static str,
    pub is_paid: bool,
    pub run: fn(&Path) -> Result<(), String>,
}

pub const CHECKS: [Check; 12] = [
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
        name: "a slow permission check is told",
        is_paid: false,
        run: a_slow_permission_check_is_told,
    },
    Check {
        name: "a background task is told from start to end",
        is_paid: false,
        run: a_background_task_is_told_from_start_to_end,
    },
    Check {
        name: "a call moved to the background is told",
        is_paid: false,
        run: a_call_moved_to_the_background_is_told,
    },
    Check {
        name: "a stopped call gets its result",
        is_paid: false,
        run: a_stopped_call_gets_its_result,
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
    run.send(&protocol::Command::Run { command: "clear" })?;
    run.send(&protocol::Command::Run { command: "clear" })?;
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

/// Read in the schema of Claude Code 2.1.322, which marks it internal, and seen with it: in auto
/// mode, once the classifier has not answered for 4 seconds whether a tool call needs the
/// user's approval, Claude Code sends `permission_check_status` with `checking`, then with `done`
/// as the classifier answers, both between the call and its result.
///
/// The check runs Claude Code against [`crate::fake_api`], whose classifier answers after 5
/// seconds. The call runs `sh -c true`, since Claude Code allows a read-only command without the
/// classifier.
pub(crate) fn a_slow_permission_check_is_told(program: &Path) -> Result<(), String> {
    let lines = on_fake(program, Duration::from_secs(5), |run| {
        run.send_message(SLOW_PERMISSION_CHECK_MESSAGE)?;
        Ok(Vec::new())
    })?;
    compare(
        &["tool use", "permission check", "tool result"],
        &lines,
        &slow_permission_check(),
    )
}

/// The message that the check of a slow permission check sends, which `fake-claude` answers
/// with [`slow_permission_check`].
pub const SLOW_PERMISSION_CHECK_MESSAGE: &str = "Run sh -c true with Bash";

/// What Claude Code writes for a tool call whose permission check is slow, as 2.1.322 did,
/// without the turn's other lines.
#[must_use]
pub fn slow_permission_check() -> Vec<String> {
    let check = |status: &str| {
        json!({
            "type": "system",
            "subtype": "permission_check_status",
            "tool_use_id": "toolu_check",
            "status": status,
            "uuid": "a",
            "session_id": "b",
        })
    };
    let lines = [
        json!({
            "type": "assistant",
            "message": {"content": [{
                "type": "tool_use",
                "id": "toolu_check",
                "name": "Bash",
                "input": {"command": "sh -c true", "description": "Run sh -c true"},
            }]},
            "parent_tool_use_id": null,
        }),
        check("checking"),
        check("done"),
        json!({
            "type": "user",
            "message": {"content": [{
                "type": "tool_result",
                "tool_use_id": "toolu_check",
                "content": "",
                "is_error": false,
            }]},
            "parent_tool_use_id": null,
        }),
    ];
    lines.iter().map(Value::to_string).collect()
}

/// Seen with Claude Code 2.1.322: a command that Claude runs in the background gets a
/// `task_started` that links its task to the call and says it runs in the background, before the
/// call's result. As the task ends, `task_notification` tells how, as `completed`, `failed` or,
/// once `TaskStop` stopped it, `stopped`. `background_tasks_changed` lists the task while it
/// runs, as the session runs, the input open, while Claude is idle too.
///
/// The check runs Claude Code against [`crate::fake_api`], and waits for each task's end before
/// it closes the input, since Claude Code waits for its background tasks once the input closes.
pub(crate) fn a_background_task_is_told_from_start_to_end(program: &Path) -> Result<(), String> {
    for (message, status) in [
        (BACKGROUND_TASK_MESSAGE, "completed"),
        (FAILED_BACKGROUND_TASK_MESSAGE, "failed"),
    ] {
        let lines = on_fake(program, Duration::ZERO, |run| {
            run.send_message(message)?;
            run.read_until(FREE_TIMEOUT, is_no_background_task)
        })?;
        compare(TASK_KINDS, &lines, &background_task(status))
            .map_err(|error| format!("a task that {status}: {error}"))?;
    }
    let lines = on_fake(program, Duration::ZERO, |run| {
        run.send_message(LONG_BACKGROUND_TASK_MESSAGE)?;
        let mut lines = run.read_until(FREE_TIMEOUT, |line| line["subtype"] == "task_started")?;
        let task_id = lines
            .last()
            .map_or("", |line| line["task_id"].as_str().unwrap_or(""));
        run.send(&protocol::Command::Message {
            id: "claudio-check-stop",
            text: &stop_message(task_id),
        })?;
        lines.extend(run.read_until(FREE_TIMEOUT, is_no_background_task)?);
        lines.extend(run.read_until(FREE_TIMEOUT, is_tool_result)?);
        Ok(lines)
    })?;
    compare(TASK_KINDS, &lines, &stopped_background_task())
        .map_err(|error| format!("a task that TaskStop stopped: {error}"))
}

/// Seen with Claude Code 2.1.322: a message sent with send now, while a command runs in the
/// foreground, moves the command to the background. Its task started in the foreground, as for
/// any command that runs for a while, then a `task_updated` says it moved, before the call's
/// result. The task's end then comes as for any background task.
pub(crate) fn a_call_moved_to_the_background_is_told(program: &Path) -> Result<(), String> {
    let lines = on_fake(program, Duration::ZERO, |run| {
        run.send_message(MOVED_CALL_MESSAGE)?;
        let mut lines = run.read_until(FREE_TIMEOUT, is_tool_use)?;
        run.send(&protocol::Command::Message {
            id: "claudio-check-move",
            text: REPLY_OK_MESSAGE,
        })?;
        run.send(&protocol::Command::SendNow {
            request_id: "claudio-check-send-now".to_string(),
            message_id: "claudio-check-move",
        })?;
        lines.extend(run.read_until(FREE_TIMEOUT, is_no_background_task)?);
        Ok(lines)
    })?;
    compare(TASK_KINDS, &lines, &moved_call())
}

/// Seen with Claude Code 2.1.322: an interrupt fails the call that runs with a result of its
/// own, so every call gets its result, however its turn ends.
pub(crate) fn a_stopped_call_gets_its_result(program: &Path) -> Result<(), String> {
    let lines = on_fake(program, Duration::ZERO, |run| {
        run.send_message(STOPPED_CALL_MESSAGE)?;
        let mut lines = run.read_until(FREE_TIMEOUT, is_tool_use)?;
        run.send(&protocol::Command::Interrupt {
            request_id: "claudio-check-interrupt".to_string(),
        })?;
        lines.extend(run.read_until(FREE_TIMEOUT, |line| line["type"] == "result")?);
        Ok(lines)
    })?;
    compare(&["tool use", "tool result"], &lines, &stopped_call())
}

/// The kinds of event that the checks of tasks compare.
const TASK_KINDS: &[&str] = &[
    "tool use",
    "task started",
    "task backgrounded",
    "background",
    "tool result",
    "task ended",
];

fn is_tool_use(line: &Value) -> bool {
    line["type"] == "assistant" && line["message"]["content"][0]["type"] == "tool_use"
}

fn is_tool_result(line: &Value) -> bool {
    line["type"] == "user" && line["message"]["content"][0]["type"] == "tool_result"
}

/// Whether `line` tells that no task runs in the background, which a check sees only once a
/// task it started ended.
fn is_no_background_task(line: &Value) -> bool {
    line["subtype"] == "background_tasks_changed"
        && line["tasks"].as_array().is_some_and(Vec::is_empty)
}

/// The messages of the checks of tasks, which `fake-claude` answers with their fixtures.
pub const BACKGROUND_TASK_MESSAGE: &str = "Run sleep 1 in the background with Bash";
pub const FAILED_BACKGROUND_TASK_MESSAGE: &str = "Run sleep 1; exit 3 in the background with Bash";
pub const LONG_BACKGROUND_TASK_MESSAGE: &str = "Run sleep 30 in the background with Bash";
pub const MOVED_CALL_MESSAGE: &str = "Run sleep 4 with Bash";
pub const STOPPED_CALL_MESSAGE: &str = "Run sleep 5 with Bash";
pub const REPLY_OK_MESSAGE: &str = "Reply ok";

/// The message that has Claude stop the task `task_id` with `TaskStop`.
#[must_use]
pub fn stop_message(task_id: &str) -> String {
    format!("Stop {task_id} with TaskStop")
}

/// The task id of the fixtures.
const TASK_ID: &str = "b040svj56";

fn tool_use(id: &str, name: &str, input: &Value) -> Value {
    json!({
        "type": "assistant",
        "message": {"content": [{"type": "tool_use", "id": id, "name": name, "input": input}]},
        "parent_tool_use_id": null,
    })
}

fn tool_result(id: &str, content: &str) -> Value {
    json!({
        "type": "user",
        "message": {"content": [{
            "type": "tool_result",
            "tool_use_id": id,
            "content": content,
            "is_error": false,
        }]},
        "parent_tool_use_id": null,
    })
}

fn background_tasks(tasks: &[&str]) -> Value {
    let tasks: Vec<Value> = tasks
        .iter()
        .map(|description| {
            json!({"task_id": TASK_ID, "task_type": "local_bash", "description": description})
        })
        .collect();
    json!({"type": "system", "subtype": "background_tasks_changed", "tasks": tasks})
}

fn task_started(call: &str, description: &str, is_backgrounded: bool) -> Value {
    json!({
        "type": "system",
        "subtype": "task_started",
        "task_id": TASK_ID,
        "tool_use_id": call,
        "task_type": "local_bash",
        "description": description,
        "is_backgrounded": is_backgrounded,
    })
}

fn task_notification(call: &str, status: &str, summary: &str) -> Value {
    json!({
        "type": "system",
        "subtype": "task_notification",
        "task_id": TASK_ID,
        "tool_use_id": call,
        "status": status,
        "summary": summary,
        "output_file": format!("/tmp/claude-1000/tasks/{TASK_ID}.output"),
    })
}

/// The result of a call whose command runs in the background.
fn running_in_background() -> String {
    format!(
        "Command running in background with ID: {TASK_ID}. Output is being written to: \
         /tmp/claude-1000/tasks/{TASK_ID}.output. You will be notified when it completes. To \
         check interim output, use Read on that file path."
    )
}

/// What Claude Code writes, as 2.1.322 did, without the turn's other lines, for a command that
/// Claude runs in the background and that ends with `status`, `completed` or `failed`.
///
/// # Panics
///
/// On any other status.
#[must_use]
pub fn background_task(status: &str) -> Vec<String> {
    let (command, summary) = match status {
        "completed" => ("sleep 1", "completed (exit code 0)"),
        "failed" => ("sleep 1; exit 3", "failed with exit code 3"),
        _ => panic!("a background task ends as completed or failed, not as {status}"),
    };
    let description = format!("Run {command}");
    let call = "toolu_background";
    let lines = [
        tool_use(
            call,
            "Bash",
            &json!({"command": command, "description": description, "run_in_background": true}),
        ),
        background_tasks(&[&description]),
        task_started(call, &description, true),
        tool_result(call, &running_in_background()),
        task_notification(
            call,
            status,
            &format!("Background command \"{description}\" {summary}"),
        ),
        background_tasks(&[]),
    ];
    lines.iter().map(Value::to_string).collect()
}

/// What Claude Code writes, as 2.1.322 did, for a command that Claude runs in the background and
/// then stops with `TaskStop`, without the turns' other lines. The lines up to the call's result
/// come of the first message, and the rest of the second one.
#[must_use]
pub fn stopped_background_task() -> Vec<String> {
    let description = "Run sleep 30";
    let (call, stop) = ("toolu_background", "toolu_stop");
    let lines = [
        tool_use(
            call,
            "Bash",
            &json!({"command": "sleep 30", "description": description, "run_in_background": true}),
        ),
        background_tasks(&[description]),
        task_started(call, description, true),
        tool_result(call, &running_in_background()),
        tool_use(stop, "TaskStop", &json!({"task_id": TASK_ID})),
        // A stopped task's summary is its description
        task_notification(call, "stopped", description),
        background_tasks(&[]),
        tool_result(
            stop,
            &json!({
                "message": format!("Successfully stopped task: {TASK_ID} (sleep 30)"),
                "task_id": TASK_ID,
                "task_type": "local_bash",
                "command": "sleep 30",
            })
            .to_string(),
        ),
    ];
    lines.iter().map(Value::to_string).collect()
}

/// What Claude Code writes, as 2.1.322 did, for a command that a message sent with send now moves
/// to the background, without the turns' other lines. The call comes of the first message, and
/// the rest of send now.
#[must_use]
pub fn moved_call() -> Vec<String> {
    let description = "Run sleep 4";
    let call = "toolu_moved";
    let lines = [
        tool_use(
            call,
            "Bash",
            &json!({"command": "sleep 4", "description": description}),
        ),
        task_started(call, description, false),
        background_tasks(&[description]),
        json!({
            "type": "system",
            "subtype": "task_updated",
            "task_id": TASK_ID,
            "patch": {"is_backgrounded": true},
        }),
        tool_result(
            call,
            &format!(
                "Command was moved to the background (ID: {TASK_ID}) so that a message that \
                 arrived while it was running can reach you; it was not interrupted. Output is \
                 being written to: /tmp/claude-1000/tasks/{TASK_ID}.output"
            ),
        ),
        task_notification(
            call,
            "completed",
            &format!("Background command \"{description}\" completed (exit code 0)"),
        ),
        background_tasks(&[]),
    ];
    lines.iter().map(Value::to_string).collect()
}

/// What Claude Code writes, as 2.1.322 did, for a call that an interrupt stops, without the
/// turn's other lines. The call comes of the message, and its result of the interrupt.
#[must_use]
pub fn stopped_call() -> Vec<String> {
    let call = "toolu_stopped";
    let mut result = tool_result(
        call,
        "The user doesn't want to proceed with this tool use. The tool use was rejected (eg. if \
         it was a file edit, the new_string was NOT written to the file). STOP what you are doing \
         and wait for the user to tell you how to proceed.",
    );
    result["message"]["content"][0]["is_error"] = json!(true);
    let lines = [
        tool_use(
            call,
            "Bash",
            &json!({"command": "sleep 5", "description": "Run sleep 5"}),
        ),
        result,
    ];
    lines.iter().map(Value::to_string).collect()
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
/// Ctrl+C does a program's work in the terminal, and Claude Code then exits, since its input
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

/// What Claude Code writes as `act` drives it, and then until it exits, its input closed, when
/// it runs against [`crate::fake_api`], whose classifier answers after `classifier_delay`. It
/// runs in auto mode, with a config directory of its own, since the user's settings can set
/// the API's address and key.
fn on_fake(
    program: &Path,
    classifier_delay: Duration,
    act: impl FnOnce(&mut Run) -> Result<Vec<Value>, String>,
) -> Result<Vec<String>, String> {
    let api = FakeApi::start(classifier_delay)?;
    let config =
        tempfile::tempdir().map_err(|error| format!("cannot create a directory: {error}"))?;
    let config_path = config.path().to_string_lossy();
    let mut run = Run::start(
        program,
        &["--model", MODEL, "--permission-mode", "auto"],
        &[
            ("ANTHROPIC_BASE_URL", &api.url),
            ("ANTHROPIC_API_KEY", "claudio-check"),
            ("CLAUDE_CONFIG_DIR", &config_path),
        ],
        &["ANTHROPIC_AUTH_TOKEN"],
    )?;
    let lines = act(&mut run).and_then(|mut lines| {
        run.close_input();
        lines.extend(run.read_to_end(FREE_TIMEOUT)?);
        Ok(lines)
    });
    // A request that no rule covers explains a timeout too, as Claude Code then retries
    api.all_covered()?;
    Ok(texts(&lines?))
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
/// named by their order of appearance among those events, so the fixture's ids and Claude
/// Code's are equal when each repeats in the same places.
fn summary(kinds: &[&str], events: &[Event]) -> Vec<String> {
    let mut seen: Vec<&str> = Vec::new();
    events
        .iter()
        .filter_map(|event| {
            let (kind, ids, rest): (&str, Vec<&str>, String) = match event {
                Event::State(state) => ("state", vec![], format!("{state:?}")),
                Event::PayingKey(key) => ("paying", vec![], key.clone()),
                Event::Reset { previous_session } => {
                    ("reset", vec![previous_session], String::new())
                }
                Event::SessionId(session) => ("session", vec![session], String::new()),
                Event::TurnEnd { failure } => ("turn end", vec![], format!("{failure:?}")),
                Event::ToolUse(tool_use) => ("tool use", vec![&tool_use.id], String::new()),
                Event::ToolResult(result) => {
                    ("tool result", vec![&result.tool_use_id], String::new())
                }
                Event::PermissionCheck {
                    tool_use_id,
                    is_checking,
                } => {
                    let status = if *is_checking { "checking" } else { "done" };
                    ("permission check", vec![tool_use_id], status.to_string())
                }
                Event::BackgroundTasks(tasks) => (
                    "background",
                    tasks.iter().map(|task| task.id.as_str()).collect(),
                    String::new(),
                ),
                Event::TaskStarted {
                    task_id,
                    tool_use_id,
                    is_backgrounded,
                } => (
                    "task started",
                    [Some(task_id), tool_use_id.as_ref()]
                        .into_iter()
                        .flatten()
                        .map(String::as_str)
                        .collect(),
                    if *is_backgrounded { "backgrounded" } else { "" }.to_string(),
                ),
                Event::TaskBackgrounded { task_id } => {
                    ("task backgrounded", vec![task_id], String::new())
                }
                Event::TaskEnded {
                    task_id, status, ..
                } => ("task ended", vec![task_id], format!("{status:?}")),
                Event::Limits { session, week } => (
                    "limits",
                    vec![],
                    format!("session {} week {}", session.is_some(), week.is_some()),
                ),
                _ => return None,
            };
            Some((kind, ids, rest))
        })
        .filter(|(kind, _, _)| kinds.contains(kind))
        .map(|(kind, ids, rest)| {
            let ids: Vec<String> = ids
                .into_iter()
                .map(|id| {
                    let index = seen.iter().position(|old| *old == id).unwrap_or_else(|| {
                        seen.push(id);
                        seen.len() - 1
                    });
                    format!("id {}", index + 1)
                })
                .collect();
            [kind.to_string(), ids.join(" ")]
                .into_iter()
                .chain([rest])
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        })
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
            tempfile::tempdir().map_err(|error| format!("cannot create a directory: {error}"))?;
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
            .map_err(|error| format!("cannot start {}: {error}", program.display()))?;
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
            .map_err(|error| format!("cannot write to Claude Code: {error}"))
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
        self.send(&protocol::Command::Run { command: "clear" })?;
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
                "cannot send SIGINT: {}",
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
                Err(error) => return Err(format!("cannot wait for Claude Code: {error}")),
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
