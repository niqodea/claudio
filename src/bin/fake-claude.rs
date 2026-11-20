//! A stand-in for Claude Code, which claudio runs as `CLAUDIO_CLAUDE=target/debug/fake-claude
//! claudio` to show its screen without calls to the API. It writes what claudio assumes Claude
//! Code writes, from the fixtures of `claudio::contract`, so it passes the checks that make no
//! call, as `tests/fake_claude.rs` checks.
//!
//! It answers each message with a described tool call of several lines, a call shown by its
//! input, and their results, then two text blocks in a row, and the message `ask` with two
//! questions of `AskUserQuestion`'s, whose turn ends once they are answered: one of one choice
//! whose options have previews, and one of several choices. The message `ask one` asks only the
//! first. The message `ask guarded` asks to allow a call that Claude Code deems dangerous. The
//! message `ask all` sends one request of each kind at once, to see them side by side: the two
//! questions, a plan, and calls to allow of Bash, Edit, Write, `WebFetch` and an MCP tool. A turn
//! that asks ends once all its requests are answered. The messages of the checks of a slow
//! permission check and of tasks get those checks' lines, and an interrupt, send now's too, gets
//! the rest of a call that waits for one.
//! `/clear` starts a new session, and `/compact` and `/cost` answer as Claude Code does.
//! `FAKE_DELAY` is how many seconds each answer waits before it starts, none when unset.
//! The message `Write a long line` gets a line of 200 words, streamed in two parts: when
//! `FAKE_GATE` names a pipe, the second part waits until someone writes to it, so a test can
//! act while the line is unfinished.
//! The message `Write tables` gets a table that fits as a grid, then one too wide for it.

use std::env;
use std::fs;
use std::io::{self, BufRead};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use claudio::contract;
use serde_json::{Value, json};

const BLOCKS: [&str; 2] = [
    "The first block of text.",
    "The second block, right after the first.",
];

const LONG_LINE_MESSAGE: &str = "Write a long line";

const TABLES_MESSAGE: &str = "Write tables";
const TABLES: &str = "A narrow table:\n\n\
    | Fruit | Color |\n|---|---|\n| Apple | red |\n| Banana | yellow |\n\n\
    A wide one:\n\n\
    | Option | Speed | Notes |\n|---|:-:|---|\n\
    | Redis | fast | Keeps everything in memory, so it is fast, and needs a server of its own to run beside the program |\n\
    | SQLite | | A single file next to the program, with no server to run at all |";
const LONG_LINE_END: &str = " and that is the end of the long line.";

const ASK_REQUEST_ID: &str = "fake-ask";

fn main() {
    let mut fake = Fake {
        session: 1,
        has_started: false,
        sends_states: env::var_os("CLAUDE_CODE_SDK_READS_SESSION_STATE").is_some(),
        on_interrupt: Vec::new(),
        on_background: Vec::new(),
        on_stop: Vec::new(),
        asks_waiting: 0,
    };
    for line in io::stdin().lock().lines().map_while(Result::ok) {
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        fake.answer(&message);
    }
}

struct Fake {
    /// The number of the session, which a clear moves to the next.
    session: u32,
    /// Whether the session sent its init, which comes with the first message.
    has_started: bool,
    /// Whether the session's state goes out, as `CLAUDE_CODE_SDK_READS_SESSION_STATE` asks.
    sends_states: bool,
    /// The lines of a turn that waits for an interrupt, which ends it.
    on_interrupt: Vec<String>,
    /// The lines of a turn that waits for a move of its call to the background, which ends it.
    on_background: Vec<String>,
    /// The lines of the end of a task that runs in the background until it is stopped.
    on_stop: Vec<String>,
    /// How many requests of the turn that asks wait for their answers.
    asks_waiting: usize,
}

