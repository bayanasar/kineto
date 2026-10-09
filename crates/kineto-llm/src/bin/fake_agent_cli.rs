//! A scripted stand-in for the agent CLIs kineto drives one call at a time
//! (Claude Code, Codex, Kimi Code, agy), for tests. It tells which one it is
//! asked to be by its arguments, records what it was given, and answers as
//! `FAKE_AGENT_SCENARIO` says: `answer`, `signed-out` or `hang`.

use std::{
    env, fs,
    io::{Read, Write},
    path::Path,
    thread,
    time::Duration,
};

fn files(root: &Path, directory: &Path, found: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files(root, &path, found);
        } else if let Ok(relative) = path.strip_prefix(root) {
            let contents = fs::read_to_string(&path).unwrap_or_default();
            found.push(format!(
                "{} = {}",
                relative.to_string_lossy().replace('\\', "/"),
                contents.replace('\n', "\\n")
            ));
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let scenario = env::var("FAKE_AGENT_SCENARIO").unwrap_or_default();
    let has = |flag: &str| args.iter().any(|arg| arg == flag);
    let kind = if has("exec") {
        "codex"
    } else if has("--agent-file") {
        "kimi"
    } else if has("--agent") {
        "agy"
    } else {
        "claude"
    };
    let mut stdin = String::new();
    if kind != "kimi" {
        let _ = std::io::stdin().read_to_string(&mut stdin);
    }
    let cwd = env::current_dir().unwrap_or_default();
    let mut found = Vec::new();
    files(&cwd, &cwd, &mut found);
    if let Ok(log) = env::var("FAKE_AGENT_LOG") {
        let record = serde_json::json!({
            "kind": kind,
            "args": args,
            "stdin": stdin,
            "cwd": cwd,
            "files": found,
            "claudecode": env::var("CLAUDECODE").is_ok(),
        });
        let _ = fs::write(log, record.to_string());
    }
    let mut out = std::io::stdout();
    let answer = r#"{"ok":true}"#;
    let lines: Vec<String> = match (kind, scenario.as_str()) {
        (_, "hang") => {
            thread::sleep(Duration::from_secs(60));
            Vec::new()
        }
        ("claude", "answer") => vec![
            r#"{"type":"system","subtype":"init","tools":[]}"#.to_owned(),
            serde_json::json!({"type":"result","subtype":"success","is_error":false,"result":answer})
                .to_string(),
        ],
        ("claude", _) => vec![
            r#"{"type":"result","subtype":"success","is_error":true,"result":"Not logged in · Please run /login"}"#
                .to_owned(),
        ],
        ("codex", "answer") => vec![
            r#"{"type":"thread.started"}"#.to_owned(),
            serde_json::json!({"type":"item.completed","item":{"type":"agent_message","text":answer}})
                .to_string(),
            r#"{"type":"turn.completed"}"#.to_owned(),
        ],
        ("codex", _) => vec![
            r#"{"type":"turn.failed","error":{"message":"401 Unauthorized"}}"#.to_owned(),
        ],
        ("kimi", "answer") => vec![
            r#"{"role":"assistant","content":"UserPromptSubmit hook"}"#.to_owned(),
            serde_json::json!({"role":"assistant","content":answer}).to_string(),
        ],
        ("kimi", _) => {
            let _ = writeln!(std::io::stderr(), "Error: not logged in. Run kimi login.");
            std::process::exit(1);
        }
        ("agy", "answer") => vec![
            serde_json::json!({"event":"result","result":{
                "status":"SUCCESS",
                "conversation_id":"c1",
                "response":r#"{"ok":true,"toolAction":"Finishing task","toolSummary":"Task completion"}"#,
            }})
            .to_string(),
        ],
        _ => vec![
            r#"{"event":"result","result":{"status":"ERROR","error":"Print mode: auth error: sign in"}}"#
                .to_owned(),
        ],
    };
    for line in lines {
        let _ = writeln!(out, "{line}");
    }
}
