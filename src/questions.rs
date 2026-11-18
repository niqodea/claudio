//! The requests of Claude's that wait for the user: the questions of an `AskUserQuestion` call,
//! a plan to carry out, or a call to allow. Each request goes to Claude Code as soon as it is
//! answered, apart from the others, and the question view shows one request at a time.
//!
//! Claude Code takes a call's answers together, and Claude reads none of them before, so the
//! question view shows a call's questions as a batch. Enter submits a question and moves on to
//! the next one not submitted, and a submitted question keeps its answer until Enter opens it
//! again. Once all are submitted, the review shows the answers, all ticked: Enter sends them, or
//! opens the ones the user unticked, which keep what was picked and written on them.
//!
//! A plan and a permission are requests of one question, which Enter sends at once: allow, or
//! refuse, which stops Claude's turn, or the user's own answer, which refuses the call with the
//! user's words for Claude to go on from.
//!
//! The question view keeps the user's place: each request keeps the question it shows, and the
//! review and each question their drafts, so going to another request, or back to the message,
//! and coming back finds everything as the user left it.
//!
//! Ref: <https://design-system.service.gov.uk/patterns/check-answers>, on checking answers
//! before sending them

use serde_json::{Map, Value};

use crate::cut;
use crate::keys::{
    ABOUT_ROWS, BLOCK_ROWS, Field, INPUT_ROWS, Mark, PREVIEWED_QUESTION_ROWS, Tone, View, questions,
};
use crate::picker::{self, Choice, Cursor, Draft, Form, Move, Pick};
use crate::protocol::{Answer, Ask, AskKind, Permission, Question};

#[derive(Default)]
pub struct Questions {
    /// The requests that wait, oldest first.
    requests: Vec<Request>,
    /// The request the question view shows, from 0.
    shown: usize,
}

/// A request of Claude's, with each of its questions.
struct Request {
    ask: Ask,
    /// The subagent that asked, by the description of the call that started it, none for Claude
    /// itself. Only a permission can come from a subagent, since Claude Code gives no subagent
    /// `AskUserQuestion` or `ExitPlanMode`, so only a permission shows it.
    /// Seen with Claude Code 2.1.322: a subagent asked to call either reported neither among its
    /// tools.
    agent: Option<String>,
    entries: Vec<Entry>,
    /// The question the question view shows, from 0, where one past the last is the review.
    shown: usize,
    /// What was picked on the review.
    review: Draft,
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
    /// The request still waits for answers.
    Show,
    /// The request is answered: each question's header with its answer, none for a plan or a
    /// permission, and the reply to its call.
    Answered {
        answers: Vec<(String, String)>,
        /// Boxed, since the input of a call keeps its fields' order, which takes room.
        ask: Box<Ask>,
        answer: Answer,
    },
}

/// The index of the option that allows a call or a plan, which `y` names, and the one that
/// refuses it, which `n` names.
const ALLOW: usize = 0;
const REFUSE: usize = 1;

impl Request {
    /// The questions of a call that asks, none for a plan or a permission.
    fn questions(&self) -> &[Question] {
        match &self.ask.kind {
            AskKind::Questions(questions) => questions,
            AskKind::Permission(_) | AskKind::Plan(_) => &[],
        }
    }

    fn is_decision(&self) -> bool {
        !matches!(self.ask.kind, AskKind::Questions(_))
    }

    fn is_submitted(&self) -> bool {
        self.entries.iter().all(|entry| entry.is_submitted)
    }

    /// Moves on from the question at `index` to the first one not submitted after it, or else
    /// to the review, with every answer ticked.
    fn move_on(&mut self, index: usize) {
        self.shown = self.next_open(index);
        if self.shown == self.entries.len() {
            self.review = all_ticked(self.entries.len());
        }
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

    /// The title of every request of its kind, and nothing else, so the title reads the same on
    /// every request of a kind: what is particular to one request is on the label row, which
    /// names it, and in the block, which shows it.
    fn kind(&self) -> &'static str {
        match &self.ask.kind {
            AskKind::Questions(_) => "Question",
            AskKind::Plan(_) => "Plan",
            AskKind::Permission(_) => "Permission",
        }
    }

    /// The name of a plan or a permission on the label row, which says which one of its kind it
    /// is, as a question's header does: the tool's name as Claude Code shows it to the user, and
    /// for a plan, its kind, since a plan has no name of its own that Claude Code sends.
    fn label(&self) -> String {
        match &self.ask.kind {
            AskKind::Permission(permission) => permission.name.clone(),
            AskKind::Plan(_) => self.kind().to_string(),
            AskKind::Questions(_) => unreachable!("a question's label is its header"),
        }
    }

    /// What refusing the request refuses.
    fn refusal(&self) -> String {
        match &self.ask.kind {
            AskKind::Questions(questions_asked) => questions(questions_asked.len()),
            AskKind::Plan(_) => "the plan".to_string(),
            AskKind::Permission(_) => "the call".to_string(),
        }
    }

