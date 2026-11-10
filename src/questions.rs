//! The questions of Claude's `AskUserQuestion` calls that wait for the user, oldest first. The
//! question view shows them one at a time, and each keeps what was picked and written on it until
//! it is answered.

use serde_json::{Map, Value};

use crate::keys::View;
use crate::picker::{Choice, Cursor, Draft, Form};
use crate::protocol::{Answer, Ask, AskKind, Question};
use crate::render::{BOLD, RESET};

#[derive(Default)]
pub struct Questions {
    calls: Vec<Call>,
    waiting: Vec<Waiting>,
    /// Whether the user pressed Enter on a message once while these questions wait, so the next
    /// Enter refuses them.
    is_refusal_confirmed: bool,
}

/// A call that asks, with the answers it has so far, each under its question's text.
struct Call {
    ask: Ask,
    answers: Map<String, Value>,
}

impl Call {
    fn questions(&self) -> &[Question] {
        match &self.ask.kind {
            AskKind::Questions(questions) => questions,
            AskKind::Permission => unreachable!("only a call with questions waits"),
        }
    }
}

/// A question that waits.
struct Waiting {
    /// The call it belongs to.
    tool_use_id: String,
    /// Which question of the call's it is, from 0.
    number: usize,
    /// The line of the full view, from 1, that it starts on.
    full_view_line: usize,
    /// What was picked and written on it so far.
    draft: Draft,
}

/// A question answered, which the conversation shows as `answer` after `label`. `reply` holds
/// its call's answer once the call has all of them.
pub struct Answered {
    pub label: String,
    pub answer: String,
    pub reply: Option<(Ask, Answer)>,
}

impl Questions {
    /// Adds the questions of a call that asks, each starting on its line of `full_view_lines`. A
    /// permission has none, since it is refused as it arrives.
    pub fn add(&mut self, ask: Ask, full_view_lines: Vec<usize>) {
        let AskKind::Questions(questions) = &ask.kind else {
            return;
        };
        self.is_refusal_confirmed = false;
        for (number, full_view_line) in (0..questions.len()).zip(full_view_lines) {
            self.waiting.push(Waiting {
                tool_use_id: ask.tool_use_id.clone(),
                number,
                full_view_line,
                draft: Draft::new(),
            });
        }
        self.calls.push(Call {
            ask,
            answers: Map::new(),
        });
    }

    pub fn is_refusal_confirmed(&self) -> bool {
        self.is_refusal_confirmed && !self.waiting.is_empty()
    }

    /// Whether a message the user sends stays in the input box, which it does the first time
    /// while questions wait. The confirmation lasts until the questions are refused or a new
    /// question arrives.
    pub fn holds_message(&mut self) -> bool {
        let holds = !self.waiting.is_empty() && !self.is_refusal_confirmed;
        self.is_refusal_confirmed |= holds;
        holds
    }

    pub fn count(&self) -> usize {
        self.waiting.len()
    }

    pub fn is_empty(&self) -> bool {
        self.waiting.is_empty()
    }

    pub fn draft(&self, index: usize) -> Draft {
        self.waiting[index].draft.clone()
    }

    pub fn keep_draft(&mut self, index: usize, draft: Draft) {
        self.waiting[index].draft = draft;
    }

    /// What the question view shows of the question at `index`: its text, and its options.
    pub fn view(&self, index: usize) -> View {
        let waiting = &self.waiting[index];
        let question = &self.call(&waiting.tool_use_id).questions()[waiting.number];
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
            full_view_line: waiting.full_view_line,
            index,
            count: self.count(),
        }
    }

    /// Answers the question at `index` with `draft`. Returns none when `draft` answers nothing,
    /// like the user's own answer left empty, and the question stays.
    pub fn answer(&mut self, index: usize, draft: &Draft) -> Option<Answered> {
        let waiting = &self.waiting[index];
        let call_index = self.call_index(&waiting.tool_use_id);
        let call = &mut self.calls[call_index];
        let questions = call.questions();
        let question = &questions[waiting.number];
        let choice = choose(question, draft)?;
        let label = question.header.clone();
        let is_complete = call.answers.len() + 1 == questions.len();
        call.answers
            .insert(question.text.clone(), Value::String(choice.clone()));
        self.waiting.remove(index);
        // Claude Code takes the answers to a call's questions together
        let reply = is_complete.then(|| {
            let call = self.calls.remove(call_index);
            let mut input = call.ask.input.clone();
            input.insert("answers".to_string(), Value::Object(call.answers));
            (call.ask, Answer::Allow { input })
        });
        Some(Answered {
            label,
            answer: choice,
            reply,
        })
    }

    /// Takes away the call the question at `index` belongs to, with all its questions, and
    /// returns what it asked, to refuse, since Claude Code refuses a call's questions together.
    pub fn remove_call(&mut self, index: usize) -> Ask {
        let tool_use_id = self.waiting[index].tool_use_id.clone();
        self.waiting
            .retain(|waiting| waiting.tool_use_id != tool_use_id);
        let call_index = self.call_index(&tool_use_id);
        self.calls.remove(call_index).ask
    }

    /// Takes away every call, and returns what they asked, to refuse.
    pub fn remove_all(&mut self) -> Vec<Ask> {
        self.is_refusal_confirmed = false;
        self.waiting.clear();
        self.calls.drain(..).map(|call| call.ask).collect()
    }

    fn call(&self, tool_use_id: &str) -> &Call {
        &self.calls[self.call_index(tool_use_id)]
    }

    fn call_index(&self, tool_use_id: &str) -> usize {
        self.calls
            .iter()
            .position(|call| call.ask.tool_use_id == tool_use_id)
            .expect("a question's call waits while the question does")
    }
}

