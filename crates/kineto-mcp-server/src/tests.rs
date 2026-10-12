use super::*;

use std::{
    env,
    io::Read,
    net::TcpStream,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use kineto_core::{
    generation::ImageProvider,
    session::{
        AgentCommand, DocumentCommand, OpenRequest, ProjectSession, Recovery, SessionError,
        SessionSink,
    },
};
use kineto_project::manifest::CanonicalProject;
use sha2::{Digest, Sha256};

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
        .respond(
            &request(
                7,
                "tools/call",
                json!({"name": name, "arguments": arguments}),
            ),
            "Test agent",
        )
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
    let tools = Tools::new(Arc::new(Desk::new()));
    let respond = |message: &Value| tools.respond(message, "Test agent");
    let answer = respond(&request(
        1,
        "initialize",
        json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}),
    ))
    .unwrap();
    assert_eq!(answer["id"], 1);
    assert_eq!(answer["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(answer["result"]["serverInfo"]["name"], "kineto");
    assert!(answer["result"]["capabilities"]["tools"].is_object());
    let newer = respond(&request(
        2,
        "initialize",
        json!({"protocolVersion": "2099-01-01"}),
    ))
    .unwrap();
    assert_eq!(newer["result"]["protocolVersion"], PROTOCOL_VERSIONS[0]);

    assert_eq!(
        respond(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"})),
        None
    );
    assert_eq!(
        respond(&request(3, "ping", Value::Null)).unwrap()["result"],
        json!({})
    );
    let listed = respond(&request(4, "tools/list", json!({}))).unwrap();
    let tools_listed = listed["result"]["tools"].as_array().unwrap();
    for tool in tools_listed {
        let name = tool["name"].as_str().unwrap();
        assert_eq!(tool["inputSchema"]["type"], "object");
        assert_eq!(
            tool["annotations"]["readOnlyHint"],
            name.starts_with("get_"),
            "{name} reads only when its name says so"
        );
        assert!(!tool["description"].as_str().unwrap().is_empty());
    }
    let names: Vec<&str> = tools_listed
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    for name in [
        "get_project",
        "get_image",
        "get_request",
        "write_screenplay",
        "lock_screenplay",
        "make_keyframes",
        "lock_keyframe",
        "make_video",
        "lock_video",
    ] {
        assert!(names.contains(&name), "{name}");
    }

    assert_eq!(
        respond(&request(5, "resources/list", json!({}))).unwrap()["error"]["code"],
        -32601
    );
    assert_eq!(
        respond(&request(
            6,
            "tools/call",
            json!({"name": "delete_everything"})
        ))
        .unwrap()["error"]["code"],
        -32602
    );
    assert_eq!(
        respond(&json!([request(8, "ping", Value::Null)])).unwrap()["error"]["code"],
        -32600
    );
    let (text, is_error) = call(&tools, "get_project", json!({}));
    assert!(is_error);
    assert_eq!(text, "No project is open in Kineto.");
    let (text, is_error) = call(&tools, "lock_screenplay", json!({}));
    assert!(is_error);
    assert!(
        text.as_str().unwrap().starts_with("No project is open"),
        "{text}"
    );
}

#[test]
fn tools_read_the_project_while_its_session_holds_the_writer_lock() {
    let temp = TempDir::new();
    let target = film(&temp);
    let _writer = CanonicalProject::open(&target).unwrap();
    let desk = Arc::new(Desk::new());
    desk.open(&target, None);
    let tools = Tools::new(desk);

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
    let desk = Arc::new(Desk::new());
    desk.open(&target, None);
    let server = Server::start(0, &settings.token, desk).unwrap();
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
    assert!(Server::start(0, "short", Arc::new(Desk::new())).is_err());
}

/// A sink that says when the session opened, and nothing else.
struct Opened(Mutex<mpsc::Sender<Result<(), String>>>);

impl SessionSink for Opened {
    fn recovered(&self, _: &Recovery) {}

    fn opened(&self, result: &Result<(), SessionError>) {
        let _ = self
            .0
            .lock()
            .unwrap()
            .send(result.as_ref().map(|_| ()).map_err(ToString::to_string));
    }

    fn completed(&self, _: u64, _: u32, _: &Result<(), SessionError>) {}

    fn video_finished(&self, _: u32, _: u16, _: &Result<(), SessionError>) {}

    fn document_completed(&self, _: u64, _: &Result<(), SessionError>) {}
}

/// The planned film open in a session with the free demo generator.
fn session(path: &Path) -> ProjectSession {
    let (sender, opened) = mpsc::channel();
    let session = ProjectSession::spawn(
        OpenRequest::Open {
            path: path.to_path_buf(),
        },
        ImageProvider::LocalDemo,
        Box::new(Opened(Mutex::new(sender))),
    )
    .unwrap();
    opened
        .recv_timeout(Duration::from_secs(60))
        .unwrap()
        .unwrap();
    session
}

#[test]
fn agents_change_the_project_through_its_session() {
    let temp = TempDir::new();
    let target = film(&temp);
    let session = session(&target);
    let desk = Arc::new(Desk::new());
    desk.open(&target, session.remote());
    let tools = Tools::new(Arc::clone(&desk));
    let shot = json!({"scene": "office_night", "shot": "office_night_02"});

    // An agent sees the image, not a path on the director's computer,
    // shrunk to what it needs.
    let mut first = shot.clone();
    first["candidate"] = json!(0);
    let (read, _) = call(&tools, "get_shot", shot.clone());
    image::RgbImage::from_pixel(2048, 1152, image::Rgb([40, 60, 90]))
        .save_with_format(
            read["keyframes"]["candidates"][0].as_str().unwrap(),
            image::ImageFormat::Jpeg,
        )
        .unwrap();
    let answer = tools
        .respond(
            &json!({"jsonrpc": "2.0", "id": 9, "method": "tools/call", "params": {"name": "get_image", "arguments": first}}),
            "Test agent",
        )
        .unwrap();
    let content = &answer["result"]["content"];
    assert_eq!(content[0]["type"], "image", "{answer}");
    assert_eq!(content[0]["mimeType"], "image/jpeg");
    let jpeg = base64::engine::general_purpose::STANDARD
        .decode(content[0]["data"].as_str().unwrap())
        .unwrap();
    let shown = image::load_from_memory(&jpeg).unwrap();
    assert_eq!((shown.width(), shown.height()), (1024, 576));

    // The free demo makes keyframes at once, as the director's button does.
    let (made, is_error) = call(&tools, "make_keyframes", shot.clone());
    assert!(!is_error, "{made}");
    assert_eq!(made["status"], "done", "{made}");
    let request = made["request"].as_u64().unwrap();
    let (asked, _) = call(&tools, "get_request", json!({"request": request}));
    assert_eq!(asked["status"], "done");
    assert_eq!(
        asked["what"],
        "make keyframes of office_night/office_night_02"
    );

    let mut choose = shot.clone();
    choose["candidate"] = json!(1);
    let (chosen, is_error) = call(&tools, "choose_keyframe", choose);
    assert!(!is_error, "{chosen}");
    let (locked, is_error) = call(&tools, "lock_keyframe", shot.clone());
    assert!(!is_error, "{locked}");
    let (read, _) = call(&tools, "get_shot", shot.clone());
    assert_eq!(read["keyframes"]["chosen"], 1, "{read}");
    assert_eq!(read["keyframes"]["locked"], true, "{read}");

    // A refusal comes back in words.
    let (refused, is_error) = call(
        &tools,
        "choose_keyframe",
        json!({"scene": "office_night", "shot": "office_night_02", "candidate": 9}),
    );
    assert!(is_error, "{refused}");
    let (missing, is_error) = call(
        &tools,
        "lock_keyframe",
        json!({"scene": "office_night", "shot": "nowhere"}),
    );
    assert!(is_error);
    assert!(
        missing.as_str().unwrap().contains("get_project"),
        "{missing}"
    );

    let (production, is_error) = call(&tools, "get_production", json!({}));
    assert!(!is_error, "{production}");
    assert_eq!(production["style"]["presets"].as_array().unwrap().len(), 4);

    drop(session);
    desk.close();
    let (closed, is_error) = call(
        &tools,
        "lock_keyframe",
        json!({"scene": "office_night", "shot": "office_night_02"}),
    );
    assert!(is_error);
    assert!(closed.as_str().unwrap().starts_with("No project is open"));
}

#[test]
fn a_paid_request_waits_for_the_director() {
    let temp = TempDir::new();
    let target = film(&temp);
    let session = session(&target);
    let desk = Arc::new(Desk::with_answer_wait(Duration::from_secs(30)));
    desk.open(&target, session.remote());
    let paid = || Work {
        command: AgentCommand::Document(DocumentCommand::LockShotPlan { revision: 1 }),
        what: "lock the shot plan".to_owned(),
        charge: Charge::Paid("about $0.12".to_owned()),
    };
    // The director, at Kineto's window.
    let director = |allow: bool| {
        let desk = Arc::clone(&desk);
        thread::spawn(move || {
            loop {
                if let Some(waiting) = desk.waiting().first() {
                    assert_eq!(waiting.kind, "run");
                    assert_eq!(waiting.agent, "Codex");
                    assert_eq!(waiting.cost.as_deref(), Some("about $0.12"));
                    assert!(desk.decide(waiting.id, allow));
                    assert!(!desk.decide(waiting.id, allow), "decided once");
                    return;
                }
                thread::sleep(Duration::from_millis(10));
            }
        })
    };

    let declining = director(false);
    assert_eq!(
        desk.run("Codex", paid()),
        Err("The director declined it in Kineto.".to_owned())
    );
    declining.join().unwrap();

    let allowing = director(true);
    let answer = desk.run("Codex", paid()).unwrap();
    allowing.join().unwrap();
    assert!(
        ["running", "done", "failed"].contains(&answer["status"].as_str().unwrap()),
        "sent once allowed: {answer}"
    );

    // Unanswered, the call answers anyway, and the request still waits.
    let patient = Desk::with_answer_wait(Duration::from_millis(50));
    patient.open(&target, session.remote());
    let answer = patient.run("Codex", paid()).unwrap();
    assert_eq!(answer["status"], "awaiting_director");
    let id = answer["request"].as_u64().unwrap();
    assert_eq!(patient.waiting().len(), 1);
    patient.close();
    assert!(
        patient.waiting().is_empty(),
        "a closed project's requests lapse"
    );
    assert_eq!(patient.request(id).unwrap()["status"], "lapsed");
    assert!(!patient.decide(id, true));
}

/// The challenge PKCE sends for `verifier`.
fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// Send `head` and `body` to the server at `port`; its status, its head
/// and its body.
fn exchange(port: u16, head: &str, body: &str) -> (u16, String, String) {
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
    let (head, body) = answer.split_once("\r\n\r\n").unwrap();
    (status, head.to_owned(), body.to_owned())
}

fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.eq_ignore_ascii_case(name).then(|| value.trim())
    })
}

