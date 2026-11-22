//! Claude Code in plain text: its replies are appended to the terminal, which wraps them, above
//! an input line that stays open for each message, and for each answer when a tool asks for
//! one.
//!
//! Arguments are passed on to `claude`, like `--resume <session>`, except for `claudio
//! install-claude-code`, which installs the Claude Code that claudio runs.
//!
//! The `claude -p` the client starts is all it knows of Claude Code: what it writes, its error
//! output and its exit. The client reads nothing else of Claude Code's, like its transcript
//! files, which keeps the client small and its assumptions on how Claude Code works few.

mod claude_code;
mod claude_input;
mod climb;
mod code;
mod config;
mod context;
pub mod contract;
mod cut;
mod fake_api;
mod input_line;
mod keys;
mod label;
mod limits;
mod markdown;
mod picker;
mod protocol;
mod questions;
mod render;
mod session;
mod status;
mod style;
mod table;
mod terminal;
mod text;
mod vi;
mod wording;
mod work;
mod wrap;

use std::env;
use std::io::{BufRead, BufReader};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{self, ChildStderr, ChildStdout, ExitCode, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Instant, SystemTime};

use claude_input::ClaudeInput;
use config::{Config, ProseWrap};
use context::Context;
use input_line::InputLine;
use keys::{Phase, Writing};
use limits::{Limits, Source};
use protocol::{Event, read_events, repeats_a_message};
use render::Output;
use session::{Conversation, Ending, Input};
use terminal::QuietTerminal;
use text::Wrap;
use wording::SessionEnd;
use work::Work;

/// What the threads share, behind one lock, so that no lock waits on another.
struct Shared {
    /// What the user does on the input line, which its label shows.
    phase: Phase,
    output: Output,
    context: Context,
    limits: Limits,
    /// The session's id, once Claude Code tells it.
    session_id: Option<String>,
    /// What only the status shows of how the session runs and of Claude Code.
    facts: status::Facts,
    /// The version of Claude Code, once it tells it.
    claude_code_version: Option<String>,
    /// The names of the commands the session runs, in order, which the command line completes.
    commands: Vec<String>,
    /// The calls, tasks and subagents of the session, which `output` draws.
    work: Work,
    /// Whether a clear waits for the tasks that run in the background to end, which only
    /// `hold_clear` and `note_background_tasks` change.
    is_clear_waiting: bool,
}

impl Shared {
    /// Notes Claude Code's version, and says when it is not the one checked to work. Each
    /// session tells it, which a clear starts anew, so only the first counts.
    fn note_version(&mut self, version: String) {
        if self.claude_code_version.is_some() {
            return;
        }
        if version != claude_code::VERSION {
            self.output.show_unchecked_version(&version);
        }
        self.claude_code_version = Some(version);
    }

    /// Holds a clear while tasks run in the background, so their ends, and Claude's turns on
    /// them, stay in the session that started them, and gives whether the clear goes now, with
    /// none running. The tasks are noted under the same lock, so the end of the last one releases
    /// the clear, as `note_background_tasks` tells.
    ///
    /// Undocumented: [`crate::contract::a_clear_follows_the_turn_on_the_last_tasks_end`]
    fn hold_clear(&mut self) -> bool {
        if self.work.is_quiet() {
            return true;
        }
        self.is_clear_waiting = true;
        false
    }

    /// Notes the tasks that run in the background, and gives whether the clear held for them
    /// goes now, with none left.
    fn note_background_tasks(&mut self, tasks: Vec<protocol::Task>) -> bool {
        self.work.note_listed(tasks);
        let releases = self.is_clear_waiting && self.work.is_quiet();
        self.is_clear_waiting &= !releases;
        releases
    }

