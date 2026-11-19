//! Claude Code's side of the conversation, as `claude -p` carries it in its stream-json format:
//! the events read from what it writes, and the commands written back.
//! Ref: sdk.d.ts in the npm package @anthropic-ai/claude-agent-sdk, which types every message
//! Claude Code documents. Anywhere in the client, the smallest item that relies on more than
//! those types is a function, type or field of its own, whose doc links to the check of what it
//! assumes, as [`crate::contract`] explains.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde::de::IgnoredAny;
use serde_json::{Map, Value, json};

/// The ids of the requests sent before the first message
pub const INITIALIZE_REQUEST_ID: &str = "initialize";
const SETTINGS_REQUEST_ID: &str = "settings";
const CONTEXT_REQUEST_ID: &str = "context";
const USAGE_REQUEST_ID: &str = "usage";

/// Something Claude Code reported.
pub enum Event {
    /// A piece of the reply Claude is writing.
    Text(String),
    /// The end of a block of the reply, with the block's whole text, whose pieces arrived as
    /// `Text`, unless Claude Code made the block itself, as for a command's output.
    TextEnd(String),
    /// The context was compacted, from as many tokens as it held to as many as it holds after,
    /// when Claude Code tells.
    Compacted {
        before: u64,
        after: Option<u64>,
    },
    /// A kind of message, or of a part of one, that claudio knows nothing of, with the kind, as
    /// `name` of `part`, and the line that carried it.
    Unknown {
        part: &'static str,
        name: String,
        line: String,
    },
    /// A call to the API that failed, with Claude Code's words for it, in place of a reply.
    ApiError(String),
    ToolUse(ToolUse),
    ToolResult(ToolResult),
    /// A tool that waits for the user's answer before it runs. Boxed, since its input and what
    /// Claude Code tells of it take room.
    Ask(Box<Ask>),
    State(State),
    /// A hook of the user's settings, named `hook` as Claude Code names it, failed with
    /// `exit_code` and `stderr`, or was cancelled, when `is_cancelled`.
    HookFailed {
        hook: String,
        exit_code: Option<i64>,
        stderr: String,
        is_cancelled: bool,
    },
    /// What Claude Code tells the user, like a hook's message, at `level`.
    Notice {
        text: String,
        level: Level,
    },
    /// A slow permission check of the tool use with `tool_use_id` started, when `is_checking`,
    /// or ended.
    PermissionCheck {
        tool_use_id: String,
        is_checking: bool,
    },
    /// The tasks that run in the background, in the order they started.
    BackgroundTasks(Vec<Task>),
    /// A task started for the tool use with `tool_use_id`, as a command does once it runs in the
    /// background, or has run for a few seconds, and runs in the background when
    /// `is_backgrounded`. A subagent's task, when `is_agent`, starts with its `Agent` call, in
    /// the foreground too.
    TaskStarted {
        task_id: String,
        tool_use_id: Option<String>,
        is_backgrounded: bool,
        is_agent: bool,
    },
    /// A task that started in the foreground moved to the background.
    TaskBackgrounded {
        task_id: String,
    },
    /// A task ended, as `status` says, and as Claude Code sums it up for Claude in `summary`.
    TaskEnded {
        task_id: String,
        status: TaskStatus,
        summary: String,
    },
    /// How much of the plan's usage limits the account used, each none when Claude Code left it
    /// out.
    Limits {
        /// The limit over five hours.
        session: Option<Limit>,
        /// The limit over seven days.
        week: Option<Limit>,
    },
    /// The end of a turn, with the reason Claude Code gives when the turn ended before its work
    /// was done.
    TurnEnd {
        failure: Option<String>,
    },
    /// Settings files Claude Code skipped, so none of their rules or hooks apply.
    SettingsErrors(Vec<SettingsError>),
    /// The key that pays for the session in place of the subscription, like `ANTHROPIC_API_KEY`.
    PayingKey(String),
    /// The id of the session, which `--resume` takes.
    SessionId(String),
    /// The version of Claude Code.
    Version(String),
    /// The names of the commands the session runs, without their `/`, Claude Code's own and the
    /// user's skills, as each session tells them as it starts.
    Commands(Vec<String>),
    /// The context was cleared, which ends the session with this id, the one `--resume` takes
    /// to go back to it. A new session follows, whose id arrives next.
    Reset {
        previous_session: String,
    },
    /// A call of Claude's to the API, whose reply starts.
    Call(Usage),
    /// The last call of the session as Claude Code starts it, none in a new session.
    LastCall(Option<Usage>),
    /// The model of the session as Claude Code starts it.
    Model(String),
    /// What the context can grow to as Claude Code starts the session.
    Ceiling(Ceiling),
    /// A sign that Claude Code works in a way claudio was not made for, as this error says, which
    /// ends the session as it starts.
    Incompatible(String),
    /// A request this client has no answer for. Claude Code waits on it until it is refused.
    Unsupported {
        request_id: String,
        subtype: String,
    },
}

/// A task that runs in the background, like a shell Claude started.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Task {
    #[serde(rename = "task_id")]
    pub id: String,
    /// What the task does, as the call that started it describes it.
    pub description: String,
    #[serde(rename = "task_type")]
    pub(crate) kind: String,
}

impl Task {
    /// Whether the task is a subagent's, rather than a command's or another kind's.
    pub fn is_agent(&self) -> bool {
        self.kind == AGENT_TASK
    }
}

/// The type of a subagent's task, as `SDKTaskStartedMessage` in `sdk.d.ts` names it.
const AGENT_TASK: &str = "local_agent";

/// How a task ended.
///
/// Undocumented: [`crate::contract::a_background_task_is_told_from_start_to_end`]
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Completed,
    Failed,
    /// Stopped before it ended, as by Claude's call of `TaskStop`.
    Stopped,
}

/// How much a notice of Claude Code's stands out.
/// Ref: `SDKInformationalMessage` in `sdk.d.ts`
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    /// Shown only in the full transcript.
    Info,
    /// Shown dim.
    Notice,
    /// Shown prominently, as a warning is.
    Suggestion,
    Warning,
}

/// Whether Claude works, waits for a message, or waits for the answer to an ask.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Idle,
    Running,
    RequiresAction,
}

/// How much of one of the plan's usage limits the account used.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limit {
    /// The share of the limit used, from 0 to 1.
    pub utilization: f64,
    /// Whether the account reached the limit.
    pub is_reached: bool,
    /// When the limit starts over, none when Claude Code left it out.
    pub resets_at: Option<SystemTime>,
}

/// What the context can grow to, in tokens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ceiling {
    /// The size at which Claude Code compacts the context.
    Compact(u64),
    /// The window the context fits in, which it grows to when Claude Code compacts it only on
    /// request.
    Window(u64),
}

/// What a call of Claude's read and wrote.
pub struct Usage {
    /// The tokens of the context the call read.
    pub tokens: u64,
    /// The TTL of what the call wrote to the cache, none when it wrote nothing or did not say.
    pub cache_ttl: Option<Duration>,
}

pub struct ToolUse {
    pub id: String,
    pub name: String,
    pub input: Map<String, Value>,
    /// The `Agent` call that started the subagent that makes the call, none when Claude itself
    /// makes it.
    pub agent_call: Option<String>,
    /// The subagent that makes the call, none when Claude itself makes it.
    pub agent: Option<String>,
}

impl ToolUse {
    /// Whether a subagent of Claude makes the call, rather than Claude itself.
    pub fn is_subagent(&self) -> bool {
        self.agent_call.is_some()
    }
}

pub struct ToolResult {
    pub tool_use_id: String,
    /// None when the result holds content but no text, like references to tools.
    pub text: Option<String>,
    pub is_error: bool,
    /// The subagent whose call the result answers, none for Claude's own call.
    pub agent: Option<String>,
}

impl ToolResult {
    /// Whether the result answers a call of a subagent of Claude, rather than Claude's own.
    pub fn is_subagent(&self) -> bool {
        self.agent.is_some()
    }
}

pub struct Ask {
    request_id: String,
    pub tool_use_id: String,
    pub tool_name: String,
    pub input: Map<String, Value>,
    pub kind: AskKind,
}

pub enum AskKind {
    /// Whether the tool may run.
    Permission(Permission),
    /// Whether Claude may leave plan mode and carry out the plan of an `ExitPlanMode` call, in
    /// Markdown.
    Plan(String),
    /// The questions of an `AskUserQuestion` call, which runs once all of them are answered.
    Questions(Vec<Question>),
}

