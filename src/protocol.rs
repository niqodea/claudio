//! Claude Code's side of the conversation, as `claude -p` carries it in its stream-json format:
//! the events read from what it writes, and the commands written back.
//! Ref: sdk.d.ts in the npm package @anthropic-ai/claude-agent-sdk, which types every message
//! Claude Code documents. Anywhere in the client, what relies on more than those types says so in
//! a comment that starts with "Undocumented", followed by how it was learned.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde::de::IgnoredAny;
use serde_json::{Map, Value, json};

/// The ids of the requests sent before the first message
const INITIALIZE_REQUEST_ID: &str = "initialize";
const SETTINGS_REQUEST_ID: &str = "settings";
const CONTEXT_REQUEST_ID: &str = "context";
const USAGE_REQUEST_ID: &str = "usage";

/// Something Claude Code reported.
pub enum Event {
    /// A piece of the reply Claude is writing.
    Text(String),
    /// The end of a block of the reply, whose pieces arrived as `Text`.
    TextEnd,
    /// A call to the API that failed, with Claude Code's words for it, in place of a reply.
    ApiError(String),
    ToolUse(ToolUse),
    ToolResult(ToolResult),
    /// A tool that waits for the user's answer before it runs.
    Ask(Ask),
    State(State),
    /// What each of the tasks that run in the background does, in the order they started.
    BackgroundTasks(Vec<String>),
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

/// Whether Claude works, waits for a message, or waits for the answer to an ask.
#[derive(Clone, Copy, Deserialize)]
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
    /// Whether a subagent of Claude makes the call, rather than Claude itself.
    pub is_subagent: bool,
}

pub struct ToolResult {
    pub tool_use_id: String,
    /// None when the result holds content but no text, like references to tools.
    pub text: Option<String>,
    pub is_error: bool,
    pub is_subagent: bool,
}

pub struct Ask {
    request_id: String,
    pub tool_use_id: String,
    pub tool_name: String,
    pub input: Map<String, Value>,
    pub kind: AskKind,
}

pub enum AskKind {
    /// Whether the tool may run, which is refused, since only auto mode is supported for now.
    /// TODO: Ask the user to approve the plan of an `ExitPlanMode` call, which Claude Code asks
    /// about once claudio supports plan mode
    Permission,
    /// The questions of an `AskUserQuestion` call, which runs once all of them are answered.
    Questions(Vec<Question>),
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
}

