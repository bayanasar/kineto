//! A scripted ACP agent for kineto's tests. It is not an agent: it speaks
//! just enough of the protocol, in the shape `FAKE_ACP_SCENARIO` names, to
//! prove how the client behaves. Each step is appended to `FAKE_ACP_LOG`.
//!
//! Scenarios: `answer` (asks for a permission and a file read, then
//! answers), `unauthenticated`, `hang`, `crash`, `refusal`, `garbage`.

use std::{
    fs::OpenOptions,
    io::{self, BufRead, Write},
    time::Duration,
};

use serde_json::{Value, json};

fn log(entry: &str) {
    if let Ok(path) = std::env::var("FAKE_ACP_LOG")
        && let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path)
    {
        let _ = writeln!(file, "{entry}");
    }
}

fn send(message: &Value) {
    let mut stdout = io::stdout().lock();
    let _ = writeln!(stdout, "{message}");
    let _ = stdout.flush();
}

fn main() {
    let scenario = std::env::var("FAKE_ACP_SCENARIO").unwrap_or_else(|_| "answer".to_owned());
    log(&format!(
        "cwd {}",
        std::env::current_dir().unwrap().display()
    ));
    let mut lines = io::stdin().lock().lines();
    let mut next = || -> Option<Value> {
        let line = lines.next()?.ok()?;
        serde_json::from_str(&line).ok()
    };
    while let Some(message) = next() {
        let method = message["method"].as_str().unwrap_or_default().to_owned();
        let id = message["id"].clone();
        log(&format!("request {method}"));
        match (scenario.as_str(), method.as_str()) {
            ("garbage", _) => {
                println!("this is not json");
                return;
            }
            (_, "initialize") => {
                send(
                    &json!({"jsonrpc":"2.0","id":id,"result":{"protocolVersion":1,"agentCapabilities":{}}}),
                );
                if scenario == "crash" {
                    std::process::exit(3);
                }
            }
            ("unauthenticated", "session/new") => send(&json!({
                "jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":"Authentication required"}
            })),
            (_, "session/new") => {
                send(&json!({"jsonrpc":"2.0","id":id,"result":{"sessionId":"s1"}}))
            }
            ("hang", "session/prompt") => std::thread::sleep(Duration::from_secs(60)),
            (_, "session/prompt") => {
                let prompt = message["params"]["prompt"][0]["text"]
                    .as_str()
                    .unwrap_or_default();
                log(&format!("prompt {} chars", prompt.chars().count()));
                // Ask for a permission; anything but a rejection is a failure
                // the answer makes visible.
                send(
                    &json!({"jsonrpc":"2.0","id":"p1","method":"session/request_permission","params":{
                    "sessionId":"s1","toolCall":{"toolCallId":"t1","title":"Bash"},
                    "options":[
                        {"optionId":"approve_once","name":"Approve once","kind":"allow_once"},
                        {"optionId":"reject","name":"Reject","kind":"reject_once"}
                    ]}}),
                );
                let reply = next().unwrap_or(Value::Null);
                let granted = reply["result"]["outcome"]["optionId"] != "reject";
                log(&format!("permission granted {granted}"));
                send(
                    &json!({"jsonrpc":"2.0","id":"f1","method":"fs/read_text_file","params":{
                    "sessionId":"s1","path":"/etc/hostname"}}),
                );
                let reply = next().unwrap_or(Value::Null);
                let read = reply.get("error").is_none();
                log(&format!("file read {read}"));
                if scenario == "refusal" {
                    send(&json!({"jsonrpc":"2.0","id":id,"result":{"stopReason":"refusal"}}));
                    continue;
                }
                let answer = if granted || read {
                    "LEAKED"
                } else {
                    "{\"ok\":"
                };
                for chunk in [answer, "true}"] {
                    send(
                        &json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1",
                        "update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":chunk}}}}),
                    );
                }
                send(
                    &json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1",
                    "update":{"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":"thinking"}}}}),
                );
                send(&json!({"jsonrpc":"2.0","id":id,"result":{"stopReason":"end_turn"}}));
            }
            (_, "session/delete") => {
                log("deleted s1");
                send(&json!({"jsonrpc":"2.0","id":id,"result":{}}));
            }
            _ => {
                send(&json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"unknown"}}))
            }
        }
    }
}