/// What Claude Code tells of a tool use it asks the user about.
pub struct Permission {
    /// The tool's name for a person, like `Search Files` for an MCP tool, or the tool's own name.
    pub name: String,
    /// The name of the MCP server the tool comes from, like `claude.ai Google Drive`.
    pub server: Option<String>,
    /// Why Claude Code asks, like the safety check that failed, none when it does not say.
    pub reason: Option<String>,
    /// What kind of check asks, like `rule` for the user's own ask rules.
    pub reason_type: Option<String>,
    /// The path that made Claude Code ask, as about a `rm -rf` of a directory.
    pub blocked_path: Option<String>,
    /// Whether the call should open on its refusal, which Claude Code asks for a call it deems
    /// dangerous, like a `rm` of many files, as its own prompt then does.
    pub is_guarded: bool,
}

#[derive(Deserialize)]
pub struct Question {
    #[serde(rename = "question")]
    pub text: String,
    /// A label of a few words, which Claude Code shows next to the question.
    pub header: String,
    pub options: Vec<Choice>,
    // The tool's schema defaults it to false
    #[serde(rename = "multiSelect", default)]
    pub is_multi_select: bool,
}

#[derive(Deserialize)]
pub struct Choice {
    pub label: String,
    pub description: String,
    pub preview: Option<String>,
}

#[derive(Deserialize)]
pub struct SettingsError {
    pub file: Option<String>,
    /// The field that failed, empty when the whole file did.
    pub path: String,
    pub message: String,
}

pub enum Answer {
    /// Run the tool with `input`.
    Allow { input: Map<String, Value> },
    /// Refuse the tool and end the turn, so Claude waits for the next message.
    Deny,
    /// Refuse the tool for the user's `reason`, which Claude reads and goes on from, in the same
    /// turn.
    Explain { reason: String },
}

pub enum Command<'a> {
    Message {
        id: &'a str,
        text: &'a str,
    },
    /// Runs one of Claude Code's commands, written without its `/`, like `clear`, which clears
    /// the context and starts a new session. Claude Code runs a command right after a turn it
    /// was told to stop.
    Run {
        command: &'a str,
    },
    Answer(&'a Ask, Answer),
    Interrupt {
        request_id: String,
    },
    /// Moves the task of the call `tool_use_id` to the background, and leaves the tasks of other
    /// calls as they are, a subagent's own calls among them. Claude Code answers with success
    /// for a call with no task that runs in the foreground too, and refuses the request in a
    /// session whose background tasks are turned off, a refusal that ends claudio's session with
    /// Claude Code's reason.
    ///
    /// Ref: `SDKControlBackgroundTasksRequest` in `sdk.d.ts`
    /// Undocumented: [`crate::contract::a_call_moved_by_its_id_makes_way_for_send_now`], for the
    /// move ending before a send now sent right after it
    Background {
        request_id: String,
        tool_use_id: &'a str,
    },
    /// Has Claude Code make way for the message with this id, which it already holds, as the
    /// send-now key of Claude Code's terminal does: it moves what the running turn waits on to
    /// the background where it can, and stops the turn otherwise. It does nothing when no turn
    /// runs or the running turn already took the message in, so it never stops the turn that
    /// answers the message.
    ///
    /// Undocumented: [`crate::contract::send_now_is_announced`]
    SendNow {
        request_id: String,
        message_id: &'a str,
    },
    /// Stops the task with this id, which runs in the background, which then ends as
    /// `stopped`, as `SDKControlStopTaskRequest` in `sdk.d.ts` types it:
    /// <https://unpkg.com/@anthropic-ai/claude-agent-sdk@0.3.325/sdk.d.ts>.
    ///
    /// Undocumented: [`crate::contract::a_stop_of_a_task_that_ended_succeeds`], for a task that
    /// ends before the stop reaches Claude Code
    StopTask {
        request_id: String,
        task_id: &'a str,
    },
    /// Starts the session. Claude Code answers with the account that pays for it. It declares
    /// that the user stops each background task on its own, `perTaskStopAffordance`, so an
    /// interrupt stops the turn alone, and the background tasks go on until they end or are
    /// stopped, as `SDKControlInitializeRequest` in `sdk.d.ts` types it:
    /// <https://unpkg.com/@anthropic-ai/claude-agent-sdk@0.3.325/sdk.d.ts>.
    Initialize,
    GetSettings,
    /// Asks for the context's usage, which tells the last call of a resumed session. The
    /// summary comes from that call's usage, where the full answer would count each part of
    /// the context with the API.
    GetContext,
    /// Asks for the plan's usage limits, which Claude Code reads from the plan's usage, where a
    /// call to the API would only send them along with its reply. The SDK's types mark the
    /// request experimental, with a shape that may change. Seen with Claude Code 2.1.319.
    GetUsage,
    Refuse {
        request_id: &'a str,
        subtype: &'a str,
    },
}

impl Command<'_> {
    /// The line that carries the command to Claude Code.
    pub fn to_line(&self) -> String {
        let message = match self {
            Command::Message { id, text } => json!({
                "type": "user",
                "session_id": "",
                "uuid": id,
                "message": {"role": "user", "content": text},
                "parent_tool_use_id": null,
                // Claude Code takes a message without an origin as from no one, and makes way
                // while it works only for a person's message
                // Ref: SDKMessageOrigin in sdk.d.ts
                "origin": {"kind": "human"},
                // Claude Code runs a message that starts with / as a command, unless it comes
                // marked as composed by the client, which delivers it as written. The mark
                // also holds back the context Claude Code attaches to the turn until Claude's
                // first tool call, so only such a message carries it.
                "client_composed": text.starts_with('/'),
            }),
            // Claude Code runs a message that starts with / as a command, so this one carries no
            // mark of a message composed by the client
            Command::Run { command } => json!({
                "type": "user",
                "session_id": "",
                "message": {"role": "user", "content": format!("/{command}")},
                "parent_tool_use_id": null,
            }),
            Command::Answer(ask, answer) => {
                let result = match answer {
                    Answer::Allow { input } => json!({
                        "behavior": "allow",
                        "updatedInput": input,
                        "toolUseID": ask.tool_use_id,
                    }),
                    Answer::Deny => json!({
                        "behavior": "deny",
                        "message": "The user refused this tool use.",
                        "interrupt": true,
                        "toolUseID": ask.tool_use_id,
                    }),
                    Answer::Explain { reason } => json!({
                        "behavior": "deny",
                        "message": format!("The user refused this tool use, and said: {reason}"),
                        "interrupt": false,
                        "toolUseID": ask.tool_use_id,
                    }),
                };
                json!({
                    "type": "control_response",
                    "response": {"subtype": "success", "request_id": ask.request_id, "response": result},
                })
            }
            Command::Interrupt { request_id } => {
                control_request(request_id, &json!({"subtype": "interrupt"}))
            }
            Command::Background {
                request_id,
                tool_use_id,
            } => control_request(
                request_id,
                &json!({"subtype": "background_tasks", "tool_use_id": tool_use_id}),
            ),
            Command::SendNow {
                request_id,
                message_id,
            } => control_request(
                request_id,
                &json!({"subtype": "interrupt", "send_now": true, "message_uuid": message_id}),
            ),
            Command::StopTask {
                request_id,
                task_id,
            } => control_request(
                request_id,
                &json!({"subtype": "stop_task", "task_id": task_id}),
            ),
            Command::Initialize => control_request(
                INITIALIZE_REQUEST_ID,
                &json!({"subtype": "initialize", "perTaskStopAffordance": true}),
            ),
            Command::GetSettings => {
                control_request(SETTINGS_REQUEST_ID, &json!({"subtype": "get_settings"}))
            }
            Command::GetContext => control_request(
                CONTEXT_REQUEST_ID,
                &json!({"subtype": "get_context_usage", "detail": "summary"}),
            ),
            // Without the scan of the week's transcripts, which tells nothing of the limits
            Command::GetUsage => control_request(
                USAGE_REQUEST_ID,
                &json!({"subtype": "get_usage", "skip_behaviors": true}),
            ),
            Command::Refuse {
                request_id,
                subtype,
            } => json!({
                "type": "control_response",
                "response": {
                    "subtype": "error",
                    "request_id": request_id,
                    "error": format!("This client does not support {subtype}"),
                },
            }),
        };
        format!("{message}\n")
    }
}