impl Fake {
    /// Answers a control request, and returns whether the request ended the turn that waited
    /// for it.
    fn answer_request(&mut self, message: &Value) -> bool {
        match message["request"]["subtype"].as_str() {
            Some("initialize") => send(&contract::initialize_response(&account())),
            Some("get_settings") => respond(message, &settings()),
            Some("get_context_usage") => respond(message, &context_usage()),
            Some("mcp_status") => respond(message, &mcp_status()),
            Some("get_usage") => respond(message, &plan_usage()),
            Some("interrupt") if !self.on_interrupt.is_empty() => {
                for line in std::mem::take(&mut self.on_interrupt) {
                    send(&line);
                }
                self.end_turn();
                return true;
            }
            // A task that ended, or that never ran, stops all the same
            Some("stop_task") => {
                for line in std::mem::take(&mut self.on_stop) {
                    send(&line);
                }
                respond(message, &json!({}));
            }
            Some("background_tasks") if !self.on_background.is_empty() => {
                let lines = std::mem::take(&mut self.on_background);
                self.send_task(&lines, 3);
                return true;
            }
            _ => {}
        }
        false
    }

    fn answer(&mut self, message: &Value) {
        if self.answer_request(message) {
            return;
        }
        // The last answer to a turn's requests ends the turn
        let answered = message["response"]["request_id"].as_str();
        if answered.is_some_and(|id| id.starts_with(ASK_REQUEST_ID)) {
            self.asks_waiting -= 1;
            if self.asks_waiting == 0 {
                send(&json!({"type": "result", "subtype": "success"}).to_string());
                self.state("idle");
            }
        }
        if message["type"] != "user" {
            return;
        }
        let text = message["message"]["content"].as_str().unwrap_or_default();
        self.state("running");
        if text == "/clear" && message["client_composed"] != true {
            let cleared = self.session_id();
            self.session += 1;
            self.has_started = true;
            for line in contract::clear(&cleared, &self.session_id()) {
                send(&line);
            }
            self.state("idle");
            return;
        }
        if !self.has_started {
            self.has_started = true;
            send(&contract::init(&self.session_id()));
        }
        let asks = match text.trim() {
            "ask" => vec![ask(2)],
            "ask one" => vec![ask(1)],
            "ask guarded" => vec![ask_guarded()],
            "ask all" => ask_all(),
            _ => Vec::new(),
        };
        if !asks.is_empty() {
            return self.send_asks(asks);
        }
        if let Some((lines, turn_lines)) = subagent(text.trim()) {
            return self.send_task(&lines, turn_lines);
        }
        match text.trim() {
            contract::SLOW_PERMISSION_CHECK_MESSAGE => {
                return self.send_turn(&contract::slow_permission_check());
            }
            contract::BACKGROUND_TASK_MESSAGE => {
                return self.send_task(&contract::background_task("completed"), 4);
            }
            contract::FAILED_BACKGROUND_TASK_MESSAGE => {
                return self.send_task(&contract::background_task("failed"), 4);
            }
            contract::LONG_BACKGROUND_TASK_MESSAGE => {
                let lines = contract::stopped_background_task();
                // The task's end, without the call of TaskStop around it
                self.on_stop = lines[5..7].to_vec();
                return self.send_turn(&lines[..4]);
            }
            contract::REPLY_OK_MESSAGE => return self.send_turn(&[text_line("ok")]),
            LONG_LINE_MESSAGE => return self.send_long_line(),
            TABLES_MESSAGE => return self.send_turn(&[text_line(TABLES)]),
            contract::MOVED_CALL_MESSAGE => {
                return self.wait_for_interrupt(&contract::moved_call());
            }
            contract::MOVED_SUBAGENT_MESSAGE => {
                return self.wait_for_background(&contract::moved_subagent());
            }
            contract::STOPPED_CALL_MESSAGE => {
                return self.wait_for_interrupt(&contract::stopped_call());
            }
            text if text.starts_with("Stop ") && text.ends_with(" with TaskStop") => {
                self.on_stop.clear();
                return self.send_turn(&contract::stopped_background_task()[4..]);
            }
            _ => {}
        }
        if message["client_composed"] != true
            && let Some(lines) = command(text, &self.session_id())
        {
            for line in lines {
                send(&line.to_string());
            }
            return self.state("idle");
        }
        let delay = env::var("FAKE_DELAY")
            .ok()
            .and_then(|delay| delay.parse().ok())
            .unwrap_or(0.0);
        thread::sleep(Duration::from_secs_f64(delay));
        for line in turn() {
            send(&line.to_string());
        }
        self.state("idle");
    }

