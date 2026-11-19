//! The work of a session: the calls that Claude and its subagents make, the tasks the calls
//! start, and the subagents, which outlive their runs. The conversation shows the work as it
//! goes, and the keys stop it, by what this module knows of it.
//!
//! A task is one of two kinds, a command or a subagent, and runs in one of two places: in the
//! foreground, where the turn waits for it, or in the background, where the turn goes on
//! without it. A task starts in either place, and moves only from the foreground to the
//! background. A subagent's own calls belong to it, and end with it.

use std::collections::HashMap;
use std::fmt::{self, Display, Formatter};

use crate::protocol::{TaskStatus, ToolUse};

/// What names a call and ties the result's line to the call's line: the call's count among
/// Claude's calls, like `#0a`, or, for a subagent's call, which shows only in the full view,
/// the tag of the call that started the subagent, then the call's count among the subagent's,
/// like `#0a#03`, so a search for `#0a` finds the subagent's calls too. A tag is one word, which
/// a search finds whole, of two hexadecimal digits each, which keep the columns of a line in
/// place. Counts wrap from ff to 00, on the assumption that a call's result arrives before 255
/// more calls of its agent start. A call that waits longer shares its tag with a newer one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tag {
    Claude(u8),
    /// A subagent's call, after the count of the call that started the subagent, unknown for a
    /// call that comes before the subagent's task. Claude Code tells the task first, as the
    /// checks of subagents saw, and a call that came before would show `#??`.
    Subagent(Option<u8>, u8),
}

impl Display for Tag {
    fn fmt(&self, formatter: &mut Formatter) -> fmt::Result {
        match self {
            Tag::Claude(call) => write!(formatter, "#{call:02x}"),
            Tag::Subagent(Some(start), call) => write!(formatter, "#{start:02x}#{call:02x}"),
            Tag::Subagent(None, call) => write!(formatter, "#??#{call:02x}"),
        }
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
    owner: Option<String>,
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
    /// The count of the `Agent` call that first started it.
    pub start: Option<u8>,
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
    subagents: HashMap<String, Subagent>,
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
        let tag = if let Some(agent) = &tool_use.agent {
            let subagent = self.subagents.entry(agent.clone()).or_default();
            let call = subagent.next_call;
            subagent.next_call = call.wrapping_add(1);
            subagent.calls += 1;
            Tag::Subagent(subagent.start, call)
        } else {
            let call = self.next_call;
            self.next_call = call.wrapping_add(1);
            Tag::Claude(call)
        };
        let call = Call {
            id: tool_use.id.clone(),
            tool: tool_use.name.clone(),
            tag,
            line: 0,
            owner: tool_use.agent.clone(),
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
            let subagent = self.subagents.entry(id.clone()).or_default();
            let start = call.as_ref().and_then(|call| match call.tag {
                Tag::Claude(count) => Some(count),
                Tag::Subagent(..) => None,
            });
            *subagent = Subagent {
                start: subagent.start.or(start),
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
            .filter(|call| matches!(call.tag, Tag::Claude(_)))
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
            Origin::Waiting(call) => Some(self.calls.get(call)?.tag),
            Origin::Answered(call) => Some(call.tag),
            Origin::Unseen => None,
        }
    }

    pub fn subagent(&self, id: &str) -> Option<&Subagent> {
        self.subagents.get(id)
    }

    /// The subagent that makes the call `id`, by what it does, none for Claude itself.
    pub fn owner_of(&self, id: &str) -> Option<String> {
        let owner = self.calls.get(id)?.owner.as_ref()?;
        self.subagents.get(owner)?.description.clone()
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
            tasks.contains_key(id) || calls.values().any(|call| call.owner.as_ref() == Some(id))
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
            agent_call: agent.map(String::from),
            agent: agent.map(String::from),
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
        assert_eq!(work.task_tag("running"), Some(Tag::Claude(1)));
        // Nothing runs once the subagent ends, and the count goes on
        work.end_task("running", end());
        work.start_session();
        assert!(work.subagent("running").is_none());
        assert_eq!(work.add_call(&tool_use("c2", "Bash", None)), Tag::Claude(2));
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
        assert_eq!((subagent.start, subagent.calls), (Some(0), 0));
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
        assert_eq!(call.tag, Tag::Claude(0));
        assert!(work.task_tag("t").is_none());
    }
}