pub enum Command<'a> {
    Message {
        id: &'a str,
        text: &'a str,
    },
    /// Clears the context, as Claude Code's `/clear` does, which starts a new session. Claude
    /// Code runs the command right after a turn it was told to stop.
    Clear,
    Answer(&'a Ask, Answer),
    Interrupt {
        request_id: String,
    },
    /// Has Claude Code make way for the message with this id, which it already holds, as the
    /// send-now key of Claude Code's terminal does: it moves what the running turn waits on to
    /// the background where it can, and stops the turn otherwise. It does nothing when no turn
    /// runs or the running turn already took the message in, so it never stops the turn that
    /// answers the message.
    ///
    /// Undocumented, from Claude Code's changelog: its fields are Claude Code's own, absent from
    /// the SDK's types. Claude Code announces them
    /// as `interrupt_send_now_v1` when it starts, and one without them takes the request as a
    /// plain interrupt.
    /// Ref: <https://code.claude.com/docs/en/changelog>, on send now moving running tools to the
    /// background
    SendNow {
        request_id: String,
        message_id: &'a str,
    },
    /// Starts the session. Claude Code answers with the account that pays for it.
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
            Command::Clear => json!({
                "type": "user",
                "session_id": "",
                "message": {"role": "user", "content": "/clear"},
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
                };
                json!({
                    "type": "control_response",
                    "response": {"subtype": "success", "request_id": ask.request_id, "response": result},
                })
            }
            Command::Interrupt { request_id } => json!({
                "type": "control_request",
                "request_id": request_id,
                "request": {"subtype": "interrupt"},
            }),
            Command::SendNow {
                request_id,
                message_id,
            } => json!({
                "type": "control_request",
                "request_id": request_id,
                "request": {"subtype": "interrupt", "send_now": true, "message_uuid": message_id},
            }),
            Command::Initialize => json!({
                "type": "control_request",
                "request_id": INITIALIZE_REQUEST_ID,
                "request": {"subtype": "initialize"},
            }),
            Command::GetSettings => json!({
                "type": "control_request",
                "request_id": SETTINGS_REQUEST_ID,
                "request": {"subtype": "get_settings"},
            }),
            Command::GetContext => json!({
                "type": "control_request",
                "request_id": CONTEXT_REQUEST_ID,
                "request": {"subtype": "get_context_usage", "detail": "summary"},
            }),
            // Without the scan of the week's transcripts, which tells nothing of the limits
            Command::GetUsage => json!({
                "type": "control_request",
                "request_id": USAGE_REQUEST_ID,
                "request": {"subtype": "get_usage", "skip_behaviors": true},
            }),
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

/// Reads the events in one line Claude Code wrote, none for a message that reports nothing a
/// person reads or answers.
pub fn read_events(line: &str) -> Result<Vec<Event>, String> {
    let message: Message = serde_json::from_str(line)
        .map_err(|error| format!("Unreadable message from Claude Code: {error}: {line}"))?;
    let events = match message {
        Message::StreamEvent {
            event:
                StreamEvent::ContentBlockDelta {
                    delta: Delta::TextDelta { text },
                },
            parent_tool_use_id: None,
        } => vec![Event::Text(text)],
        // A subagent calls the API with a context and cache of its own
        Message::StreamEvent {
            event: StreamEvent::MessageStart { message },
            parent_tool_use_id: None,
        } => vec![Event::Call(message.usage.into())],
        // Claude Code sends an API error as a reply of its own making, whole and never streamed
        Message::Assistant {
            message,
            parent_tool_use_id: None,
            error: Some(_),
        } => vec![Event::ApiError(message.content.into_text())],
        // Claude Code sends each block whole once it has streamed it, so the text of a block
        // already arrived piece by piece
        Message::Assistant {
            message,
            parent_tool_use_id,
            error: _,
        } => message
            .content
            .into_blocks()
            .into_iter()
            .filter_map(|block| match block {
                Block::Text { .. } if parent_tool_use_id.is_none() => Some(Event::TextEnd),
                Block::ToolUse { id, name, input } => Some(Event::ToolUse(ToolUse {
                    id,
                    name,
                    input,
                    is_subagent: parent_tool_use_id.is_some(),
                })),
                Block::Text { .. } | Block::ToolResult { .. } | Block::Other => None,
            })
            .collect(),
        Message::User {
            message,
            parent_tool_use_id,
        } => message
            .content
            .into_blocks()
            .into_iter()
            .filter_map(|block| match block {
                Block::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => Some(Event::ToolResult(ToolResult {
                    tool_use_id,
                    text: content.map_or(Some(String::new()), ResultContent::into_text),
                    is_error,
                    is_subagent: parent_tool_use_id.is_some(),
                })),
                Block::Text { .. } | Block::ToolUse { .. } | Block::Other => None,
            })
            .collect(),
        Message::ControlRequest {
            request_id,
            request,
        } => vec![read_request(request_id, request)?],
        Message::ControlResponse {
            response:
                Reply::Success {
                    request_id,
                    response,
                },
        } => read_response(&request_id, response)?,
        // The context's usage tells whether the cache may be cold, and claudio has nothing to
        // tell it by otherwise
        Message::ControlResponse {
            response: Reply::Error { request_id, error },
        } if request_id == CONTEXT_REQUEST_ID => vec![Event::Incompatible(format!(
            "Claude Code refused the context's usage: {error}"
        ))],
        Message::ControlResponse {
            response: Reply::Error { request_id, error },
        } => return Err(format!("Claude Code refused request {request_id}: {error}")),
        Message::Result { subtype } if subtype == "success" => {
            vec![Event::TurnEnd { failure: None }]
        }
        Message::Result { subtype } => vec![Event::TurnEnd {
            failure: Some(subtype),
        }],
        Message::RateLimitEvent { rate_limit_info } => vec![read_limits(rate_limit_info)],
        Message::System(System::Init { session_id }) => vec![Event::SessionId(session_id)],
        Message::ConversationReset { session_id } => vec![Event::Reset {
            previous_session: session_id,
        }],
        Message::System(System::SessionStateChanged { state }) => vec![Event::State(state)],
        Message::System(System::BackgroundTasksChanged { tasks }) => vec![Event::BackgroundTasks(
            tasks.into_iter().map(|task| task.description).collect(),
        )],
        Message::StreamEvent { .. } | Message::System(System::Other) | Message::Other => vec![],
    };
    Ok(events)
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
fn read_response(request_id: &str, response: Option<Value>) -> Result<Vec<Event>, String> {
    match request_id {
        INITIALIZE_REQUEST_ID => read_account(response),
        SETTINGS_REQUEST_ID => read_settings(response),
        CONTEXT_REQUEST_ID => {
            Ok(read_context_usage(response)
                .unwrap_or_else(|error| vec![Event::Incompatible(error)]))
        }
        USAGE_REQUEST_ID => read_plan_usage(response),
        // The other requests sent are interrupts, whose success needs no word
        _ => Ok(vec![]),
    }
}

/// The key that pays for the session, none when the subscription does.
fn read_account(response: Option<Value>) -> Result<Vec<Event>, String> {
    let initialize: Initialize = serde_json::from_value(response.unwrap_or_default())
        .map_err(|error| format!("Unreadable account from Claude Code: {error}"))?;
    let account = initialize.account;
    // Undocumented, seen with Claude Code 2.1.319: ANTHROPIC_API_KEY fills apiKeySource with its
    // name, ANTHROPIC_AUTH_TOKEN fills tokenSource with its name, and the subscription's login
    // leaves apiKeySource out and tokenSource out or claude.ai
    let key = account.api_key_source.or(account
        .token_source
        .filter(|source| source == "ANTHROPIC_AUTH_TOKEN"));
    Ok(key.map(Event::PayingKey).into_iter().collect())
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
    let kind = if request.tool_name == "AskUserQuestion" {
        let input: QuestionsInput = serde_json::from_value(Value::Object(request.input.clone()))
            .map_err(|error| format!("Unreadable questions from Claude: {error}"))?;
        AskKind::Questions(input.questions)
    } else {
        AskKind::Permission
    };
    Ok(Event::Ask(Ask {
        request_id,
        tool_use_id: request.tool_use_id,
        tool_name: request.tool_name,
        input: request.input,
        kind,
    }))
}

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
        /// The kind of API error the message reports, on a message Claude Code makes for one.
        error: Option<String>,
    },
    User {
        message: Body,
        parent_tool_use_id: Option<String>,
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
    /// Undocumented, seen with Claude Code 2.1.322: `/clear` sends this, then the new
    /// session's init, then a result that ends a turn of no calls. Its `session_id` is the
    /// cleared session's.
    ConversationReset {
        session_id: String,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RateLimitInfo {
    // Undocumented, read in the schema of Claude Code 2.1.316, which marks it internal. Only this
    // field has every limit: the documented fields describe only the limit that applies now
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

#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum WindowStatus {
    Rejected,
    #[serde(other)]
    Other,
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

#[derive(Deserialize)]
#[serde(tag = "subtype", rename_all = "snake_case")]
enum System {
    /// Carries the id of the session Claude Code runs.
    Init {
        session_id: String,
    },
    // Undocumented, read in Claude Code 2.1.316 and seen with it: sent only when
    // CLAUDE_CODE_SDK_READS_SESSION_STATE is set, which the SDK's types do not name, though they
    // type the message
    SessionStateChanged {
        state: State,
    },
    /// Sent each time a task starts or ends, with every task that still runs.
    BackgroundTasksChanged {
        tasks: Vec<BackgroundTask>,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct BackgroundTask {
    description: String,
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
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Delta {
    TextDelta {
        text: String,
    },
    #[serde(other)]
    Other,
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
                Block::ToolUse { .. } | Block::ToolResult { .. } | Block::Other => None,
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
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ResultContent {
    Text(String),
    Blocks(Vec<ResultBlock>),
}

impl ResultContent {
    /// The content's text, none when it holds blocks but none of them text.
    fn into_text(self) -> Option<String> {
        match self {
            ResultContent::Text(text) => Some(text),
            ResultContent::Blocks(blocks) => {
                let has_blocks = !blocks.is_empty();
                let texts: Vec<String> = blocks
                    .into_iter()
                    .filter_map(|block| match block {
                        ResultBlock::Text { text } => Some(text),
                        ResultBlock::Other => None,
                    })
                    .collect();
                if has_blocks && texts.is_empty() {
                    None
                } else {
                    Some(texts.join("\n"))
                }
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
    #[serde(other)]
    Other,
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

#[derive(Deserialize)]
struct CanUseTool {
    tool_name: String,
    input: Map<String, Value>,
    tool_use_id: String,
}

#[derive(Deserialize)]
struct QuestionsInput {
    questions: Vec<Question>,
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn a_change_of_the_background_tasks_tells_what_each_one_does() {
        // As Claude Code 2.1.316 sent it
        let line = json!({
            "type": "system",
            "subtype": "background_tasks_changed",
            "tasks": [{
                "task_id": "bbaaokkwa",
                "run_id": "0mw6q3kby-4c564330",
                "task_type": "local_bash",
                "description": "Sleep 20 seconds in background",
            }],
        })
        .to_string();
        let events = read_events(&line).expect("a readable line");
        let [Event::BackgroundTasks(tasks)] = events.as_slice() else {
            panic!("background tasks");
        };
        assert_eq!(tasks, &["Sleep 20 seconds in background"]);
    }

    #[test]
    fn a_rate_limit_event_tells_how_much_of_each_limit_was_used() {
        // As Claude Code 2.1.316 sent it
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
        })
        .to_string();
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

    fn initialize_response(account: &Value) -> String {
        json!({
            "type": "control_response",
            "response": {"subtype": "success", "request_id": INITIALIZE_REQUEST_ID, "response": {
                "commands": [],
                "account": account,
            }},
        })
        .to_string()
    }

    fn paying_key(account: &Value) -> Option<String> {
        let events = read_events(&initialize_response(account)).expect("a readable line");
        match events.as_slice() {
            [] => None,
            [Event::PayingKey(key)] => Some(key.clone()),
            _ => panic!("a paying key at most"),
        }
    }

    #[test]
    fn the_subscription_pays_with_no_key() {
        assert_eq!(
            paying_key(&json!({"subscriptionType": "Claude Team"})),
            None
        );
        assert_eq!(paying_key(&json!({"tokenSource": "claude.ai"})), None);
    }

    #[test]
    fn an_api_key_pays() {
        let account = json!({"tokenSource": "claude.ai", "apiKeySource": "ANTHROPIC_API_KEY"});
        assert_eq!(paying_key(&account).as_deref(), Some("ANTHROPIC_API_KEY"));
    }

    #[test]
    fn an_auth_token_pays() {
        let account = json!({"tokenSource": "ANTHROPIC_AUTH_TOKEN"});
        assert_eq!(
            paying_key(&account).as_deref(),
            Some("ANTHROPIC_AUTH_TOKEN")
        );
    }

    #[test]
    fn claude_code_tells_the_session() {
        let line = json!({"type": "system", "subtype": "init", "session_id": "a1", "tools": []});
        let events = read_events(&line.to_string()).expect("a readable line");
        assert!(matches!(events.as_slice(), [Event::SessionId(id)] if id == "a1"));
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
        let line = json!({
            "type": "conversation_reset",
            "new_conversation_id": "c2",
            "trigger": "clear",
            "session_id": "a1",
        })
        .to_string();
        let events = read_events(&line).expect("a readable line");
        assert!(matches!(
            events.as_slice(),
            [Event::Reset { previous_session }] if previous_session == "a1"
        ));
    }

    #[test]
    fn a_clear_runs_as_a_command() {
        let line: Value = serde_json::from_str(&Command::Clear.to_line()).expect("JSON");
        assert_eq!(line["message"]["content"], "/clear");
        assert!(line.get("client_composed").is_none());
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
    fn an_exit_plan_mode_request_asks_for_permission() {
        let request = serde_json::json!({
            "subtype": "can_use_tool",
            "tool_name": "ExitPlanMode",
            "input": {"plan": "# Plan", "planFilePath": "/plan.md"},
            "tool_use_id": "1",
        });
        let Ok(Event::Ask(ask)) = read_request("request-1".to_string(), request) else {
            panic!("an ask");
        };
        assert!(matches!(ask.kind, AskKind::Permission));
    }
}
