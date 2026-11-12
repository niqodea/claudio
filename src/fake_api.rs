//! A fake of Anthropic's Messages API on localhost, so a check can make the real Claude Code meet
//! what the API does only now and then, like a slow answer, offline and for free. Its Claude
//! follows one rule for each kind of request:
//!
//! - a turn whose last user message asks `Run <command> with Bash` gets a call of Bash with the
//!   command
//! - a turn whose last user message holds a tool's result gets the text `ok`
//! - the question of the auto mode classifier, the one request that stops at `</severity>`, gets
//!   a severity of 5, which allows the call, after the delay the fake started with
//! - Claude Code's check of the connection, `HEAD /api/hello`, gets an empty answer
//!
//! Any other request fails the check. The answers follow the API's docs, and Claude Code is what
//! reads them, so a check fails once Claude Code stops accepting them.
//! Ref: <https://abseil.io/resources/swe-book/html/ch13.html#faking-id00043>, on preferring a fake to
//! stubs and mocks
//! Ref: <https://platform.claude.com/docs/en/build-with-claude/streaming>, on the events of a
//! streamed answer

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

/// The fake, which answers each request on a thread of its own. The thread that waits for
/// requests outlives the check, until the process ends.
pub struct FakeApi {
    /// The address to give Claude Code in `ANTHROPIC_BASE_URL`.
    pub url: String,
    errors: Receiver<String>,
}

impl FakeApi {
    /// Starts the fake on a free port, with the classifier answering after `classifier_delay`.
    pub fn start(classifier_delay: Duration) -> Result<Self, String> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .map_err(|error| format!("cannot start the fake API: {error}"))?;
        let port = listener
            .local_addr()
            .map_err(|error| format!("cannot read the fake API's port: {error}"))?
            .port();
        let (sender, errors) = mpsc::channel();
        thread::spawn(move || {
            for stream in listener.incoming().map_while(Result::ok) {
                let sender = sender.clone();
                thread::spawn(move || answer(&stream, classifier_delay, &sender));
            }
        });
        Ok(FakeApi {
            url: format!("http://127.0.0.1:{port}"),
            errors,
        })
    }

    /// Fails with each request that no rule covered.
    pub fn all_covered(&self) -> Result<(), String> {
        let errors: Vec<String> = self.errors.try_iter().collect();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }
}

/// Answers the request on `stream` by its rule, or with an error that `errors` gets.
fn answer(stream: &TcpStream, classifier_delay: Duration, errors: &Sender<String>) {
    let result = read_request(stream).and_then(|(start, body)| {
        if start.starts_with("HEAD /api/hello ") {
            return write(stream, "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        }
        if !start.starts_with("POST /v1/messages") {
            return Err(format!("Claude Code sent {start}, which no rule of the fake covers"));
        }
        let request: Value = serde_json::from_slice(&body)
            .map_err(|error| format!("a call's body is no JSON: {error}"))?;
        let (block, stop_reason) = reply(&request, classifier_delay)?;
        let message = message(&block, stop_reason);
        if request["stream"] == true {
            write(stream, &streamed(&message, &block))
        } else {
            let body = message.to_string();
            write(
                stream,
                &format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                ),
            )
        }
    });
    if let Err(error) = result {
        let _ = errors.send(error);
        let _ = write(
            stream,
            "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
    }
}

/// The one content block of Claude's reply to `request`, by the rules of the module's doc, and
/// why Claude stopped.
fn reply(request: &Value, classifier_delay: Duration) -> Result<(Value, &'static str), String> {
    let is_classifier = request["stop_sequences"]
        .as_array()
        .is_some_and(|stops| stops.contains(&json!("</severity>")));
    if is_classifier {
        thread::sleep(classifier_delay);
        return Ok((
            json!({"type": "text", "text": "<severity>5"}),
            "stop_sequence",
        ));
    }
    let messages = request["messages"].as_array().cloned().unwrap_or_default();
    // Claude Code can add a system message after the user's
    let last = messages.iter().rfind(|message| message["role"] == "user");
    let blocks = match &last.map(|last| &last["content"]) {
        Some(Value::Array(blocks)) => blocks.clone(),
        Some(text @ Value::String(_)) => vec![json!({"type": "text", "text": text})],
        _ => Vec::new(),
    };
    if blocks.iter().any(|block| block["type"] == "tool_result") {
        return Ok((json!({"type": "text", "text": "ok"}), "end_turn"));
    }
    let command = blocks.iter().find_map(|block| {
        let text = block["text"].as_str()?.trim();
        text.strip_prefix("Run ")?.strip_suffix(" with Bash")
    });
    match command {
        Some(command) => Ok((
            json!({
                "type": "tool_use",
                // One id per turn, as each turn adds messages
                "id": format!("toolu_fake_{}", messages.len()),
                "name": "Bash",
                "input": {"command": command, "description": format!("Run {command}")},
            }),
            "tool_use",
        )),
        None => Err("Claude Code made a call that no rule of the fake covers".to_string()),
    }
}

fn message(block: &Value, stop_reason: &str) -> Value {
    json!({
        "id": "msg_fake",
        "type": "message",
        "role": "assistant",
        "model": "fake",
        "content": [block],
        "stop_reason": stop_reason,
        "stop_sequence": null,
        "usage": {"input_tokens": 1, "output_tokens": 1},
    })
}

/// The HTTP response that streams `message`, whose one content block is `block`. The block
/// starts empty, and its one delta fills it. Closing the connection ends the response.
fn streamed(message: &Value, block: &Value) -> String {
    let (start, delta) = if block["type"] == "tool_use" {
        let mut start = block.clone();
        start["input"] = json!({});
        let delta = json!({"type": "input_json_delta", "partial_json": block["input"].to_string()});
        (start, delta)
    } else {
        let delta = json!({"type": "text_delta", "text": block["text"]});
        (json!({"type": "text", "text": ""}), delta)
    };
    let mut empty = message.clone();
    empty["content"] = json!([]);
    empty["stop_reason"] = Value::Null;
    let events = [
        json!({"type": "message_start", "message": empty}),
        json!({"type": "content_block_start", "index": 0, "content_block": start}),
        json!({"type": "content_block_delta", "index": 0, "delta": delta}),
        json!({"type": "content_block_stop", "index": 0}),
        json!({
            "type": "message_delta",
            "delta": {"stop_reason": message["stop_reason"], "stop_sequence": null},
            "usage": {"output_tokens": 1},
        }),
        json!({"type": "message_stop"}),
    ];
    let events = events.iter().map(|event| {
        let name = event["type"].as_str().unwrap_or_default();
        format!("event: {name}\ndata: {event}\n\n")
    });
    let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";
    [head.to_string()]
        .into_iter()
        .chain(events)
        .collect::<Vec<_>>()
        .concat()
}

/// The first line of the HTTP request on `stream`, like `POST /v1/messages HTTP/1.1`, and its
/// body.
fn read_request(stream: &TcpStream) -> Result<(String, Vec<u8>), String> {
    let mut reader = BufReader::new(stream);
    let mut start = String::new();
    let mut length = 0;
    loop {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|error| format!("cannot read a request: {error}"))?;
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if start.is_empty() {
            start = line.to_string();
        } else if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0; length];
    reader
        .read_exact(&mut body)
        .map_err(|error| format!("cannot read a request's body: {error}"))?;
    Ok((start, body))
}

fn write(mut stream: &TcpStream, response: &str) -> Result<(), String> {
    stream
        .write_all(response.as_bytes())
        .map_err(|error| format!("cannot answer a request: {error}"))
}