    /// Sends the requests of a turn that asks, which ends once all are answered, each after its
    /// call, as Claude Code 2.1.322 shows a call before it asks about it.
    fn send_asks(&mut self, asks: Vec<Value>) {
        self.asks_waiting = asks.len();
        for (number, request) in asks.into_iter().enumerate() {
            let call = json!({
                "type": "assistant",
                "message": {"content": [{
                    "type": "tool_use",
                    "id": request["tool_use_id"],
                    "name": request["tool_name"],
                    "input": request["input"],
                }]},
                "parent_tool_use_id": null,
            });
            send(&call.to_string());
            let control = json!({
                "type": "control_request",
                "request_id": format!("{ASK_REQUEST_ID}-{number}"),
                "request": request,
            });
            send(&control.to_string());
        }
    }

    /// Sends `lines` as a turn, and ends it.
    fn send_turn(&self, lines: &[String]) {
        for line in lines {
            send(line);
        }
        self.end_turn();
    }

    /// Sends the long line's start, waits for `FAKE_GATE`, and sends its end, as a turn.
    fn send_long_line(&self) {
        let start = (1..=200)
            .map(|n| format!("w{n:03}"))
            .collect::<Vec<_>>()
            .join(" ");
        send(&text_delta(0, &start).to_string());
        if let Some(gate) = env::var_os("FAKE_GATE") {
            // Returns once the writer closes the pipe
            fs::read(gate).expect("the gate is a pipe to read");
        }
        send(&text_delta(0, LONG_LINE_END).to_string());
        self.send_turn(&[text_line(&format!("{start}{LONG_LINE_END}"))]);
    }

    /// Sends the lines of a background task: its first `turn_lines`, up to the call's result, as
    /// a turn, and the task's end after it, at once, which Claude then answers in a turn of its
    /// own, as [`contract::clear_after_task`] tells.
    fn send_task(&self, lines: &[String], turn_lines: usize) {
        let (turn, end) = lines.split_at(turn_lines);
        self.send_turn(turn);
        if end.is_empty() {
            return;
        }
        for line in end {
            send(line);
        }
        self.send_turn(&[text_line("ok")]);
    }

    /// Sends the call that starts `lines`, and keeps the rest for the interrupt that ends it.
    fn wait_for_interrupt(&mut self, lines: &[String]) {
        let (call, rest) = lines.split_first().expect("a call");
        send(call);
        self.on_interrupt = rest.to_vec();
    }

    /// Sends the subagent's call and start, and its command's, which start `lines`, and keeps
    /// the rest for the move to the background that ends the turn.
    fn wait_for_background(&mut self, lines: &[String]) {
        let (start, rest) = lines.split_at(4);
        for line in start {
            send(line);
        }
        self.on_background = rest.to_vec();
    }

    fn end_turn(&self) {
        send(&json!({"type": "result", "subtype": "success"}).to_string());
        self.state("idle");
    }

    fn session_id(&self) -> String {
        format!("fake-session-{}", self.session)
    }

    fn state(&self, state: &str) {
        if self.sends_states {
            send(&contract::session_state(state));
        }
    }
}

fn send(line: &str) {
    println!("{line}");
}

/// A part of Claude's reply as it streams, in the block `index`.
fn text_delta(index: usize, text: &str) -> Value {
    json!({
        "type": "stream_event",
        "event": {
            "type": "content_block_delta",
            "index": index,
            "delta": {"type": "text_delta", "text": text},
        },
        "parent_tool_use_id": null,
    })
}