    /// What the question view shows of a plan or a permission: the plan, or the call's fields
    /// and then the lines about the request, in the block under the title, and its options,
    /// which ask whether to allow it.
    fn decision_view(&self) -> (Vec<Field>, Vec<Choice>) {
        let fields = match &self.ask.kind {
            AskKind::Plan(plan) => vec![text_field(plan, Tone::Plain, BLOCK_ROWS)],
            AskKind::Permission(permission) => {
                let mut fields = call_fields(
                    &self.ask.tool_name,
                    &self.ask.input,
                    permission.server.as_deref(),
                );
                fields.extend(about_fields(
                    reason(permission).as_deref(),
                    self.agent.as_deref(),
                ));
                fields
            }
            AskKind::Questions(_) => unreachable!("a call's questions have options of their own"),
        };
        let keyed = |label: &str, key: char| Choice {
            key: Some(key),
            ..Choice::new(label, "", None)
        };
        // A call Claude Code deems dangerous says so on its approval, which waits for a second
        // Enter, so no stray key allows it
        let approval = match &self.ask.kind {
            AskKind::Permission(permission) if permission.is_guarded => Choice {
                risk: Some("allows a call Claude Code deems dangerous".to_string()),
                ..keyed("allow (dangerous)", 'y')
            },
            _ => keyed("allow", 'y'),
        };
        let choices = vec![approval, keyed("refuse, and stop Claude", 'n')];
        (fields, choices)
    }

    /// The answer that `draft` gives to a plan or a permission, none when it gives none, like
    /// the user's own answer left empty.
    fn decide(&self, draft: &Draft) -> Option<Answer> {
        let chosen = draft.pick.chosen(Form::One, &draft.text);
        match chosen.options.first() {
            Some(&ALLOW) => Some(Answer::Allow {
                input: self.ask.input.clone(),
            }),
            Some(_) => Some(Answer::Deny),
            None if chosen.is_own => Some(Answer::Explain {
                reason: draft.text.trim().to_string(),
            }),
            None => None,
        }
    }
}

/// A field of a tool's layout.
struct Laid {
    name: &'static str,
    rows: usize,
    /// The sign in front of the field, and whether it marks each of its rows.
    sign: char,
    every_row: bool,
    tone: Tone,
    /// Whether the value is a file's path or a URL.
    is_path: bool,
    /// claudio's note when the call leaves the field out.
    missing: &'static str,
    /// The value as it shows.
    show: fn(&Value) -> String,
}

impl Laid {
    /// A field that shows its value as it is, which is no path, and `not given` when left out.
    const fn new(name: &'static str, rows: usize, sign: char, every_row: bool, tone: Tone) -> Self {
        Self {
            name,
            rows,
            sign,
            every_row,
            tone,
            is_path: false,
            missing: "not given",
            show: value_text,
        }
    }
}

/// `value` as text: a string as it is, anything else as JSON.
fn value_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        _ => value.to_string(),
    }
}

/// Bash's `timeout`, in milliseconds, in the units a person reads it in, like `90s` or `2m`.
fn timeout_text(value: &Value) -> String {
    let Some(milliseconds) = value.as_u64() else {
        return value_text(value);
    };
    let seconds = milliseconds / 1000;
    match (milliseconds % 1000, seconds / 60, seconds % 60) {
        (0, 0, seconds) => format!("{seconds}s"),
        (0, minutes, 0) => format!("{minutes}m"),
        (0, minutes, seconds) => format!("{minutes}m {seconds}s"),
        _ => format!("{milliseconds}ms"),
    }
}

/// Bash's `run_in_background`, as where the command runs.
fn background_text(value: &Value) -> String {
    match value {
        Value::Bool(true) => "in background".to_string(),
        Value::Bool(false) => "in foreground".to_string(),
        _ => value_text(value),
    }
}

/// The fields of a call of these tools, in the order they show, at the top of the block, each
/// marked with the sign that reads as what it is: a shell's `$` in front of a command, `#` in
/// front of a comment and `&` in front of where it runs, a diff's `-` and `+` in front of what
/// an edit takes out and puts in, `@` in front of where a call acts, `│` in front of a body of
/// text, and `t` in front of a timeout. The field that matters most, like a command, or what an
/// edit puts in, which allowing it writes, takes the most rows, and the rows of a layout add up
/// to `INPUT_ROWS`, so every field of it shows on a call that gives them all. A call of any other
/// tool shows each field on a row of its own, by its name.
const LAYOUTS: [(&str, &[Laid]); 4] = [
    (
        "Bash",
        &[
            Laid::new("command", 3, '$', false, Tone::Plain),
            Laid::new("description", 1, '#', false, Tone::Comment),
            Laid {
                missing: "default",
                show: timeout_text,
                ..Laid::new("timeout", 1, 't', false, Tone::Plain)
            },
            Laid {
                missing: "in foreground",
                show: background_text,
                ..Laid::new("run_in_background", 1, '&', false, Tone::Plain)
            },
        ],
    ),
    (
        "Edit",
        &[
            Laid {
                is_path: true,
                ..Laid::new("file_path", 1, '@', false, Tone::Plain)
            },
            Laid::new("old_string", 2, '-', true, Tone::Removed),
            Laid::new("new_string", 3, '+', true, Tone::Added),
        ],
    ),
    (
        "Write",
        &[
            Laid {
                is_path: true,
                ..Laid::new("file_path", 1, '@', false, Tone::Plain)
            },
            Laid::new("content", 5, '│', true, Tone::Plain),
        ],
    ),
    (
        "WebFetch",
        &[
            Laid {
                is_path: true,
                ..Laid::new("url", 1, '@', false, Tone::Plain)
            },
            Laid::new("prompt", 5, '│', true, Tone::Plain),
        ],
    ),
];

/// The name of the field that names the server of an MCP tool. Only the word MCP tells the user
/// that the tool comes from an MCP server, so the name says it, and the value says which one.
const MCP_SERVER: &str = "mcp_server";

