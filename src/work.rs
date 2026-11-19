//! The work of a session: the calls that Claude and its subagents make, the tasks the calls
//! start, and the subagents, which outlive their runs. The conversation shows the work as it
//! goes, and the keys stop it, by what this module knows of it.
//!
//! A task is one of two kinds, a command or a subagent, and runs in one of two places: in the
//! foreground, where the turn waits for it, or in the background, where the turn goes on
//! without it. A task starts in either place, and moves only from the foreground to the
//! background. A subagent's own calls belong to it, and end with it.
//!
//! A subagent is known by what Claude Code states of it, and nothing claudio infers, so a line
//! never names the wrong one:
//! - its identity is its id, `SubagentId`, which every message of it, its task and its
//!   requests carry;
//! - it shows to the user by its tag, from the `Agent` call that first started it, with what it
//!   does where the line has room, like `#0a Review the diff`, on every line about it;
//! - what Claude writes to reach it, like `SendMessage`'s `to`, shows as its tag when it is an
//!   id claudio knows, and as written otherwise.
//!
//! So a subagent that Claude messages by name shows by that name, rather than by its tag. Claude
//! Code addresses a subagent by the `name` its `Agent` call asks for, but reports the name it
//! gave in none of what it sends: the `Agent` call's result holds the subagent's id alone, and
//! so does its task's start. The name it gave can differ from the one asked for, since Claude
//! Code 2.1.322 renamed a subagent whose name a running one held, like `reviewer-2`, and freed a
//! name only 30 seconds after its holder ended. A name can also reach another session, or a
//! teammate, as `SendMessage`'s `to` does.
//! Ref: `AgentInput` and `AgentOutput` in `sdk-tools.d.ts`, and `SDKTaskStartedMessage` in
//! `sdk.d.ts`, of <https://unpkg.com/@anthropic-ai/claude-agent-sdk@0.3.325/>
//! Ref: <https://code.claude.com/docs/en/agent-sdk/subagents>, which identifies a subagent by
//! the `agentId` of its `Agent` call's result, and by `agent_id` on its messages
//! Ref: <https://github.com/anthropics/claude-code/issues/100198>, which asks for a subagent's
//! name in hooks, where Claude Code leaves it out too
//! Ref: <https://github.com/anthropics/claude-code/issues/89091>, where a subagent's type, given
//! in place of its id, failed as an address

use std::borrow::Borrow;
use std::collections::HashMap;
use std::fmt::{self, Display, Formatter};

use crate::protocol::{TaskStatus, ToolUse};

/// What names a call and ties the result's line to the call's line: the counts of the calls
/// that lead to it, from Claude's own down, each among the calls of its agent. Claude's call is
/// like `#0a`, a call of the subagent it started like `#0a#03`, and one of a subagent that
/// subagent started like `#0a#03#00`, at any depth, so a search for a tag finds every call under
/// it too. A tag is one word, which a search finds whole, of two hexadecimal digits a count,
/// which keep the columns of a line in place. Counts wrap from ff to 00, on the assumption that
/// a call's result arrives before 255 more calls of its agent start. A call that waits longer
/// shares its tag with a newer one.
#[derive(Clone, Debug, PartialEq)]
pub struct Tag(
    /// The counts, of which the first is unknown, `??`, for a subagent's call that comes before
    /// the subagent's task, which names the call that started it. That never happens in
    /// practice: Claude Code 2.1.322 tells the task first, as
    /// [`crate::contract::a_subagent_is_told_from_start_to_end`] checks. The SDK's types promise
    /// no order, though, so a call can come first, and its tag then says what is unknown rather
    /// than guess it.
    Vec<Option<u8>>,
);

impl Tag {
    /// The tag of Claude's own call of count `count`.
    pub fn claude(count: u8) -> Self {
        Tag(vec![Some(count)])
    }

    /// The tag of the call of count `count` of a subagent, which the call tagged `start` started.
    fn under(start: Option<&Tag>, count: u8) -> Self {
        let mut counts = start.map_or_else(|| vec![None], |start| start.0.clone());
        counts.push(Some(count));
        Tag(counts)
    }

    /// Whether Claude made the call itself, rather than one of its subagents.
    pub fn is_claude(&self) -> bool {
        self.0.len() == 1
    }
}

impl Display for Tag {
    fn fmt(&self, formatter: &mut Formatter) -> fmt::Result {
        for count in &self.0 {
            match count {
                Some(count) => write!(formatter, "#{count:02x}")?,
                None => formatter.write_str("#??")?,
            }
        }
        Ok(())
    }
}

