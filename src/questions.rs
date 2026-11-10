//! The questions of Claude's that wait for the user, oldest first: each plan to approve, and each
//! question of an `AskUserQuestion` call. The question view shows them one at a time, and each
//! keeps what was typed to it until it is answered.
//!
//! An answer's first line is read strictly, and an answer that does not read as one answers
//! nothing and stays in the box, so nothing is ever guessed. The lines below it are the user's
//! notes on a question, never read, like the body below a commit message's subject.

use serde_json::{Map, Value, json};

use crate::keys::View;
use crate::protocol::{Answer, Ask, AskKind, Question};
use crate::render::{plan_lines, question_lines};

#[derive(Default)]
pub struct Questions {
    calls: Vec<Call>,
    waiting: Vec<Waiting>,
    /// Whether the user pressed Enter on a message once while these questions wait, so the next
    /// Enter refuses them.
    is_refusal_confirmed: bool,
}

/// A call that asks, with the answers and notes it has so far, each under its question's text.
struct Call {
    ask: Ask,
    answers: Map<String, Value>,
    annotations: Map<String, Value>,
}

/// A question that waits.
struct Waiting {
    /// The call it belongs to.
    tool_use_id: String,
    kind: Kind,
    /// The line of the full view, from 1, that it starts on.
    full_view_line: usize,
    /// What was typed to it so far.
    draft: String,
}

#[derive(Clone, Copy)]
enum Kind {
    Plan,
    /// The question of the call's at this index.
    Question(usize),
}

/// A question answered, which the conversation shows as `answer` after `label`, and the notes
/// after it. `reply` holds its call's answer once the call has all of them.
pub struct Answered {
    pub label: String,
    pub answer: String,
    pub notes: Option<String>,
    pub reply: Option<(Ask, Answer)>,
}