/// A field of `rows` rows that shows `value` in `tone`, after `mark`.
fn field(mark: Mark, value: String, tone: Tone, rows: usize) -> Field {
    Field {
        mark,
        value,
        is_path: false,
        tone,
        rows,
    }
}

/// The mark `sign`, in front of each row of the value on `every_row`. A sign is dim, but for a
/// diff's, whose red and green say what it takes out and puts in.
fn sign(sign: char, every_row: bool, tone: Tone) -> Mark {
    Mark::Sign {
        sign,
        every_row,
        tone: match tone {
            Tone::Removed | Tone::Added => tone,
            _ => Tone::Comment,
        },
    }
}

/// The fields of a call of `tool` with `input`, in `INPUT_ROWS` rows: first the MCP server the
/// tool comes from, if any, by `MCP_SERVER`, then the fields of the tool's layout, then the
/// others, by name. Each field keeps its rows when its value is shorter, so a field shows in the
/// same place on every call of the tool, and rows the fields leave are left empty. The fields
/// are fitted by `cut::fit`: a field the layout lacks, like Bash's `timeout`, takes a row from
/// the last field of more rows, and the last row swallows the fields that still do not fit.
fn call_fields(tool: &str, input: &Map<String, Value>, mcp_server: Option<&str>) -> Vec<Field> {
    let name = |name: &str| Mark::Name(name.to_string());
    let layout = LAYOUTS
        .iter()
        .find(|(name, _)| *name == tool)
        .map_or(&[][..], |(_, layout)| *layout);
    // Edit's `replace_all` changes what the file's line means, so it goes with it
    let is_folded = |name: &str| tool == "Edit" && name == "replace_all";
    let mut fields: Vec<Field> = (mcp_server.iter())
        .map(|server| field(name(MCP_SERVER), (*server).to_string(), Tone::Plain, 1))
        .collect();
    for laid in layout {
        let mark = sign(laid.sign, laid.every_row, laid.tone);
        let mut shown = match input.get(laid.name) {
            Some(value) => Field {
                is_path: laid.is_path,
                ..field(mark, (laid.show)(value), laid.tone, laid.rows)
            },
            None => field(mark, laid.missing.to_string(), Tone::Note, laid.rows),
        };
        if laid.name == "file_path" && input.get("replace_all") == Some(&Value::Bool(true)) {
            shown.value.push_str(" (all matches)");
        }
        fields.push(shown);
    }
    fields.extend(
        (input.iter())
            .filter(|(field_name, _)| {
                !is_folded(field_name) && !layout.iter().any(|laid| laid.name == *field_name)
            })
            .map(|(field_name, value)| field(name(field_name), value_text(value), Tone::Plain, 1)),
    );
    if layout.is_empty() && input.is_empty() {
        fields.push(field(name("input"), "none".to_string(), Tone::Note, 1));
    }
    let mut fields = cut::fit(&fields, INPUT_ROWS, |_| 0);
    let taken = fields.iter().map(|field| field.rows).sum::<usize>();
    if taken < INPUT_ROWS {
        fields.push(field(
            Mark::None,
            String::new(),
            Tone::Plain,
            INPUT_ROWS - taken,
        ));
    }
    fields
}

/// The lines about a permission request, in its `ABOUT_ROWS` rows at the block's end: why Claude
/// Code asks, after `?`, then who asked, after `↳`, the subagent or the main agent. Who asked
/// comes last, so a short screen, which cuts the block from its end, hides it before the reason,
/// since it matters less to the answer. Each is always there, as a grey note when Claude Code
/// says nothing, so that both keep their rows on every request.
fn about_fields(reason: Option<&str>, agent: Option<&str>) -> [Field; ABOUT_ROWS] {
    let line = |mark: char, value: Option<&str>, missing: &str| {
        let (value, tone) = match value {
            Some(value) => (value, Tone::Plain),
            None => (missing, Tone::Note),
        };
        field(sign(mark, false, Tone::Plain), value.to_string(), tone, 1)
    };
    [
        line('?', reason, "no reason given"),
        line('↳', agent, "main agent"),
    ]
}

/// Why Claude Code asks about a call: its `decision_reason`, or else a sentence for its
/// `decision_reason_type`, or else its `blocked_path`. None when it sends none of them.
fn reason(permission: &Permission) -> Option<String> {
    (permission.reason.clone())
        .or_else(|| permission.reason_type.as_deref().map(reason_type_text))
        .or_else(|| (permission.blocked_path.as_ref()).map(|path| format!("blocked path {path}")))
}

/// A sentence that says which check asks about the call, for four values of
/// `decision_reason_type`. Each of them is a bare name, like `rule`, that does not say what
/// asked, and four sentences are little to keep.
/// Ref: `SDKControlPermissionRequest` in `sdk.d.ts`, which lists the values
fn reason_type_text(reason_type: &str) -> String {
    match reason_type {
        "rule" => "A rule in your settings asks before this call",
        "mode" => "The permission mode asks before this call",
        "subcommandResults" => "Some parts of this command need approval",
        "permissionPromptTool" => "The permission prompt tool asks about this call",
        other => other,
    }
    .to_string()
}

/// The review's draft with each of `count` answers ticked.
fn all_ticked(count: usize) -> Draft {
    Draft {
        pick: Pick {
            cursor: Cursor::Option(0),
            ticked: (0..count).collect(),
            is_own_ticked: false,
        },
        text: String::new(),
    }
}

/// The block under the title of a question, of the review or of a plan: its text, in `tone`, in
/// `rows` rows.
fn text_field(text: &str, tone: Tone, rows: usize) -> Field {
    Field {
        mark: Mark::None,
        value: text.to_string(),
        is_path: false,
        tone,
        rows,
    }
}