#[test]
fn an_agent_in_the_cloud_connects_once_the_director_allows_it() {
    let temp = TempDir::new();
    let target = film(&temp);
    let desk = Arc::new(Desk::new());
    desk.open(&target, None);
    let agents = Arc::new(Agents::load(&temp.0.join(AGENTS_FILE)).unwrap());
    let issuer = "https://kineto.example:10000";
    let server = PublicServer::start(0, issuer, Arc::clone(&desk), Arc::clone(&agents)).unwrap();
    let port = server.port();
    // A tunnel may say any host.
    let host = "Host: kineto.example:10000";

    // MCP without a token points at how to get one.
    let initialize = request(1, "initialize", json!({"protocolVersion": "2025-11-25"})).to_string();
    let (status, head, _) = exchange(
        port,
        &format!("POST /mcp HTTP/1.1\r\n{host}\r\nContent-Type: application/json"),
        &initialize,
    );
    assert_eq!(status, 401);
    assert_eq!(
        header(&head, "www-authenticate"),
        Some(
            "Bearer resource_metadata=\"https://kineto.example:10000/.well-known/oauth-protected-resource/mcp\", scope=\"kineto\""
        )
    );
    let (status, _, body) = exchange(
        port,
        &format!("GET /.well-known/oauth-protected-resource/mcp HTTP/1.1\r\n{host}"),
        "",
    );
    assert_eq!(status, 200);
    let resource: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(resource["resource"], "https://kineto.example:10000/mcp");
    assert_eq!(resource["authorization_servers"], json!([issuer]));
    let (_, _, body) = exchange(
        port,
        &format!("GET /.well-known/oauth-authorization-server HTTP/1.1\r\n{host}"),
        "",
    );
    let metadata: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(metadata["issuer"], issuer);
    assert_eq!(
        metadata["code_challenge_methods_supported"],
        json!(["S256"])
    );
    assert_eq!(
        metadata["registration_endpoint"],
        "https://kineto.example:10000/register"
    );

    // It registers.
    let redirect = "https://chatgpt.com/connector_platform_oauth_redirect";
    let (status, _, body) = exchange(
        port,
        &format!("POST /register HTTP/1.1\r\n{host}\r\nContent-Type: application/json"),
        &json!({"client_name": "ChatGPT", "redirect_uris": [redirect]}).to_string(),
    );
    assert_eq!(status, 201, "{body}");
    let registered: Value = serde_json::from_str(&body).unwrap();
    let client = registered["client_id"].as_str().unwrap().to_owned();
    for refused in [
        json!({"client_name": "x", "redirect_uris": ["http://attacker.example/cb"]}),
        json!({"client_name": "x", "redirect_uris": ["https://a.example/cb#frag"]}),
        json!({"client_name": "x", "redirect_uris": []}),
    ] {
        let (status, _, _) = exchange(
            port,
            &format!("POST /register HTTP/1.1\r\n{host}\r\nContent-Type: application/json"),
            &refused.to_string(),
        );
        assert_eq!(status, 400, "{refused}");
    }

    // The director is asked, and a page waits meanwhile.
    let verifier = "the-verifier-is-long-enough-for-pkce-0123456789abcdef";
    let authorize = |verifier: &str| {
        format!(
            "GET /authorize?response_type=code&client_id={client}&redirect_uri={}&code_challenge={}&code_challenge_method=S256&state=xyz&resource={} HTTP/1.1\r\n{host}",
            http::encode(redirect),
            challenge(verifier),
            http::encode("https://kineto.example:10000/mcp"),
        )
    };
    let (status, _, page) = exchange(port, &authorize(verifier), "");
    assert_eq!(status, 200, "{page}");
    assert!(page.contains("Allow ChatGPT in Kineto"), "{page}");
    let pending = page
        .split("pending=")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .to_owned();
    let waiting = desk.waiting();
    assert_eq!(waiting.len(), 1);
    assert_eq!(waiting[0].kind, "connect");
    assert_eq!(waiting[0].agent, "ChatGPT");
    assert!(waiting[0].what.contains("chatgpt.com"), "{:?}", waiting[0]);
    assert!(desk.decide(waiting[0].id, true));

    let (status, head, _) = exchange(
        port,
        &format!("GET /authorize?pending={pending} HTTP/1.1\r\n{host}"),
        "",
    );
    assert_eq!(status, 302);
    let location = header(&head, "location").unwrap();
    assert!(
        location.starts_with(&format!("{redirect}?code=")),
        "{location}"
    );
    assert!(location.contains("&state=xyz"), "{location}");
    assert!(
        location.ends_with("&iss=https%3A%2F%2Fkineto.example%3A10000"),
        "{location}"
    );
    let code = location
        .split("code=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap()
        .to_owned();

    // A wrong verifier spends the code.
    let token = |fields: &str| {
        exchange(
            port,
            &format!(
                "POST /token HTTP/1.1\r\n{host}\r\nContent-Type: application/x-www-form-urlencoded"
            ),
            fields,
        )
    };
    let exchange_code = |code: &str, verifier: &str| {
        token(&format!(
            "grant_type=authorization_code&code={code}&redirect_uri={}&client_id={client}&code_verifier={verifier}",
            http::encode(redirect)
        ))
    };
    let (status, _, body) = exchange_code(&code, verifier);
    assert_eq!(status, 200, "{body}");
    let tokens: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(tokens["token_type"], "Bearer");
    let (status, _, body) = exchange_code(&code, verifier);
    assert_eq!(status, 400, "a code is used once");
    assert!(body.contains("invalid_grant"));

    let access = tokens["access_token"].as_str().unwrap();
    let call = request(
        2,
        "tools/call",
        json!({"name": "get_project", "arguments": {}}),
    )
    .to_string();
    let mcp = |access: &str| {
        exchange(
            port,
            &format!(
                "POST /mcp HTTP/1.1\r\n{host}\r\nContent-Type: application/json\r\nAuthorization: Bearer {access}"
            ),
            &call,
        )
    };
    let (status, _, body) = mcp(access);
    assert_eq!(status, 200, "{body}");
    let body: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(body["result"]["isError"], false, "{body}");
    let (status, head, _) = mcp("not-a-token");
    assert_eq!(status, 401);
    assert!(
        header(&head, "www-authenticate")
            .unwrap()
            .contains("invalid_token")
    );
    let connected = agents.connected();
    assert_eq!(connected.len(), 1);
    assert_eq!(connected[0].name, "ChatGPT");
    assert_eq!(connected[0].site, "chatgpt.com");

    // Refreshing turns both tokens over.
    let refresh = tokens["refresh_token"].as_str().unwrap();
    let (status, _, body) = token(&format!(
        "grant_type=refresh_token&refresh_token={refresh}&client_id={client}"
    ));
    assert_eq!(status, 200, "{body}");
    let renewed: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(mcp(access).0, 401, "the old access token is gone");
    assert_eq!(mcp(renewed["access_token"].as_str().unwrap()).0, 200);
    let (status, _, _) = token(&format!(
        "grant_type=refresh_token&refresh_token={refresh}&client_id={client}"
    ));
    assert_eq!(status, 400, "a refresh token is used once");

    // Only hashes are kept, by their owner alone.
    let kept = fs::read_to_string(temp.0.join(AGENTS_FILE)).unwrap();
    assert!(!kept.contains(renewed["access_token"].as_str().unwrap()));
    assert!(!kept.contains(renewed["refresh_token"].as_str().unwrap()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(temp.0.join(AGENTS_FILE))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    // Declined, the agent is told so where it asked to be sent back.
    let (_, _, page) = exchange(port, &authorize(verifier), "");
    let pending = page
        .split("pending=")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .to_owned();
    assert!(desk.decide(desk.waiting()[0].id, false));
    let (status, head, _) = exchange(
        port,
        &format!("GET /authorize?pending={pending} HTTP/1.1\r\n{host}"),
        "",
    );
    assert_eq!(status, 302);
    assert!(
        header(&head, "location")
            .unwrap()
            .contains("error=access_denied")
    );

    // Anyone can ask through the public address, so only a few wait at
    // once, and the window never fills.
    let mut statuses = Vec::new();
    for _ in 0..6 {
        statuses.push(exchange(port, &authorize(verifier), "").0);
    }
    assert_eq!(statuses, [200, 200, 200, 200, 200, 429]);
    assert_eq!(desk.waiting().len(), 5);
    for waiting in desk.waiting() {
        assert!(desk.decide(waiting.id, false));
    }

    // A page elsewhere, an unknown client, and a redirect it never
    // registered are refused.
    let (status, _, _) = exchange(
        port,
        &format!(
            "POST /register HTTP/1.1\r\n{host}\r\nOrigin: https://attacker.example\r\nContent-Type: application/json"
        ),
        "{}",
    );
    assert_eq!(status, 403);
    let (status, _, _) = exchange(
        port,
        &format!("GET /authorize?response_type=code&client_id=nobody HTTP/1.1\r\n{host}"),
        "",
    );
    assert_eq!(status, 400);
    let (status, _, _) = exchange(
        port,
        &format!(
            "GET /authorize?response_type=code&client_id={client}&redirect_uri={} HTTP/1.1\r\n{host}",
            http::encode("https://attacker.example/cb")
        ),
        "",
    );
    assert_eq!(status, 400);
    assert_eq!(
        exchange(
            port,
            &format!("GET /mcp/{} HTTP/1.1\r\n{host}", "0".repeat(64)),
            ""
        )
        .0,
        404
    );

    // Disconnected, its tokens stop working.
    assert!(agents.disconnect(&client).unwrap());
    assert_eq!(mcp(renewed["access_token"].as_str().unwrap()).0, 401);
    assert!(agents.connected().is_empty());
}

#[test]
fn registering_never_shuts_out_an_agent_the_director_allowed() {
    let desk = Arc::new(Desk::new());
    let agents = Arc::new(Agents::in_memory());
    let issuer = "https://kineto.example:10000";
    let redirect = "https://chatgpt.com/connector_platform_oauth_redirect";
    let verifier = "the-verifier-is-long-enough-for-pkce-0123456789abcdef";
    // Many requests: they go to the agents directly, and a few through the
    // listener.
    let asked = |method: &str, target: &str, kind: &str, body: String| {
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        http::Request {
            method: method.to_owned(),
            path: path.to_owned(),
            query: query.to_owned(),
            headers: vec![("content-type".to_owned(), kind.to_owned())],
            body: body.into_bytes(),
        }
    };
    let register = |name: &str| {
        let response = agents.register(&asked(
            "POST",
            "/register",
            "application/json",
            json!({"client_name": name, "redirect_uris": [redirect]}).to_string(),
        ));
        let body: Value = serde_json::from_slice(&response.body).unwrap();
        (
            response.status,
            body["client_id"].as_str().map(str::to_owned),
            body,
        )
    };
    // The page that waits for the director.
    let authorize = |client: &str| {
        let response = agents.authorize(
            &desk,
            issuer,
            &asked(
                "GET",
                &format!(
                    "/authorize?response_type=code&client_id={client}&redirect_uri={}&code_challenge={}&code_challenge_method=S256",
                    http::encode(redirect),
                    challenge(verifier),
                ),
                "",
                String::new(),
            ),
        );
        (response.status, String::from_utf8(response.body).unwrap())
    };
    // The code the page sends back to, once the director allowed it.
    let allowed_code = |page: &str| {
        let pending = page
            .split("pending=")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        assert!(desk.decide(desk.waiting()[0].id, true));
        let response = agents.authorize(
            &desk,
            issuer,
            &asked(
                "GET",
                &format!("/authorize?pending={pending}"),
                "",
                String::new(),
            ),
        );
        assert_eq!(response.status, 302);
        let (_, location) = response
            .headers
            .iter()
            .find(|(name, _)| *name == "Location")
            .unwrap();
        location
            .split("code=")
            .nth(1)
            .unwrap()
            .split('&')
            .next()
            .unwrap()
            .to_owned()
    };
    let token = |fields: String| {
        let response = agents.token(
            issuer,
            &asked(
                "POST",
                "/token",
                "application/x-www-form-urlencoded",
                fields,
            ),
        );
        (
            response.status,
            serde_json::from_slice::<Value>(&response.body).unwrap(),
        )
    };
    let exchange_code = |client: &str, code: &str| {
        token(format!(
            "grant_type=authorization_code&code={code}&client_id={client}&code_verifier={verifier}"
        ))
    };

    // The director allows as many agents as Kineto keeps.
    let mut connected = Vec::new();
    for index in 0..64 {
        let (status, client, body) = register(&format!("Agent {index}"));
        assert_eq!(status, 201, "{body}");
        let client = client.unwrap();
        let (_, page) = authorize(&client);
        let code = allowed_code(&page);
        let (status, tokens) = exchange_code(&client, &code);
        assert_eq!(status, 200, "{tokens}");
        connected.push((client, tokens));
    }

    // Registering again and again through the public address disconnects
    // none of them.
    let server = PublicServer::start(0, issuer, Arc::clone(&desk), Arc::clone(&agents)).unwrap();
    let port = server.port();
    let host = "Host: kineto.example:10000";
    let (status, _, body) = exchange(
        port,
        &format!("POST /register HTTP/1.1\r\n{host}\r\nContent-Type: application/json"),
        &json!({"client_name": "Flood", "redirect_uris": [redirect]}).to_string(),
    );
    assert_eq!(status, 403, "{body}");
    assert!(body.contains("access_denied"), "{body}");
    for index in 0..100 {
        let (status, client, body) = register(&format!("Flood {index}"));
        assert_eq!(status, 403, "{body}");
        assert_eq!(client, None);
    }
    assert_eq!(agents.connected().len(), 64);
    for (index, (_, tokens)) in connected.iter().enumerate() {
        assert_eq!(
            agents.bearer(tokens["access_token"].as_str().unwrap()),
            Some(format!("Agent {index}"))
        );
    }
    let (client, tokens) = &connected[63];
    let call = request(
        1,
        "tools/call",
        json!({"name": "get_project", "arguments": {}}),
    );
    let (status, _, body) = exchange(
        port,
        &format!(
            "POST /mcp HTTP/1.1\r\n{host}\r\nContent-Type: application/json\r\nAuthorization: Bearer {}",
            tokens["access_token"].as_str().unwrap()
        ),
        &call.to_string(),
    );
    assert_eq!(status, 200, "{body}");
    let (status, renewed) = token(format!(
        "grant_type=refresh_token&refresh_token={}&client_id={client}",
        tokens["refresh_token"].as_str().unwrap()
    ));
    assert_eq!(status, 200, "{renewed}");

    // Once the director disconnects one, an agent may register in its
    // place, and is kept while the director is asked and while its code
    // waits to be exchanged.
    assert!(agents.disconnect(&connected[0].0).unwrap());
    let (status, late, _) = register("Late");
    assert_eq!(status, 201);
    let late = late.unwrap();
    let (status, page) = authorize(&late);
    assert_eq!(status, 200, "{page}");
    assert_eq!(register("Flood").0, 403, "Late is waiting for the director");
    let code = allowed_code(&page);
    assert_eq!(register("Flood").0, 403, "Late's code is not exchanged yet");
    let (status, tokens) = exchange_code(&late, &code);
    assert_eq!(status, 200, "{tokens}");
    assert_eq!(register("Flood").0, 403);
    assert_eq!(agents.connected().len(), 64);

    // An agent the director was never asked about makes way for the next.
    assert!(agents.disconnect(&connected[1].0).unwrap());
    let (status, idle, _) = register("Idle");
    assert_eq!(status, 201);
    let (status, next, _) = register("Next");
    assert_eq!(status, 201);
    assert_eq!(authorize(&idle.unwrap()).0, 400, "Idle made way");
    assert_eq!(authorize(&next.unwrap()).0, 200);
    assert_eq!(agents.connected().len(), 63);
    assert!(
        agents
            .bearer(tokens["access_token"].as_str().unwrap())
            .is_some()
    );
}

#[test]
fn a_public_address_is_an_https_origin() {
    assert_eq!(
        public_origin(" https://anton.example.ts.net:10000/ "),
        Ok("https://anton.example.ts.net:10000".to_owned())
    );
    for refused in [
        "http://anton.example.ts.net",
        "https://anton.example.ts.net/kineto",
        "https://user@anton.example.ts.net",
        "https://",
        "anton.example.ts.net",
    ] {
        assert!(public_origin(refused).is_err(), "{refused}");
    }
    let mut settings = ServerSettings {
        public_url: "https://anton.example.ts.net:10000".to_owned(),
        public_port: 41_234,
        tunnel: vec![
            "tailscale".to_owned(),
            "funnel".to_owned(),
            "--https=10000".to_owned(),
            "http://127.0.0.1:{port}".to_owned(),
        ],
        ..ServerSettings::default()
    };
    assert_eq!(
        settings.public_address(),
        "https://anton.example.ts.net:10000/mcp"
    );
    assert_eq!(settings.tunnel_command()[3], "http://127.0.0.1:41234");
    settings.public_url.clear();
    assert_eq!(settings.public_address(), "");
    assert!(tunnel_words(&["ok".to_owned(), String::new()]).is_err());
    assert!(tunnel_words(&["a\nb".to_owned()]).is_err());
}