/// A reply of Claude's that is `text` alone.
fn text_line(text: &str) -> String {
    json!({
        "type": "assistant",
        "message": {"content": [{"type": "text", "text": text}]},
        "parent_tool_use_id": null,
    })
    .to_string()
}

/// What Claude Code writes for the command `text` in the session `session`, as 2.1.322 does,
/// none for a text that is no command the fake knows. `/compact` reports its progress, starts
/// the session again, marks the compaction, hands Claude the summary, and replays its output
/// from Claude Code's history, with no answer of its own. `/cost` answers with a message that
/// is never streamed, as any command that answers does.
fn command(text: &str, session: &str) -> Option<Vec<Value>> {
    let lines = match text {
        "/compact" => vec![
            json!({"type": "system", "subtype": "status", "status": "compacting"}),
            json!({"type": "system", "subtype": "status", "status": null, "compact_result": "success"}),
            serde_json::from_str(&contract::init(session)).expect("an init of JSON"),
            json!({
                "type": "system",
                "subtype": "compact_boundary",
                "compact_metadata": {"trigger": "manual", "pre_tokens": 19_812, "post_tokens": 6_578},
            }),
            json!({
                "type": "user",
                "message": {"role": "user", "content": "This session is being continued from a previous conversation that ran out of context."},
                "parent_tool_use_id": null,
                "isSynthetic": true,
            }),
            json!({
                "type": "user",
                "message": {"role": "user", "content": "<local-command-stdout>Compacted </local-command-stdout>"},
                "parent_tool_use_id": null,
                "isReplay": true,
            }),
        ],
        "/cost" => vec![json!({
            "type": "assistant",
            "message": {
                "model": "<synthetic>",
                "content": [{
                    "type": "text",
                    "text": "You are currently using your subscription to power your Claude Code usage\n\nCurrent session: 26% used · resets 6pm",
                }],
            },
            "parent_tool_use_id": null,
        })],
        _ => return None,
    };
    Some(
        lines
            .into_iter()
            .chain([json!({"type": "result", "subtype": "success"})])
            .collect(),
    )
}

/// Answers the control request `message` with `response`.
fn respond(message: &Value, response: &Value) {
    let line = json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": message["request_id"],
            "response": response,
        },
    });
    send(&line.to_string());
}

/// The account that pays, from the variables that name a key, as Claude Code tells it.
fn account() -> Value {
    let is_set = |name| env::var_os(name).is_some_and(|value| !value.is_empty());
    if is_set("ANTHROPIC_API_KEY") {
        contract::api_key_account()
    } else if is_set("ANTHROPIC_AUTH_TOKEN") {
        contract::auth_token_account()
    } else {
        contract::subscription_account()
    }
}

/// The settings, which every file passed, with the effort that applies.
fn settings() -> Value {
    json!({"applied": {"effort": "high"}})
}

/// The usage of a new session's context, which has had no call, and Claude Code's own count of
/// its tokens. It also tells the model, the window and where Claude Code compacts.
fn context_usage() -> Value {
    json!({
        "totalTokens": 21_513,
        "model": "claude-opus-5-5",
        "maxTokens": 1_000_000,
        "isAutoCompactEnabled": true,
        "autoCompactThreshold": 967_000,
        "apiUsage": null,
    })
}

/// The MCP servers, one connected and one that failed.
fn mcp_status() -> Value {
    json!({"mcpServers": [
        {"name": "docs", "status": "connected"},
        {"name": "github", "status": "failed", "error": "connection refused"},
    ]})
}