impl Questions {
    /// Adds a request of Claude's, made by the subagent `agent`, none for Claude itself, with
    /// each of its questions starting on its line of `full_view_lines`. A guarded call opens on
    /// the refusal, so that an Enter by habit does not allow it.
    pub fn add(&mut self, ask: Ask, agent: Option<String>, full_view_lines: Vec<usize>) {
        let first = match &ask.kind {
            AskKind::Permission(permission) if permission.is_guarded => Cursor::Option(REFUSE),
            _ => Cursor::Option(0),
        };
        let count = match &ask.kind {
            AskKind::Questions(questions) => questions.len(),
            AskKind::Permission(_) | AskKind::Plan(_) => 1,
        };
        let entries: Vec<Entry> = full_view_lines
            .into_iter()
            .take(count)
            .map(|full_view_line| {
                let mut draft = Draft::new();
                draft.pick.cursor = first;
                Entry {
                    full_view_line,
                    draft,
                    is_submitted: false,
                }
            })
            .collect();
        let review = all_ticked(entries.len());
        self.requests.push(Request {
            ask,
            agent,
            entries,
            shown: 0,
            review,
        });
    }

    /// How many requests wait. A call's questions count as one request.
    pub fn count(&self) -> usize {
        self.requests.len()
    }

    /// Opens the question view on the request it showed last, or on the last request when that
    /// one is gone, and returns whether any request waits.
    pub fn open(&mut self) -> bool {
        let Some(last) = self.requests.len().checked_sub(1) else {
            return false;
        };
        self.shown = self.shown.min(last);
        true
    }

    /// Moves the question view as `to` says. Next and previous go round the request, where the
    /// review comes after the last question once all are submitted, and a question the request
    /// lacks keeps the one shown. Moves between requests go round them too, onto the question
    /// each request showed last, and a request that does not wait keeps the one shown.
    pub fn go(&mut self, to: Move) {
        let count = self.requests.len();
        match to {
            Move::NextRequest => self.shown = (self.shown + 1) % count,
            Move::PreviousRequest => self.shown = (self.shown + count - 1) % count,
            Move::ToRequest(request) if request < count => self.shown = request,
            Move::ToRequest(_) => {}
            Move::Next | Move::Previous | Move::To(_) => {
                let request = self.request_mut();
                let index = request.shown;
                let stops = request.entries.len() + usize::from(request.is_submitted());
                let next = match to {
                    Move::Next => (index + 1) % stops,
                    Move::Previous => (index + stops - 1) % stops,
                    Move::To(question) if question < request.entries.len() => question,
                    _ => index,
                };
                request.shown = next;
            }
        }
    }

    /// The draft of the question shown, or of the review.
    pub fn draft(&self) -> Draft {
        let request = self.request();
        match request.entries.get(request.shown) {
            Some(entry) => entry.draft.clone(),
            None => request.review.clone(),
        }
    }

    /// Keeps `draft` on the question shown, or on the review.
    pub fn keep_draft(&mut self, draft: Draft) {
        let request = self.request_mut();
        match request.entries.get_mut(request.shown) {
            Some(entry) => entry.draft = draft,
            None => request.review = draft,
        }
    }

    /// What the question view shows: a question's text and its options, the call or the plan
    /// asked about with the options to allow or refuse it, or the review's answers.
    pub fn view(&self) -> View {
        let request = self.request();
        let index = request.shown;
        let progress: Vec<bool> = (request.entries.iter())
            .map(|entry| entry.is_submitted)
            .collect();
        let requests: Vec<bool> = self.requests.iter().map(Request::is_submitted).collect();
        let view = |label: String, choices, own_label: &str, form| View {
            label,
            title: request.kind().to_string(),
            refusal: request.refusal(),
            request_index: self.shown,
            requests: requests.clone(),
            fields: Vec::new(),
            choices,
            own_label: own_label.to_string(),
            form,
            full_view_line: request.entries[index.min(request.entries.len() - 1)].full_view_line,
            index,
            sends_at_once: progress.len() == 1,
            progress: progress.clone(),
            is_submitted: request
                .entries
                .get(index)
                .is_some_and(|entry| entry.is_submitted),
        };
        if request.is_decision() {
            let (fields, choices) = request.decision_view();
            let own_label = "refuse, and tell Claude why";
            return View {
                fields,
                ..view(request.label(), choices, own_label, Form::One)
            };
        }
        if index == request.entries.len() {
            let choices = (request.answers().into_iter())
                .map(|(header, answer)| Choice::new(&format!("{header}: {answer}"), "", None))
                .collect();
            return View {
                fields: vec![text_field("Send these answers?", Tone::Bold, BLOCK_ROWS)],
                ..view("Review".to_string(), choices, "", Form::Review)
            };
        }
        let question = &request.questions()[index];
        let choices: Vec<Choice> = (question.options.iter())
            .map(|choice| Choice::new(&choice.label, &choice.description, choice.preview.clone()))
            .collect();
        // The preview takes the block's rows after the question's
        let rows = if picker::has_previews(&choices) {
            PREVIEWED_QUESTION_ROWS
        } else {
            BLOCK_ROWS
        };
        let (own_label, form) = if question.is_multi_select {
            ("more of your own", Form::Several)
        } else {
            ("your own answer", Form::One)
        };
        View {
            fields: vec![text_field(&question.text, Tone::Bold, rows)],
            ..view(question.header.clone(), choices, own_label, form)
        }
    }