/// What `draft` answers to `question`, none when it answers nothing. Claude gets the options'
/// labels and the user's words, never the numbers, which only the question view shows.
///
/// A question of one option takes the option the cursor is on, or else the user's own answer. A
/// question of several options takes the options ticked, and the user's own answer when it is
/// ticked, or with neither, the option the cursor is on.
fn choose(question: &Question, draft: &Draft) -> Option<String> {
    let label = |index: usize| question.options[index].label.as_str();
    let own = draft.text.trim();
    let pick = &draft.pick;
    let focused = match pick.cursor {
        Cursor::Option(index) => Some(label(index)),
        Cursor::Own | Cursor::Writing => None,
    };
    if !question.is_multi_select {
        return focused
            .map(str::to_string)
            .or_else(|| (!own.is_empty()).then(|| own.to_string()));
    }
    let mut items: Vec<&str> = pick.ticked.iter().map(|&index| label(index)).collect();
    if pick.is_own_ticked && !own.is_empty() {
        items.push(own);
    }
    if items.is_empty() {
        items.extend(focused);
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

    fn answer(questions: &mut Questions, index: usize, draft: &Draft) -> String {
        match questions.answer(index, draft) {
            Some(answered) => answered.answer,
            None => panic!("{draft:?} answers"),
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
    fn a_call_is_answered_once_all_its_questions_are() {
        let mut questions = Questions::default();
        questions.add(two_questions("a"), vec![1, 2]);
        let Some(Answered { reply, .. }) = questions.answer(0, &draft(Cursor::Option(1), ""))
        else {
            panic!("an answer");
        };
        assert!(reply.is_none());
        let mut ticked = Draft::new();
        ticked.pick.ticked = BTreeSet::from([2, 0]);
        let Some(Answered { reply, .. }) = questions.answer(0, &ticked) else {
            panic!("an answer");
        };
        let Some((_, Answer::Allow { input })) = reply else {
            panic!("the call's answers");
        };
        assert_eq!(
            input["answers"],
            json!({"Which database?": "SQLite", "Which checks?": "Lint, Types"})
        );
        assert!(questions.is_empty());
    }

    #[test]
    fn one_choice_takes_the_option_under_the_cursor_or_the_users_own_words() {
        let mut questions = Questions::default();
        questions.add(two_questions("a"), vec![1, 2]);
        questions.add(two_questions("b"), vec![1, 2]);
        for empty in [draft(Cursor::Own, ""), draft(Cursor::Writing, "  ")] {
            assert!(questions.answer(0, &empty).is_none(), "{empty:?}");
        }
        assert_eq!(
            answer(
                &mut questions,
                0,
                &draft(Cursor::Writing, " DuckDB, in a file ")
            ),
            "DuckDB, in a file"
        );
        // An option under the cursor answers over the user's own words
        assert_eq!(
            answer(&mut questions, 1, &draft(Cursor::Option(0), "DuckDB")),
            "Postgres"
        );
    }

    #[test]
    fn several_choices_take_the_ticked_options_and_the_users_own_words() {
        let mut questions = Questions::default();
        for id in ["a", "b", "c", "d"] {
            questions.add(two_questions(id), vec![1, 2]);
        }
        assert!(questions.answer(1, &draft(Cursor::Own, "")).is_none());
        let mut ticked = draft(Cursor::Option(0), "fuzzing");
        ticked.pick.ticked = BTreeSet::from([1, 0]);
        ticked.pick.is_own_ticked = true;
        assert_eq!(answer(&mut questions, 1, &ticked), "Lint, Tests, fuzzing");
        // The user's own words stay out while unticked
        assert_eq!(
            answer(&mut questions, 2, &draft(Cursor::Option(1), "fuzzing")),
            "Tests"
        );
        // With nothing ticked, the option under the cursor
        assert_eq!(
            answer(&mut questions, 3, &draft(Cursor::Option(2), "")),
            "Types"
        );
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
    fn refusing_a_question_takes_away_its_whole_call() {
        let mut questions = Questions::default();
        questions.add(two_questions("a"), vec![1, 2]);
        questions.add(one_question("b"), vec![1]);
        let ask = questions.remove_call(1);
        assert_eq!(ask.tool_use_id, "a");
        assert_eq!(questions.count(), 1);
        assert_eq!(questions.view(0).label, "Cache");
    }
}