    /// `id`, the session's, unless the session is new and had no call, so it holds nothing to
    /// resume, as the status says with `none yet`.
    fn resumable<'a>(&self, id: Option<&'a str>) -> Option<&'a str> {
        id.filter(|_| !self.context.is_new())
    }

    /// Shows what `event` tells, notes what it changes of the work and of the session, and hands
    /// the main thread with `forward` what it has to act on.
    fn note(&mut self, event: Event, forward: &dyn Fn(Input)) {
        match event {
            Event::Text(text) => self.output.show_text(&text),
            Event::TextEnd(text) => self.output.show_text_end(&text),
            Event::Narration(text) => self.output.show_narration(&text),
            Event::Compacted { before, after } => self.output.show_compacted(before, after),
            Event::Unknown { part, name, line } => {
                self.output.show_unknown(part, &name, &line);
            }
            Event::ApiError(text) => self.output.show_api_error(&text),
            Event::HookFailed {
                hook,
                exit_code,
                stderr,
                is_cancelled,
            } => self
                .output
                .show_hook_failure(&hook, exit_code, &stderr, is_cancelled),
            Event::Notice { text, level } => self.output.show_claude_code_notice(&text, level),
            Event::ToolUse(tool_use) => self.output.show_tool_use(&mut self.work, &tool_use),
            Event::ToolResult(result) => {
                self.output.show_tool_result(&mut self.work, &result);
            }
            Event::TurnEnd { failure } => {
                self.output.show_turn_end(failure.as_deref());
                forward(Input::TurnEnded);
            }
            Event::SettingsErrors(errors) => self.output.show_settings_errors(&errors),
            Event::PayingKey(key) => {
                self.output.show_paying_key(&key);
                self.facts.paying_key = Some(key);
            }
            Event::Plan(plan) => self.facts.plan = Some(plan),
            Event::FastMode(state) => self.facts.fast_mode = Some(state),
            Event::PermissionMode(mode) => self.facts.permission_mode = Some(mode),
            Event::Effort(effort) => self.facts.effort = Some(effort),
            Event::McpServers(servers) => self.facts.mcp_servers = Some(servers),
            Event::SessionId(id) => self.session_id = Some(id),
            Event::Version(version) => self.note_version(version),
            Event::Commands(commands) => self.note_commands(commands),
            Event::Reset { previous_session } => {
                self.work.start_session();
                let resume = self.resumable(Some(&previous_session));
                self.output.show_session_end(SessionEnd::Cleared, resume);
                self.context.reset();
            }
            Event::State(state) => self.output.note_state(state),
            Event::PermissionCheck {
                tool_use_id,
                is_checking,
            } => self.output.note_permission_check(&tool_use_id, is_checking),
            Event::Limits { session, week } => {
                let arrived_at = SystemTime::now();
                self.limits
                    .note(session, week, Source::Reply { arrived_at });
            }
            Event::BackgroundTasks(tasks) => {
                if self.note_background_tasks(tasks) {
                    forward(Input::HeldClear);
                }
            }
            Event::TaskStarted {
                task_id,
                tool_use_id,
                kind,
                place,
                description,
            } => (self.work).start_task(task_id, tool_use_id.as_deref(), kind, place, &description),
            Event::TaskBackgrounded { task_id } => {
                self.work.move_to_background(&task_id);
            }
            Event::TaskEnded {
                task_id,
                status,
                summary,
            } => (self.output).show_task_end(&mut self.work, &task_id, status, &summary),
            Event::Call(usage) => {
                self.context.note_call(&usage, Instant::now());
            }
            Event::LastCall(usage) => self.context.note_last_call(usage.as_ref()),
            Event::CountedTokens(tokens) => self.context.note_counted(tokens),
            Event::Model(model) => self.facts.model = Some(model),
            Event::Ceiling(ceiling) => self.context.note_ceiling(ceiling),
            Event::Ask(ask) => forward(Input::Ask(ask)),
            Event::Incompatible(error) => forward(Input::Incompatible(error)),
            Event::Unsupported {
                request_id,
                subtype,
            } => forward(Input::Unsupported {
                request_id,
                subtype,
            }),
        }
    }

    /// Notes the commands the session runs, in order, for the command line to complete.
    fn note_commands(&mut self, mut commands: Vec<String>) {
        commands.sort();
        self.commands = commands;
    }
}