    /// Acts on Enter with `draft` on the question shown. A plan or a permission is answered at
    /// once. Else Enter submits a question and moves on, or opens a submitted one again, and
    /// keeps a question that `draft` answers nothing on, like the user's own answer left empty.
    /// A batch of one question has nothing to review, so its answer goes at once. On the review,
    /// it answers the batch once every answer is ticked, and else opens the unticked ones.
    pub fn send(&mut self, draft: Draft) -> Sent {
        let request = self.request_mut();
        let index = request.shown;
        if request.is_decision() {
            let Some(answer) = request.decide(&draft) else {
                request.entries[0].draft = draft;
                return Sent::Show;
            };
            let ask = Box::new(self.requests.remove(self.shown).ask);
            return Sent::Answered {
                answers: Vec::new(),
                ask,
                answer,
            };
        }
        let count = request.entries.len();
        if index == count {
            if draft.pick.ticked.len() == count {
                return self.answer();
            }
            for (number, entry) in request.entries.iter_mut().enumerate() {
                entry.is_submitted = draft.pick.ticked.contains(&number);
            }
            request.move_on(count - 1);
            return Sent::Show;
        }
        let answers = choose(&request.questions()[index], &draft).is_some();
        let entry = &mut request.entries[index];
        entry.draft = draft;
        if entry.is_submitted || !answers {
            entry.is_submitted = false;
            return Sent::Show;
        }
        entry.is_submitted = true;
        if count == 1 {
            return self.answer();
        }
        request.move_on(index);
        Sent::Show
    }

    /// Takes away the request the question view shows, and returns what it asked, to refuse,
    /// since Claude Code refuses a call's questions together.
    pub fn remove_shown(&mut self) -> Ask {
        self.requests.remove(self.shown).ask
    }

    /// Takes away every request, and returns what they asked, to refuse.
    pub fn remove_all(&mut self) -> Vec<Ask> {
        self.requests.drain(..).map(|request| request.ask).collect()
    }

    /// Takes away the batch the question view shows, answered.
    fn answer(&mut self) -> Sent {
        let request = self.requests.remove(self.shown);
        let answers = request.answers();
        let map: Map<String, Value> = request
            .questions()
            .iter()
            .zip(&answers)
            .map(|(question, (_, answer))| (question.text.clone(), Value::String(answer.clone())))
            .collect();
        let mut input = request.ask.input.clone();
        input.insert("answers".to_string(), Value::Object(map));
        Sent::Answered {
            answers,
            ask: Box::new(request.ask),
            answer: Answer::Allow { input },
        }
    }

    fn request(&self) -> &Request {
        &self.requests[self.shown]
    }