/// A subagent's id, which Claude Code gives it as it starts: the one thing that tells subagents
/// apart. Its task has the same id, and it stays the same when Claude resumes the subagent, as
/// `agent_id` of `SDKAssistantMessage` in `sdk.d.ts` says.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SubagentId(String);

impl SubagentId {
    pub fn new(id: impl Into<String>) -> Self {
        SubagentId(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for SubagentId {
    fn borrow(&self) -> &str {
        &self.0
    }
}

/// A call that waits for its result, whose line names the tool and repeats the tag.
#[derive(Clone)]
pub struct Call {
    pub id: String,
    pub tool: String,
    pub tag: Tag,
    /// The full view's line, from 1, that the call's own line is on, where a request to allow
    /// the call opens the full view.
    pub line: usize,
    /// The subagent that makes the call, none for Claude's own.
    owner: Option<SubagentId>,
    /// The task the call started, whose id is the subagent's for a call that started one.
    pub task: Option<String>,
}

/// What a task is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Command,
    Subagent,
}

/// Where a task runs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Place {
    /// The turn waits for the task.
    Foreground,
    /// The turn goes on without the task.
    Background,
}

/// How a task ended.
#[derive(Clone)]
pub struct End {
    pub status: TaskStatus,
    /// Claude Code's summary of the end for Claude.
    pub summary: String,
}

/// A task that a call started, from its start to its end.
struct Task {
    kind: Kind,
    place: Place,
    origin: Origin,
    /// How the task ended, kept while its call waits for its result, which shows it.
    end: Option<End>,
    /// Whether Claude Code listed the task among those that run in the background.
    was_listed: bool,
}

/// The call that started a task, as far as it went.
enum Origin {
    /// The call waits for its result, among the calls, by its id.
    Waiting(String),
    /// The call's result came, and the task, which outlives it, holds it.
    Answered(Call),
    /// No call that claudio saw.
    Unseen,
}

/// A subagent, which Claude can resume by its id at any time in the session, so it outlives
/// each run.
#[derive(Default)]
pub struct Subagent {
    /// The tag of the `Agent` call that first started it, at whatever depth.
    pub start: Option<Tag>,
    /// What the subagent does, as its first start describes it.
    description: Option<String>,
    /// The count of its next call in its run.
    next_call: u8,
    /// The calls it made in its run, which the line of its end counts.
    pub calls: u32,
}

/// What a call's result settles of the call and of the task it started.
pub enum Resulted {
    /// A call that started no task that outlives it, or one the work never saw.
    Done(Option<Call>),
    /// A call whose task goes on in the background, and its end, when that came first.
    Background(Call, Option<End>),
    /// An `Agent` call whose subagent ran in the foreground, and ended with the result, which is
    /// its report, and its end, when its task told it first.
    Subagent(Call, Option<End>),
}

/// The calls, tasks and subagents of the session.
#[derive(Default)]
pub struct Work {
    /// Each call that waits for its result, by its id. Every call gets its result, however its
    /// turn ends, and the result removes the call.
    ///
    /// Undocumented: [`crate::contract::a_stopped_call_gets_its_result`]
    calls: HashMap<String, Call>,
    /// Each task from its start to its end, by its id, which a subagent's task shares with it.
    tasks: HashMap<String, Task>,
    /// The subagents of the session, by their ids. A subagent stays after it ends, so the tag of
    /// the call that first started it names it again when Claude resumes it. So the list grows
    /// by one small entry for each subagent the session starts, about 100 bytes, and a reset
    /// forgets the ended ones, since the new session cannot resume them. Claude Code 2.1.322
    /// bounds its own list instead: it forgets an ended subagent 30 seconds after its end, and
    /// loads it back from its transcript on disk when Claude resumes it, which claudio cannot
    /// read.
    subagents: HashMap<SubagentId, Subagent>,
    /// The tasks that run in the background, as Claude Code last listed them. The list is what
    /// claudio counts and stops, while the tasks above are what it tells apart and names: a
    /// task's start, end and moves come as events of their own, which the list follows.
    listed: Vec<crate::protocol::Task>,
    /// The count of Claude's next call.
    next_call: u8,
}

impl Work {
    /// Adds `tool_use`, a call made now, and gives its tag. A subagent's call counts among its
    /// own run's calls. The full view's line of the call comes once the call's line is written,
    /// which shows the tag: `set_line`.
    pub fn add_call(&mut self, tool_use: &ToolUse) -> Tag {
        let tag = if let Some(owner) = &tool_use.subagent {
            let subagent = self.subagents.entry(owner.clone()).or_default();
            let call = subagent.next_call;
            subagent.next_call = call.wrapping_add(1);
            subagent.calls += 1;
            Tag::under(subagent.start.as_ref(), call)
        } else {
            let call = self.next_call;
            self.next_call = call.wrapping_add(1);
            Tag::claude(call)
        };
        let call = Call {
            id: tool_use.id.clone(),
            tool: tool_use.name.clone(),
            tag: tag.clone(),
            line: 0,
            owner: tool_use.subagent.clone(),
            task: None,
        };
        self.calls.insert(tool_use.id.clone(), call);
        tag
    }

