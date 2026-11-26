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
//! A check is hermetic, live or live and paid, as [`Setup`] tells, by what it runs Claude Code
//! with.
//! Ref: <https://martinfowler.com/bliki/ContractTest.html>, on checking a test double against
//! the real service

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{Value, json};
use tempfile::TempDir;

use crate::claude_code;
use crate::fake_api::{self, FakeApi};
use crate::limits;
use crate::protocol::{self, Event, read_events};

/// A check, which fails with what Claude Code did instead.
pub struct Check {
    pub name: &'static str,
    pub setup: Setup,
}

/// How a check starts Claude Code, which trades hermeticity, a result that depends on nothing
/// but the check, for fidelity to a user's session. A check that needs no answer of Claude's is
/// live, which costs nothing. One that does is hermetic when the fake can give the answer, and
/// live and paid when only the API can. Each setup gives its check the one way it has to start
/// Claude Code, so a check runs as its setup says.
/// Ref: <https://abseil.io/resources/swe-book/html/ch14.html#the_system_under_test>, on
/// hermeticity and fidelity: "Often these two factors are in direct conflict"
/// Ref: <https://abseil.io/resources/swe-book/html/ch11.html#test_size>, on telling tests apart
/// by the resources they need, like the network beyond localhost
#[derive(Clone, Copy)]
pub enum Setup {
    /// Claude Code runs against the fake of `fake_api`, with a config directory of its own: the
    /// check is free, works offline and gives the same result on every run, while Claude Code
    /// cannot fetch its flags with the user's login, so each keeps its default.
    Hermetic(fn(&Hermetic) -> Result<(), String>),
    /// Claude Code runs with the user's login, so it sees the flags a session gets, and the
    /// check sends only what Claude Code answers itself, like `/clear`, so it is free.
    Live(fn(&Live) -> Result<(), String>),
    /// Claude Code runs with the user's login, and Claude answers with [`MODEL`], a few cents in
    /// all, so the check runs only with `--paid`.
    LivePaid(fn(&Live) -> Result<(), String>),
}

impl Setup {
    /// The setup's name, as `check-claude-code` shows it.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Setup::Hermetic(_) => "hermetic",
            Setup::Live(_) => "live",
            Setup::LivePaid(_) => "live paid",
        }
    }

    #[must_use]
    pub fn is_paid(self) -> bool {
        matches!(self, Setup::LivePaid(_))
    }

    /// Runs the check against `program`, which the setup starts.
    ///
    /// # Errors
    ///
    /// Fails with what Claude Code did instead of what the check expects.
    pub fn run(self, program: &Path) -> Result<(), String> {
        match self {
            Setup::Hermetic(check) => check(&Hermetic { program }),
            Setup::Live(check) => check(&Live {
                program,
                args: &[],
                timeout: FREE_TIMEOUT,
            }),
            Setup::LivePaid(check) => check(&Live {
                program,
                args: &["--model", MODEL],
                timeout: PAID_TIMEOUT,
            }),
        }
    }
}

/// The checks, by setup.
pub const CHECKS: [Check; 21] = [
    Check {
        name: "a slow permission check is told",
        setup: Setup::Hermetic(a_slow_permission_check_is_told),
    },
    Check {
        name: "a narration is listed",
        setup: Setup::Hermetic(a_narration_is_listed),
    },
    Check {
        name: "a background task is told from start to end",
        setup: Setup::Hermetic(a_background_task_is_told_from_start_to_end),
    },
    Check {
        name: "a call moved to the background is told",
        setup: Setup::Hermetic(a_call_moved_to_the_background_is_told),
    },
    Check {
        name: "a call moved by its id makes way for send now",
        setup: Setup::Hermetic(a_call_moved_by_its_id_makes_way_for_send_now),
    },
    Check {
        name: "a subagent is told from start to end",
        setup: Setup::Hermetic(a_subagent_is_told_from_start_to_end),
    },
    Check {
        name: "a message to a subagent resumes it",
        setup: Setup::Hermetic(a_message_to_a_subagent_resumes_it),
    },
    Check {
        name: "a stopped call gets its result",
        setup: Setup::Hermetic(a_stopped_call_gets_its_result),
    },
    Check {
        name: "a clear follows the turn on the last task's end",
        setup: Setup::Hermetic(a_clear_follows_the_turn_on_the_last_tasks_end),
    },
    Check {
        name: "a stop of a task that ended succeeds",
        setup: Setup::Hermetic(a_stop_of_a_task_that_ended_succeeds),
    },
    Check {
        name: "session state comes with its variable",
        setup: Setup::Live(session_state_comes_with_its_variable),
    },
    Check {
        name: "send now is announced",
        setup: Setup::Live(send_now_is_announced),
    },
    Check {
        name: "the paying key is named by its source",
        setup: Setup::Live(the_paying_key_is_named_by_its_source),
    },
    Check {
        name: "the start lists the commands",
        setup: Setup::Live(the_start_lists_the_commands),
    },
    Check {
        name: "the start tells every fact of the status",
        setup: Setup::Live(the_start_tells_every_fact_of_the_status),
    },
    Check {
        name: "a clear resets the session",
        setup: Setup::Live(a_clear_resets_the_session),
    },
    Check {
        name: "a rate limit event has every window",
        setup: Setup::LivePaid(a_rate_limit_event_has_every_window),
    },
    Check {
        name: "the usage is shared for a minute",
        setup: Setup::Live(the_usage_is_shared_for_a_minute),
    },
    Check {
        name: "a permission after the input closes fails",
        setup: Setup::LivePaid(a_permission_after_the_input_closes_fails),
    },
    Check {
        name: "SIGINT stops Claude Code after its input closes",
        setup: Setup::LivePaid(sigint_stops_claude_code_after_its_input_closes),
    },
    Check {
        name: "the output closes as Claude Code exits",
        setup: Setup::LivePaid(the_output_closes_as_claude_code_exits),
    },
];