/// What claudio does with a kind of what Claude Code sends, as the README's "What claudio
/// relies on" tells. A kind can take one class or another by what it holds.
enum Class {
    /// It matters to the user, who sees these events of it.
    Shown(Vec<Event>),
    /// It tells the user nothing, for this reason.
    Skipped(#[expect(dead_code, reason = "a skip must say why, for the reader")] &'static str),
    /// claudio knows nothing of it, and this event says so.
    Flagged(Event),
}

use Class::{Flagged, Shown, Skipped};

impl Class {
    fn into_events(self) -> Vec<Event> {
        match self {
            Shown(events) => events,
            Skipped(_) => Vec::new(),
            Flagged(event) => vec![event],
        }
    }
}

/// Reads the events in one line Claude Code wrote, none for a message that reports nothing a
/// person reads or answers.
pub fn read_events(line: &str) -> Result<Vec<Event>, String> {
    let message: Message = serde_json::from_str(line)
        .map_err(|error| format!("Unreadable message from Claude Code: {error}: {line}"))?;
    let class = match message {
        Message::StreamEvent {
            event,
            parent_tool_use_id,
        } => read_stream_event(event, parent_tool_use_id.is_some(), line),
        // Claude Code sends an API error as a reply of its own making, whole and never streamed
        Message::Assistant {
            message,
            parent_tool_use_id: None,
            error: Some(_),
            agent_id: _,
        } => Shown(vec![Event::ApiError(message.content.into_text())]),
        // Claude Code sends each block whole once it has streamed it
        Message::Assistant {
            message,
            parent_tool_use_id,
            agent_id,
            error: _,
        } => {
            let agent = subagent(agent_id, parent_tool_use_id.as_ref());
            Shown(read_assistant(
                message,
                parent_tool_use_id.as_ref(),
                agent.as_ref(),
                line,
            ))
        }
        Message::User {
            is_replay: true, ..
        } => Skipped("it repeats what Claude Code's history holds, like a command's output"),
        Message::User {
            message,
            parent_tool_use_id,
            agent_id,
            is_replay: false,
        } => {
            let agent = subagent(agent_id, parent_tool_use_id.as_ref());
            Shown(read_user(message, agent.as_ref(), line))
        }
        Message::ControlRequest {
            request_id,
            request,
        } => Shown(vec![read_request(request_id, request)?]),
        Message::ControlResponse {
            response:
                Reply::Success {
                    request_id,
                    response,
                },
        } => Shown(read_response(&request_id, response)?),
        // The context's usage tells whether the cache may be cold, and claudio has nothing to
        // tell it by otherwise
        Message::ControlResponse {
            response: Reply::Error { request_id, error },
        } if request_id == CONTEXT_REQUEST_ID => Shown(vec![Event::Incompatible(format!(
            "Claude Code refused the context's usage: {error}"
        ))]),
        Message::ControlResponse {
            response: Reply::Error { request_id, error },
        } => return Err(format!("Claude Code refused request {request_id}: {error}")),
        Message::Result { subtype } if subtype == "success" => {
            Shown(vec![Event::TurnEnd { failure: None }])
        }
        Message::Result { subtype } => Shown(vec![Event::TurnEnd {
            failure: Some(subtype),
        }]),
        Message::RateLimitEvent { rate_limit_info } => Shown(vec![read_limits(rate_limit_info)]),
        Message::ConversationReset { session_id } => Shown(vec![Event::Reset {
            previous_session: session_id,
        }]),
        Message::System(system) => read_system(system, line),
        Message::KeepAlive => Skipped("a heartbeat has nothing to show"),
        Message::ToolProgress => Skipped("a tool's call and result tell enough of how long it ran"),
        Message::CommandLifecycle => {
            Skipped("the session's state tells when the turn of a message claudio sent runs")
        }
        Message::Unknown => Flagged(unknown(line, "message", "/type")),
    };
    Ok(class.into_events())
}

/// What claudio does with a streamed piece of a reply, a subagent's when `is_subagent`, from
/// `line`.
fn read_stream_event(event: StreamEvent, is_subagent: bool, line: &str) -> Class {
    match event {
        StreamEvent::Unknown => Flagged(unknown(line, "stream event", "/event/type")),
        StreamEvent::ContentBlockDelta {
            delta: Delta::Unknown,
        } => Flagged(unknown(line, "streamed piece", "/event/delta/type")),
        _ if is_subagent => Skipped(
            "a subagent's text shows in what it reports back to Claude, and it calls the API with a context and cache of its own",
        ),
        StreamEvent::ContentBlockDelta {
            delta: Delta::Text { text },
        } => Shown(vec![Event::Text(text)]),
        StreamEvent::MessageStart { message } => Shown(vec![Event::Call(message.usage.into())]),
        StreamEvent::ContentBlockDelta {
            delta: Delta::InputJson,
        } => Skipped("a tool's input shows whole with its call"),
        StreamEvent::ContentBlockDelta {
            delta: Delta::Thinking | Delta::Signature,
        } => Skipped("claudio leaves Claude's thinking out"),
        StreamEvent::ContentBlockStart
        | StreamEvent::ContentBlockStop
        | StreamEvent::MessageDelta
        | StreamEvent::MessageStop => {
            Skipped("the whole message that follows the stream tells what claudio shows of it")
        }
    }
}

/// The subagent that sent a message, by its `agent_id`, or by the `Agent` call that started it,
/// `agent_call`, for a message without one, none for Claude's own message. The types promise
/// `agent_id` on every message of a subagent, so the call is only a fallback, under which a
/// subagent whose messages carried the id only now and then would count as two.
fn subagent(agent_id: Option<String>, agent_call: Option<&String>) -> Option<String> {
    agent_id.or_else(|| agent_call.cloned())
}

/// The events of a reply's blocks, from `line`, those of the subagent `agent`, which the
/// `Agent` call `agent_call` started, when they are some.
fn read_assistant(
    message: Body,
    agent_call: Option<&String>,
    agent: Option<&String>,
    line: &str,
) -> Vec<Event> {
    let is_subagent = agent_call.is_some();
    (message.content.into_blocks().into_iter().enumerate())
        .flat_map(|(index, block)| {
            let class = match block {
                Block::Text { text } if !is_subagent => Shown(vec![Event::TextEnd(text)]),
                Block::Text { .. } => {
                    Skipped("a subagent's text shows in what it reports back to Claude")
                }
                Block::ToolUse { id, name, input } => Shown(vec![Event::ToolUse(ToolUse {
                    id,
                    name,
                    input,
                    agent_call: agent_call.cloned(),
                    agent: agent.cloned(),
                })]),
                Block::Thinking => Skipped("claudio leaves Claude's thinking out"),
                Block::ToolResult { .. } => Skipped("a tool's result comes in a user message"),
                Block::Unknown => Flagged(unknown(line, "content block", &block_type(index))),
            };
            class.into_events()
        })
        .collect()
}

/// The events of the tool results a user message carries, those of the subagent `agent` when it
/// is some, from `line`.
fn read_user(message: Body, agent: Option<&String>, line: &str) -> Vec<Event> {
    (message.content.into_blocks().into_iter().enumerate())
        .flat_map(|(index, block)| match block {
            Block::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                let (text, unknown_block) =
                    content.map_or((Some(String::new()), None), ResultContent::into_text);
                let result = Shown(vec![Event::ToolResult(ToolResult {
                    tool_use_id,
                    text,
                    is_error,
                    agent: agent.cloned(),
                })]);
                let unknown_block = unknown_block.map(|inner| {
                    let pointer = format!("/message/content/{index}/content/{inner}/type");
                    Flagged(unknown(line, "tool result block", &pointer))
                });
                [Some(result), unknown_block]
                    .into_iter()
                    .flatten()
                    .collect()
            }
            Block::Text { .. } => vec![Skipped("the user's own text shows as they sent it")],
            Block::Thinking | Block::ToolUse { .. } => {
                vec![Skipped("only Claude's messages carry them")]
            }
            Block::Unknown => vec![Flagged(unknown(line, "content block", &block_type(index)))],
        })
        .flat_map(Class::into_events)
        .collect()
}

/// The kind of thing that `line` carries as `part`, which claudio knows nothing of, by the
/// type that `pointer` points to in the line.
fn unknown(line: &str, part: &'static str, pointer: &str) -> Event {
    let value: Value = serde_json::from_str(line).unwrap_or_default();
    let name = value
        .pointer(pointer)
        .map_or_else(|| "?".to_string(), Value::to_string);
    Event::Unknown {
        part,
        name: name.trim_matches('"').to_string(),
        line: line.to_string(),
    }
}

/// The pointer to the type of the content block at `index` of a message.
fn block_type(index: usize) -> String {
    format!("/message/content/{index}/type")
}