    /// Notes that the full view shows the call `id`'s line on `line`, from 1.
    pub fn set_line(&mut self, id: &str, line: usize) {
        if let Some(call) = self.calls.get_mut(id) {
            call.line = line;
        }
    }

    pub fn call(&self, id: &str) -> Option<&Call> {
        self.calls.get(id)
    }

    /// Notes that the call `call` started the task `id`, of `kind`, in `place`, which does what
    /// `description` says. A subagent's task starts a run of it, which counts its calls anew,
    /// and keeps the tag of the call that first started it, as one Claude resumes does.
    pub fn start_task(
        &mut self,
        id: String,
        call: Option<&str>,
        kind: Kind,
        place: Place,
        description: &str,
    ) {
        let call = call.and_then(|call| self.calls.get_mut(call));
        if kind == Kind::Subagent {
            let subagent = (self.subagents)
                .entry(SubagentId::new(id.clone()))
                .or_default();
            let start = call.as_ref().map(|call| call.tag.clone());
            *subagent = Subagent {
                start: subagent.start.take().or(start),
                description: (subagent.description.take()).or_else(|| Some(description.into())),
                ..Subagent::default()
            };
        }
        let origin = match call {
            Some(call) => {
                call.task = Some(id.clone());
                Origin::Waiting(call.id.clone())
            }
            None => Origin::Unseen,
        };
        let task = Task {
            kind,
            place,
            origin,
            end: None,
            was_listed: false,
        };
        self.tasks.insert(id, task);
    }

    /// Notes that the task `id` moved to the background.
    pub fn move_to_background(&mut self, id: &str) {
        if let Some(task) = self.tasks.get_mut(id) {
            task.place = Place::Background;
        }
    }

    /// Notes that the task `id` ended, and gives the call that started it, for a task whose
    /// call's result came already, whose end shows now. A task whose call waits for its result
    /// keeps its end for the result to show.
    ///
    /// Undocumented: [`crate::contract::a_subagent_is_told_from_start_to_end`], for a subagent,
    /// which counts as running until then
    pub fn end_task(&mut self, id: &str, end: End) -> Option<(Call, End)> {
        let task = self.tasks.get_mut(id)?;
        if let Origin::Waiting(_) = task.origin {
            task.end = Some(end);
            return None;
        }
        match self.tasks.remove(id)?.origin {
            Origin::Answered(call) => Some((call, end)),
            Origin::Waiting(_) | Origin::Unseen => None,
        }
    }

    /// Settles the result of the call `id`, which removes it, and the task it started, unless
    /// the task goes on in the background, where it holds the call from then on.
    pub fn take_result(&mut self, id: &str) -> Resulted {
        let Some(call) = self.calls.remove(id) else {
            return Resulted::Done(None);
        };
        let task_id = call.task.clone().unwrap_or_default();
        let Some(task) = self.tasks.get_mut(&task_id) else {
            return Resulted::Done(Some(call));
        };
        match (task.place, task.kind) {
            (Place::Background, _) if task.end.is_none() => {
                task.origin = Origin::Answered(call.clone());
                Resulted::Background(call, None)
            }
            (Place::Background, _) => {
                let end = self.tasks.remove(&task_id).and_then(|task| task.end);
                Resulted::Background(call, end)
            }
            (Place::Foreground, Kind::Subagent) => {
                let end = self.tasks.remove(&task_id).and_then(|task| task.end);
                Resulted::Subagent(call, end)
            }
            (Place::Foreground, Kind::Command) => {
                self.tasks.remove(&task_id);
                Resulted::Done(Some(call))
            }
        }
    }

    /// Notes the tasks that run in the background, as Claude Code lists them. A task that an
    /// earlier list held and this one does not has ended, so it is forgotten, once its call's
    /// result came, even if its end never came. A task that no list held yet stays, since it may
    /// join a later list.
    pub fn note_listed(&mut self, listed: Vec<crate::protocol::Task>) {
        self.tasks.retain(|id, task| {
            let is_listed = listed.iter().any(|listed| listed.id == *id);
            let waits = matches!(task.origin, Origin::Waiting(_));
            let has_left = task.was_listed && !is_listed && !waits;
            task.was_listed |= is_listed;
            !has_left
        });
        self.listed = listed;
    }