/// The model of the paid checks, the cheapest.
pub const MODEL: &str = "haiku";

/// How long a free check waits for Claude Code, which answers at once.
const FREE_TIMEOUT: Duration = Duration::from_secs(60);

/// How long a paid check waits for Claude Code, which calls the API.
const PAID_TIMEOUT: Duration = Duration::from_secs(180);

/// With `CLAUDE_CODE_SDK_READS_SESSION_STATE` set, as the Agent SDK sets it for the Claude Code
/// it starts, Claude Code sends `session_state_changed` as a turn starts and ends, and without
/// it, never. The SDK's types type the message, but name no variable.
pub(crate) fn session_state_comes_with_its_variable(claude: &Live) -> Result<(), String> {
    let with = claude.start()?.clear_once()?;
    compare(&["state"], &with, &session_states_of_a_turn())?;
    let without = claude
        .start_with(&[], &[], &["CLAUDE_CODE_SDK_READS_SESSION_STATE"])?
        .clear_once()?;
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

/// From Claude Code's changelog and seen with Claude Code: the fields of a send-now interrupt are
/// Claude Code's own, absent from the SDK's types, and Claude Code announces them as
/// `interrupt_send_now_v1` in the capabilities of its init. One without them takes the request
/// as a plain interrupt.
/// Ref: <https://code.claude.com/docs/en/changelog>, on send now moving running tools to the
/// background
pub(crate) fn send_now_is_announced(claude: &Live) -> Result<(), String> {
    // A clear starts a session, whose init comes without a call
    let lines = claude.start()?.clear_once()?;
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

/// Seen with Claude Code: `ANTHROPIC_API_KEY` fills the account's
/// `apiKeySource` with its name, `ANTHROPIC_AUTH_TOKEN` fills `tokenSource` with its name, and
/// the subscription's login leaves `apiKeySource` out and `tokenSource` out or `claude.ai`. The
/// check needs a login to the subscription, and no settings file that sets either variable.
pub(crate) fn the_paying_key_is_named_by_its_source(claude: &Live) -> Result<(), String> {
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
        let mut run = claude.start_with(&[], &env, &removed)?;
        run.send(&protocol::Command::Initialize)?;
        run.close_input();
        let lines = run.read_to_end()?;
        let real = texts(&lines);
        compare(&["paying"], &real, &[initialize_response(&account)])
            .map_err(|error| format!("with {}: {error}", key.unwrap_or("the subscription")))?;
    }
    Ok(())
}

/// Seen with Claude Code: the answer to `initialize` lists the commands the session
/// runs, by name, as the init that comes with the first message does, before any message. The
/// SDK's `supportedCommands` reads them there.
/// Ref: <https://code.claude.com/docs/en/agent-sdk/slash-commands>, on the commands a session
/// runs
pub(crate) fn the_start_lists_the_commands(claude: &Live) -> Result<(), String> {
    let mut run = claude.start()?;
    run.send(&protocol::Command::Initialize)?;
    run.close_input();
    let lines = run.read_to_end()?;
    let lists = events(&texts(&lines)).into_iter().any(|event| {
        matches!(event, Event::Commands(commands) if commands.iter().any(|name| name == "compact"))
    });
    if lists {
        Ok(())
    } else {
        Err("the answer to initialize lists no compact among its commands".to_string())
    }
}

/// The answers to the requests claudio sends as the session starts tell every fact the status
/// shows of how the session runs and of its context, so none of them waits for the first message.
/// Seen with Claude Code, whose answer to `initialize` tells the permission mode as
/// `current_permission_mode`, which `sdk.d.ts` leaves out of `SDKControlInitializeResponse`.
pub(crate) fn the_start_tells_every_fact_of_the_status(claude: &Live) -> Result<(), String> {
    type IsTold = fn(&Event) -> bool;
    let facts: [(&str, IsTold); 8] = [
        ("model", |event| matches!(event, Event::Model(_))),
        ("effort", |event| matches!(event, Event::Effort(_))),
        ("fast mode", |event| matches!(event, Event::FastMode(_))),
        ("permission mode", |event| {
            matches!(event, Event::PermissionMode(_))
        }),
        ("plan or paying key", |event| {
            matches!(event, Event::Plan(_) | Event::PayingKey(_))
        }),
        ("version", |event| matches!(event, Event::Version(_))),
        ("context's tokens", |event| {
            matches!(event, Event::CountedTokens(_))
        }),
        ("context's ceiling", |event| {
            matches!(event, Event::Ceiling(_))
        }),
    ];
    let mut run = claude.start()?;
    for command in [
        protocol::Command::Initialize,
        protocol::Command::GetSettings,
        protocol::Command::GetContext,
    ] {
        run.send(&command)?;
    }
    run.close_input();
    let events = events(&texts(&run.read_to_end()?));
    let missing: Vec<&str> = (facts.into_iter())
        .filter(|(_, is_told)| !events.iter().any(is_told))
        .map(|(fact, _)| fact)
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!("the start tells no {}", missing.join(", no ")))
    }
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

/// Some of the commands Claude Code runs in a session of its own, which the fake lists.
const COMMANDS: [&str; 5] = ["clear", "compact", "context", "model", "usage"];

