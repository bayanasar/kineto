use super::*;

use std::{
    env,
    sync::atomic::AtomicU64,
    time::{SystemTime, UNIX_EPOCH},
};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = env::temp_dir().join(format!(
            "kineto-mcp-server-{}-{nonce}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// The planned film, in `temp`.
fn film(temp: &TempDir) -> PathBuf {
    let target = temp.0.join("film");
    copy_tree(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/projects/planned"),
        &target,
    );
    target
}

fn request(id: u64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

/// What a tool answered: its text, parsed when it is JSON, and whether it
/// is an error.
fn call(tools: &Tools, name: &str, arguments: Value) -> (Value, bool) {
    let answer = tools
        .respond(&request(
            7,
            "tools/call",
            json!({"name": name, "arguments": arguments}),
        ))
        .unwrap();
    let result = &answer["result"];
    let text = result["content"][0]["text"].as_str().unwrap();
    (
        serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_owned())),
        result["isError"].as_bool().unwrap(),
    )
}

#[test]
fn settings_keep_an_address_its_owner_alone_can_read() {
    let temp = TempDir::new();
    let path = temp.0.join("kineto").join(SERVER_FILE);
    let mut settings = ServerSettings::load(&path).unwrap();
    assert_eq!(settings, ServerSettings::default());
    assert_eq!(settings.address(), "", "no address until it listens");

    settings.renew_token().unwrap();
    assert_eq!(settings.token.len(), 64);
    assert!(settings.token.bytes().all(|byte| byte.is_ascii_hexdigit()));
    let first = settings.token.clone();
    settings.renew_token().unwrap();
    assert_ne!(settings.token, first);

    settings.enabled = true;
    settings.port = 47_615;
    settings.save(&path).unwrap();
    assert_eq!(ServerSettings::load(&path).unwrap(), settings);
    assert_eq!(
        settings.address(),
        format!("http://127.0.0.1:47615/mcp/{}", settings.token)
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    fs::write(&path, "enabled = [").unwrap();
    assert!(ServerSettings::load(&path).is_err());
}

#[test]
fn it_speaks_mcp_and_lists_read_only_tools() {
    let tools = Tools::new(Arc::new(Mutex::new(None)));
    let answer = tools
        .respond(&request(
            1,
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}),
        ))
        .unwrap();
    assert_eq!(answer["id"], 1);
    assert_eq!(answer["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(answer["result"]["serverInfo"]["name"], "kineto");
    assert!(answer["result"]["capabilities"]["tools"].is_object());
    let newer = tools
        .respond(&request(
            2,
            "initialize",
            json!({"protocolVersion": "2099-01-01"}),
        ))
        .unwrap();
    assert_eq!(newer["result"]["protocolVersion"], PROTOCOL_VERSIONS[0]);

    assert_eq!(
        tools.respond(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"})),
        None
    );
    assert_eq!(
        tools.respond(&request(3, "ping", Value::Null)).unwrap()["result"],
        json!({})
    );
    let listed = tools.respond(&request(4, "tools/list", json!({}))).unwrap();
    let names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| {
            assert_eq!(tool["annotations"]["readOnlyHint"], true);
            assert_eq!(tool["inputSchema"]["type"], "object");
            tool["name"].as_str().unwrap()
        })
        .collect();
    assert_eq!(
        names,
        ["get_project", "get_story", "get_document", "get_shot"]
    );

    assert_eq!(
        tools
            .respond(&request(5, "resources/list", json!({})))
            .unwrap()["error"]["code"],
        -32601
    );
    assert_eq!(
        tools
            .respond(&request(
                6,
                "tools/call",
                json!({"name": "delete_everything"})
            ))
            .unwrap()["error"]["code"],
        -32602
    );
    assert_eq!(
        tools
            .respond(&json!([request(8, "ping", Value::Null)]))
            .unwrap()["error"]["code"],
        -32600
    );
    let (text, is_error) = call(&tools, "get_project", json!({}));
    assert!(is_error);
    assert_eq!(text, "No project is open in Kineto.");
}

#[test]
fn tools_read_the_project_while_its_session_holds_the_writer_lock() {
    let temp = TempDir::new();
    let target = film(&temp);
    let _writer = CanonicalProject::open(&target).unwrap();
    let tools = Tools::new(Arc::new(Mutex::new(Some(target.clone()))));

    let (project, is_error) = call(&tools, "get_project", json!({}));
    assert!(!is_error, "{project}");
    assert!(!project["title"].as_str().unwrap().is_empty());
    assert!(!project["steps"].as_array().unwrap().is_empty());
    let scenes = project["scenes"].as_array().unwrap();
    let shots = scenes[0]["shots"].as_array().unwrap();
    assert!(
        shots
            .iter()
            .any(|shot| shot["shot"] == "office_night_02" && shot["keyframe"] == "made"),
        "{scenes:?}"
    );

    let (story, is_error) = call(&tools, "get_story", json!({}));
    assert!(!is_error, "{story}");
    assert!(!story["source"]["text"].as_str().unwrap().is_empty());
    assert_eq!(story["source"]["truncated"], false);

    let (plan, is_error) = call(&tools, "get_document", json!({"kind": "shot_plan"}));
    assert!(!is_error, "{plan}");
    assert_eq!(plan["kind"], "shot_plan");
    assert!(plan["revision"].as_u64().is_some(), "{plan}");
    assert!(plan["document"].is_object());
    let (_, is_error) = call(&tools, "get_document", json!({"kind": "../../secrets"}));
    assert!(is_error);

    let (shot, is_error) = call(
        &tools,
        "get_shot",
        json!({"scene": "office_night", "shot": "office_night_02"}),
    );
    assert!(!is_error, "{shot}");
    let candidates = shot["keyframes"]["candidates"].as_array().unwrap();
    assert!(!candidates.is_empty());
    let image = candidates[0].as_str().unwrap();
    assert!(
        Path::new(image).is_absolute() && Path::new(image).exists(),
        "{image}"
    );
    assert_eq!(shot["video"]["state"], "none");
    for arguments in [
        json!({"scene": "office_night", "shot": "nowhere"}),
        json!({"scene": "../..", "shot": "etc"}),
    ] {
        let (_, is_error) = call(&tools, "get_shot", arguments);
        assert!(is_error);
    }
}

/// Send `head` and `body` to the server at `port`; its status and body.
fn http(port: u16, head: &str, body: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(
        stream,
        "{head}\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut answer = String::new();
    stream.read_to_string(&mut answer).unwrap();
    let status = answer
        .split_whitespace()
        .nth(1)
        .and_then(|status| status.parse().ok())
        .unwrap();
    let body = answer
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.to_owned())
        .unwrap_or_default();
    (status, body)
}

#[test]
fn only_its_address_on_this_computer_reaches_it() {
    let temp = TempDir::new();
    let target = film(&temp);
    let mut settings = ServerSettings::default();
    settings.renew_token().unwrap();
    let server = Server::start(0, &settings.token, Arc::new(Mutex::new(Some(target)))).unwrap();
    let port = server.port();
    let path = format!("/mcp/{}", settings.token);
    let host = format!("Host: 127.0.0.1:{port}");
    let json = "Content-Type: application/json";
    let initialize = request(1, "initialize", json!({"protocolVersion": "2025-11-25"})).to_string();

    let (status, body) = http(
        port,
        &format!("POST {path} HTTP/1.1\r\n{host}\r\n{json}"),
        &initialize,
    );
    assert_eq!(status, 200, "{body}");
    let body: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(body["result"]["protocolVersion"], "2025-11-25");

    let call = request(
        2,
        "tools/call",
        json!({"name": "get_project", "arguments": {}}),
    )
    .to_string();
    let (status, body) = http(
        port,
        &format!(
            "POST {path} HTTP/1.1\r\nHost: localhost:{port}\r\n{json}\r\nMCP-Protocol-Version: 2025-11-25\r\nOrigin: http://localhost:{port}"
        ),
        &call,
    );
    assert_eq!(status, 200);
    let body: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(body["result"]["isError"], false);

    let notification = json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string();
    let refusals = [
        // Another token, or none.
        (
            format!("POST /mcp/{} HTTP/1.1\r\n{host}\r\n{json}", "0".repeat(64)),
            &initialize,
            404,
        ),
        (
            format!("POST /mcp HTTP/1.1\r\n{host}\r\n{json}"),
            &initialize,
            404,
        ),
        // A name rebound to this computer by a page elsewhere.
        (
            format!("POST {path} HTTP/1.1\r\nHost: attacker.example:{port}\r\n{json}"),
            &initialize,
            403,
        ),
        (
            format!("POST {path} HTTP/1.1\r\n{host}\r\n{json}\r\nOrigin: https://attacker.example"),
            &initialize,
            403,
        ),
        (
            format!("POST {path} HTTP/1.1\r\n{host}\r\n{json}\r\nOrigin: null"),
            &initialize,
            403,
        ),
        // No stream to open, and JSON only.
        (
            format!("GET {path} HTTP/1.1\r\n{host}"),
            &String::new(),
            405,
        ),
        (
            format!("POST {path} HTTP/1.1\r\n{host}\r\nContent-Type: text/plain"),
            &initialize,
            415,
        ),
        (
            format!("POST {path} HTTP/1.1\r\n{host}\r\n{json}\r\nMCP-Protocol-Version: 1999-01-01"),
            &initialize,
            400,
        ),
        (
            format!("POST {path} HTTP/1.1\r\n{host}\r\n{json}"),
            &"{not json".to_owned(),
            400,
        ),
        (
            format!("POST {path} HTTP/1.1\r\n{host}\r\n{json}"),
            &notification,
            202,
        ),
    ];
    for (head, body, expected) in refusals {
        assert_eq!(http(port, &head, body).0, expected, "{head}");
    }

    drop(server);
    assert!(
        TcpStream::connect(("127.0.0.1", port)).is_err(),
        "nothing listens once it is off"
    );
    assert!(Server::start(0, "short", Arc::new(Mutex::new(None))).is_err());
}