    /// The tasks that run in the background, as Claude Code last listed them.
    pub fn listed(&self) -> &[crate::protocol::Task] {
        &self.listed
    }

    /// Whether no task runs in the background, as Claude Code last listed them.
    pub fn is_quiet(&self) -> bool {
        self.listed.is_empty()
    }

    /// The subagents that run, in `place`, or in either place for none.
    pub fn running_subagents(&self, place: Option<Place>) -> usize {
        (self.tasks.values())
            .filter(|task| task.kind == Kind::Subagent && task.end.is_none())
            .filter(|task| place.is_none_or(|place| task.place == place))
            .count()
    }

    /// The calls of Claude's own whose tasks run in the foreground, which Claude waits on, by
    /// their ids. A subagent's calls belong to the subagent's run.
    pub fn foreground_calls(&self) -> Vec<String> {
        (self.tasks.values())
            .filter(|task| task.place == Place::Foreground && task.end.is_none())
            .filter_map(|task| match &task.origin {
                Origin::Waiting(call) => self.calls.get(call),
                Origin::Answered(_) | Origin::Unseen => None,
            })
            .filter(|call| call.tag.is_claude())
            .map(|call| call.id.clone())
            .collect()
    }

    /// The call `id` of the tool `tool`, as the label row names it: by its tool and its tag,
    /// like `Bash #03`, and for a call whose subagent runs in the foreground, with how many
    /// others run in the foreground beside it, like `Agent #01 +2`, and `+N` for 10 or more.
    pub fn name_call(&self, tool: &str, id: &str) -> String {
        let Some(call) = self.calls.get(id) else {
            return tool.to_string();
        };
        let task = call.task.as_ref().and_then(|task| self.tasks.get(task));
        let others = match task {
            Some(task) if task.kind == Kind::Subagent => self
                .running_subagents(Some(Place::Foreground))
                .saturating_sub(1),
            _ => 0,
        };
        let others = match others {
            0 => String::new(),
            1..10 => format!(" +{others}"),
            _ => " +N".to_string(),
        };
        format!("{tool} {}{others}", call.tag)
    }

    /// The tag of the call that started the task `id`, which a call that names the task, like
    /// `TaskStop`'s, names it by.
    pub fn task_tag(&self, id: &str) -> Option<Tag> {
        match &self.tasks.get(id)?.origin {
            Origin::Waiting(call) => Some(self.calls.get(call)?.tag.clone()),
            Origin::Answered(call) => Some(call.tag.clone()),
            Origin::Unseen => None,
        }
    }

    pub fn subagent(&self, id: &str) -> Option<&Subagent> {
        self.subagents.get(id)
    }

    /// The subagent `id` as a line names it: by its tag and what it does, like `#0a Review the
    /// diff`, or by its id, as Claude Code gives it, for a subagent whose start claudio did not
    /// see.
    pub fn name_subagent(&self, id: &SubagentId) -> String {
        let subagent = self.subagents.get(id);
        let tag = (subagent.and_then(|subagent| subagent.start.as_ref())).map(Tag::to_string);
        let description = subagent.and_then(|subagent| subagent.description.clone());
        let parts: Vec<String> = [tag, description].into_iter().flatten().collect();
        if parts.is_empty() {
            id.as_str().to_string()
        } else {
            parts.join(" ")
        }
    }

