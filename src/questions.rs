//! The questions of Claude's `AskUserQuestion` calls that wait for the user. Claude Code takes a
//! call's answers together, and Claude reads none of them before, so the question view shows a
//! call's questions as a batch. Enter submits a question and moves on to the next one not
//! submitted, and a submitted question keeps its answer until Enter opens it again. Once all are
//! submitted, the review shows the answers, all ticked: Enter sends them, or opens the ones the
//! user unticked, which keep what was picked and written on them.
//!
//! Ref: <https://design-system.service.gov.uk/patterns/check-answers>, on checking answers
//! before sending them

use std::collections::VecDeque;

use serde_json::{Map, Value};

use crate::keys::View;
use crate::picker::{Choice, Cursor, Draft, Form, Pick};
use crate::protocol::{Answer, Ask, AskKind, Question};
use crate::render::{BOLD, RESET};

#[derive(Default)]
pub struct Questions {
    /// The calls that ask, oldest first, of which the question view shows the first. Claude asks
    /// in one call at a time in practice, and calls asked together wait in line.
    batches: VecDeque<Batch>,
    /// Whether the user pressed Enter on a message once while these questions wait, so the next
    /// Enter refuses them.
    is_refusal_confirmed: bool,
}

/// A call that asks, with each of its questions.
struct Batch {
    ask: Ask,
    entries: Vec<Entry>,
}

/// A question that waits.
struct Entry {
    /// The line of the full view, from 1, that it starts on.
    full_view_line: usize,
    /// What was picked and written on it so far.
    draft: Draft,
    is_submitted: bool,
}

/// What sending from the question view leads to.
pub enum Sent {
    /// The question view shows the question at this index, or the review one past the last.
    Show(usize),
    /// The batch is answered: each question's header with its answer, and the reply to its call.
    Answered {
        answers: Vec<(String, String)>,
        ask: Ask,
        answer: Answer,
    },
}

impl Batch {
    fn questions(&self) -> &[Question] {
        match &self.ask.kind {
            AskKind::Questions(questions) => questions,
            AskKind::Permission => unreachable!("only a call with questions waits"),
        }
    }

    fn is_submitted(&self) -> bool {
        self.entries.iter().all(|entry| entry.is_submitted)
    }

    /// The first question not submitted after the one at `index`, round from the first, or else
    /// the review.
    fn next_open(&self, index: usize) -> usize {
        let count = self.entries.len();
        (1..=count)
            .map(|step| (index + step) % count)
            .find(|&next| !self.entries[next].is_submitted)
            .unwrap_or(count)
    }

    /// Each question's header with what its draft answers.
    fn answers(&self) -> Vec<(String, String)> {
        self.questions()
            .iter()
            .zip(&self.entries)
            .map(|(question, entry)| {
                let answer = choose(question, &entry.draft).expect("a submitted question answers");
                (question.header.clone(), answer)
            })
            .collect()
    }
}

impl Questions {
    /// Adds the questions of a call that asks, each starting on its line of `full_view_lines`. A
    /// permission has none, since it is refused as it arrives.
    pub fn add(&mut self, ask: Ask, full_view_lines: Vec<usize>) {
        let AskKind::Questions(questions) = &ask.kind else {
            return;
        };
        self.is_refusal_confirmed = false;
        let entries = full_view_lines
            .into_iter()
            .take(questions.len())
            .map(|full_view_line| Entry {
                full_view_line,
                draft: Draft::new(),
                is_submitted: false,
            })
            .collect();
        self.batches.push_back(Batch { ask, entries });
    }

    pub fn is_refusal_confirmed(&self) -> bool {
        self.is_refusal_confirmed && !self.is_empty()
    }

    /// Whether a message the user sends stays in the input box, which it does the first time
    /// while questions wait. The confirmation lasts until the questions are refused or a new
    /// question arrives.
    pub fn holds_message(&mut self) -> bool {
        let holds = !self.is_empty() && !self.is_refusal_confirmed;
        self.is_refusal_confirmed |= holds;
        holds
    }