/// The plan's usage, with limits that reset in two hours and in four days.
fn plan_usage() -> Value {
    let at = |seconds: u64| {
        let at = jiff::Timestamp::try_from(now() + Duration::from_secs(seconds))
            .expect("a time after 1970");
        at.strftime("%Y-%m-%dT%H:%M:%S+00:00").to_string()
    };
    json!({
        "rate_limits": {
            "five_hour": {"utilization": 62, "resets_at": at(2 * 3600)},
            "seven_day": {"utilization": 81, "resets_at": at(4 * 86_400)},
        },
    })
}

fn now() -> SystemTime {
    SystemTime::now()
}

/// The first `count` questions of the message `ask`.
fn ask(count: usize) -> Value {
    let option =
        |label: &str, description: &str| json!({"label": label, "description": description});
    let with_preview = |label: &str, description: &str, preview: &str| json!({"label": label, "description": description, "preview": preview});
    let questions = [
        json!({
            "question": "Which greeting function style do you prefer?",
            "header": "Code style",
            "options": [
                with_preview(
                    "Arrow function",
                    "Short one-liner.",
                    "```js\nconst greet = (name) => `Hello, ${name}!`;\n```",
                ),
                with_preview(
                    "Classic function",
                    "Named function declaration.",
                    "```js\nfunction greet(name) {\n  return `Hello, ${name}!`;\n}\n```",
                ),
            ],
        }),
        json!({
            "question": "Which checks should CI run?",
            "header": "Checks",
            "multiSelect": true,
            "options": [option("Lint", ""), option("Tests", ""), option("Types", "")],
        }),
    ];
    json!({
        "subtype": "can_use_tool",
        "tool_name": "AskUserQuestion",
        "tool_use_id": "fake-ask-call",
        "input": {"questions": &questions[..count]},
    })
}

/// The permission of the message `ask guarded`: a call that Claude Code deems dangerous, so
/// that one key must not allow it.
fn ask_guarded() -> Value {
    json!({
        "subtype": "can_use_tool",
        "tool_name": "Bash",
        "display_name": "Bash",
        "tool_use_id": "fake-guarded-call",
        "input": {"command": "rm -rf build", "description": "Delete the build directory"},
        "decision_reason": "rm removes files",
        "default_to_no": true,
    })
}

/// The requests of the message `ask all`: one of each kind, as Claude Code sends them.
fn ask_all() -> Vec<Value> {
    // Claude Code mostly sends a `decision_reason_type` and no `decision_reason`
    let call = |id: &str, tool: &str, input: Value, reason_type: &str| {
        json!({
            "subtype": "can_use_tool",
            "tool_name": tool,
            "display_name": tool,
            "tool_use_id": id,
            "input": input,
            "decision_reason_type": reason_type,
        })
    };
    let plan = "# Plan: add a greeting\n\n\
        ## Steps\n\n\
        1. Add `greet` to `src/greet.js`, as an arrow function that takes a name and returns the \
        greeting.\n\
        2. Export it from `src/index.js`.\n\
        3. Add a test in `test/greet.test.js` that greets `World`.\n\n\
        ## Checks\n\n\
        - `npm test` passes.\n\
        - `npm run lint` passes.";
    let mcp = json!({
        "subtype": "can_use_tool",
        "tool_name": "mcp__claude_ai_Google_Drive__search_files",
        "display_name": "Search Files",
        "mcp_server": {"name": "claude.ai Google Drive", "source": "claudeai"},
        "tool_use_id": "fake-mcp-call",
        "input": {"query": "greeting style guide", "page_size": 10},
        "decision_reason_type": "rule",
    });
    vec![
        ask(2),
        json!({
            "subtype": "can_use_tool",
            "tool_name": "ExitPlanMode",
            "tool_use_id": "fake-plan-call",
            "input": {"plan": plan},
        }),
        call(
            "fake-bash-call",
            "Bash",
            json!({"command": "npm test -- --watch=false", "description": "Run the tests", "timeout": 120_000}),
            "subcommandResults",
        ),
        call(
            "fake-edit-call",
            "Edit",
            json!({
                "file_path": "/tmp/project/src/index.js",
                "old_string": "export { main };",
                "new_string": "import { greet } from './greet.js';\n\nexport { main, greet };",
            }),
            "mode",
        ),
        call(
            "fake-write-call",
            "Write",
            json!({
                "file_path": "/tmp/project/src/greet.js",
                "content": "export const greet = (name) => `Hello, ${name}!`;\n",
            }),
            "mode",
        ),
        call(
            "fake-fetch-call",
            "WebFetch",
            json!({
                "url": "https://developer.mozilla.org/docs/Web/JavaScript/Reference/Functions/Arrow_functions",
                "prompt": "When should a function be an arrow function?",
            }),
            "rule",
        ),
        mcp,
    ]
}