impl Questions {
    /// Adds the questions of a call that asks, each starting on its line of `full_view_lines`. A
    /// permission has none, since it is refused as it arrives.
    pub fn add(&mut self, ask: Ask, full_view_lines: Vec<usize>) {
        let kinds: Vec<Kind> = match &ask.kind {
            AskKind::Permission => return,
            AskKind::Plan(_) => vec![Kind::Plan],
            AskKind::Questions(questions) => (0..questions.len()).map(Kind::Question).collect(),
        };
        self.is_refusal_confirmed = false;
        for (kind, full_view_line) in kinds.into_iter().zip(full_view_lines) {
            self.waiting.push(Waiting {
                tool_use_id: ask.tool_use_id.clone(),
                kind,
                full_view_line,
                draft: String::new(),
            });
        }
        self.calls.push(Call {
            ask,
            answers: Map::new(),
            annotations: Map::new(),
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

    pub fn draft(&self, index: usize) -> &str {
        &self.waiting[index].draft
    }

    pub fn keep_draft(&mut self, index: usize, text: String) {
        self.waiting[index].draft = text;
    }

    /// What the question view shows of the question at `index`. The label shows the forms an
    /// answer takes in the notation of a command's usage line: keys apart with `/` for one of
    /// them, as in a `y/n` prompt, keys run together for any of them run together, as in
    /// `ls -la`, `<…>` for words that must follow and `[…]` for words that may.
    pub fn view(&self, index: usize) -> View {
        let waiting = &self.waiting[index];
        let call = self.call(&waiting.tool_use_id);
        let (label, lines) = match (waiting.kind, &call.ask.kind) {
            (Kind::Question(number), AskKind::Questions(questions)) => {
                let question = &questions[number];
                let keys = ('a'..).take(question.options.len());
                let forms = if question.is_multi_select {
                    format!("{} [more, …]", keys.collect::<String>())
                } else {
                    let own = own_key(question);
                    let keys: Vec<String> = keys.map(String::from).collect();
                    format!("{}, or {own} <own>", keys.join("/"))
                };
                (
                    format!("{}? {forms}", question.header),
                    question_lines(question),
                )
            }
            // Claude Code's approval of a plan carries no text, so only a refusal takes words
            (_, AskKind::Plan(plan)) => ("approve? y, or n [reason]".to_string(), plan_lines(plan)),
            _ => unreachable!("a question has the kind of its call"),
        };
        View {
            label,
            lines,
            full_view_line: waiting.full_view_line,
            index,
            count: self.count(),
        }
    }

    /// Answers the question at `index` with `text`, or answers nothing when the text does not
    /// read as an answer, and the question keeps it.
    pub fn answer(&mut self, index: usize, text: &str) -> Option<Answered> {
        let waiting = &self.waiting[index];
        match waiting.kind {
            Kind::Plan => self.answer_plan(index, text),
            Kind::Question(number) => {
                let (line, notes) = text.trim().split_once('\n').unwrap_or((text, ""));
                let notes = notes.trim();
                let tool_use_id = waiting.tool_use_id.clone();
                let call_index = self.call_index(&tool_use_id);
                let call = &mut self.calls[call_index];
                let AskKind::Questions(questions) = &call.ask.kind else {
                    unreachable!("a question has the kind of its call");
                };
                let question = &questions[number];
                let choice = choose(question, line)?;
                let label = question.header.clone();
                call.answers
                    .insert(question.text.clone(), Value::String(choice.clone()));
                // Undocumented, read in Claude Code 2.1.318: the answer Claude reads ends with
                // `notes: ` and the notes under the question's text in `annotations`
                if !notes.is_empty() {
                    call.annotations
                        .insert(question.text.clone(), json!({"notes": notes}));
                }
                // Claude Code takes the answers to a call's questions together
                let is_complete = call.answers.len() == questions.len();
                self.waiting.remove(index);
                let reply = is_complete.then(|| {
                    let call = self.calls.remove(call_index);
                    let mut input = call.ask.input.clone();
                    input.insert("answers".to_string(), Value::Object(call.answers));
                    if !call.annotations.is_empty() {
                        input.insert("annotations".to_string(), Value::Object(call.annotations));
                    }
                    (call.ask, Answer::Allow { input })
                });
                Some(Answered {
                    label,
                    answer: choice,
                    notes: (!notes.is_empty()).then(|| notes.to_string()),
                    reply,
                })
            }
        }
    }

    /// Answers the plan at `index`: `y` or `yes` alone approves it, and `n` or `no` refuses it,
    /// with the words after it as the reason. A refusal with no reason stops Claude's turn, and
    /// one with a reason lets Claude go on planning from it.
    fn answer_plan(&mut self, index: usize, text: &str) -> Option<Answered> {
        let (first, reason) = split_first_word(text);
        let answer = match first.to_lowercase().as_str() {
            "y" | "yes" if reason.is_empty() => None,
            "n" | "no" if reason.is_empty() => Some(Answer::Deny),
            "n" | "no" => Some(Answer::DenyFor(reason.to_string())),
            _ => return None,
        };
        let word = match &answer {
            None => "approved".to_string(),
            Some(Answer::DenyFor(reason)) => format!("refused: {reason}"),
            Some(_) => "refused".to_string(),
        };
        let ask = self.remove_call(index);
        let answer = answer.unwrap_or_else(|| Answer::Allow {
            input: ask.input.clone(),
        });
        Some(Answered {
            label: "plan".to_string(),
            answer: word,
            notes: None,
            reply: Some((ask, answer)),
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

/// The key of the answer of the user's own to a question of one choice: the letter after its
/// options'. Claude Code accepts any text as an answer, and offers it on every question.
pub fn own_key(question: &Question) -> char {
    ('a'..)
        .nth(question.options.len())
        .expect("a question has at most 4 options")
}

/// The first word of `text` and the rest, both trimmed. A comma, colon or period right after the
/// first word only parts it from the rest, as in `no, use SQLite`.
fn split_first_word(text: &str) -> (&str, &str) {
    let text = text.trim();
    let (first, rest) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
    let first = first.trim_end_matches([',', ':', '.']);
    (first, rest.trim())
}

/// What `line` answers to `question`, none when it does not read as an answer. Claude gets the
/// options' labels and the user's words, never the keys, which only the question view shows.
///
/// A question of one choice takes one key alone, or the own key and the user's own answer after
/// it, so an answer of the user's own is never taken for a key. A question of several choices
/// takes keys run together, or `-` for none, and after them more items of the user's own, apart
/// with commas, since an item can be several words.
fn choose(question: &Question, line: &str) -> Option<String> {
    let (first, rest) = split_first_word(line);
    let options = &question.options;
    let index_of = |key: char| {
        let index = usize::try_from(u32::from(key).checked_sub(u32::from('a'))?).ok()?;
        (index < options.len()).then_some(index)
    };
    if !question.is_multi_select {
        let mut keys = first.chars().flat_map(char::to_lowercase);
        let key = keys.next()?;
        if keys.next().is_some() {
            return None;
        }
        return match (index_of(key), rest.is_empty()) {
            (Some(index), true) => Some(options[index].label.clone()),
            (None, false) if key == own_key(question) => Some(rest.to_string()),
            _ => None,
        };
    }
    let mut picks = if first == "-" {
        vec![]
    } else {
        first
            .chars()
            .flat_map(char::to_lowercase)
            .map(index_of)
            .collect::<Option<Vec<usize>>>()?
    };
    picks.sort_unstable();
    picks.dedup();
    let items: Vec<&str> = picks
        .iter()
        .map(|&index| options[index].label.as_str())
        .chain(
            rest.split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty()),
        )
        .collect();
    (!items.is_empty()).then(|| items.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::read_request;

    fn ask(id: &str, input: &Value) -> Ask {
        let tool_name = if input.get("plan").is_some() {
            "ExitPlanMode"
        } else {
            "AskUserQuestion"
        };
        let request = json!({
            "subtype": "can_use_tool",
            "tool_name": tool_name,
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

    fn plan(id: &str) -> Ask {
        ask(id, &json!({"plan": "# Plan"}))
    }

    fn answer(questions: &mut Questions, index: usize, text: &str) -> String {
        match questions.answer(index, text) {
            Some(answered) => answered.answer,
            None => panic!("{text:?} answers"),
        }
    }

    #[test]
    fn the_label_shows_the_forms_an_answer_takes() {
        let mut questions = Questions::default();
        questions.add(two_questions("a"), vec![1, 2]);
        questions.add(plan("b"), vec![1]);
        assert_eq!(questions.count(), 3);
        assert_eq!(questions.view(0).label, "Database? a/b, or c <own>");
        assert_eq!(questions.view(1).label, "Checks? abc [more, …]");
        assert_eq!(questions.view(2).label, "approve? y, or n [reason]");
    }

    #[test]
    fn a_call_is_answered_once_all_its_questions_are() {
        let mut questions = Questions::default();
        questions.add(two_questions("a"), vec![1, 2]);
        let Some(Answered { reply, .. }) = questions.answer(0, "B\nit is one file") else {
            panic!("an answer");
        };
        assert!(reply.is_none());
        let Some(Answered { reply, .. }) = questions.answer(0, "ca") else {
            panic!("an answer");
        };
        let Some((_, Answer::Allow { input })) = reply else {
            panic!("the call's answers");
        };
        assert_eq!(
            input["answers"],
            json!({"Which database?": "SQLite", "Which checks?": "Lint, Types"})
        );
        assert_eq!(
            input["annotations"],
            json!({"Which database?": {"notes": "it is one file"}})
        );
        assert!(questions.is_empty());
    }

    #[test]
    fn one_choice_takes_a_key_alone_or_the_own_key_and_words() {
        let mut questions = Questions::default();
        questions.add(two_questions("a"), vec![1, 2]);
        for text in ["", "ab", "a fast one", "c", "d", "apple", "1"] {
            assert!(questions.answer(0, text).is_none(), "{text:?}");
        }
        assert_eq!(
            answer(&mut questions, 0, "c DuckDB, in a file"),
            "DuckDB, in a file"
        );
    }

    #[test]
    fn several_choices_take_keys_run_together_and_more_items_apart_with_commas() {
        let mut questions = Questions::default();
        questions.add(two_questions("a"), vec![1, 2]);
        questions.add(two_questions("b"), vec![1, 2]);
        questions.add(two_questions("c"), vec![1, 2]);
        for text in ["", "d", "abd", "-", "lint"] {
            assert!(questions.answer(1, text).is_none(), "{text:?}");
        }
        assert_eq!(
            answer(&mut questions, 1, "ba property tests, fuzzing"),
            "Lint, Tests, property tests, fuzzing"
        );
        assert_eq!(answer(&mut questions, 2, "- fuzzing"), "fuzzing");
        // Keys apart are the first key and an item of the user's own, as written
        assert_eq!(answer(&mut questions, 3, "a b c"), "Lint, b c");
    }

    #[test]
    fn a_plan_takes_y_alone_or_n_with_a_reason_or_none() {
        let mut questions = Questions::default();
        questions.add(plan("a"), vec![1]);
        for text in ["", "yes and also tests", "y but", "sure", "nope"] {
            assert!(questions.answer(0, text).is_none(), "{text:?}");
        }
        let Some(Answered { answer, reply, .. }) = questions.answer(0, "No, use SQLite") else {
            panic!("an answer");
        };
        assert_eq!(answer, "refused: use SQLite");
        assert!(matches!(reply, Some((_, Answer::DenyFor(reason))) if reason == "use SQLite"));
        questions.add(plan("b"), vec![1]);
        let Some(Answered { reply, .. }) = questions.answer(0, "n") else {
            panic!("an answer");
        };
        assert!(matches!(reply, Some((_, Answer::Deny))));
        questions.add(plan("c"), vec![1]);
        let Some(Answered { reply, .. }) = questions.answer(0, "Yes") else {
            panic!("an answer");
        };
        assert!(matches!(reply, Some((_, Answer::Allow { .. }))));
    }

    #[test]
    fn a_message_refuses_the_questions_only_the_second_time_and_none_unseen() {
        let mut questions = Questions::default();
        assert!(!questions.holds_message());
        questions.add(plan("a"), vec![1]);
        assert!(questions.holds_message());
        assert!(questions.is_refusal_confirmed());
        questions.add(plan("b"), vec![2]);
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
        questions.add(plan("b"), vec![1]);
        let ask = questions.remove_call(1);
        assert_eq!(ask.tool_use_id, "a");
        assert_eq!(questions.count(), 1);
        assert_eq!(questions.view(0).label, "approve? y, or n [reason]");
    }
}