fn read_system(system: System, line: &str) -> Class {
    match system {
        System::Init {
            session_id,
            claude_code_version,
            slash_commands,
        } => Shown(vec![
            Event::SessionId(session_id),
            Event::Version(claude_code_version),
            // The docs' examples name them with their `/` and without
            Event::Commands(
                (slash_commands.iter())
                    .map(|name| name.trim_start_matches('/').to_string())
                    .collect(),
            ),
        ]),
        System::SessionStateChanged { state } => Shown(vec![Event::State(state)]),
        System::BackgroundTasksChanged { tasks } => Shown(vec![Event::BackgroundTasks(tasks)]),
        System::TaskStarted {
            task_id,
            tool_use_id,
            is_backgrounded,
            task_type,
        } => Shown(vec![Event::TaskStarted {
            task_id,
            tool_use_id,
            is_backgrounded,
            is_agent: task_type.as_deref() == Some(AGENT_TASK),
        }]),
        System::TaskUpdated {
            task_id,
            patch: TaskPatch {
                is_backgrounded: Some(true),
            },
        } => Shown(vec![Event::TaskBackgrounded { task_id }]),
        System::TaskNotification {
            task_id,
            status,
            summary,
        } => Shown(vec![Event::TaskEnded {
            task_id,
            status,
            summary,
        }]),
        System::CompactBoundary { compact_metadata } => Shown(vec![Event::Compacted {
            before: compact_metadata.pre_tokens,
            after: compact_metadata.post_tokens,
        }]),
        System::PermissionCheckStatus {
            agent_id: Some(_), ..
        } => Skipped("the label row shows what Claude does, not what its subagents do"),
        System::PermissionCheckStatus {
            tool_use_id,
            status,
            agent_id: None,
        } => Shown(vec![Event::PermissionCheck {
            tool_use_id,
            is_checking: status == CheckStatus::Checking,
        }]),
        System::HookResponse {
            outcome: HookOutcome::Success,
            ..
        } => Skipped("a hook that succeeded needs nothing from the user"),
        System::HookResponse {
            hook_name,
            outcome,
            exit_code,
            stderr,
        } => Shown(vec![Event::HookFailed {
            hook: hook_name,
            exit_code,
            stderr,
            is_cancelled: outcome == HookOutcome::Cancelled,
        }]),
        System::Informational { content, level } => Shown(vec![Event::Notice {
            text: content,
            level,
        }]),
        System::Status => {
            Skipped("the label row tells what Claude Code does from the session's state")
        }
        System::ThinkingTokens => Skipped("claudio leaves Claude's thinking out"),
        System::SessionTitleChanged => Skipped("claudio shows the session's title nowhere"),
        System::CommandsChanged => {
            Skipped("the init that starts each session lists the commands to complete")
        }
        System::TaskUpdated { .. } => {
            Skipped("a task's other changes, like its end, come in messages of their own")
        }
        System::TaskProgress => Skipped("a task shows only as it starts and ends"),
        System::HookStarted | System::HookProgress => {
            Skipped("how a hook ends tells all that matters of it")
        }
        System::PermissionDenied => Skipped("the tool's failed result shows the refusal"),
        System::Unknown => Flagged(unknown(line, "system message", "/subtype")),
    }
}

/// Whether a line of Claude Code's error output repeats what one of its messages shows.
pub fn repeats_a_message(line: &str) -> bool {
    // Claude Code tags the API error of a model it does not know, which its reply reports
    line.starts_with("[claude-code:unrecognized_model]")
}

fn read_limits(info: RateLimitInfo) -> Event {
    let windows = info.unified_windows;
    Event::Limits {
        session: windows.five_hour.map(Limit::from),
        week: windows.seven_day.map(Limit::from),
    }
}

/// The events in the response to one of the requests sent before the first message.
/// A request of claudio's to Claude Code, `request`, whose reply carries `request_id` back.
fn control_request(request_id: &str, request: &Value) -> Value {
    json!({"type": "control_request", "request_id": request_id, "request": request})
}

fn read_response(request_id: &str, response: Option<Value>) -> Result<Vec<Event>, String> {
    match request_id {
        INITIALIZE_REQUEST_ID => read_initialize(response),
        SETTINGS_REQUEST_ID => read_settings(response),
        CONTEXT_REQUEST_ID => {
            Ok(read_context_usage(response)
                .unwrap_or_else(|error| vec![Event::Incompatible(error)]))
        }
        USAGE_REQUEST_ID => read_plan_usage(response),
        // The other requests sent are interrupts, moves to the background and stops of tasks,
        // whose success needs no word
        _ => Ok(vec![]),
    }
}

/// What the session's start tells: the key that pays for it, none when the subscription does,
/// and the commands it runs, which the init that comes with the first message tells again.
///
/// Undocumented: [`crate::contract::the_paying_key_is_named_by_its_source`],
/// [`crate::contract::the_start_lists_the_commands`]
fn read_initialize(response: Option<Value>) -> Result<Vec<Event>, String> {
    let initialize: Initialize = serde_json::from_value(response.unwrap_or_default())
        .map_err(|error| format!("Unreadable account from Claude Code: {error}"))?;
    let account = initialize.account;
    let key = account.api_key_source.or(account
        .token_source
        .filter(|source| source == "ANTHROPIC_AUTH_TOKEN"));
    let commands = (initialize.commands.into_iter())
        .map(|command| command.name)
        .collect();
    Ok(key
        .map(Event::PayingKey)
        .into_iter()
        .chain([Event::Commands(commands)])
        .collect())
}

/// The settings files Claude Code skipped, none when it read them all.
fn read_settings(response: Option<Value>) -> Result<Vec<Event>, String> {
    let settings: Settings = serde_json::from_value(response.unwrap_or_default())
        .map_err(|error| format!("Unreadable settings from Claude Code: {error}"))?;
    if settings.errors.is_empty() {
        Ok(vec![])
    } else {
        Ok(vec![Event::SettingsErrors(settings.errors)])
    }
}

/// The last call of the session, which the context's usage tells, with the model and the
/// context's ceiling. A session without a call has no usage yet.
fn read_context_usage(response: Option<Value>) -> Result<Vec<Event>, String> {
    let context: ContextUsage = serde_json::from_value(response.unwrap_or_default())
        .map_err(|error| format!("Unreadable context usage from Claude Code: {error}"))?;
    let ceiling = match (
        context.is_auto_compact_enabled,
        context.auto_compact_threshold,
    ) {
        (true, Some(threshold)) => Ceiling::Compact(threshold),
        _ => Ceiling::Window(context.max_tokens),
    };
    Ok(vec![
        Event::LastCall(context.api_usage.map(Usage::from)),
        Event::Model(context.model),
        Event::Ceiling(ceiling),
    ])
}

/// The plan's usage limits, none when they do not apply, like to a session an API key pays for.
fn read_plan_usage(response: Option<Value>) -> Result<Vec<Event>, String> {
    let usage: PlanUsage = serde_json::from_value(response.unwrap_or_default())
        .map_err(|error| format!("Unreadable usage from Claude Code: {error}"))?;
    let Some(limits) = usage.rate_limits else {
        return Ok(vec![]);
    };
    Ok(vec![Event::Limits {
        session: read_plan_limit(limits.five_hour)?,
        week: read_plan_limit(limits.seven_day)?,
    }])
}

/// A limit of the plan's usage, none when Claude Code leaves out how much of it the account
/// used. The usage tells no status, so the limit counts as reached once it is all used.
fn read_plan_limit(limit: Option<PlanLimit>) -> Result<Option<Limit>, String> {
    let Some(PlanLimit {
        utilization: Some(percent),
        resets_at,
    }) = limit
    else {
        return Ok(None);
    };
    let resets_at = resets_at
        .map(|time| {
            time.parse::<jiff::Timestamp>()
                .map(SystemTime::from)
                .map_err(|error| format!("Unreadable reset time from Claude Code: {error}"))
        })
        .transpose()?;
    Ok(Some(Limit {
        utilization: percent / 100.0,
        is_reached: percent >= 100.0,
        resets_at,
    }))
}

pub(crate) fn read_request(request_id: String, request: Value) -> Result<Event, String> {
    let subtype = request["subtype"].as_str().unwrap_or_default().to_string();
    if subtype != "can_use_tool" {
        return Ok(Event::Unsupported {
            request_id,
            subtype,
        });
    }
    let request: CanUseTool = serde_json::from_value(request)
        .map_err(|error| format!("Unreadable permission request from Claude Code: {error}"))?;
    let kind = match request.tool_name.as_str() {
        "AskUserQuestion" => {
            let input: QuestionsInput =
                serde_json::from_value(Value::Object(request.input.clone()))
                    .map_err(|error| format!("Unreadable questions from Claude: {error}"))?;
            AskKind::Questions(input.questions)
        }
        "ExitPlanMode" => {
            let plan = request.input.get("plan").and_then(Value::as_str);
            AskKind::Plan(plan.unwrap_or_default().to_string())
        }
        _ => AskKind::Permission(Permission {
            name: (request.display_name.as_deref())
                .map_or_else(|| request.tool_name.clone(), strip_escapes),
            server: request.mcp_server.map(|server| strip_escapes(&server.name)),
            // The SDK's types warn that it may carry ANSI escapes
            reason: request.decision_reason.as_deref().map(strip_escapes),
            reason_type: request.decision_reason_type.as_deref().map(strip_escapes),
            blocked_path: request.blocked_path.as_deref().map(strip_escapes),
            is_guarded: request.default_to_no,
        }),
    };
    Ok(Event::Ask(Box::new(Ask {
        request_id,
        tool_use_id: request.tool_use_id,
        tool_name: request.tool_name,
        input: request.input,
        kind,
    })))
}