/// The answer to `initialize` for `account`, with the commands the session runs.
#[must_use]
pub fn initialize_response(account: &Value) -> String {
    let commands: Vec<Value> = (COMMANDS.iter())
        .map(|name| json!({"name": name, "description": "", "argumentHint": ""}))
        .collect();
    let line = json!({
        "type": "control_response",
        "response": {"subtype": "success", "request_id": protocol::INITIALIZE_REQUEST_ID, "response": {
            "commands": commands,
            "account": account,
            "current_permission_mode": "default",
            "fast_mode_state": "off",
            "claude_code_version": claude_code::VERSION,
        }},
    });
    line.to_string()
}

/// Seen with Claude Code: `/clear` sends `conversation_reset`, whose `session_id` is
/// the cleared session's, then the new session's init, then a result that ends a turn of no
/// calls.
pub(crate) fn a_clear_resets_the_session(claude: &Live) -> Result<(), String> {
    let mut run = claude.start()?;
    run.send(&protocol::Command::Run { command: "clear" })?;
    run.send(&protocol::Command::Run { command: "clear" })?;
    run.close_input();
    let lines = run.read_to_end()?;
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
/// interrupts it takes and the commands it runs.
#[must_use]
pub fn init(session: &str) -> String {
    let line = json!({
        "type": "system",
        "subtype": "init",
        "session_id": session,
        "claude_code_version": claude_code::VERSION,
        "capabilities": ["interrupt_send_now_v1"],
        "slash_commands": COMMANDS,
        "permissionMode": "default",
    });
    line.to_string()
}

/// Seen with Claude Code: in auto mode, once the classifier has not answered for 4
/// seconds whether a tool call needs the user's approval, Claude Code sends
/// `permission_check_status` with `checking`, then with `done` as the classifier answers, both
/// between the call and its result.
///
/// The check runs Claude Code against [`crate::fake_api`], whose classifier answers after 5
/// seconds. The call runs `sh -c true`, since Claude Code allows a read-only command without the
/// classifier.
pub(crate) fn a_slow_permission_check_is_told(fake: &Hermetic) -> Result<(), String> {
    let lines = fake.run_with_slow_classifier(|run| {
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

/// What Claude Code writes for a tool call whose permission check is slow,
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

/// Seen with Claude Code: a narration, text Claude wrote for the user between tool
/// calls, comes as a thinking block that holds the API's summary of the text, and the line that
/// carries it lists the block's index in `narration_block_indexes`. Claude Code's schema marks
/// the field `@internal`, and says it lists the thinking blocks whose signature the API marks as
/// a narration, "so a renderer can label them as summaries without decoding the signature
/// envelope". The fake signs its narration with that mark, which real sessions put on every
/// thinking block that holds text, and ends it in an empty line, as real summaries end.
pub(crate) fn a_narration_is_listed(fake: &Hermetic) -> Result<(), String> {
    let lines = fake.run(|run| {
        run.send_message(NARRATION_MESSAGE)?;
        Ok(Vec::new())
    })?;
    compare(&["narration", "text end"], &lines, &narration())
}

/// The message that the check of a narration sends, which `fake-claude` answers with
/// [`narration`].
pub const NARRATION_MESSAGE: &str = "Narrate ok";

/// What Claude Code writes for a narration and the text after it, without the
/// turn's other lines.
#[must_use]
pub fn narration() -> Vec<String> {
    let narration = json!({
        "type": "thinking",
        "thinking": "ok\n\n",
        "signature": fake_api::NARRATION_SIGNATURE,
    });
    let text = json!({"type": "text", "text": "done"});
    vec![
        json!({
            "type": "assistant",
            "message": {"content": [narration]},
            "parent_tool_use_id": null,
            "narration_block_indexes": [0],
        })
        .to_string(),
        json!({"type": "assistant", "message": {"content": [text]}, "parent_tool_use_id": null})
            .to_string(),
    ]
}

/// Seen with Claude Code: a command that Claude runs in the background gets a
/// `task_started` that links its task to the call and says it runs in the background, before the
/// call's result. As the task ends, `task_notification` tells how, as `completed`, `failed` or,
/// once `TaskStop` stopped it, `stopped`. `background_tasks_changed` lists the task while it
/// runs, as the session runs, the input open, while Claude is idle too.
///
/// The check runs Claude Code against [`crate::fake_api`], and waits for each task's end before
/// it closes the input, since Claude Code waits for its background tasks once the input closes.
pub(crate) fn a_background_task_is_told_from_start_to_end(fake: &Hermetic) -> Result<(), String> {
    for (message, status) in [
        (BACKGROUND_TASK_MESSAGE, "completed"),
        (FAILED_BACKGROUND_TASK_MESSAGE, "failed"),
    ] {
        let lines = fake.run(|run| {
            run.send_message(message)?;
            run.read_until(is_no_background_task)
        })?;
        compare(TASK_KINDS, &lines, &background_task(status))
            .map_err(|error| format!("a task that {status}: {error}"))?;
    }
    let lines = fake.run(|run| {
        run.send_message(LONG_BACKGROUND_TASK_MESSAGE)?;
        let mut lines = run.read_until(|line| line["subtype"] == "task_started")?;
        let task_id = lines
            .last()
            .map_or("", |line| line["task_id"].as_str().unwrap_or(""));
        run.send(&protocol::Command::Message {
            id: "claudio-check-stop",
            text: &stop_message(task_id),
        })?;
        lines.extend(run.read_until(is_no_background_task)?);
        lines.extend(run.read_until(is_tool_result)?);
        Ok(lines)
    })?;
    compare(TASK_KINDS, &lines, &stopped_background_task())
        .map_err(|error| format!("a task that TaskStop stopped: {error}"))
}

/// Seen with Claude Code: a message sent with send now, while a command runs in the
/// foreground, moves the command to the background. Its task started in the foreground, as for
/// any command that runs for a while, then a `task_updated` says it moved, before the call's
/// result. The task's end then comes as for any background task.
pub(crate) fn a_call_moved_to_the_background_is_told(fake: &Hermetic) -> Result<(), String> {
    let lines = fake.run(|run| {
        run.send_message(MOVED_CALL_MESSAGE)?;
        let mut lines = run.read_until(is_tool_use)?;
        run.send(&protocol::Command::Message {
            id: "claudio-check-move",
            text: REPLY_OK_MESSAGE,
        })?;
        run.send(&protocol::Command::SendNow {
            request_id: "claudio-check-send-now".to_string(),
            message_id: "claudio-check-move",
        })?;
        lines.extend(run.read_until(is_no_background_task)?);
        Ok(lines)
    })?;
    compare(TASK_KINDS, &lines, &moved_call())
}

/// Seen with Claude Code: a move to the background by a call's id, sent right before a
/// message with send now, ends before Claude Code reads the send now, which then finds nothing
/// that runs in the foreground, and spares the subagent the call started. Claude Code alone
/// stops the turn for a subagent that runs in the foreground, and the subagent with it. The
/// move leaves the subagent's own command in the foreground, where the subagent waits for it.
pub(crate) fn a_call_moved_by_its_id_makes_way_for_send_now(fake: &Hermetic) -> Result<(), String> {
    let lines = fake.run(|run| {
        run.send_message(MOVED_SUBAGENT_MESSAGE)?;
        let mut lines = run.read_until(|line| {
            line["subtype"] == "task_started" && line["task_type"] == "local_bash"
        })?;
        let agent_call = lines
            .iter()
            .find(|line| line["subtype"] == "task_started")
            .map_or("", |line| line["tool_use_id"].as_str().unwrap_or(""));
        run.send(&protocol::Command::Background {
            request_id: "claudio-check-background".to_string(),
            tool_use_id: agent_call,
        })?;
        run.send(&protocol::Command::Message {
            id: "claudio-check-move",
            text: REPLY_OK_MESSAGE,
        })?;
        run.send(&protocol::Command::SendNow {
            request_id: "claudio-check-send-now".to_string(),
            message_id: "claudio-check-move",
        })?;
        lines.extend(run.read_until(is_no_background_task)?);
        Ok(lines)
    })?;
    compare(TASK_KINDS, &lines, &moved_subagent())
}

/// The message of the check of a subagent moved by its call's id, whose subagent runs a command
/// for a while.
pub const MOVED_SUBAGENT_MESSAGE: &str = "Run sleep 4 with Bash in a subagent";

/// What Claude Code writes for a subagent that runs a command in the
/// foreground, moved to the background by its call's id, without the turns' other lines.
#[must_use]
pub fn moved_subagent() -> Vec<String> {
    let (call, command, handback) = ("toolu_agent", "toolu_command", "toolu_handback");
    let description = "Run sleep 4 with Bash";
    let input = json!({
        "description": description,
        "prompt": description,
        "subagent_type": "general-purpose",
        "run_in_background": false,
    });
    let mut command_use = tool_use(
        command,
        "Bash",
        &json!({"command": "sleep 4", "description": "Run sleep 4"}),
    );
    let mut command_result = tool_result(command, "");
    let mut handback_use = tool_use(handback, "SubagentHandback", &json!({"message": "ok"}));
    let mut handback_result = tool_result(
        handback,
        r#"{"success":true,"message":"Report delivered to your caller."}"#,
    );
    for line in [
        &mut command_use,
        &mut command_result,
        &mut handback_use,
        &mut handback_result,
    ] {
        line["parent_tool_use_id"] = json!(call);
        line["agent_id"] = json!(AGENT_TASK_ID);
    }
    let agent_tasks = |tasks: Vec<Value>| json!({"type": "system", "subtype": "background_tasks_changed", "tasks": tasks});
    let lines = [
        tool_use(call, "Agent", &input),
        json!({
            "type": "system",
            "subtype": "task_started",
            "task_id": AGENT_TASK_ID,
            "tool_use_id": call,
            "description": description,
            "subagent_type": "general-purpose",
            "is_backgrounded": false,
            "spawn_depth": 1,
            "task_type": "local_agent",
        }),
        command_use,
        task_started(command, "Run sleep 4", false),
        agent_tasks(vec![json!({
            "task_id": AGENT_TASK_ID,
            "task_type": "local_agent",
            "subagent_type": "general-purpose",
            "description": description,
        })]),
        json!({
            "type": "system",
            "subtype": "task_updated",
            "task_id": AGENT_TASK_ID,
            "patch": {"is_backgrounded": true},
        }),
        tool_result(
            call,
            &format!("Async agent launched successfully.\nagentId: {AGENT_TASK_ID}"),
        ),
        task_notification(
            command,
            "completed",
            "Background command \"Run sleep 4\" completed (exit code 0)",
        ),
        command_result,
        handback_use,
        handback_result,
        json!({
            "type": "system",
            "subtype": "task_notification",
            "task_id": AGENT_TASK_ID,
            "tool_use_id": call,
            "status": "completed",
            "summary": "This agent's report was delivered to you as a message from \"a259478d0d71f063d\" (its SubagentHandback call). Read it there; it is not repeated here.",
            "output_file": format!("/tmp/claude-1000/tasks/{AGENT_TASK_ID}.output"),
        }),
        agent_tasks(Vec::new()),
    ];
    lines.iter().map(Value::to_string).collect()
}

/// Seen with Claude Code: a subagent's task starts with its `Agent` call, as
/// `task_started`, and ends as `task_notification` tells, whether it runs in the foreground or
/// in the background. `background_tasks_changed` lists only one that runs in the background.
///
/// A subagent that Claude asks for in the foreground stays there in this check, while Claude
/// Code's interactive mode, seen in a terminal, sends every subagent to the background, by its
/// fork subagent feature. The check runs on the flags' defaults, since Claude Code fetches its
/// flags from Anthropic with the user's credentials, which the fake lacks, so a user's sessions
/// may differ. In one that sends every subagent to the background, claudio shows them all as
/// ones in the background, which it shows as well.
pub(crate) fn a_subagent_is_told_from_start_to_end(fake: &Hermetic) -> Result<(), String> {
    for (message, is_backgrounded) in [
        (SUBAGENT_MESSAGE, false),
        (BACKGROUND_SUBAGENT_MESSAGE, true),
    ] {
        let lines = fake.run(|run| {
            run.send_message(message)?;
            run.read_until(|line| line["subtype"] == "task_notification")
        })?;
        compare(
            &["task started", "task ended"],
            &lines,
            &subagent(is_backgrounded),
        )
        .map_err(|error| format!("{message}: {error}"))?;
    }
    Ok(())
}

/// The messages of the check of subagents, which `fake-claude` answers with their fixtures.
pub const SUBAGENT_MESSAGE: &str = "Reply ok in a subagent";
pub const BACKGROUND_SUBAGENT_MESSAGE: &str = "Reply ok in a background subagent";

/// The task id of the subagent of the fixtures.
const AGENT_TASK_ID: &str = "a259478d0d71f063d";

/// What Claude Code writes for a subagent that replies ok, in the background
/// when `is_backgrounded`, without the turn's other lines.
#[must_use]
pub fn subagent(is_backgrounded: bool) -> Vec<String> {
    let (call, handback) = ("toolu_agent", "toolu_handback");
    let input = json!({
        "description": "Reply ok",
        "prompt": "Reply ok",
        "subagent_type": "general-purpose",
        "run_in_background": is_backgrounded,
    });
    let mut handback_use = tool_use(handback, "SubagentHandback", &json!({"message": "ok"}));
    let mut handback_result = tool_result(
        handback,
        r#"{"success":true,"message":"Report delivered to your caller."}"#,
    );
    for line in [&mut handback_use, &mut handback_result] {
        line["parent_tool_use_id"] = json!(call);
        line["agent_id"] = json!(AGENT_TASK_ID);
    }
    let agent_tasks = |listed: bool| {
        let tasks = if listed {
            vec![json!({
                "task_id": AGENT_TASK_ID,
                "task_type": "local_agent",
                "subagent_type": "general-purpose",
                "description": "Reply ok",
            })]
        } else {
            Vec::new()
        };
        json!({"type": "system", "subtype": "background_tasks_changed", "tasks": tasks})
    };
    let started = json!({
        "type": "system",
        "subtype": "task_started",
        "task_id": AGENT_TASK_ID,
        "tool_use_id": call,
        "description": "Reply ok",
        "subagent_type": "general-purpose",
        "is_backgrounded": is_backgrounded,
        "spawn_depth": 1,
        "task_type": "local_agent",
    });
    let ended = json!({
        "type": "system",
        "subtype": "task_notification",
        "task_id": AGENT_TASK_ID,
        "tool_use_id": call,
        "status": "completed",
        "summary": "This agent's report was delivered to you as a message from \"a259478d0d71f063d\" (its SubagentHandback call). Read it there; it is not repeated here.",
        "output_file": format!("/tmp/claude-1000/tasks/{AGENT_TASK_ID}.output"),
    });
    let lines = if is_backgrounded {
        vec![
            tool_use(call, "Agent", &input),
            agent_tasks(true),
            started,
            tool_result(
                call,
                &format!("Async agent launched successfully.\nagentId: {AGENT_TASK_ID}"),
            ),
            handback_use,
            handback_result,
            ended,
            agent_tasks(false),
        ]
    } else {
        vec![
            tool_use(call, "Agent", &input),
            started,
            handback_use,
            handback_result,
            ended,
            tool_result(call, "ok"),
        ]
    };
    lines.iter().map(Value::to_string).collect()
}

/// Seen with Claude Code: a `SendMessage` whose `to` is the id of a subagent that
/// ended, which its task's id is, resumes it, as a task of the same id that the call starts,
/// in the background. `SendMessage` is absent from the SDK's types of tools, and only Claude
/// Code's result for an `Agent` call tells Claude to send to the id.
pub(crate) fn a_message_to_a_subagent_resumes_it(fake: &Hermetic) -> Result<(), String> {
    let lines = fake.run(|run| {
        run.send_message(SUBAGENT_MESSAGE)?;
        let mut lines = run.read_until(|line| line["type"] == "result")?;
        let agent = lines
            .iter()
            .find(|line| line["subtype"] == "task_started")
            .map_or("", |line| line["task_id"].as_str().unwrap_or(""));
        run.send(&protocol::Command::Message {
            id: "claudio-check-resume",
            text: &resume_message(agent),
        })?;
        lines.extend(run.read_until(|line| line["subtype"] == "task_notification")?);
        Ok(lines)
    })?;
    compare(&["task started", "task ended"], &lines, &resumed_subagent())
}

/// The message that has Claude resume the subagent `agent` with `SendMessage`.
#[must_use]
pub fn resume_message(agent: &str) -> String {
    format!("Resume {agent} with SendMessage")
}

/// What Claude Code writes for a subagent that replies ok, and that Claude
/// then resumes with `SendMessage`, without the turns' other lines. The lines of
/// [`subagent`] come of the first message, and the rest of the second one.
#[must_use]
pub fn resumed_subagent() -> Vec<String> {
    let (call, send, handback) = ("toolu_agent", "toolu_send", "toolu_handback_again");
    let mut handback_use = tool_use(handback, "SubagentHandback", &json!({"message": "ok"}));
    let mut handback_result = tool_result(
        handback,
        r#"{"success":true,"message":"Report delivered to your caller."}"#,
    );
    // The resumed subagent's calls name the call that first started it
    for line in [&mut handback_use, &mut handback_result] {
        line["parent_tool_use_id"] = json!(call);
        line["agent_id"] = json!(AGENT_TASK_ID);
    }
    let agent_tasks = |tasks: Vec<Value>| json!({"type": "system", "subtype": "background_tasks_changed", "tasks": tasks});
    let resumed = [
        tool_use(
            send,
            "SendMessage",
            &json!({"to": AGENT_TASK_ID, "summary": "Reply ok", "message": "Reply ok"}),
        ),
        agent_tasks(vec![json!({
            "task_id": AGENT_TASK_ID,
            "task_type": "local_agent",
            "subagent_type": "general-purpose",
            "description": "Reply ok",
        })]),
        json!({
            "type": "system",
            "subtype": "task_started",
            "task_id": AGENT_TASK_ID,
            "tool_use_id": send,
            "description": "Reply ok",
            "subagent_type": "general-purpose",
            "is_backgrounded": true,
            "spawn_depth": 1,
            "task_type": "local_agent",
        }),
        tool_result(
            send,
            &json!({
                "success": true,
                "message": "Resuming agent a259478",
                "resumedAgentId": AGENT_TASK_ID,
            })
            .to_string(),
        ),
        handback_use,
        handback_result,
        json!({
            "type": "system",
            "subtype": "task_notification",
            "task_id": AGENT_TASK_ID,
            "tool_use_id": send,
            "status": "completed",
            "summary": "This agent's report was delivered to you as a message from \"a259478d0d71f063d\" (its SubagentHandback call). Read it there; it is not repeated here.",
            "output_file": format!("/tmp/claude-1000/tasks/{AGENT_TASK_ID}.output"),
        }),
        agent_tasks(Vec::new()),
    ];
    (subagent(false).into_iter())
        .chain(resumed.iter().map(Value::to_string))
        .collect()
}

/// Seen with Claude Code: an interrupt fails the call that runs with a result of its
/// own, so every call gets its result, however its turn ends.
pub(crate) fn a_stopped_call_gets_its_result(fake: &Hermetic) -> Result<(), String> {
    let lines = fake.run(|run| {
        run.send_message(STOPPED_CALL_MESSAGE)?;
        let mut lines = run.read_until(is_tool_use)?;
        run.send(&protocol::Command::Interrupt {
            request_id: "claudio-check-interrupt".to_string(),
        })?;
        lines.extend(run.read_until(|line| line["type"] == "result")?);
        Ok(lines)
    })?;
    compare(&["tool use", "tool result"], &lines, &stopped_call())
}

/// Seen with Claude Code: as a task ends in the background, Claude Code tells its end
/// and starts a turn on it before it lists no task left, so a `/clear` sent once it lists none
/// runs after that turn, and the task's end stays in the session that started it. A subagent's
/// end was seen to come the same way.
pub(crate) fn a_clear_follows_the_turn_on_the_last_tasks_end(
    fake: &Hermetic,
) -> Result<(), String> {
    let lines = fake.run(|run| {
        run.send_message(BACKGROUND_TASK_MESSAGE)?;
        run.read_until(is_no_background_task)?;
        run.send(&protocol::Command::Run { command: "clear" })?;
        run.read_until(|line| line["type"] == "conversation_reset")
    })?;
    compare(&["turn end", "reset"], &lines, &clear_after_task())
}

/// What Claude Code writes for a `/clear` sent once the last task that ran in
/// the background ended: the end of the turn on the task's end, then the clear.
#[must_use]
pub fn clear_after_task() -> Vec<String> {
    let turn_end = json!({"type": "result", "subtype": "success"}).to_string();
    [turn_end]
        .into_iter()
        .chain(clear("first", "second"))
        .collect()
}

/// Seen with Claude Code: `stop_task` succeeds for a task that ended already, as for one
/// that runs, so a stop sent as the task ends never fails.
pub(crate) fn a_stop_of_a_task_that_ended_succeeds(fake: &Hermetic) -> Result<(), String> {
    let lines = fake.run(|run| {
        run.send_message(BACKGROUND_TASK_MESSAGE)?;
        let lines = run.read_until(is_no_background_task)?;
        let task_id = (lines.iter())
            .find(|line| line["subtype"] == "task_started")
            .map_or("", |line| line["task_id"].as_str().unwrap_or(""));
        run.send(&protocol::Command::StopTask {
            request_id: "claudio-check-stop".to_string(),
            task_id,
        })?;
        run.read_until(|line| {
            line["type"] == "control_response"
                && line["response"]["request_id"] == "claudio-check-stop"
        })
    })?;
    let reply = (lines.iter().rev())
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|line| line["response"]["request_id"] == "claudio-check-stop")
        .map(|line| line["response"].clone());
    match reply {
        Some(reply) if reply["subtype"] == "success" => Ok(()),
        reply => Err(format!("the stop got {reply:?}")),
    }
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

/// What Claude Code writes for a command that Claude runs in the background and that ends with
/// `status`, `completed` or `failed`, without the turn's other lines.
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

/// What Claude Code writes for a command that Claude runs in the background and
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

/// What Claude Code writes for a command that a message sent with send now moves
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

/// What Claude Code writes for a call that an interrupt stops, without the
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

/// Seen with Claude Code: only `unifiedWindows` holds every limit of the plan, where the
/// documented fields describe only the limit that applies now. The check needs a login to the
/// subscription, whose plan has the limits.
pub(crate) fn a_rate_limit_event_has_every_window(claude: &Live) -> Result<(), String> {
    let mut run = claude.start()?;
    run.send_message("Reply with the single word ok.")?;
    run.close_input();
    let lines = run.read_to_end()?;
    let mut real = summary(&["limits"], &events(&texts(&lines)));
    // A turn can tell the limits more than once, each time whole
    real.dedup();
    same(&real, &summary(&["limits"], &events(&[rate_limit_event()])))
}

/// Seen with Claude Code: it keeps its last fetch of the plan's usage, with when it
/// fetched it by this machine's clock, as `cachedUsageUtilization` in its config file, which all
/// its processes share, and saves a new fetch a few milliseconds after it answers `get_usage`. It
/// answers `get_usage` from that copy, without a new fetch, until the copy is a minute old.
/// `sdk.d.ts` says nothing of the copy. The check needs a login to the subscription, whose plan
/// has the limits.
///
/// The check tells a fetch by the copy's time changing in the file. After the second answer, it
/// reads the file once Claude Code exited, which takes Claude Code about a second, while a save
/// lands a few milliseconds after its answer. A Claude Code that exited before it saved would
/// hide a second fetch, and the check would pass.
///
/// Counting the fetches instead would take a fake of the usage endpoint, which Claude Code asks
/// only with a subscription's login, and only on Anthropic's own host: a proxy that Claude Code
/// connects through, a certificate for that host that Claude Code trusts, and a fake login that
/// Claude Code accepts, too much for one check.
pub(crate) fn the_usage_is_shared_for_a_minute(claude: &Live) -> Result<(), String> {
    let mut run = claude.start()?;
    run.send(&protocol::Command::Initialize)?;
    let config = limits::config_path().ok_or("Claude Code's config directory is unknown")?;
    ask_usage(&mut run)?;
    let deadline = Instant::now() + claude.timeout;
    let first = loop {
        if let Some(copy) = limits::read_copy(&config)
            && SystemTime::now()
                .duration_since(copy.fetched_at)
                .unwrap_or_default()
                < MINUTE
        {
            break copy;
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "Claude Code kept no copy of the usage under a minute old in {:?}",
                claude.timeout
            ));
        }
        thread::sleep(Duration::from_millis(10));
    };
    if first.session.is_none() || first.week.is_none() {
        return Err(
            "Claude Code's copy of the usage tells no limit over five hours or seven days"
                .to_string(),
        );
    }
    ask_usage(&mut run)?;
    run.close_input();
    run.read_to_end()?;
    let second = limits::read_copy(&config).ok_or("Claude Code's copy of the usage went away")?;
    // A new fetch replaces the first only once the first is a minute old
    if second.fetched_at != first.fetched_at && second.fetched_at < first.fetched_at + MINUTE {
        return Err(format!(
            "Claude Code fetched the usage again {:?} after the first fetch, where claudio \
             assumes a minute",
            (second.fetched_at.duration_since(first.fetched_at)).unwrap_or_default()
        ));
    }
    Ok(())
}

const MINUTE: Duration = Duration::from_mins(1);

/// Asks for the plan's usage, and waits for the answer.
fn ask_usage(run: &mut Run) -> Result<(), String> {
    run.send(&protocol::Command::GetUsage)?;
    run.read_until(|line| line["response"]["request_id"] == protocol::USAGE_REQUEST_ID)?;
    Ok(())
}

/// A rate limit event, as Claude Code sends it.
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

/// Seen with Claude Code: once its input closed, Claude Code fails each permission it
/// would ask for, with "Stream closed" in the tool's result.
pub(crate) fn a_permission_after_the_input_closes_fails(claude: &Live) -> Result<(), String> {
    let mut run = claude.start_with(&["--permission-mode", "default"], &[], &[])?;
    run.send_message(
        "Use the Bash tool to run this command, and nothing else: touch claudio-check",
    )?;
    run.close_input();
    let lines = run.read_to_end()?;
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

/// Seen with Claude Code: SIGINT stops Claude's turn and its background tasks, as
/// Ctrl+C does a program's work in the terminal, and Claude Code then exits, since its input
/// closed.
pub(crate) fn sigint_stops_claude_code_after_its_input_closes(claude: &Live) -> Result<(), String> {
    let mut run = claude.start_with(&["--allowedTools", "Bash(sleep:*)"], &[], &[])?;
    run.send_message(
        "Run the Bash command `sleep 60` with run_in_background set to true, then reply with the single word started.",
    )?;
    run.close_input();
    run.read_until(|line| {
        line["subtype"] == "background_tasks_changed"
            && line["tasks"]
                .as_array()
                .is_some_and(|tasks| !tasks.is_empty())
    })?;
    run.interrupt()?;
    run.timeout = Duration::from_secs(15);
    run.wait().map_err(|error| format!("after SIGINT, {error}"))
}

/// Seen with Claude Code: the processes Claude Code starts write to files of their own,
/// so its output closes as it exits, even while a process it started still runs.
pub(crate) fn the_output_closes_as_claude_code_exits(claude: &Live) -> Result<(), String> {
    let mut run = claude.start_with(&["--allowedTools", "Bash(sleep:*)"], &[], &[])?;
    run.send_message(
        "Run this exact Bash command, without run_in_background: sleep 60 & Then reply with the single word started.",
    )?;
    run.close_input();
    run.wait()?;
    // The output holds what Claude Code wrote before it exited, which a reader takes at once
    run.timeout = Duration::from_secs(2);
    let lines = run
        .read_to_end()
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

/// What a hermetic check starts Claude Code with.
pub struct Hermetic<'a> {
    program: &'a Path,
}

impl Hermetic<'_> {
    /// What Claude Code writes as `act` drives it, with the fake's classifier answering at once.
    fn run(
        &self,
        act: impl FnOnce(&mut Run) -> Result<Vec<Value>, String>,
    ) -> Result<Vec<String>, String> {
        self.run_with_classifier(Duration::ZERO, act)
    }

    /// What Claude Code writes as `act` drives it, with the fake's classifier answering after 5
    /// seconds.
    fn run_with_slow_classifier(
        &self,
        act: impl FnOnce(&mut Run) -> Result<Vec<Value>, String>,
    ) -> Result<Vec<String>, String> {
        self.run_with_classifier(Duration::from_secs(5), act)
    }

    /// What Claude Code writes as `act` drives it, and then until it exits, its input closed,
    /// when it runs against [`crate::fake_api`], whose classifier answers after
    /// `classifier_delay`. It runs in auto mode, with a config directory of its own, since the
    /// user's settings can set the API's address and key.
    fn run_with_classifier(
        &self,
        classifier_delay: Duration,
        act: impl FnOnce(&mut Run) -> Result<Vec<Value>, String>,
    ) -> Result<Vec<String>, String> {
        let api = FakeApi::start(classifier_delay)?;
        let config =
            tempfile::tempdir().map_err(|error| format!("cannot create a directory: {error}"))?;
        let config_path = config.path().to_string_lossy();
        let mut run = Run::start(
            self.program,
            &["--model", MODEL, "--permission-mode", "auto"],
            &[
                ("ANTHROPIC_BASE_URL", &api.url),
                ("ANTHROPIC_API_KEY", "claudio-check"),
                ("CLAUDE_CONFIG_DIR", &config_path),
            ],
            &["ANTHROPIC_AUTH_TOKEN"],
            FREE_TIMEOUT,
        )?;
        let lines = act(&mut run).and_then(|mut lines| {
            run.close_input();
            lines.extend(run.read_to_end()?);
            Ok(lines)
        });
        // A request that no rule covers explains a timeout too, as Claude Code then retries
        api.all_covered()?;
        Ok(texts(&lines?))
    }
}

/// What a live check starts Claude Code with: the user's login, and the setup's arguments.
pub struct Live<'a> {
    program: &'a Path,
    args: &'static [&'static str],
    timeout: Duration,
}

