//! The MCP client against the fake archive server: tools listed and called
//! only as allowed, nothing offered back, and every way a server can
//! misbehave failing the call.

use std::{path::PathBuf, time::Duration};

use kineto_plugins::mcp::{McpConfig, McpError, McpLimits, McpServerEntry, call_tool, list_tools};
use serde_json::json;

fn server(mode: Option<&str>) -> McpServerEntry {
    McpServerEntry {
        id: "archive".to_owned(),
        executable: PathBuf::from(env!("CARGO_BIN_EXE_kineto-fake-mcp")),
        args: mode.map(str::to_owned).into_iter().collect(),
        secrets: vec!["ARCHIVE_TOKEN".to_owned()],
        tools: vec!["search_wardrobe".to_owned(), "location_notes".to_owned()],
        enabled: true,
    }
}

fn secret(name: &str) -> Option<String> {
    (name == "ARCHIVE_TOKEN").then(|| "t0ken".to_owned())
}

fn call(
    mode: Option<&str>,
    tool: &str,
    limits: McpLimits,
) -> Result<kineto_plugins::mcp::ToolResult, McpError> {
    call_tool(
        &server(mode),
        &secret,
        tool,
        &json!({"query": "the filmmaker"}),
        limits,
    )
}

#[test]
fn only_allowed_tools_are_listed_and_called() {
    let tools = list_tools(&server(None), &secret, McpLimits::DEFAULT).unwrap();
    let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_str()).collect();
    assert_eq!(names, ["search_wardrobe", "location_notes"]);
    assert_eq!(tools[0].input_schema["required"][0], "query");

    let answered = call(None, "search_wardrobe", McpLimits::DEFAULT).unwrap();
    assert_eq!(answered.text.len(), 1);
    assert!(answered.text[0].starts_with("Wardrobe notes for the filmmaker"));
    assert!(!answered.is_error);
    assert!(answered.dropped.is_empty());

    // Refused before anything starts: the executable does not even exist.
    let absent = McpServerEntry {
        executable: PathBuf::from("/nonexistent/archive-mcp"),
        ..server(None)
    };
    assert!(matches!(
        call_tool(
            &absent,
            &secret,
            "delete_everything",
            &json!({}),
            McpLimits::DEFAULT
        ),
        Err(McpError::NotAllowed(_))
    ));
    let disabled = McpServerEntry {
        enabled: false,
        ..absent.clone()
    };
    assert!(matches!(
        call_tool(
            &disabled,
            &secret,
            "search_wardrobe",
            &json!({}),
            McpLimits::DEFAULT
        ),
        Err(McpError::NotAllowed(_))
    ));
    assert!(matches!(
        list_tools(&disabled, &secret, McpLimits::DEFAULT),
        Err(McpError::NotAllowed(_))
    ));
    assert!(matches!(
        call_tool(
            &absent,
            &secret,
            "search_wardrobe",
            &json!("not an object"),
            McpLimits::DEFAULT
        ),
        Err(McpError::NotAllowed(_))
    ));
    assert!(matches!(
        call_tool(
            &absent,
            &secret,
            "search_wardrobe",
            &json!({}),
            McpLimits::DEFAULT
        ),
        Err(McpError::Spawn(_))
    ));
}

#[test]
fn a_server_is_offered_nothing_back_and_sees_only_its_environment() {
    let answered = call(Some("--sneaky"), "search_wardrobe", McpLimits::DEFAULT).unwrap();
    let text = &answered.text[0];
    for refused in ["sampling/createMessage", "roots/list", "elicitation/create"] {
        assert!(text.contains(&format!("{refused} refused: true")), "{text}");
    }
    assert!(
        text.contains("ping refused: false"),
        "a ping is answered: {text}"
    );

    let answered = call(Some("--env"), "search_wardrobe", McpLimits::DEFAULT).unwrap();
    let (names, cwd) = answered.text[0].split_once(" in ").unwrap();
    assert_eq!(names, "ARCHIVE_TOKEN,HOME,PATH");
    assert!(cwd.contains("kineto-plugin-"), "{cwd}");

    let answered = call(Some("--image"), "search_wardrobe", McpLimits::DEFAULT).unwrap();
    assert_eq!(answered.dropped, ["image"]);
    assert_eq!(answered.text.len(), 1);
    assert!(
        call(Some("--failing"), "search_wardrobe", McpLimits::DEFAULT)
            .unwrap()
            .is_error
    );
}

#[test]
fn a_server_that_misbehaves_fails_the_call() {
    let short = McpLimits {
        call: Duration::from_millis(500),
        ..McpLimits::DEFAULT
    };
    assert!(matches!(
        call(Some("--sleep"), "search_wardrobe", short),
        Err(McpError::Timeout)
    ));
    assert!(matches!(
        call(Some("--huge"), "search_wardrobe", McpLimits::DEFAULT),
        Err(McpError::Oversized(_))
    ));
    assert!(matches!(
        call(Some("--garbage"), "search_wardrobe", McpLimits::DEFAULT),
        Err(McpError::Protocol(_))
    ));
    let old = call(Some("--old"), "search_wardrobe", McpLimits::DEFAULT).unwrap_err();
    assert!(old.to_string().contains("1999-01-01"), "{old}");
}

#[test]
fn servers_are_read_with_their_rules() {
    let config = McpConfig::parse(
        r#"
        [[server]]
        id = "studio-archive"
        executable = "/opt/studio/bin/archive-mcp"
        secrets = ["ARCHIVE_TOKEN"]
        tools = ["search_wardrobe"]

        [[server]]
        id = "old"
        executable = "/opt/old"
        tools = ["anything"]
        enabled = false
        "#,
    )
    .unwrap();
    let archive = config.enabled("studio-archive").unwrap();
    assert!(archive.allows("search_wardrobe"));
    assert!(!archive.allows("delete_everything"));
    assert!(config.enabled("old").is_none());
    for (text, why) in [
        (
            "[[server]]\nid = \"a\"\nexecutable = \"/x\"\ntools = []",
            "at least one tool",
        ),
        (
            "[[server]]\nid = \"a\"\nexecutable = \"x\"\ntools = [\"t\"]",
            "relative executable",
        ),
        (
            "[[server]]\nid = \"A b\"\nexecutable = \"/x\"\ntools = [\"t\"]",
            "is not a server ID",
        ),
    ] {
        let error = McpConfig::parse(text).unwrap_err().to_string();
        assert!(error.contains(why), "{error}");
    }
}