    fn request_mut(&mut self) -> &mut Request {
        &mut self.requests[self.shown]
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
            Ok(crate::protocol::Event::Ask(ask)) => *ask,
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
            AskKind::Permission(_) | AskKind::Plan(_) => panic!("questions"),
        }
    }

    /// Sends `draft` on the question shown, and returns the question shown after it.
    fn send_on(questions: &mut Questions, draft: Draft) -> usize {
        match questions.send(draft) {
            Sent::Show => questions.view().index,
            Sent::Answered { .. } => panic!("a question shows"),
        }
    }

    #[test]
    fn the_view_offers_a_questions_options_in_its_form() {
        let mut questions = Questions::default();
        questions.add(two_questions("a"), None, vec![1, 2]);
        assert_eq!(questions.count(), 1);
        let view = questions.view();
        assert_eq!(view.label, "Database");
        assert_eq!(view.form, Form::One);
        assert_eq!(view.choices.len(), 2);
        questions.go(Move::Next);
        assert_eq!(questions.view().form, Form::Several);
    }

    #[test]
    fn enter_submits_and_moves_on_and_opens_a_submitted_question_again() {
        let mut questions = Questions::default();
        questions.add(two_questions("a"), None, vec![1, 2]);
        questions.go(Move::To(1));
        assert_eq!(send_on(&mut questions, with_ticks(2, &[2])), 0);
        assert_eq!(questions.view().progress, [false, true]);
        questions.go(Move::Next);
        assert!(questions.view().is_submitted);
        let again = questions.draft();
        assert_eq!(send_on(&mut questions, again), 1);
        assert!(!questions.view().is_submitted);
        assert_eq!(questions.draft().pick.cursor, Cursor::Option(2));
        // The user's own answer left empty answers nothing
        questions.go(Move::To(0));
        assert_eq!(send_on(&mut questions, draft(Cursor::Own, "")), 0);
        assert!(!questions.view().is_submitted);
    }

    #[test]
    fn the_review_comes_once_all_are_submitted_and_sends_them_together() {
        let mut questions = Questions::default();
        questions.add(two_questions("a"), None, vec![1, 2]);
        send_on(&mut questions, draft(Cursor::Option(1), ""));
        let mut ticked = Draft::new();
        ticked.pick.ticked = BTreeSet::from([2, 0]);
        assert_eq!(send_on(&mut questions, ticked), 2);
        questions.go(Move::Next);
        assert_eq!(questions.view().index, 0);
        questions.go(Move::Previous);
        assert_eq!(questions.view().index, 2);
        questions.go(Move::To(1));
        assert_eq!(questions.view().index, 1);
        questions.go(Move::Next);
        questions.go(Move::To(3));
        let review = questions.view();
        assert_eq!(review.index, 2);
        assert_eq!(review.form, Form::Review);
        assert_eq!(review.choices[1].label, "Checks: Lint, Types");
        let Sent::Answered {
            answers,
            answer: Answer::Allow { input },
            ..
        } = questions.send(questions.draft())
        else {
            panic!("the batch's answers");
        };
        assert_eq!(answers[0], ("Database".to_string(), "SQLite".to_string()));
        assert_eq!(
            input["answers"],
            json!({"Which database?": "SQLite", "Which checks?": "Lint, Types"})
        );
        assert_eq!(questions.count(), 0);
    }

    #[test]
    fn the_review_opens_the_unticked_answers_keeping_their_drafts() {
        let mut questions = Questions::default();
        questions.add(two_questions("a"), None, vec![1, 2]);
        send_on(&mut questions, draft(Cursor::Option(1), ""));
        send_on(&mut questions, with_ticks(0, &[0]));
        let mut review = questions.draft();
        review.pick.ticked.remove(&0);
        assert_eq!(send_on(&mut questions, review), 0);
        assert_eq!(questions.view().progress, [false, true]);
        assert_eq!(questions.draft().pick.cursor, Cursor::Option(1));
        // Submitting it again brings the review back with every answer ticked
        let again = questions.draft();
        assert_eq!(send_on(&mut questions, again), 2);
        assert_eq!(questions.draft().pick.ticked, BTreeSet::from([0, 1]));
    }

    #[test]
    fn the_view_keeps_the_users_place_in_each_request_and_on_the_review() {
        let mut questions = Questions::default();
        questions.add(two_questions("a"), None, vec![1, 2]);
        questions.add(permission("b", false), None, vec![3]);
        questions.go(Move::To(1));
        questions.go(Move::NextRequest);
        questions.go(Move::PreviousRequest);
        assert_eq!(questions.view().index, 1);
        // Going back to the message and opening the view again keeps it too
        assert!(questions.open());
        assert_eq!(
            (questions.view().request_index, questions.view().index),
            (0, 1)
        );
        // An answer unticked on the review stays unticked after going away from it
        questions.go(Move::To(0));
        send_on(&mut questions, draft(Cursor::Option(1), ""));
        send_on(&mut questions, with_ticks(0, &[0]));
        let mut review = questions.draft();
        review.pick.ticked.remove(&0);
        questions.keep_draft(review);
        questions.go(Move::Next);
        questions.go(Move::NextRequest);
        questions.go(Move::PreviousRequest);
        assert_eq!(questions.view().index, 0);
        questions.go(Move::Previous);
        assert_eq!(questions.view().index, 2);
        assert_eq!(questions.draft().pick.ticked, BTreeSet::from([1]));
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
    fn a_single_question_sends_its_answer_at_once() {
        let mut questions = Questions::default();
        questions.add(one_question("a"), None, vec![1]);
        assert!(questions.view().sends_at_once);
        let Sent::Answered { answers, .. } = questions.send(draft(Cursor::Option(1), "")) else {
            panic!("the answer, with no review");
        };
        assert_eq!(answers.len(), 1);
        assert_eq!(questions.count(), 0);
    }

    fn permission(id: &str, is_guarded: bool) -> Ask {
        let request = json!({
            "subtype": "can_use_tool",
            "tool_name": "Bash",
            "input": {"command": "rm -rf build"},
            "tool_use_id": id,
            "default_to_no": is_guarded,
        });
        match read_request(format!("request-{id}"), request) {
            Ok(crate::protocol::Event::Ask(ask)) => *ask,
            _ => panic!("an ask"),
        }
    }

    #[test]
    fn requests_wait_each_on_its_own_and_the_view_moves_between_them() {
        let mut questions = Questions::default();
        questions.add(two_questions("a"), None, vec![1, 2]);
        questions.add(one_question("b"), None, vec![3]);
        questions.add(permission("c", false), None, vec![4]);
        assert_eq!(questions.count(), 3);
        assert_eq!(questions.view().title, "Question");
        questions.go(Move::ToRequest(1));
        let view = questions.view();
        assert_eq!(view.title, "Question");
        assert_eq!(view.label, "Cache");
        assert_eq!(view.request_index, 1);
        questions.go(Move::PreviousRequest);
        questions.go(Move::ToRequest(3));
        assert_eq!(questions.view().request_index, 0);
        questions.go(Move::NextRequest);
        assert_eq!(questions.remove_shown().tool_use_id, "b");
        // The view stays in place, on the request that slid into it
        assert!(questions.open());
        let view = questions.view();
        assert_eq!((view.title.as_str(), view.request_index), ("Permission", 1));
        assert_eq!(questions.remove_shown().tool_use_id, "c");
        assert!(questions.open());
        assert_eq!(questions.view().label, "Database");
        assert_eq!(questions.remove_shown().tool_use_id, "a");
        assert!(!questions.open());
    }

    #[test]
    fn a_permission_is_allowed_or_refused_at_once_and_the_users_words_explain_a_refusal() {
        let decide = |draft: Draft| {
            let mut questions = Questions::default();
            questions.add(permission("a", false), None, vec![1]);
            match questions.send(draft) {
                Sent::Answered { answer, .. } => answer,
                Sent::Show => panic!("an answer"),
            }
        };
        assert!(matches!(
            decide(draft(Cursor::Option(0), "")),
            Answer::Allow { input } if input["command"] == "rm -rf build"
        ));
        assert!(matches!(decide(draft(Cursor::Option(1), "")), Answer::Deny));
        assert!(matches!(
            decide(draft(Cursor::Writing, " use cargo clean ")),
            Answer::Explain { reason } if reason == "use cargo clean"
        ));
        let mut questions = Questions::default();
        questions.add(permission("a", false), None, vec![1]);
        assert_eq!(send_on(&mut questions, draft(Cursor::Writing, "")), 0);
        let view = questions.view();
        assert_eq!(view.refusal, "the call");
        assert!(view.sends_at_once);
        assert_eq!(view.choices[0].key, Some('y'));
        // The call shows in the block under the title
        assert!(
            view.fields
                .iter()
                .any(|field| field.value == "rm -rf build")
        );
    }

    #[test]
    fn a_plan_shows_as_written_in_the_block_under_the_rule() {
        let request = json!({
            "subtype": "can_use_tool",
            "tool_name": "ExitPlanMode",
            "input": {"plan": "# Plan\n\n1. Add `greet`"},
            "tool_use_id": "b",
        });
        let Ok(crate::protocol::Event::Ask(plan)) = read_request("request-b".to_string(), request)
        else {
            panic!("a plan");
        };
        let mut questions = Questions::default();
        questions.add(*plan, None, vec![1]);
        let view = questions.view();
        assert_eq!(view.fields.len(), 1);
        assert_eq!(view.fields[0].value, "# Plan\n\n1. Add `greet`");
        assert_eq!(view.fields[0].rows, BLOCK_ROWS);
    }

    #[test]
    fn a_guarded_permission_opens_on_the_refusal_and_its_approval_names_the_risk() {
        let mut questions = Questions::default();
        questions.add(permission("a", true), None, vec![1]);
        assert_eq!(questions.draft().pick.cursor, Cursor::Option(REFUSE));
        let approval = &questions.view().choices[ALLOW];
        assert_eq!(approval.label, "allow (dangerous)");
        assert_eq!(
            approval.risk.as_deref(),
            Some("allows a call Claude Code deems dangerous")
        );
        questions.add(permission("b", false), None, vec![2]);
        questions.go(Move::NextRequest);
        assert_eq!(questions.view().choices[ALLOW].risk, None);
    }

    #[test]
    fn a_permission_without_words_for_its_reason_tells_the_kind_of_check_or_the_path() {
        let reason_of = |extra: Value| {
            let mut request = json!({
                "subtype": "can_use_tool",
                "tool_name": "Bash",
                "input": {"command": "echo hi"},
                "tool_use_id": "a",
            });
            request
                .as_object_mut()
                .expect("an object")
                .extend(extra.as_object().expect("an object").clone());
            let Ok(crate::protocol::Event::Ask(ask)) = read_request("r".to_string(), request)
            else {
                panic!("an ask");
            };
            let mut questions = Questions::default();
            questions.add(*ask, None, vec![1]);
            let fields = questions.view().fields;
            let reason = fields[fields.len() - ABOUT_ROWS].clone();
            (reason.value, reason.tone)
        };
        assert_eq!(
            reason_of(
                json!({"decision_reason": "rm removes files", "decision_reason_type": "safetyCheck"})
            ),
            ("rm removes files".to_string(), Tone::Plain)
        );
        assert_eq!(
            reason_of(json!({"decision_reason_type": "rule", "blocked_path": "/x"})),
            (
                "A rule in your settings asks before this call".to_string(),
                Tone::Plain
            )
        );
        assert_eq!(
            reason_of(json!({"decision_reason_type": "sandboxOverride"})),
            ("sandboxOverride".to_string(), Tone::Plain)
        );
        assert_eq!(
            reason_of(json!({"blocked_path": "/tmp/sub"})),
            ("blocked path /tmp/sub".to_string(), Tone::Plain)
        );
        assert_eq!(
            reason_of(json!({})),
            ("no reason given".to_string(), Tone::Note)
        );
    }

    #[test]
    fn a_permission_names_its_tool_as_claude_code_names_it_for_a_person() {
        let request = json!({
            "subtype": "can_use_tool",
            "tool_name": "mcp__claude_ai_Google_Drive__search_files",
            "display_name": "Search Files",
            "mcp_server": {"name": "claude.ai Google Drive", "source": "claudeai"},
            "input": {"query": "x"},
            "tool_use_id": "a",
        });
        let Ok(crate::protocol::Event::Ask(ask)) = read_request("request-a".to_string(), request)
        else {
            panic!("an ask");
        };
        let mut questions = Questions::default();
        questions.add(*ask, Some("Explore".to_string()), vec![1]);
        let view = questions.view();
        // The title names the kind alone, the label row the tool, and the block the rest
        assert_eq!(view.title, "Permission");
        assert_eq!(view.label, "Search Files");
        assert_eq!(
            (view.fields[0].mark.clone(), view.fields[0].value.as_str()),
            (Mark::Name(MCP_SERVER.to_string()), "claude.ai Google Drive")
        );
        assert_eq!(view.fields[1].mark, Mark::Name("query".to_string()));
        let agent = view.fields.last().expect("who asked");
        assert_eq!((agent.value.as_str(), agent.tone), ("Explore", Tone::Plain));
        // Without a name to show the user, the tool's own name
        let mut questions = Questions::default();
        questions.add(permission("b", false), None, vec![2]);
        assert_eq!(questions.view().label, "Bash");
    }

    #[test]
    fn a_permission_ends_in_why_claude_code_asks_then_who_asked_on_the_same_rows_always() {
        let shown = |fields: &[Field]| {
            (fields.iter())
                .map(|field| (field.value.clone(), field.tone))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            shown(&about_fields(Some("why"), Some("Explore"))),
            [
                ("why".to_string(), Tone::Plain),
                ("Explore".to_string(), Tone::Plain)
            ]
        );
        assert_eq!(
            shown(&about_fields(None, None)),
            [
                ("no reason given".to_string(), Tone::Note),
                ("main agent".to_string(), Tone::Note)
            ]
        );
        // The call's fields take the same rows above them, whatever the call gives
        let mut questions = Questions::default();
        questions.add(permission("a", false), None, vec![1]);
        let fields = questions.view().fields;
        assert_eq!(
            fields.iter().map(|field| field.rows).sum::<usize>(),
            BLOCK_ROWS
        );
        let marks = fields[fields.len() - ABOUT_ROWS..]
            .iter()
            .map(|field| field.mark.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            marks,
            [sign('?', false, Tone::Plain), sign('↳', false, Tone::Plain)]
        );
    }

    #[test]
    fn every_layout_fills_the_rows_of_a_calls_fields() {
        for (tool, layout) in LAYOUTS {
            let rows = layout.iter().map(|laid| laid.rows).sum::<usize>();
            assert_eq!(rows, INPUT_ROWS, "{tool}");
        }
    }

    /// The marks, as text, and rows of a block's fields.
    fn shape(fields: &[Field]) -> Vec<(String, usize)> {
        let mark = |field: &Field| match &field.mark {
            Mark::None => String::new(),
            Mark::Sign { sign, .. } => sign.to_string(),
            Mark::Name(name) => name.clone(),
            Mark::Quote(key) => key.clone().unwrap_or_default(),
        };
        (fields.iter())
            .map(|field| (mark(field), field.rows))
            .collect()
    }

    #[test]
    fn a_call_of_a_known_tool_takes_its_layout_and_its_other_fields_take_rows_from_the_last() {
        let input = |json: Value| json.as_object().expect("an object").clone();
        let bash = input(json!({"command": "ls", "description": "List", "timeout": 90000}));
        let fields = call_fields("Bash", &bash, None);
        let expected = [("$", 3), ("#", 1), ("t", 1), ("&", 1)];
        assert_eq!(
            shape(&fields),
            expected.map(|(name, rows)| (name.to_string(), rows))
        );
        assert_eq!(fields[2].value, "1m 30s");
        assert_eq!(
            (fields[3].value.as_str(), fields[3].tone),
            ("in foreground", Tone::Note)
        );
        assert_eq!(
            fields.iter().map(|field| field.rows).sum::<usize>(),
            INPUT_ROWS
        );
        // A field the layout lacks shows by name, on a row the command gives up
        let bash = input(json!({"command": "ls", "run_in_background": true, "new": 1}));
        let fields = call_fields("Bash", &bash, None);
        let expected = [("$", 2), ("#", 1), ("t", 1), ("&", 1), ("new", 1)];
        assert_eq!(
            shape(&fields),
            expected.map(|(name, rows)| (name.to_string(), rows))
        );
        assert_eq!(
            (fields[2].value.as_str(), fields[2].tone),
            ("default", Tone::Note)
        );
        assert_eq!(fields[3].value, "in background");
        // A sign is the field's, whatever the value, given or not
        let left_out = call_fields("Bash", &input(json!({"command": "ls"})), None);
        assert_eq!(fields[3].mark, left_out[3].mark);
        let edit = input(json!({"file_path": "a.rs", "new_string": "y"}));
        let fields = call_fields("Edit", &edit, None);
        assert_eq!(fields[1].tone, Tone::Note);
        assert!(matches!(
            fields[1].mark,
            Mark::Sign {
                tone: Tone::Removed,
                ..
            }
        ));
        let edit = input(json!({
            "replace_all": true, "file_path": "a.rs", "old_string": "x", "new_string": "y"
        }));
        let fields = call_fields("Edit", &edit, None);
        let expected = [("@", 1), ("-", 2), ("+", 3)];
        assert_eq!(
            shape(&fields),
            expected.map(|(mark, rows)| (mark.to_string(), rows))
        );
        assert_eq!(fields[0].value, "a.rs (all matches)");
        assert_eq!(fields[1].tone, Tone::Removed);
        assert_eq!(fields[2].tone, Tone::Added);
    }

    #[test]
    fn a_call_of_another_tool_shows_a_field_a_row_and_counts_the_fields_that_do_not_fit() {
        let input = json!({
            "a": "1", "b": "2", "c": "3", "d": "4", "e": "5", "f": "6", "g": 7, "h": 8, "i": 9
        });
        let fields = call_fields(
            "mcp__x__y",
            input.as_object().expect("an object"),
            Some("x"),
        );
        assert_eq!(fields.len(), INPUT_ROWS);
        assert_eq!(fields[0].mark, Mark::Name(MCP_SERVER.to_string()));
        assert_eq!(fields[4].mark, Mark::Name("d".to_string()));
        assert_eq!(fields[5].value, "… 5 more fields · Ctrl+O shows them");
        let fields = call_fields("ListAgents", &Map::new(), None);
        assert_eq!(fields[0].value, "none");
        assert_eq!(shape(&fields)[1], (String::new(), INPUT_ROWS - 1));
    }

    #[test]
    fn a_timeout_shows_in_the_units_a_person_reads() {
        let shown = [0, 45000, 120_000, 90000, 1500].map(|ms| timeout_text(&json!(ms)));
        assert_eq!(shown, ["0s", "45s", "2m", "1m 30s", "1500ms"]);
    }
}