    pub fn count(&self) -> usize {
        self.batches.iter().map(|batch| batch.entries.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.batches.is_empty()
    }

    /// Where the question view opens: the first question not submitted, or else the review, or
    /// none when none waits.
    pub fn first(&self) -> Option<usize> {
        let batch = self.batches.front()?;
        Some(batch.next_open(batch.entries.len() - 1))
    }

    /// The question next to the one at `index`, round the batch, where the review comes
    /// after the last question once all are submitted.
    pub fn step(&self, index: usize, is_forward: bool) -> usize {
        let batch = self.batch();
        let stops = batch.entries.len() + usize::from(batch.is_submitted());
        if is_forward {
            (index + 1) % stops
        } else {
            (index + stops - 1) % stops
        }
    }

    /// The draft of the question at `index`, or on the review, every answer ticked.
    pub fn draft(&self, index: usize) -> Draft {
        let batch = self.batch();
        match batch.entries.get(index) {
            Some(entry) => entry.draft.clone(),
            None => Draft {
                pick: Pick {
                    cursor: Cursor::Option(0),
                    ticked: (0..batch.entries.len()).collect(),
                    is_own_ticked: false,
                },
                text: String::new(),
            },
        }
    }

    /// Keeps `draft` on the question at `index`, which the review has none of.
    pub fn keep_draft(&mut self, index: usize, draft: Draft) {
        if let Some(entry) = self.batch_mut().entries.get_mut(index) {
            entry.draft = draft;
        }
    }

    /// What the question view shows at `index`: a question's text and its options, or the
    /// review's answers.
    pub fn view(&self, index: usize) -> View {
        let batch = self.batch();
        let progress: Vec<bool> = batch
            .entries
            .iter()
            .map(|entry| entry.is_submitted)
            .collect();
        let Some(entry) = batch.entries.get(index) else {
            let choices = batch
                .answers()
                .into_iter()
                .map(|(header, answer)| Choice {
                    label: format!("{header}: {answer}"),
                    description: String::new(),
                    preview: None,
                })
                .collect();
            return View {
                label: "Review".to_string(),
                lines: vec![format!("{BOLD}Send these answers?{RESET}")],
                choices,
                own_label: String::new(),
                form: Form::Review,
                full_view_line: batch.entries[0].full_view_line,
                index,
                progress,
                is_submitted: false,
            };
        };
        let question = &batch.questions()[index];
        let choices = question
            .options
            .iter()
            .map(|choice| Choice {
                label: choice.label.clone(),
                description: choice.description.clone(),
                preview: choice.preview.clone(),
            })
            .collect();
        let (own_label, form) = if question.is_multi_select {
            ("more of your own", Form::Several)
        } else {
            ("your own answer", Form::One)
        };
        View {
            label: question.header.clone(),
            lines: vec![format!("{BOLD}{}{RESET}", question.text)],
            choices,
            own_label: own_label.to_string(),
            form,
            full_view_line: entry.full_view_line,
            index,
            progress,
            is_submitted: entry.is_submitted,
        }
    }

    /// Acts on Enter at `index` with `draft`. Enter submits a question and moves on, or opens a
    /// submitted one again, and keeps a question that `draft` answers nothing on, like the user's
    /// own answer left empty. On the review, it answers the batch once every answer is ticked,
    /// and else opens the unticked ones.
    pub fn send(&mut self, index: usize, draft: Draft) -> Sent {
        let batch = self.batch_mut();
        let count = batch.entries.len();
        if index == count {
            if draft.pick.ticked.len() == count {
                return self.answer();
            }
            for (number, entry) in batch.entries.iter_mut().enumerate() {
                entry.is_submitted = draft.pick.ticked.contains(&number);
            }
            return Sent::Show(batch.next_open(count - 1));
        }
        let answers = choose(&batch.questions()[index], &draft).is_some();
        let entry = &mut batch.entries[index];
        entry.draft = draft;
        if entry.is_submitted || !answers {
            entry.is_submitted = false;
            return Sent::Show(index);
        }
        entry.is_submitted = true;
        Sent::Show(batch.next_open(index))
    }

    /// Takes away the batch the question view shows, and returns what it asked, to refuse, since
    /// Claude Code refuses a call's questions together.
    pub fn remove_first(&mut self) -> Ask {
        self.batches.pop_front().expect("a batch shows").ask
    }

    /// Takes away every call, and returns what they asked, to refuse.
    pub fn remove_all(&mut self) -> Vec<Ask> {
        self.is_refusal_confirmed = false;
        self.batches.drain(..).map(|batch| batch.ask).collect()
    }

    /// Takes away the batch the question view shows, answered.
    fn answer(&mut self) -> Sent {
        let batch = self.batches.pop_front().expect("a batch shows");
        let answers = batch.answers();
        let map: Map<String, Value> = batch
            .questions()
            .iter()
            .zip(&answers)
            .map(|(question, (_, answer))| (question.text.clone(), Value::String(answer.clone())))
            .collect();
        let mut input = batch.ask.input.clone();
        input.insert("answers".to_string(), Value::Object(map));
        Sent::Answered {
            answers,
            ask: batch.ask,
            answer: Answer::Allow { input },
        }
    }

    fn batch(&self) -> &Batch {
        self.batches.front().expect("a batch shows")
    }

    fn batch_mut(&mut self) -> &mut Batch {
        self.batches.front_mut().expect("a batch shows")
    }
}

/// What `draft` answers to `question`, none when it answers nothing. Claude gets the options'
/// labels and the user's words, never the numbers, which only the question view shows.
fn choose(question: &Question, draft: &Draft) -> Option<String> {
    let form = if question.is_multi_select {
        Form::Several
    } else {
        Form::One
    };
    let chosen = draft.pick.chosen(form, &draft.text);
    let mut items: Vec<&str> = chosen
        .options
        .iter()
        .map(|&index| question.options[index].label.as_str())
        .collect();
    if chosen.is_own {
        items.push(draft.text.trim());
    }
    (!items.is_empty()).then(|| items.join(", "))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use serde_json::json;

    use super::*;
    use crate::protocol::read_request;

    fn ask(id: &str, input: &Value) -> Ask {
        let request = json!({
            "subtype": "can_use_tool",
            "tool_name": "AskUserQuestion",
            "input": input,
            "tool_use_id": id,
        });
        match read_request(format!("request-{id}"), request) {
            Ok(crate::protocol::Event::Ask(ask)) => ask,
            _ => panic!("an ask"),
        }
    }

    fn two_questions(id: &str) -> Ask {
        let option = |label: &str| json!({"label": label, "description": ""});
        ask(
            id,
            &json!({"questions": [
                {"question": "Which database?", "header": "Database",
                 "options": [option("Postgres"), option("SQLite")]},
                {"question": "Which checks?", "header": "Checks", "multiSelect": true,
                 "options": [option("Lint"), option("Tests"), option("Types")]},
            ]}),
        )
    }

    fn one_question(id: &str) -> Ask {
        let option = |label: &str| json!({"label": label, "description": ""});
        ask(
            id,
            &json!({"questions": [
                {"question": "Which cache?", "header": "Cache",
                 "options": [option("Memory"), option("Disk")]},
            ]}),
        )
    }

    fn draft(cursor: Cursor, text: &str) -> Draft {
        let mut draft = Draft::new();
        draft.pick.cursor = cursor;
        draft.text = text.to_string();
        draft
    }

    fn with_ticks(cursor: usize, ticked: &[usize]) -> Draft {
        let mut draft = draft(Cursor::Option(cursor), "");
        draft.pick.ticked = ticked.iter().copied().collect();
        draft
    }

    fn questions_of(ask: &Ask) -> &[Question] {
        match &ask.kind {
            AskKind::Questions(questions) => questions,
            AskKind::Permission => panic!("questions"),
        }
    }

    fn shown(sent: &Sent) -> usize {
        match sent {
            Sent::Show(index) => *index,
            Sent::Answered { .. } => panic!("a question shows"),
        }
    }

    #[test]
    fn the_view_offers_a_questions_options_in_its_form() {
        let mut questions = Questions::default();
        questions.add(two_questions("a"), vec![1, 2]);
        assert_eq!(questions.count(), 2);
        let view = questions.view(0);
        assert_eq!(view.label, "Database");
        assert_eq!(view.form, Form::One);
        assert_eq!(view.choices.len(), 2);
        assert_eq!(questions.view(1).form, Form::Several);
    }

    #[test]
    fn enter_submits_and_moves_on_and_opens_a_submitted_question_again() {
        let mut questions = Questions::default();
        questions.add(two_questions("a"), vec![1, 2]);
        assert_eq!(shown(&questions.send(1, with_ticks(2, &[2]))), 0);
        assert_eq!(questions.view(0).progress, [false, true]);
        assert_eq!(questions.step(0, true), 1);
        assert!(questions.view(1).is_submitted);
        assert_eq!(shown(&questions.send(1, questions.draft(1))), 1);
        assert!(!questions.view(1).is_submitted);
        assert_eq!(questions.draft(1).pick.cursor, Cursor::Option(2));
        // The user's own answer left empty answers nothing
        assert_eq!(shown(&questions.send(0, draft(Cursor::Own, ""))), 0);
        assert!(!questions.view(0).is_submitted);
    }

    #[test]
    fn the_review_comes_once_all_are_submitted_and_sends_them_together() {
        let mut questions = Questions::default();
        questions.add(two_questions("a"), vec![1, 2]);
        questions.send(0, draft(Cursor::Option(1), ""));
        let mut ticked = Draft::new();
        ticked.pick.ticked = BTreeSet::from([2, 0]);
        assert_eq!(shown(&questions.send(1, ticked)), 2);
        assert_eq!(questions.step(1, true), 2);
        assert_eq!(questions.step(0, false), 2);
        let review = questions.view(2);
        assert_eq!(review.form, Form::Review);
        assert_eq!(review.choices[1].label, "Checks: Lint, Types");
        let Sent::Answered {
            answers,
            answer: Answer::Allow { input },
            ..
        } = questions.send(2, questions.draft(2))
        else {
            panic!("the batch's answers");
        };
        assert_eq!(answers[0], ("Database".to_string(), "SQLite".to_string()));
        assert_eq!(
            input["answers"],
            json!({"Which database?": "SQLite", "Which checks?": "Lint, Types"})
        );
        assert!(questions.is_empty());
    }

    #[test]
    fn the_review_opens_the_unticked_answers_keeping_their_drafts() {
        let mut questions = Questions::default();
        questions.add(two_questions("a"), vec![1, 2]);
        questions.send(0, draft(Cursor::Option(1), ""));
        questions.send(1, with_ticks(0, &[0]));
        let mut review = questions.draft(2);
        review.pick.ticked.remove(&0);
        assert_eq!(shown(&questions.send(2, review)), 0);
        assert_eq!(questions.view(0).progress, [false, true]);
        assert_eq!(questions.draft(0).pick.cursor, Cursor::Option(1));
        assert_eq!(questions.first(), Some(0));
    }

    #[test]
    fn one_choice_takes_the_option_under_the_cursor_or_the_users_own_words() {
        let ask = two_questions("a");
        let database = &questions_of(&ask)[0];
        for empty in [draft(Cursor::Own, ""), draft(Cursor::Writing, "  ")] {
            assert!(choose(database, &empty).is_none(), "{empty:?}");
        }
        let own = draft(Cursor::Writing, " DuckDB, in a file ");
        assert_eq!(choose(database, &own).as_deref(), Some("DuckDB, in a file"));
        // An option under the cursor answers over the user's own words
        let over = draft(Cursor::Option(0), "DuckDB");
        assert_eq!(choose(database, &over).as_deref(), Some("Postgres"));
    }

    #[test]
    fn several_choices_take_the_ticked_options_and_the_users_own_words() {
        let ask = two_questions("a");
        let checks = &questions_of(&ask)[1];
        assert!(choose(checks, &draft(Cursor::Own, "")).is_none());
        let mut ticked = draft(Cursor::Option(0), "fuzzing");
        ticked.pick.ticked = BTreeSet::from([1, 0]);
        ticked.pick.is_own_ticked = true;
        assert_eq!(
            choose(checks, &ticked).as_deref(),
            Some("Lint, Tests, fuzzing")
        );
        // The user's own words stay out while unticked
        let mut unticked = with_ticks(0, &[1]);
        unticked.text = "fuzzing".to_string();
        assert_eq!(choose(checks, &unticked).as_deref(), Some("Tests"));
        // With nothing ticked, nothing, whatever the cursor is on
        assert!(choose(checks, &draft(Cursor::Option(2), "")).is_none());
    }

    #[test]
    fn a_message_refuses_the_questions_only_the_second_time_and_none_unseen() {
        let mut questions = Questions::default();
        assert!(!questions.holds_message());
        questions.add(one_question("a"), vec![1]);
        assert!(questions.holds_message());
        assert!(questions.is_refusal_confirmed());
        questions.add(one_question("b"), vec![2]);
        assert!(!questions.is_refusal_confirmed());
        assert!(questions.holds_message());
        assert!(!questions.holds_message());
        questions.remove_all();
        assert!(!questions.is_refusal_confirmed());
    }

    #[test]
    fn calls_asked_together_wait_in_line() {
        let mut questions = Questions::default();
        questions.add(two_questions("a"), vec![1, 2]);
        questions.add(one_question("b"), vec![3]);
        assert_eq!(questions.count(), 3);
        assert_eq!(questions.remove_first().tool_use_id, "a");
        assert_eq!(questions.first(), Some(0));
        assert_eq!(questions.view(0).label, "Cache");
    }
}