/// A kind of message Claude Code sends, whose [`Class`] the reading gives. A kind joins once a
/// session sends it.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Message {
    StreamEvent {
        event: StreamEvent,
        parent_tool_use_id: Option<String>,
    },
    Assistant {
        message: Body,
        parent_tool_use_id: Option<String>,
        agent_id: Option<String>,
        /// The kind of API error the message reports, on a message Claude Code makes for one.
        error: Option<String>,
    },
    User {
        message: Body,
        parent_tool_use_id: Option<String>,
        agent_id: Option<String>,
        /// Whether the message repeats what Claude Code's history holds, like a command's
        /// output after `/compact`.
        #[serde(rename = "isReplay", default)]
        is_replay: bool,
    },
    ControlRequest {
        request_id: String,
        request: Value,
    },
    ControlResponse {
        response: Reply,
    },
    Result {
        subtype: String,
    },
    RateLimitEvent {
        rate_limit_info: RateLimitInfo,
    },
    System(System),
    /// Sent as `/clear` starts a new session, with the id of the session it cleared.
    ///
    /// Undocumented: [`crate::contract::a_clear_resets_the_session`]
    ConversationReset {
        session_id: String,
    },
    /// A heartbeat.
    KeepAlive,
    /// How long a tool has run.
    ToolProgress,
    /// Where a message claudio sent is in Claude Code's queue.
    CommandLifecycle,
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RateLimitInfo {
    /// Every limit of the plan.
    ///
    /// Undocumented: [`crate::contract::a_rate_limit_event_has_every_window`]
    #[serde(default)]
    unified_windows: UnifiedWindows,
}

#[derive(Default, Deserialize)]
struct UnifiedWindows {
    five_hour: Option<Window>,
    seven_day: Option<Window>,
}

#[derive(Deserialize)]
struct Window {
    utilization: f64,
    status: Option<WindowStatus>,
    #[serde(rename = "resetsAt")]
    resets_at_seconds: Option<u64>,
}

/// Every status the SDK's types give, so a new one is a change of the field, which makes the
/// line unreadable, as any other field that is not as typed.
#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum WindowStatus {
    Allowed,
    AllowedWarning,
    Rejected,
}

impl From<Window> for Limit {
    fn from(window: Window) -> Self {
        Limit {
            utilization: window.utilization,
            is_reached: window.status == Some(WindowStatus::Rejected),
            resets_at: window
                .resets_at_seconds
                .map(|seconds| UNIX_EPOCH + Duration::from_secs(seconds)),
        }
    }
}