    /// Starts the work of a new session, as a clear does. It keeps the count of Claude's calls,
    /// so a tag names one call in the full view, which holds every session, and what still runs,
    /// with what names it as it ends: the calls that wait for their results, the tasks that run
    /// or whose calls wait, and the subagents of those tasks and calls. Everything else goes, like
    /// the subagents that ended, which the new session cannot resume. claudio clears once
    /// nothing runs, so only the count stays then, and the rest stays for a clear that came
    /// while work ran, which Claude Code 2.1.322 lets run on.
    pub fn start_session(&mut self) {
        let calls = &self.calls;
        (self.tasks)
            .retain(|_, task| task.end.is_none() || matches!(task.origin, Origin::Waiting(_)));
        let tasks = &self.tasks;
        (self.subagents).retain(|id, _| {
            tasks.contains_key(id.as_str())
                || calls.values().any(|call| call.owner.as_ref() == Some(id))
        });
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Map;

    use super::*;

    fn tool_use(id: &str, name: &str, agent: Option<&str>) -> ToolUse {
        ToolUse {
            id: id.to_string(),
            name: name.to_string(),
            input: Map::new(),
            subagent: agent.map(SubagentId::new),
        }
    }

    fn end() -> End {
        End {
            status: TaskStatus::Completed,
            summary: String::new(),
        }
    }

    #[test]
    fn a_new_session_keeps_the_count_and_what_still_runs() {
        let mut work = Work::default();
        work.add_call(&tool_use("c0", "Agent", None));
        work.start_task(
            "ended".to_string(),
            Some("c0"),
            Kind::Subagent,
            Place::Foreground,
            "",
        );
        work.end_task("ended", end());
        work.take_result("c0");
        work.add_call(&tool_use("c1", "Agent", None));
        work.start_task(
            "running".to_string(),
            Some("c1"),
            Kind::Subagent,
            Place::Background,
            "",
        );
        work.take_result("c1");
        work.start_session();
        assert!(work.subagent("ended").is_none());
        assert!(work.subagent("running").is_some());
        assert_eq!(work.task_tag("running"), Some(Tag::claude(1)));
        // Nothing runs once the subagent ends, and the count goes on
        work.end_task("running", end());
        work.start_session();
        assert!(work.subagent("running").is_none());
        assert_eq!(work.add_call(&tool_use("c2", "Bash", None)), Tag::claude(2));
    }

    #[test]
    fn a_call_s_tag_leads_down_from_claude_s_call_at_any_depth() {
        let mut work = Work::default();
        let tags: Vec<String> = [
            ("c0", "Agent", None, Some("a")),
            ("c1", "Agent", Some("a"), Some("b")),
            ("c2", "Agent", Some("b"), Some("c")),
            ("c3", "Bash", Some("c"), None),
        ]
        .into_iter()
        .map(|(id, tool, owner, started)| {
            let tag = work.add_call(&tool_use(id, tool, owner));
            if let Some(started) = started {
                work.start_task(
                    started.to_string(),
                    Some(id),
                    Kind::Subagent,
                    Place::Foreground,
                    "",
                );
            }
            tag.to_string()
        })
        .collect();
        assert_eq!(tags, ["#00", "#00#00", "#00#00#00", "#00#00#00#00"]);
        assert!(
            !work
                .add_call(&tool_use("c4", "Bash", Some("c")))
                .is_claude()
        );
        // A call of a subagent whose start never showed
        assert_eq!(
            work.add_call(&tool_use("c5", "Bash", Some("x")))
                .to_string(),
            "#??#00"
        );
    }

    #[test]
    fn a_subagent_shows_by_its_tag_and_what_it_does_and_else_by_its_id() {
        let mut work = Work::default();
        work.add_call(&tool_use("c0", "Agent", None));
        let id = SubagentId::new("a");
        work.start_task(
            "a".to_string(),
            Some("c0"),
            Kind::Subagent,
            Place::Background,
            "Review",
        );
        assert_eq!(work.name_subagent(&id), "#00 Review");
        assert_eq!(work.name_subagent(&SubagentId::new("b")), "b");
    }

    #[test]
    fn a_resumed_subagent_keeps_its_first_start_and_counts_anew() {
        let mut work = Work::default();
        work.add_call(&tool_use("c0", "Agent", None));
        work.start_task(
            "a".to_string(),
            Some("c0"),
            Kind::Subagent,
            Place::Foreground,
            "",
        );
        work.add_call(&tool_use("c1", "Bash", Some("a")));
        work.end_task("a", end());
        work.take_result("c0");
        work.add_call(&tool_use("c2", "SendMessage", None));
        work.start_task(
            "a".to_string(),
            Some("c2"),
            Kind::Subagent,
            Place::Background,
            "",
        );
        let subagent = work.subagent("a").expect("the subagent stays");
        assert_eq!(
            (subagent.start.clone(), subagent.calls),
            (Some(Tag::claude(0)), 0)
        );
    }

    #[test]
    fn a_task_ends_with_its_call_or_on_its_own_once_the_call_is_done() {
        let mut work = Work::default();
        work.add_call(&tool_use("c0", "Bash", None));
        work.start_task(
            "t".to_string(),
            Some("c0"),
            Kind::Command,
            Place::Foreground,
            "",
        );
        work.move_to_background("t");
        assert!(matches!(
            work.take_result("c0"),
            Resulted::Background(_, None)
        ));
        let (call, _) = work.end_task("t", end()).expect("the end shows now");
        assert_eq!(call.tag, Tag::claude(0));
        assert!(work.task_tag("t").is_none());
    }
}