/// Runs claudio, with the arguments it was started with, and ends with the exit code it ends
/// with.
#[must_use]
pub fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ExitCode, String> {
    let mut args = env::args().skip(1).peekable();
    if args.peek().map(String::as_str) == Some(claude_code::INSTALL_COMMAND) {
        if args.nth(1).is_some() {
            return Err(format!(
                "claudio {} takes no arguments",
                claude_code::INSTALL_COMMAND
            ));
        }
        claude_code::install()?;
        return Ok(ExitCode::SUCCESS);
    }
    if args.peek().map(String::as_str) == Some(CHECK_COMMAND) {
        let is_paid = match args.nth(1).as_deref() {
            None => false,
            Some("--paid") if args.next().is_none() => true,
            Some(_) => return Err(format!("usage: claudio {CHECK_COMMAND} [--paid]")),
        };
        return check_claude_code(is_paid);
    }
    let program = claude_code::program()?;
    let config = Config::read()?;
    let quiet_terminal = QuietTerminal::new()?;
    let prose_wrap = terminal::prose_wrap(config.prose_wrap)?;
    // vi when EDITOR is unset or empty, since POSIX requires a vi on every system it covers
    let editor_command = env::var("EDITOR")
        .ok()
        .filter(|editor| !editor.is_empty())
        .unwrap_or_else(|| "vi".to_string());
    let mut claude = claude_code::command(&program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        // Its error output reaches the terminal through the output, above the input line
        .stderr(Stdio::piped())
        // Keys reach only this process, which turns Ctrl+C into an interrupt of the turn
        .process_group(0)
        .spawn()
        .map_err(|error| format!("cannot start {}: {error}", program.display()))?;
    let to_claude = claude.stdin.take().expect("stdin is piped");
    let from_claude = claude.stdout.take().expect("stdout is piped");
    let errors_from_claude = claude.stderr.take().expect("stderr is piped");

    // Removed when the client ends. A signal that ends the client, like SIGHUP when its terminal
    // closes, leaves it in the temporary directory
    let full_view = tempfile::Builder::new()
        .prefix("claudio-")
        .tempfile()
        .map_err(|error| format!("cannot create the file of the full view: {error}"))?;
    let shared = Arc::new(Mutex::new(Shared {
        phase: Phase::Writing(Writing { waiting: 0 }),
        output: Output::new(
            full_view
                .reopen()
                .map_err(|error| format!("cannot open the file of the full view: {error}"))?,
            config.max_noisy_line_columns,
            config.max_table_columns,
            prose_wrap,
        ),
        context: Context::new(),
        limits: Limits::new(),
        session_id: None,
        facts: status::Facts::default(),
        claude_code_version: None,
        commands: Vec::new(),
        work: Work::default(),
        is_clear_waiting: false,
    }));
    let break_signal = Arc::new(AtomicBool::new(false));
    let (input_sender, inputs) = mpsc::channel();

    let reader_shared = Arc::clone(&shared);
    let reader_break_signal = Arc::clone(&break_signal);
    let reader = thread::spawn(move || {
        read_claude(from_claude, &reader_shared, &|input| {
            // The main thread stops reading only on its way out
            let _ = input_sender.send(input);
            reader_break_signal.store(true, Ordering::Relaxed);
        });
    });
    let errors_shared = Arc::clone(&shared);
    let errors_reader = thread::spawn(move || {
        read_claude_errors(errors_from_claude, &errors_shared);
    });

    if config.prose_wrap == ProseWrap::AskSpaces && prose_wrap == Wrap::Edge {
        lock(&shared).output.show_no_wrap_at_spaces();
    }
    let to_claude = ClaudeInput::new(to_claude);
    let input = InputLine::new(
        editor_command,
        full_view.path().to_path_buf(),
        quiet_terminal.original(),
        &config,
        Arc::clone(&break_signal),
        Arc::clone(&shared),
        to_claude.clone(),
    )?;
    let conversation = Conversation::new(
        input,
        Arc::clone(&shared),
        inputs,
        break_signal,
        to_claude,
        &claude,
    );
    let result = conversation
        .run()
        .and_then(|ending| ending.map_or(Ok(()), Ending::run));
    end(claude, [reader, errors_reader], &shared, result)
}

/// The subcommand that checks what claudio assumes of Claude Code.
const CHECK_COMMAND: &str = "check-claude-code";

/// Runs the checks of what claudio assumes of Claude Code against the Claude Code it runs, the
/// paid ones only when `is_paid`, and fails when one fails.
fn check_claude_code(is_paid: bool) -> Result<ExitCode, String> {
    let program = claude_code::program()?;
    println!("Checking {}", program.display());
    if is_paid {
        println!(
            "The paid checks call the API with {}, a few cents in all",
            contract::MODEL
        );
    }
    let mut failed = 0;
    let width = (contract::CHECKS.iter())
        .map(|check| check.setup.name().len())
        .max()
        .unwrap_or(0);
    for check in &contract::CHECKS {
        let setup = format!("{:width$}", check.setup.name());
        if check.setup.is_paid() && !is_paid {
            println!("skip  {setup}  {} (--paid runs it)", check.name);
            continue;
        }
        match check.setup.run(&program) {
            Ok(()) => println!("ok    {setup}  {}", check.name),
            Err(error) => {
                failed += 1;
                println!("FAIL  {setup}  {}: {error}", check.name);
            }
        }
    }
    Ok(if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// Ends the session the user or Claude Code ended, or that failed with `result`'s error, and
/// shows the rest of the output: all Claude Code wrote, how it ended, and the final line. The
/// session is over, so Claude Code's input closed, and Claude Code exits once its work is done.
fn end(
    mut claude: process::Child,
    readers: [JoinHandle<()>; 2],
    shared: &Mutex<Shared>,
    result: Result<(), String>,
) -> Result<ExitCode, String> {
    let status = claude
        .wait()
        .map_err(|error| format!("cannot wait for claude: {error}"))?;
    join(readers);
    let shared = &mut lock(shared);
    let session = (shared.resumable(shared.session_id.as_deref())).map(str::to_string);
    let output = &mut shared.output;
    let code = match result {
        Ok(()) => exit_code(status),
        Err(error) => {
            output.show_error(&error);
            ExitCode::FAILURE
        }
    };
    output.show_end(describe_exit(status).as_deref());
    output.show_session_end(SessionEnd::Ended, session.as_deref());
    // The text ends with a line break, since the end ended every line
    print!("{}", output.take_text().to_terminal("\n"));
    Ok(code)
}

/// Waits for the readers of Claude Code's output, which end as it exits, once they took every
/// line it wrote. A reader that panicked already said why.
///
/// Undocumented: [`crate::contract::the_output_closes_as_claude_code_exits`]
fn join(readers: [JoinHandle<()>; 2]) {
    for reader in readers {
        let _ = reader.join();
    }
}

/// How Claude Code exited, none when with status 0.
fn describe_exit(status: ExitStatus) -> Option<String> {
    // Like "exit status: 1", or "signal: 9 (SIGKILL)"
    (!status.success()).then(|| format!("Claude Code ended with {status}"))
}

/// The status a shell gives for a program that ended as Claude Code did, which the client ends
/// with, so it passes Claude Code's on.
fn exit_code(status: ExitStatus) -> ExitCode {
    let code = status
        .code()
        .or_else(|| status.signal().map(|signal| 128 + signal))
        .and_then(|code| u8::try_from(code).ok())
        .unwrap_or(1);
    ExitCode::from(code)
}

/// Shows what Claude Code reports, notes what its calls hold in the cache, and hands the main
/// thread what it has to act on.
fn read_claude(from_claude: ChildStdout, shared: &Mutex<Shared>, forward: &dyn Fn(Input)) {
    for line in BufReader::new(from_claude).lines() {
        let line = match line {
            Ok(line) => line,
            Err(error) => {
                lock(shared)
                    .output
                    .show_error(&format!("cannot read from claude: {error}"));
                break;
            }
        };
        let events = match read_events(&line) {
            Ok(events) => events,
            // Claude Code still runs, so a message claudio cannot read ends no session
            Err(error) => {
                lock(shared).output.show_error(&error);
                continue;
            }
        };
        for event in events {
            lock(shared).note(event, forward);
        }
    }
    forward(Input::Closed);
}

/// Shows each line of Claude Code's error output that adds to what its messages show.
fn read_claude_errors(errors_from_claude: ChildStderr, shared: &Mutex<Shared>) {
    for line in BufReader::new(errors_from_claude).lines() {
        let line = match line {
            Ok(line) => line,
            Err(error) => {
                lock(shared)
                    .output
                    .show_error(&format!("cannot read the error output of claude: {error}"));
                return;
            }
        };
        if !repeats_a_message(&line) {
            lock(shared).output.show_stderr(&line);
        }
    }
}

/// Locks a value the threads share. A thread that panicked while holding it ends the program
/// anyway.
fn lock<T>(shared: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}