/// A kind of system message, shown, skipped on purpose, or flagged as `Unknown`, as for
/// [`Message`].
#[derive(Deserialize)]
#[serde(tag = "subtype", rename_all = "snake_case")]
enum System {
    /// Carries the id of the session Claude Code runs, Claude Code's version, and the commands
    /// the session runs, those that need no interactive terminal, built in or the user's own.
    /// Ref: <https://code.claude.com/docs/en/agent-sdk/slash-commands>, on the commands the init
    /// lists
    Init {
        session_id: String,
        claude_code_version: String,
        #[serde(default)]
        slash_commands: Vec<String>,
    },
    /// Sent as a turn starts and ends.
    ///
    /// Undocumented: [`crate::contract::session_state_comes_with_its_variable`]
    SessionStateChanged {
        state: State,
    },
    /// Sent each time a task goes to the background or ends there, with every task that runs
    /// there, as the session runs and not only once its input closes.
    ///
    /// Undocumented: [`crate::contract::a_background_task_is_told_from_start_to_end`]
    BackgroundTasksChanged {
        tasks: Vec<Task>,
    },
    /// Sent once the context was compacted, as by `/compact`.
    CompactBoundary {
        compact_metadata: CompactMetadata,
    },
    /// What Claude Code does, like compacting.
    Status,
    /// How far Claude thinks.
    ThinkingTokens,
    /// The title Claude Code gives the session.
    SessionTitleChanged,
    /// The slash commands and skills the session offers, sent again as they change, like after
    /// `/clear`.
    CommandsChanged,
    /// Sent as a task starts, before the result of the call that started it, which
    /// `tool_use_id` names, as for a command that runs in the background, as `is_backgrounded`
    /// says, or that has run for a few seconds in the foreground.
    ///
    /// Undocumented: [`crate::contract::a_background_task_is_told_from_start_to_end`],
    /// [`crate::contract::a_call_moved_to_the_background_is_told`],
    /// [`crate::contract::a_subagent_is_told_from_start_to_end`], for a subagent, whose task
    /// starts in the foreground too
    TaskStarted {
        task_id: String,
        tool_use_id: Option<String>,
        is_backgrounded: bool,
        task_type: Option<String>,
    },
    /// Sent as a task changes, like as it ends, or as it moves to the background, before the
    /// result of the call that started it.
    ///
    /// Undocumented: [`crate::contract::a_call_moved_to_the_background_is_told`]
    TaskUpdated {
        task_id: String,
        patch: TaskPatch,
    },
    TaskProgress,
    /// Sent as a task ends, in the background or in the foreground.
    TaskNotification {
        task_id: String,
        status: TaskStatus,
        summary: String,
    },
    HookStarted,
    HookProgress,
    /// How a hook of the user's settings ended.
    HookResponse {
        hook_name: String,
        outcome: HookOutcome,
        exit_code: Option<i64>,
        stderr: String,
    },
    /// What Claude Code tells the user, like a hook's message or a refused model.
    Informational {
        content: String,
        level: Level,
    },
    /// A tool use that auto mode refused.
    PermissionDenied,
    /// Sent once the auto mode classifier has not answered for 4 seconds whether a tool use
    /// needs the user's approval, and again as it answers, so the wait shows as a check rather
    /// than as a slow tool.
    ///
    /// Undocumented: [`crate::contract::a_slow_permission_check_is_told`]
    PermissionCheckStatus {
        tool_use_id: String,
        /// Set when a subagent's tool use waits.
        agent_id: Option<String>,
        status: CheckStatus,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum HookOutcome {
    Success,
    Error,
    Cancelled,
}

#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum CheckStatus {
    Checking,
    Done,
}

/// What changed of a task.
#[derive(Deserialize)]
struct TaskPatch {
    is_backgrounded: Option<bool>,
}

#[derive(Deserialize)]
struct CompactMetadata {
    pre_tokens: u64,
    post_tokens: Option<u64>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum StreamEvent {
    MessageStart {
        message: StartedMessage,
    },
    ContentBlockDelta {
        delta: Delta,
    },
    ContentBlockStart,
    ContentBlockStop,
    MessageDelta,
    MessageStop,
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum Delta {
    #[serde(rename = "text_delta")]
    Text { text: String },
    /// A piece of a tool's input.
    #[serde(rename = "input_json_delta")]
    InputJson,
    // Pieces of Claude's thinking
    #[serde(rename = "thinking_delta")]
    Thinking,
    #[serde(rename = "signature_delta")]
    Signature,
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
struct StartedMessage {
    usage: ApiUsage,
}

/// The usage the API reports for a call. Each count is absent or null where the API leaves it
/// out, which counts as none.
#[derive(Deserialize)]
struct ApiUsage {
    #[serde(default)]
    input_tokens: Option<u64>,
    #[serde(default)]
    cache_creation_input_tokens: Option<u64>,
    #[serde(default)]
    cache_read_input_tokens: Option<u64>,
    /// What the call wrote to the cache, split by TTL.
    #[serde(default)]
    cache_creation: Option<CacheCreation>,
}

#[derive(Deserialize)]
struct CacheCreation {
    #[serde(default)]
    ephemeral_5m_input_tokens: Option<u64>,
    #[serde(default)]
    ephemeral_1h_input_tokens: Option<u64>,
}

impl From<ApiUsage> for Usage {
    fn from(usage: ApiUsage) -> Self {
        let tokens = [
            usage.input_tokens,
            usage.cache_creation_input_tokens,
            usage.cache_read_input_tokens,
        ]
        .into_iter()
        .flatten()
        .sum();
        let cache_ttl = usage.cache_creation.and_then(|creation| {
            if creation.ephemeral_1h_input_tokens.unwrap_or_default() > 0 {
                Some(Duration::from_hours(1))
            } else if creation.ephemeral_5m_input_tokens.unwrap_or_default() > 0 {
                Some(Duration::from_mins(5))
            } else {
                None
            }
        });
        Usage { tokens, cache_ttl }
    }
}

#[derive(Deserialize)]
struct Body {
    content: Content,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Content {
    Blocks(Vec<Block>),
    // Tried after Blocks, since it takes any value
    Text(IgnoredAny),
}

impl Content {
    fn into_blocks(self) -> Vec<Block> {
        match self {
            Content::Blocks(blocks) => blocks,
            // A message of plain text holds no tool use or result
            Content::Text(_) => vec![],
        }
    }

    /// The text of the content's text blocks, a line apart.
    fn into_text(self) -> String {
        let texts: Vec<String> = self
            .into_blocks()
            .into_iter()
            .filter_map(|block| match block {
                Block::Text { text } => Some(text),
                Block::Thinking
                | Block::ToolUse { .. }
                | Block::ToolResult { .. }
                | Block::Unknown => None,
            })
            .collect();
        texts.join("\n")
    }
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Block {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Map<String, Value>,
    },
    ToolResult {
        tool_use_id: String,
        content: Option<ResultContent>,
        #[serde(default)]
        is_error: bool,
    },
    /// Claude's thinking.
    Thinking,
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ResultContent {
    Text(String),
    Blocks(Vec<ResultBlock>),
}

impl ResultContent {
    /// The content's text, none when it holds blocks but none of them text, with the index of
    /// the first block of a kind claudio knows nothing of.
    fn into_text(self) -> (Option<String>, Option<usize>) {
        match self {
            ResultContent::Text(text) => (Some(text), None),
            ResultContent::Blocks(blocks) => {
                let has_blocks = !blocks.is_empty();
                let unknown = blocks
                    .iter()
                    .position(|block| matches!(block, ResultBlock::Unknown));
                let texts: Vec<String> = blocks
                    .into_iter()
                    .filter_map(|block| match block {
                        ResultBlock::Text { text } => Some(text),
                        ResultBlock::Image | ResultBlock::ToolReference | ResultBlock::Unknown => {
                            None
                        }
                    })
                    .collect();
                let text = (!has_blocks || !texts.is_empty()).then(|| texts.join("\n"));
                (text, unknown)
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ResultBlock {
    Text {
        text: String,
    },
    // Results without text, like an image the Read tool read, or the tools ToolSearch found
    Image,
    ToolReference,
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
#[serde(tag = "subtype", rename_all = "snake_case")]
enum Reply {
    Success {
        request_id: String,
        response: Option<Value>,
    },
    Error {
        request_id: String,
        error: String,
    },
}

#[derive(Deserialize)]
struct Initialize {
    account: Account,
    #[serde(default)]
    commands: Vec<CommandName>,
}

/// A command the session runs, by the name it takes after `/`.
#[derive(Deserialize)]
struct CommandName {
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Account {
    api_key_source: Option<String>,
    token_source: Option<String>,
}

#[derive(Deserialize)]
struct Settings {
    // Claude Code leaves the list out when no file failed
    #[serde(default)]
    errors: Vec<SettingsError>,
}

#[derive(Deserialize)]
struct ContextUsage {
    /// The last call's usage, null in a new session. The key is required, so a renamed key
    /// shows as an error, where a missing key would read as a new session's.
    /// Ref: `SDKControlGetContextUsageResponse` in `sdk.d.ts`, whose `apiUsage` is `{…} | null`
    #[serde(rename = "apiUsage", deserialize_with = "Option::deserialize")]
    api_usage: Option<ApiUsage>,
    model: String,
    /// The window the context fits in.
    #[serde(rename = "maxTokens")]
    max_tokens: u64,
    #[serde(rename = "isAutoCompactEnabled")]
    is_auto_compact_enabled: bool,
    /// The tokens at which Claude Code compacts the context on its own.
    #[serde(rename = "autoCompactThreshold")]
    auto_compact_threshold: Option<u64>,
}

#[derive(Deserialize)]
struct PlanUsage {
    /// Absent when the plan's limits do not apply.
    rate_limits: Option<PlanLimits>,
}

#[derive(Deserialize)]
struct PlanLimits {
    five_hour: Option<PlanLimit>,
    seven_day: Option<PlanLimit>,
}

#[derive(Deserialize)]
struct PlanLimit {
    /// The share of the limit used, from 0 to 100.
    utilization: Option<f64>,
    /// In ISO 8601.
    resets_at: Option<String>,
}

/// `text` without its escape sequences, and without the control characters other than line
/// breaks, so it cannot move the cursor or change colors where it shows.
fn strip_escapes(text: &str) -> String {
    let mut stripped = String::new();
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '\x1b' {
            // A CSI ends with a letter, and any other sequence with the character after the escape
            if characters.next_if_eq(&'[').is_some() {
                characters.find(char::is_ascii_alphabetic);
            } else {
                characters.next();
            }
        } else if !character.is_control() || character == '\n' {
            stripped.push(character);
        }
    }
    stripped
}

/// Claude Code's request to use a tool, `can_use_tool`, which asks whether Claude may make a
/// call, as `SDKControlPermissionRequest` in `sdk.d.ts` types it:
/// <https://unpkg.com/@anthropic-ai/claude-agent-sdk@0.3.325/sdk.d.ts>.
///
/// claudio reads the fields below, and leaves out the others the types declare, each for a
/// reason:
/// - `description`: duplicate information. It is Claude Code's summary of the call for its own
///   dialog, made of what the request already carries: a copy of the `description` in a Bash
///   call's input, or else a part of the input, like a Write's file, sometimes followed by
///   `decision_reason`. The block shows all of it from the input and the reason. Not to be
///   confused with the `description` in a Bash call's input, Claude's own words, which the
///   block shows after `#`.
/// - `title`: a heading for Claude Code's own dialog. Claude Code 2.1.322 sends it only for a
///   tool the person answers on a card of its own, like setting up Claude in Chrome, which only
///   Claude's apps offer, and for an ask a hook words itself, so no call claudio meets has one
///   yet. Should a tool claudio meets start sending one, show it.
/// - `matched_ask_rule`: the user's ask rule that matched too, when another check's reason is
///   the one given. The reason given is true, and the case is rare.
/// - `permission_suggestions` and `suppress_always_allow_rule`: the rules an option "allow, and
///   don't ask again" would save, and whether to leave that option out. claudio leaves the
///   option out on purpose, so each call is answered on its own.
/// - `classifier_approvable`: whether a program that approves calls by its own classifier may
///   approve this one. claudio approves nothing by itself.
/// - `requires_user_interaction`: whether a one-tap answer, as from a phone's notification, is
///   not enough, and the person must open the session. claudio always shows the whole call.
/// - `agent_id`: the subagent that asked, which claudio does not read here yet.
#[derive(Deserialize)]
struct CanUseTool {
    tool_name: String,
    input: Map<String, Value>,
    tool_use_id: String,
    /// Absent before Claude Code named tools for a person.
    display_name: Option<String>,
    mcp_server: Option<McpServer>,
    /// Ref: `SDKControlPermissionRequest` in `sdk.d.ts`, which marks it human-readable
    decision_reason: Option<String>,
    decision_reason_type: Option<String>,
    blocked_path: Option<String>,
    /// Ref: `SDKControlPermissionRequest` in `sdk.d.ts`: an ask that a stray key must not
    /// approve
    #[serde(default)]
    default_to_no: bool,
}

/// The MCP server a tool comes from, as `McpServerProvenance` in `sdk.d.ts` types it.
#[derive(Deserialize)]
struct McpServer {
    name: String,
}

#[derive(Deserialize)]
struct QuestionsInput {
    questions: Vec<Question>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract;

    fn message_start(parent_tool_use_id: &Value) -> String {
        json!({
            "type": "stream_event",
            "parent_tool_use_id": parent_tool_use_id,
            "event": {"type": "message_start", "message": {"usage": {
                "input_tokens": 2,
                "cache_creation_input_tokens": 12_141,
                "cache_read_input_tokens": 9_896,
                "cache_creation": {"ephemeral_5m_input_tokens": 0, "ephemeral_1h_input_tokens": 12_141},
            }}},
        })
        .to_string()
    }

    #[test]
    fn a_reply_of_claude_tells_the_context_and_the_cache_ttl() {
        let events = read_events(&message_start(&Value::Null)).expect("a readable line");
        let [Event::Call(usage)] = events.as_slice() else {
            panic!("a call");
        };
        assert_eq!(usage.tokens, 22_039);
        assert_eq!(usage.cache_ttl, Some(Duration::from_hours(1)));
    }

    #[test]
    fn a_change_of_the_background_tasks_tells_each_task() {
        let line = &contract::background_task("completed")[1];
        let events = read_events(line).expect("a readable line");
        let [Event::BackgroundTasks(tasks)] = events.as_slice() else {
            panic!("background tasks");
        };
        assert_eq!(tasks[0].description, "Run sleep 1");
    }

    #[test]
    fn only_a_move_to_the_background_tells_of_a_task_change() {
        let moved = &contract::moved_call()[3];
        let events = read_events(moved).expect("a readable line");
        assert!(matches!(
            events.as_slice(),
            [Event::TaskBackgrounded { .. }]
        ));
        let ended = json!({"type": "system", "subtype": "task_updated", "task_id": "t", "patch": {"status": "completed", "end_time": 1}});
        assert!(
            read_events(&ended.to_string())
                .expect("a readable line")
                .is_empty()
        );
    }

    #[test]
    fn a_task_tells_the_call_that_started_it_and_how_it_ended() {
        let lines = contract::background_task("completed");
        let started = read_events(&lines[2]).expect("a readable line");
        let [
            Event::TaskStarted {
                tool_use_id,
                is_backgrounded,
                ..
            },
        ] = started.as_slice()
        else {
            panic!("a task's start");
        };
        assert_eq!(tool_use_id.as_deref(), Some("toolu_background"));
        assert!(is_backgrounded);
        let ended = read_events(&lines[4]).expect("a readable line");
        let [Event::TaskEnded { status, .. }] = ended.as_slice() else {
            panic!("a task's end");
        };
        assert_eq!(*status, TaskStatus::Completed);
    }

    #[test]
    fn a_rate_limit_event_tells_how_much_of_each_limit_was_used() {
        let line = contract::rate_limit_event();
        let events = read_events(&line).expect("a readable line");
        let [Event::Limits { session, week }] = events.as_slice() else {
            panic!("limits");
        };
        assert_eq!(
            *session,
            Some(Limit {
                utilization: 0.39,
                is_reached: false,
                resets_at: Some(since_epoch(1_794_088_800)),
            })
        );
        assert_eq!(
            *week,
            Some(Limit {
                utilization: 1.0,
                is_reached: true,
                resets_at: Some(since_epoch(1_794_492_000)),
            })
        );
    }

    fn since_epoch(seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(seconds)
    }

    #[test]
    fn a_reply_of_a_subagent_tells_nothing_of_the_context() {
        let events = read_events(&message_start(&json!("toolu_1"))).expect("a readable line");
        assert!(events.is_empty());
    }

    fn paying_key(account: &Value) -> Option<String> {
        let events = read_events(&contract::initialize_response(account)).expect("a readable line");
        match events.as_slice() {
            [Event::Commands(_)] => None,
            [Event::PayingKey(key), Event::Commands(_)] => Some(key.clone()),
            _ => panic!("a paying key at most, then the commands"),
        }
    }

    #[test]
    fn the_subscription_pays_with_no_key() {
        assert_eq!(paying_key(&contract::subscription_account()), None);
    }

    #[test]
    fn an_api_key_pays() {
        let account = contract::api_key_account();
        assert_eq!(paying_key(&account).as_deref(), Some("ANTHROPIC_API_KEY"));
    }

    #[test]
    fn an_auth_token_pays() {
        let account = contract::auth_token_account();
        assert_eq!(
            paying_key(&account).as_deref(),
            Some("ANTHROPIC_AUTH_TOKEN")
        );
    }

    #[test]
    fn claude_code_tells_the_session() {
        let line = json!({
            "type": "system",
            "subtype": "init",
            "session_id": "a1",
            "claude_code_version": "2.1.322",
            "tools": [],
            "slash_commands": ["compact", "/model"],
        });
        let events = read_events(&line.to_string()).expect("a readable line");
        assert!(matches!(
            events.as_slice(),
            [Event::SessionId(id), Event::Version(version), Event::Commands(commands)]
                if id == "a1" && version == "2.1.322" && commands == &["compact", "model"]
        ));
    }

    /// A reply about the context's usage, from Claude Code that compacts the context at
    /// `auto_compact` tokens, none when it compacts only on request.
    fn context_response(api_usage: &Value, max_tokens: u64, auto_compact: Option<u64>) -> String {
        json!({
            "type": "control_response",
            "response": {"subtype": "success", "request_id": CONTEXT_REQUEST_ID, "response": {
                "totalTokens": 13_351,
                "maxTokens": max_tokens,
                "model": "claude-opus-5-5",
                "isAutoCompactEnabled": auto_compact.is_some(),
                "autoCompactThreshold": auto_compact,
                "apiUsage": api_usage,
            }},
        })
        .to_string()
    }

    fn usage_response(rate_limits: &Value) -> String {
        json!({
            "type": "control_response",
            "response": {"subtype": "success", "request_id": USAGE_REQUEST_ID, "response": {
                "subscription_type": "team",
                "rate_limits_available": !rate_limits.is_null(),
                "rate_limits": rate_limits,
            }},
        })
        .to_string()
    }

    #[test]
    fn the_plans_usage_tells_both_limits_with_when_they_reset() {
        let rate_limits = json!({
            "five_hour": {"utilization": 46, "resets_at": "2026-11-08T17:00:00.486618+00:00"},
            "seven_day": {"utilization": 100, "resets_at": "2026-11-11T16:00:00+01:00"},
        });
        let events = read_events(&usage_response(&rate_limits)).expect("a readable line");
        let [Event::Limits { session, week }] = events.as_slice() else {
            panic!("limits");
        };
        assert_eq!(
            *session,
            Some(Limit {
                utilization: 0.46,
                is_reached: false,
                resets_at: Some(since_epoch(1_794_157_200) + Duration::from_micros(486_618)),
            })
        );
        assert_eq!(
            *week,
            Some(Limit {
                utilization: 1.0,
                is_reached: true,
                resets_at: Some(since_epoch(1_794_409_200)),
            })
        );
    }

    #[test]
    fn a_session_without_the_plans_limits_tells_none() {
        let events = read_events(&usage_response(&Value::Null)).expect("a readable line");
        assert!(events.is_empty());
    }

    #[test]
    fn a_new_session_has_no_call_to_tell() {
        let line = context_response(&Value::Null, 1_000_000, Some(967_000));
        let events = read_events(&line).expect("a readable line");
        assert!(matches!(events.as_slice(), [Event::LastCall(None), ..]));
    }

    #[test]
    fn the_context_usage_tells_the_model_and_where_claude_code_compacts() {
        let line = context_response(&Value::Null, 1_000_000, Some(967_000));
        let events = read_events(&line).expect("a readable line");
        let [_, Event::Model(model), Event::Ceiling(ceiling)] = events.as_slice() else {
            panic!("the model and the ceiling");
        };
        assert_eq!(model, "claude-opus-5-5");
        assert_eq!(*ceiling, Ceiling::Compact(967_000));
    }

    #[test]
    fn a_context_compacted_only_on_request_grows_to_the_window() {
        let line = context_response(&Value::Null, 1_000_000, None);
        let events = read_events(&line).expect("a readable line");
        assert!(matches!(
            events.as_slice(),
            [.., Event::Ceiling(Ceiling::Window(1_000_000))]
        ));
    }

    #[test]
    fn a_cleared_context_tells_the_session_it_ends() {
        let line = &contract::two_clears()[0];
        let events = read_events(line).expect("a readable line");
        assert!(matches!(
            events.as_slice(),
            [Event::Reset { previous_session }] if previous_session == "first"
        ));
    }

    #[test]
    fn a_turn_tells_its_session_states() {
        let states: Vec<_> = contract::session_states_of_a_turn()
            .iter()
            .flat_map(|line| read_events(line).expect("a readable line"))
            .collect();
        assert!(matches!(
            states.as_slice(),
            [Event::State(State::Running), Event::State(State::Idle)]
        ));
    }

    #[test]
    fn a_new_session_tells_no_last_call() {
        let line = context_response(&Value::Null, 1_000_000, None);
        let events = read_events(&line).expect("a readable line");
        assert!(matches!(events.as_slice(), [Event::LastCall(None), ..]));
    }

    #[test]
    fn a_context_usage_without_its_last_call_is_incompatible() {
        let mut line: Value =
            serde_json::from_str(&context_response(&Value::Null, 1_000_000, None)).expect("JSON");
        line["response"]["response"]
            .as_object_mut()
            .expect("an object")
            .remove("apiUsage");
        let events = read_events(&line.to_string()).expect("a readable line");
        assert!(matches!(
            events.as_slice(),
            [Event::Incompatible(error)] if error.contains("apiUsage")
        ));
    }

    #[test]
    fn a_refused_context_usage_is_incompatible() {
        let line = json!({
            "type": "control_response",
            "response": {"subtype": "error", "request_id": CONTEXT_REQUEST_ID, "error": "unknown request"},
        })
        .to_string();
        let events = read_events(&line).expect("a readable line");
        assert!(matches!(events.as_slice(), [Event::Incompatible(_)]));
    }

    #[test]
    fn a_resumed_session_tells_its_last_call() {
        let api_usage = json!({
            "input_tokens": 2,
            "output_tokens": 5,
            "cache_creation_input_tokens": 12_141,
            "cache_read_input_tokens": 9_896,
        });
        let line = context_response(&api_usage, 1_000_000, Some(967_000));
        let events = read_events(&line).expect("a readable line");
        let [Event::LastCall(Some(usage)), ..] = events.as_slice() else {
            panic!("a resumed call");
        };
        assert_eq!(usage.tokens, 22_039);
    }

    #[test]
    fn an_api_error_tells_its_text() {
        let line = json!({
            "type": "assistant",
            "parent_tool_use_id": null,
            "error": "model_not_found",
            "message": {"content": [{"type": "text", "text": "There's an issue with the selected model"}]},
        })
        .to_string();
        let events = read_events(&line).expect("a readable line");
        let [Event::ApiError(text)] = events.as_slice() else {
            panic!("an API error");
        };
        assert_eq!(text, "There's an issue with the selected model");
    }

    #[test]
    fn an_exit_plan_mode_request_asks_to_approve_its_plan() {
        let request = serde_json::json!({
            "subtype": "can_use_tool",
            "tool_name": "ExitPlanMode",
            "input": {"plan": "# Plan", "planFilePath": "/plan.md"},
            "tool_use_id": "1",
        });
        let Ok(Event::Ask(ask)) = read_request("request-1".to_string(), request) else {
            panic!("an ask");
        };
        assert!(matches!(&ask.kind, AskKind::Plan(plan) if plan == "# Plan"));
    }

    #[test]
    fn a_permission_request_tells_why_it_asks_and_whether_one_key_may_allow_it() {
        let request = serde_json::json!({
            "subtype": "can_use_tool",
            "tool_name": "Bash",
            "input": {"command": "rm -rf build"},
            "tool_use_id": "1",
            "decision_reason": "\u{1b}[1mrm\u{1b}[0m removes files",
            "default_to_no": true,
        });
        let Ok(Event::Ask(ask)) = read_request("request-1".to_string(), request) else {
            panic!("an ask");
        };
        let AskKind::Permission(permission) = ask.kind else {
            panic!("a permission");
        };
        assert_eq!(permission.reason.as_deref(), Some("rm removes files"));
        assert!(permission.is_guarded);
    }

    #[test]
    fn a_block_of_text_ends_with_its_whole_text() {
        let line = json!({
            "type": "assistant",
            "message": {"model": "<synthetic>", "content": [{"type": "text", "text": "Usage"}]},
            "parent_tool_use_id": null,
        });
        let events = read_events(&line.to_string()).expect("a readable line");
        assert!(matches!(events.as_slice(), [Event::TextEnd(text)] if text == "Usage"));
    }

    #[test]
    fn a_compaction_tells_the_tokens_before_and_after() {
        let line = json!({
            "type": "system",
            "subtype": "compact_boundary",
            "compact_metadata": {"trigger": "manual", "pre_tokens": 19_812, "post_tokens": 6_578},
        });
        let events = read_events(&line.to_string()).expect("a readable line");
        assert!(matches!(
            events.as_slice(),
            [Event::Compacted {
                before: 19_812,
                after: Some(6_578)
            }]
        ));
    }

    #[test]
    fn a_permission_check_of_an_unknown_status_is_unreadable() {
        let line = json!({
            "type": "system",
            "subtype": "permission_check_status",
            "tool_use_id": "toolu_1",
            "status": "paused",
        });
        assert!(read_events(&line.to_string()).is_err());
    }

    #[test]
    fn a_subagent_permission_check_tells_nothing() {
        let line = json!({
            "type": "system",
            "subtype": "permission_check_status",
            "tool_use_id": "toolu_1",
            "agent_id": "agent_1",
            "status": "checking",
        });
        let events = read_events(&line.to_string()).expect("a readable line");
        assert!(events.is_empty());
    }

    #[test]
    fn a_failed_hook_tells_its_name_exit_code_and_error_output() {
        let line = json!({
            "type": "system",
            "subtype": "hook_response",
            "hook_id": "a",
            "hook_name": "Stop",
            "hook_event": "Stop",
            "output": "",
            "stdout": "",
            "stderr": "no tmux server",
            "exit_code": 1,
            "outcome": "error",
            "uuid": "b",
            "session_id": "c",
        });
        let events = read_events(&line.to_string()).expect("a readable line");
        assert!(matches!(
            &events[..],
            [Event::HookFailed { hook, exit_code: Some(1), stderr, is_cancelled: false }]
                if hook == "Stop" && stderr == "no tmux server"
        ));
    }

    #[test]
    fn a_kind_skipped_on_purpose_tells_nothing() {
        let lines = [
            json!({"type": "system", "subtype": "thinking_tokens", "estimated_tokens": 5}),
            json!({"type": "system", "subtype": "session_title_changed", "title": "A title"}),
            json!({"type": "system", "subtype": "commands_changed", "commands": [{"name": "init"}]}),
            json!({"type": "system", "subtype": "hook_started", "hook_event": "Stop"}),
            json!({"type": "system", "subtype": "hook_progress", "hook_event": "Stop"}),
            json!({
                "type": "system",
                "subtype": "hook_response",
                "hook_name": "Stop",
                "outcome": "success",
                "exit_code": 0,
                "stderr": "",
            }),
            json!({"type": "system", "subtype": "permission_denied", "tool_name": "Bash"}),
            json!({"type": "tool_progress", "tool_name": "Bash", "elapsed_time_seconds": 3}),
        ];
        for line in lines {
            let events = read_events(&line.to_string()).expect("a readable line");
            assert!(events.is_empty(), "{line}");
        }
    }

    #[test]
    fn a_compaction_shows_once_and_nothing_else_of_it_does() {
        // As Claude Code 2.1.322 sends a /compact, its init and result left out
        let lines = [
            json!({"type": "system", "subtype": "status", "status": "compacting"}),
            json!({"type": "system", "subtype": "status", "status": null, "compact_result": "success"}),
            json!({
                "type": "system",
                "subtype": "compact_boundary",
                "compact_metadata": {"trigger": "manual", "pre_tokens": 19_670, "post_tokens": 6_399},
            }),
            json!({
                "type": "user",
                "message": {"role": "user", "content": "This session is being continued from a previous conversation."},
                "parent_tool_use_id": null,
                "isSynthetic": true,
            }),
            json!({
                "type": "user",
                "message": {"role": "user", "content": "<local-command-stdout>Compacted </local-command-stdout>"},
                "parent_tool_use_id": null,
                "isReplay": true,
            }),
        ];
        let events: Vec<Event> = lines
            .iter()
            .flat_map(|line| read_events(&line.to_string()).expect("a readable line"))
            .collect();
        assert!(matches!(
            events.as_slice(),
            [Event::Compacted {
                before: 19_670,
                after: Some(6_399)
            }]
        ));
    }

    #[test]
    fn a_replay_tells_nothing() {
        let line = json!({
            "type": "user",
            "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "1", "content": "a"}]},
            "parent_tool_use_id": null,
            "isReplay": true,
        });
        let events = read_events(&line.to_string()).expect("a readable line");
        assert!(events.is_empty());
    }

    #[test]
    fn an_unknown_kind_is_named_at_any_depth() {
        let cases = [
            (json!({"type": "auth_status"}), "message", "auth_status"),
            (
                json!({"type": "system", "subtype": "api_retry"}),
                "system message",
                "api_retry",
            ),
            (
                json!({
                    "type": "stream_event",
                    "event": {"type": "content_block_delta", "delta": {"type": "citations_delta"}},
                    "parent_tool_use_id": null,
                }),
                "streamed piece",
                "citations_delta",
            ),
            (
                json!({
                    "type": "assistant",
                    "message": {"content": [{"type": "text", "text": "a"}, {"type": "redacted_thinking"}]},
                    "parent_tool_use_id": null,
                }),
                "content block",
                "redacted_thinking",
            ),
            (
                json!({
                    "type": "user",
                    "message": {"content": [{
                        "type": "tool_result",
                        "tool_use_id": "1",
                        "content": [{"type": "text", "text": "a"}, {"type": "document"}],
                    }]},
                    "parent_tool_use_id": null,
                }),
                "tool result block",
                "document",
            ),
        ];
        for (line, expected_part, expected_name) in cases {
            let events = read_events(&line.to_string()).expect("a readable line");
            assert!(
                events.iter().any(|event| matches!(
                    event,
                    Event::Unknown { part, name, .. } if *part == expected_part && name == expected_name
                )),
                "{line}"
            );
        }
    }

    #[test]
    fn a_known_kind_with_a_broken_field_is_unreadable() {
        let line = json!({"type": "system", "subtype": "background_tasks_changed"});
        assert!(read_events(&line.to_string()).is_err());
    }

    #[test]
    fn an_unknown_limit_status_is_unreadable() {
        let line = json!({
            "type": "rate_limit_event",
            "rate_limit_info": {"unifiedWindows": {"five_hour": {"utilization": 0.5, "status": "paused"}}},
        });
        assert!(read_events(&line.to_string()).is_err());
    }

    #[test]
    fn a_command_runs_without_the_mark_of_the_client() {
        let line: Value =
            serde_json::from_str(&Command::Run { command: "compact" }.to_line()).expect("JSON");
        assert_eq!(line["message"]["content"], "/compact");
        assert!(line.get("client_composed").is_none());
    }
}
