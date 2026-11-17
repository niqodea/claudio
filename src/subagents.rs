//! The subagents of Claude in a session, which claudio names by the call of Claude's that first
//! started each, and counts as they run.

use std::collections::HashMap;

/// The subagents of the session, by their ids, which are their tasks' ids too.
///
/// A subagent stays after it ends, since Claude can resume it by its id at any time in the
/// session, and the tag of the call that first started it names it again then. So the list
/// grows by one small entry for each subagent the session starts, about 100 bytes, and a reset
/// forgets the ended ones, since the new session cannot resume them. Claude Code bounds its own
/// list instead: it forgets an ended subagent 30 seconds after its end, and loads it back from
/// its transcript on disk when Claude resumes it, which claudio cannot read. Read in Claude
/// Code 2.1.322, as the `evictAfter` of its tasks.
#[derive(Default)]
pub struct Subagents {
    by_id: HashMap<String, Subagent>,
}

#[derive(Default)]
pub struct Subagent {
    /// The count of the call that first started it, an `Agent` call.
    pub start: Option<u8>,
    /// The count of its next call in its run.
    next_call: u8,
    /// The calls it made in its run, which the line of its end counts.
    pub calls: u32,
    /// Whether it runs in the background, while it runs.
    is_backgrounded: Option<bool>,
}

impl Subagent {
    /// The count of the subagent's next call in its run, which wraps from ff to 00, as the
    /// counts of Claude's own calls do.
    pub fn next_call(&mut self) -> u8 {
        let call = self.next_call;
        self.next_call = call.wrapping_add(1);
        self.calls += 1;
        call
    }
}

impl Subagents {
    pub fn get(&self, id: &str) -> Option<&Subagent> {
        self.by_id.get(id)
    }

    /// The subagent `id`, which a call of it that comes before its task's start adds.
    pub fn get_or_add(&mut self, id: &str) -> &mut Subagent {
        self.by_id.entry(id.to_string()).or_default()
    }

    /// Notes that the subagent `id` started a run, in the background when `is_backgrounded`,
    /// with the call of the count `start`. A resumed subagent keeps the count of the call that
    /// first started it, and each run counts its calls anew.
    pub fn note_started(&mut self, id: &str, start: Option<u8>, is_backgrounded: bool) {
        let subagent = self.get_or_add(id);
        *subagent = Subagent {
            start: subagent.start.or(start),
            is_backgrounded: Some(is_backgrounded),
            ..Subagent::default()
        };
    }

    /// Notes that the subagent `id`, which runs, moved to the background.
    pub fn note_backgrounded(&mut self, id: &str) {
        if let Some(subagent) = self.by_id.get_mut(id)
            && subagent.is_backgrounded.is_some()
        {
            subagent.is_backgrounded = Some(true);
        }
    }

    /// Notes that the run of the subagent `id` ended.
    pub fn note_ended(&mut self, id: &str) {
        if let Some(subagent) = self.by_id.get_mut(id) {
            subagent.is_backgrounded = None;
        }
    }

    /// The subagents that run, in the foreground or in the background.
    pub fn running(&self) -> usize {
        (self.by_id.values())
            .filter(|subagent| subagent.is_backgrounded.is_some())
            .count()
    }

    /// The subagents that run in the foreground, which Claude waits for.
    pub fn foreground(&self) -> usize {
        (self.by_id.values())
            .filter(|subagent| subagent.is_backgrounded == Some(false))
            .count()
    }

    /// Forgets the subagents that ended, as a new session starts, which cannot resume them. One
    /// that runs stays, since Claude Code keeps a subagent that runs across a clear, as its
    /// `hasAgentTaskSurvivingClear` in 2.1.322 tells. Should it stop it instead, the subagent
    /// ends, and stays until the next reset.
    pub fn forget_ended(&mut self) {
        (self.by_id).retain(|_, subagent| subagent.is_backgrounded.is_some());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reset_forgets_only_the_subagents_that_ended() {
        let mut subagents = Subagents::default();
        subagents.note_started("ended", Some(0), false);
        subagents.note_ended("ended");
        subagents.note_started("running", Some(1), true);
        subagents.forget_ended();
        assert!(subagents.get("ended").is_none());
        assert_eq!(subagents.running(), 1);
    }

    #[test]
    fn a_resumed_subagent_keeps_its_first_start_and_counts_anew() {
        let mut subagents = Subagents::default();
        subagents.note_started("a", Some(1), false);
        subagents.get_or_add("a").next_call();
        subagents.note_ended("a");
        subagents.note_started("a", Some(7), true);
        let subagent = subagents.get("a").expect("the subagent stays");
        assert_eq!((subagent.start, subagent.calls), (Some(1), 0));
    }
}