impl Live<'_> {
    /// Starts Claude Code with the setup's arguments.
    fn start(&self) -> Result<Run, String> {
        self.start_with(&[], &[], &[])
    }

    /// Starts Claude Code with the setup's arguments, then `args`, `env` set and `removed` unset.
    fn start_with(
        &self,
        args: &[&str],
        env: &[(&str, &str)],
        removed: &[&str],
    ) -> Result<Run, String> {
        let args: Vec<&str> = self.args.iter().chain(args).copied().collect();
        Run::start(self.program, &args, env, removed, self.timeout)
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
/// named by their order of appearance among those events, so the fixture's ids and Claude
/// Code's are equal when each repeats in the same places.
fn summary(kinds: &[&str], events: &[Event]) -> Vec<String> {
    let mut seen: Vec<&str> = Vec::new();
    events
        .iter()
        .filter_map(|event| {
            let (kind, ids, rest): (&str, Vec<&str>, String) = match event {
                Event::State(state) => ("state", vec![], format!("{state:?}")),
                Event::TextEnd(text) => ("text end", vec![], text.clone()),
                Event::Narration(text) => ("narration", vec![], text.clone()),
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
                    kind,
                    place,
                    description: _,
                } => (
                    "task started",
                    [Some(task_id), tool_use_id.as_ref()]
                        .into_iter()
                        .flatten()
                        .map(String::as_str)
                        .collect(),
                    format!("{kind:?} {place:?}"),
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
    /// How long a read or a wait waits for Claude Code.
    timeout: Duration,
    _directory: TempDir,
}

impl Run {
    /// Starts `program` with `args` after claudio's own, `env` set and `removed` unset, to wait
    /// for it up to `timeout`.
    fn start(
        program: &Path,
        args: &[&str],
        env: &[(&str, &str)],
        removed: &[&str],
        timeout: Duration,
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
            timeout,
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
        let lines = self.read_to_end()?;
        Ok(texts(&lines))
    }

    /// The lines Claude Code writes, up to the first one that `is_last` takes, within the timeout.
    fn read_until(&mut self, is_last: impl Fn(&Value) -> bool) -> Result<Vec<Value>, String> {
        let timeout = self.timeout;
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

    /// The lines Claude Code writes until its output closes, within the timeout.
    fn read_to_end(&mut self) -> Result<Vec<Value>, String> {
        let timeout = self.timeout;
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

    /// Waits for Claude Code to exit, within the timeout.
    fn wait(&mut self) -> Result<(), String> {
        let timeout = self.timeout;
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