/// The lines of a turn: a call to the API that writes the whole context to the cache, the plan's
/// limits, a tool call and its result, then two text blocks, and the turn's end.
fn turn() -> Vec<Value> {
    let since_epoch = now()
        .duration_since(UNIX_EPOCH)
        .expect("a time after 1970")
        .as_secs();
    let mut lines = vec![
        json!({
            "type": "stream_event",
            "event": {"type": "message_start", "message": {"usage": {
                "input_tokens": 2,
                "cache_creation_input_tokens": 41_201,
                "cache_creation": {"ephemeral_1h_input_tokens": 41_201},
            }}},
            "parent_tool_use_id": null,
        }),
        json!({
            "type": "rate_limit_event",
            "rate_limit_info": {"status": "allowed", "unifiedWindows": {
                "five_hour": {"utilization": 0.62, "resetsAt": since_epoch + 2 * 3600},
                "seven_day": {"utilization": 0.81, "resetsAt": since_epoch + 4 * 86_400},
            }},
        }),
        json!({
            "type": "assistant",
            "message": {"content": [{
                "type": "tool_use",
                "id": "fake-call",
                "name": "Bash",
                "input": {"command": "cd a\nmake", "description": "Build a"},
            }]},
            "parent_tool_use_id": null,
        }),
        json!({
            "type": "user",
            "message": {"content": [{
                "type": "tool_result",
                "tool_use_id": "fake-call",
                "content": "compiled a.c\nlinked a",
                "is_error": false,
            }]},
            "parent_tool_use_id": null,
        }),
        json!({
            "type": "assistant",
            "message": {"content": [{
                "type": "tool_use",
                "id": "fake-read",
                "name": "Read",
                "input": {"file_path": "a/Makefile"},
            }]},
            "parent_tool_use_id": null,
        }),
        json!({
            "type": "user",
            "message": {"content": [{
                "type": "tool_result",
                "tool_use_id": "fake-read",
                "content": "a: a.c",
                "is_error": false,
            }]},
            "parent_tool_use_id": null,
        }),
    ];
    for (index, text) in BLOCKS.into_iter().enumerate() {
        lines.push(text_delta(index, text));
        lines.push(json!({
            "type": "assistant",
            "message": {"content": [{"type": "text", "text": text}]},
            "parent_tool_use_id": null,
        }));
    }
    lines.push(json!({"type": "result", "subtype": "success"}));
    lines
}

/// The lines of the subagent that `text` asks for, or asks to resume, as a check of subagents
/// sends it, and how many of them make the turn, before the end of a subagent in the
/// background.
fn subagent(text: &str) -> Option<(Vec<String>, usize)> {
    let lines = match text {
        contract::SUBAGENT_MESSAGE => contract::subagent(false),
        contract::BACKGROUND_SUBAGENT_MESSAGE => return Some((contract::subagent(true), 4)),
        text if text.starts_with("Resume ") && text.ends_with(" with SendMessage") => {
            let lines = contract::resumed_subagent();
            let resumed = lines[contract::subagent(false).len()..].to_vec();
            return Some((resumed, 4));
        }
        _ => return None,
    };
    let turn_lines = lines.len();
    Some((lines, turn_lines))
}
