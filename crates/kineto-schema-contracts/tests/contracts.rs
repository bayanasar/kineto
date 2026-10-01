use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    num::NonZeroU16,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use kineto_core::generation::{self, ImageProvider, NoHooks};
use kineto_job_runtime::{JobRuntime, JobRuntimePolicy, ProviderKey};
use kineto_jobs::{BackoffPolicy, FallbackPolicy, JobId, PaidFallback, ProviderExecutionPolicy};
use kineto_project::{
    ArtifactId, InputHash,
    manifest::{
        CanonicalProject, PROJECT_FORMAT_VERSION, ProjectDefaults, ProjectManifest, ProjectSource,
        ProjectWorkflow,
    },
    shot::ShotWorkflow,
};
use serde_json::Value;

const DRAFT_2020_12: &str = "https://json-schema.org/draft/2020-12/schema";

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after Unix epoch")
            .as_nanos();
        // SystemTime is only microsecond-precise on macOS, so tests running in
        // parallel could otherwise share a directory name.
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let nonce = format!(
            "{nonce}-{}",
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        let name = format!("kineto-schema-contracts-{}-{nonce}", std::process::id());
        Self(std::env::temp_dir().join(name))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn schema(name: &str) -> Value {
    let path = repo_root().join("schemas").join(name);
    let bytes = fs::read(&path)
        .unwrap_or_else(|error| panic!("failed to read schema {}: {error}", path.display()));
    let value: Value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|error| panic!("failed to parse schema {}: {error}", path.display()));
    ensure_supported_schema(&value, "$")
        .unwrap_or_else(|error| panic!("unsupported schema {}: {error}", path.display()));
    assert_eq!(
        value.get("$schema").and_then(Value::as_str),
        Some(DRAFT_2020_12),
        "{} must declare JSON Schema draft 2020-12",
        path.display()
    );
    value
}

fn json_file(path: &Path) -> Value {
    let bytes =
        fs::read(path).unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()))
}

fn toml_file_as_json(path: &Path) -> Value {
    let text = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    let value: toml::Value = toml::from_str(&text)
        .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()));
    serde_json::to_value(value)
        .unwrap_or_else(|error| panic!("failed to convert {} to JSON: {error}", path.display()))
}

fn validate(schema: &Value, instance: &Value, label: &str) {
    validate_value(schema, instance, "$")
        .unwrap_or_else(|error| panic!("{label} violates committed schema: {error}"));
}

fn ensure_supported_schema(schema: &Value, path: &str) -> Result<(), String> {
    let object = schema
        .as_object()
        .ok_or_else(|| format!("{path}: schemas must be JSON objects"))?;
    const SUPPORTED: &[&str] = &[
        "$id",
        "$schema",
        "additionalProperties",
        "const",
        "description",
        "enum",
        "format",
        "items",
        "maxItems",
        "maxLength",
        "maximum",
        "minItems",
        "minLength",
        "minimum",
        "pattern",
        "properties",
        "required",
        "title",
        "type",
        "uniqueItems",
    ];
    for key in object.keys() {
        if !SUPPORTED.contains(&key.as_str()) {
            return Err(format!("{path}: unsupported JSON Schema keyword {key:?}"));
        }
    }

    if let Some(expected_type) = object.get("type") {
        ensure_supported_type(expected_type, path)?;
    }
    if let Some(pattern) = object.get("pattern") {
        let pattern = pattern
            .as_str()
            .ok_or_else(|| format!("{path}.pattern must be a string"))?;
        if !is_supported_pattern(pattern) {
            return Err(format!(
                "{path}: unsupported JSON Schema pattern {pattern:?}"
            ));
        }
    }
    if let Some(format) = object.get("format") {
        match format.as_str() {
            Some("date-time") => {}
            Some(format) => return Err(format!("{path}: unsupported format {format:?}")),
            None => return Err(format!("{path}.format must be a string")),
        }
    }
    if let Some(additional) = object.get("additionalProperties")
        && !additional.is_boolean()
    {
        return Err(format!(
            "{path}.additionalProperties must be a boolean in the contract subset"
        ));
    }
    if let Some(unique_items) = object.get("uniqueItems")
        && !unique_items.is_boolean()
    {
        return Err(format!("{path}.uniqueItems must be a boolean"));
    }
    if let Some(required) = object.get("required") {
        let required = required
            .as_array()
            .ok_or_else(|| format!("{path}.required must be an array"))?;
        if required.iter().any(|entry| !entry.is_string()) {
            return Err(format!("{path}.required entries must be strings"));
        }
    }
    if let Some(values) = object.get("enum")
        && !values.is_array()
    {
        return Err(format!("{path}.enum must be an array"));
    }
    for keyword in ["minLength", "maxLength", "minItems", "maxItems"] {
        if let Some(value) = object.get(keyword)
            && value.as_u64().is_none()
        {
            return Err(format!("{path}.{keyword} must be a non-negative integer"));
        }
    }
    for keyword in ["minimum", "maximum"] {
        if let Some(value) = object.get(keyword)
            && !value.is_number()
        {
            return Err(format!("{path}.{keyword} must be numeric"));
        }
    }

    if let Some(properties) = object.get("properties") {
        let properties = properties
            .as_object()
            .ok_or_else(|| format!("{path}.properties must be an object"))?;
        for (name, property_schema) in properties {
            ensure_supported_schema(property_schema, &format!("{path}.properties.{name}"))?;
        }
    }
    if let Some(items) = object.get("items") {
        ensure_supported_schema(items, &format!("{path}.items"))?;
    }
    Ok(())
}

fn ensure_supported_type(expected: &Value, path: &str) -> Result<(), String> {
    let check = |kind: &str| match kind {
        "array" | "integer" | "null" | "number" | "object" | "string" => Ok(()),
        _ => Err(format!("{path}: unsupported JSON Schema type {kind:?}")),
    };

    match expected {
        Value::String(kind) => check(kind),
        Value::Array(kinds) => {
            if kinds.is_empty() {
                return Err(format!("{path}.type must not be an empty array"));
            }
            for kind in kinds {
                let kind = kind
                    .as_str()
                    .ok_or_else(|| format!("{path}.type entries must be strings"))?;
                check(kind)?;
            }
            Ok(())
        }
        _ => Err(format!("{path}.type must be a string or array of strings")),
    }
}

fn validate_value(schema: &Value, instance: &Value, path: &str) -> Result<(), String> {
    let object = schema
        .as_object()
        .ok_or_else(|| format!("{path}: schema must be an object"))?;

    if let Some(expected) = object.get("type")
        && !matches_type(expected, instance)?
    {
        return Err(format!(
            "{path}: expected type {}, got {}",
            display_type(expected),
            instance_type(instance)
        ));
    }

    if let Some(expected) = object.get("const")
        && instance != expected
    {
        return Err(format!("{path}: value must equal schema const {expected}"));
    }

    if let Some(values) = object.get("enum") {
        let values = values
            .as_array()
            .ok_or_else(|| format!("{path}: schema enum must be an array"))?;
        if !values.iter().any(|value| value == instance) {
            return Err(format!("{path}: value {instance} is not in schema enum"));
        }
    }

    if let Some(value) = instance.as_str() {
        if let Some(minimum) = object.get("minLength").and_then(Value::as_u64)
            && value.chars().count() < usize::try_from(minimum).unwrap()
        {
            return Err(format!(
                "{path}: string is shorter than minLength {minimum}"
            ));
        }
        if let Some(maximum) = object.get("maxLength").and_then(Value::as_u64)
            && value.chars().count() > usize::try_from(maximum).unwrap()
        {
            return Err(format!("{path}: string is longer than maxLength {maximum}"));
        }
        if let Some(pattern) = object.get("pattern").and_then(Value::as_str)
            && !matches_pattern(pattern, value)?
        {
            return Err(format!("{path}: {value:?} does not match {pattern:?}"));
        }
        if let Some(format) = object.get("format").and_then(Value::as_str) {
            match format {
                "date-time" if is_rfc3339_timestamp(value) => {}
                "date-time" => return Err(format!("{path}: {value:?} is not RFC 3339 date-time")),
                _ => return Err(format!("{path}: unsupported format {format:?}")),
            }
        }
    }

    if instance.is_number() {
        let Some(numeric) = instance.as_f64() else {
            return Err(format!(
                "{path}: number is not representable for contract validation"
            ));
        };
        if let Some(minimum) = object.get("minimum").and_then(Value::as_f64)
            && numeric < minimum
        {
            return Err(format!(
                "{path}: number {numeric} is below minimum {minimum}"
            ));
        }
        if let Some(maximum) = object.get("maximum").and_then(Value::as_f64)
            && numeric > maximum
        {
            return Err(format!(
                "{path}: number {numeric} exceeds maximum {maximum}"
            ));
        }
    }

    if let Some(instance_object) = instance.as_object() {
        if let Some(required) = object.get("required") {
            let required = required
                .as_array()
                .ok_or_else(|| format!("{path}: schema required must be an array"))?;
            for field in required {
                let field = field
                    .as_str()
                    .ok_or_else(|| format!("{path}: required entries must be strings"))?;
                if !instance_object.contains_key(field) {
                    return Err(format!("{path}: missing required property {field:?}"));
                }
            }
        }

        let properties = object.get("properties").and_then(Value::as_object);
        if let Some(properties) = properties {
            for (name, property_schema) in properties {
                if let Some(value) = instance_object.get(name) {
                    validate_value(property_schema, value, &format!("{path}.{name}"))?;
                }
            }
        }

        if object.get("additionalProperties") == Some(&Value::Bool(false)) {
            let known = properties
                .map(|properties| {
                    properties
                        .keys()
                        .map(String::as_str)
                        .collect::<BTreeSet<_>>()
                })
                .unwrap_or_default();
            for name in instance_object.keys() {
                if !known.contains(name.as_str()) {
                    return Err(format!("{path}: unexpected property {name:?}"));
                }
            }
        }
    }

    if let Some(instance_array) = instance.as_array() {
        let length = instance_array.len() as u64;
        if let Some(minimum) = object.get("minItems").and_then(Value::as_u64)
            && length < minimum
        {
            return Err(format!("{path}: array is shorter than minItems {minimum}"));
        }
        if let Some(maximum) = object.get("maxItems").and_then(Value::as_u64)
            && length > maximum
        {
            return Err(format!("{path}: array is longer than maxItems {maximum}"));
        }
        if object.get("uniqueItems") == Some(&Value::Bool(true)) {
            for (index, value) in instance_array.iter().enumerate() {
                if instance_array[..index].contains(value) {
                    return Err(format!(
                        "{path}[{index}]: duplicate item violates uniqueItems"
                    ));
                }
            }
        }
        if let Some(item_schema) = object.get("items") {
            for (index, value) in instance_array.iter().enumerate() {
                validate_value(item_schema, value, &format!("{path}[{index}]"))?;
            }
        }
    }

    Ok(())
}

fn matches_type(expected: &Value, instance: &Value) -> Result<bool, String> {
    match expected {
        Value::String(kind) => type_matches(kind, instance),
        Value::Array(kinds) => {
            for kind in kinds {
                let kind = kind
                    .as_str()
                    .ok_or_else(|| "schema type arrays must contain strings".to_owned())?;
                if type_matches(kind, instance)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        _ => Err("schema type must be a string or array of strings".to_owned()),
    }
}

fn type_matches(kind: &str, instance: &Value) -> Result<bool, String> {
    Ok(match kind {
        "array" => instance.is_array(),
        "integer" => instance.as_i64().is_some() || instance.as_u64().is_some(),
        "null" => instance.is_null(),
        "number" => instance.is_number(),
        "object" => instance.is_object(),
        "string" => instance.is_string(),
        _ => return Err(format!("unsupported JSON Schema type {kind:?}")),
    })
}

fn display_type(expected: &Value) -> String {
    match expected {
        Value::String(kind) => kind.clone(),
        Value::Array(kinds) => kinds
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join("|"),
        _ => expected.to_string(),
    }
}

fn instance_type(value: &Value) -> &'static str {
    match value {
        Value::Array(_) => "array",
        Value::Bool(_) => "boolean",
        Value::Null => "null",
        Value::Number(number) if number.is_i64() || number.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::Object(_) => "object",
        Value::String(_) => "string",
    }
}

fn is_supported_pattern(pattern: &str) -> bool {
    matches!(
        pattern,
        "^[A-Za-z0-9_.-]+$"
            | "^[A-Za-z0-9_.-]{1,64}$"
            | "^[A-Za-z0-9_.-]{1,128}$"
            | "^[A-Za-z0-9_./-]+$"
            | "^sha256:[0-9a-f]{64}$"
    )
}

fn matches_pattern(pattern: &str, value: &str) -> Result<bool, String> {
    match pattern {
        "^[A-Za-z0-9_.-]+$" => Ok(!value.is_empty() && value.bytes().all(is_portable_id_byte)),
        "^[A-Za-z0-9_.-]{1,64}$" => {
            Ok((1..=64).contains(&value.len()) && value.bytes().all(is_portable_id_byte))
        }
        "^[A-Za-z0-9_.-]{1,128}$" => {
            Ok((1..=128).contains(&value.len()) && value.bytes().all(is_portable_id_byte))
        }
        "^[A-Za-z0-9_./-]+$" => Ok(!value.is_empty()
            && value
                .bytes()
                .all(|byte| byte == b'/' || is_portable_id_byte(byte))),
        "^sha256:[0-9a-f]{64}$" => Ok(value.strip_prefix("sha256:").is_some_and(|digest| {
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })),
        _ => Err(format!("unsupported JSON Schema pattern {pattern:?}")),
    }
}

fn is_portable_id_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-')
}

fn is_rfc3339_timestamp(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() < 20
        || bytes.get(4) != Some(&b'-')
        || bytes.get(7) != Some(&b'-')
        || !matches!(bytes.get(10), Some(b'T' | b't'))
        || bytes.get(13) != Some(&b':')
        || bytes.get(16) != Some(&b':')
    {
        return false;
    }

    let Some(year) = parse_decimal(&bytes[0..4]) else {
        return false;
    };
    let Some(month) = parse_decimal(&bytes[5..7]) else {
        return false;
    };
    let Some(day) = parse_decimal(&bytes[8..10]) else {
        return false;
    };
    let Some(hour) = parse_decimal(&bytes[11..13]) else {
        return false;
    };
    let Some(minute) = parse_decimal(&bytes[14..16]) else {
        return false;
    };
    let Some(second) = parse_decimal(&bytes[17..19]) else {
        return false;
    };

    if !(1..=12).contains(&month)
        || day == 0
        || day > days_in_month(year, month)
        || hour > 23
        || minute > 59
        || second > 60
        || (second == 60 && minute != 59)
    {
        return false;
    }

    let mut rest = &bytes[19..];
    if rest.first() == Some(&b'.') {
        let fraction = rest[1..]
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        if fraction == 0 {
            return false;
        }
        rest = &rest[1 + fraction..];
    }

    match rest {
        [b'Z' | b'z'] => true,
        [b'+' | b'-', offset @ ..] if offset.len() == 5 && offset[2] == b':' => {
            let Some(offset_hour) = parse_decimal(&offset[0..2]) else {
                return false;
            };
            let Some(offset_minute) = parse_decimal(&offset[3..5]) else {
                return false;
            };
            offset_hour <= 23 && offset_minute <= 59
        }
        _ => false,
    }
}

fn parse_decimal(bytes: &[u8]) -> Option<u32> {
    let mut value = 0_u32;
    for byte in bytes {
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value
            .checked_mul(10)?
            .checked_add(u32::from(*byte - b'0'))?;
    }
    Some(value)
}

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

fn is_leap_year(year: u32) -> bool {
    year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
}

fn validate_artifact_array(path: &Path, artifact_schema: &Value) {
    let value = json_file(path);
    let records = value
        .as_array()
        .unwrap_or_else(|| panic!("{} must contain an artifact array", path.display()));
    for (index, record) in records.iter().enumerate() {
        let label = format!("{}[{index}]", path.display());
        validate(artifact_schema, record, &label);
    }
}

fn collect_files(root: &Path, directory: &Path, files: &mut Vec<String>) {
    let mut entries = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", directory.display()))
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_else(|error| panic!("failed to enumerate {}: {error}", directory.display()));
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let path = entry.path();
        let metadata = entry
            .metadata()
            .unwrap_or_else(|error| panic!("failed to stat {}: {error}", path.display()));
        if metadata.is_dir() {
            collect_files(root, &path, files);
        } else if metadata.is_file() {
            files.push(
                path.strip_prefix(root)
                    .expect("golden path is below golden root")
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        } else {
            panic!(
                "golden project contains unsupported entry {}",
                path.display()
            );
        }
    }
}

fn manifest() -> ProjectManifest {
    ProjectManifest {
        format_version: PROJECT_FORMAT_VERSION,
        project_id: "schema_contract_project".to_owned(),
        title: "Schema Contract Project".to_owned(),
        created_at: "2026-09-20T00:00:00Z".to_owned(),
        source: ProjectSource {
            kind: "text".to_owned(),
            path: "source/story.txt".to_owned(),
            extra: BTreeMap::new(),
        },
        defaults: ProjectDefaults {
            language: "en".to_owned(),
            extra: BTreeMap::new(),
        },
        workflow: ProjectWorkflow {
            recipe_id: "default-film".to_owned(),
            recipe_version: 1,
            extra: BTreeMap::new(),
        },
        extra: BTreeMap::new(),
    }
}

fn runtime_policy() -> JobRuntimePolicy {
    JobRuntimePolicy {
        execution: ProviderExecutionPolicy::new(
            NonZeroU16::new(1).expect("one is non-zero"),
            BackoffPolicy::new(100, 1_000, 3).expect("test backoff is valid"),
        ),
        fallback_policy: FallbackPolicy {
            paid_fallback: PaidFallback::Allowed,
            max_paid_amount_micros: Some(1_000_000),
            require_user_approval: true,
        },
    }
}

#[test]
fn committed_schema_inventory_is_explicit_and_supported() {
    let root = repo_root().join("schemas");
    let mut files = Vec::new();
    collect_files(&root, &root, &mut files);
    assert_eq!(
        files,
        [
            "artifact.schema.json",
            "job.schema.json",
            "project.schema.json",
            "revisions.schema.json",
            "scene.schema.json",
            "screenplay-scene.schema.json",
            "screenplay.schema.json",
            "selection.schema.json",
            "shot.schema.json",
            "source.schema.json",
            "story-analysis.schema.json",
        ]
    );

    for file in files {
        let _ = schema(&file);
    }
}

#[test]
fn minimal_golden_project_is_fully_schema_accounted() {
    let project_schema = schema("project.schema.json");
    let artifact_schema = schema("artifact.schema.json");
    let selection_schema = schema("selection.schema.json");
    let scene_schema = schema("scene.schema.json");
    let shot_schema = schema("shot.schema.json");
    let root = repo_root().join("fixtures/projects/minimal");

    let mut files = Vec::new();
    collect_files(&root, &root, &mut files);
    assert_eq!(
        files,
        [
            "characters/alice/artifacts.json",
            "characters/alice/selection.json",
            "project.toml",
            "scenes/scene_001/scene.json",
            "scenes/scene_001/shots/shot_001/artifacts.json",
            "scenes/scene_001/shots/shot_001/selection.json",
            "scenes/scene_001/shots/shot_001/shot.json",
            "source/story.txt",
        ]
    );

    let project = toml_file_as_json(&root.join("project.toml"));
    validate(&project_schema, &project, "golden project.toml");
    validate_artifact_array(
        &root.join("characters/alice/artifacts.json"),
        &artifact_schema,
    );
    validate(
        &selection_schema,
        &json_file(&root.join("characters/alice/selection.json")),
        "golden character selection",
    );
    validate(
        &scene_schema,
        &json_file(&root.join("scenes/scene_001/scene.json")),
        "golden scene",
    );
    validate_artifact_array(
        &root.join("scenes/scene_001/shots/shot_001/artifacts.json"),
        &artifact_schema,
    );
    validate(
        &selection_schema,
        &json_file(&root.join("scenes/scene_001/shots/shot_001/selection.json")),
        "golden shot selection",
    );
    validate(
        &shot_schema,
        &json_file(&root.join("scenes/scene_001/shots/shot_001/shot.json")),
        "golden shot",
    );

    let source_path = project["source"]["path"]
        .as_str()
        .expect("golden project source.path must be a string");
    let source = fs::read_to_string(root.join(source_path))
        .expect("declared golden source must be valid UTF-8 text");
    assert!(!source.trim().is_empty(), "golden source must not be empty");
}

#[test]
fn real_project_and_shot_persistence_match_committed_schemas() {
    let project_schema = schema("project.schema.json");
    let artifact_schema = schema("artifact.schema.json");
    let selection_schema = schema("selection.schema.json");
    let shot_schema = schema("shot.schema.json");
    let temp = TempDir::new();
    let project = CanonicalProject::create(&temp.0, manifest(), b"schema contract story\n")
        .expect("test project should be creatable");

    validate(
        &project_schema,
        &toml_file_as_json(&temp.0.join("project.toml")),
        "production project.toml",
    );

    let mut shot = ShotWorkflow::load(&project, 1).expect("shot should load");
    shot.generate(&project)
        .expect("shot generation should persist");
    let shot_root = temp.0.join("scenes/scene_001/shots/shot_001");

    validate(
        &shot_schema,
        &json_file(&shot_root.join("shot.json")),
        "production shot.json",
    );
    validate_artifact_array(&shot_root.join("artifacts.json"), &artifact_schema);
    validate(
        &selection_schema,
        &json_file(&shot_root.join("selection.json")),
        "production selection.json",
    );
}

#[test]
fn bootstrap_scene_written_at_creation_matches_committed_schemas() {
    let scene_schema = schema("scene.schema.json");
    let shot_schema = schema("shot.schema.json");
    let temp = TempDir::new();
    let project = CanonicalProject::create_with_files(
        &temp.0,
        manifest(),
        b"schema contract story\n",
        &kineto_core::bootstrap::scene_files(),
    )
    .expect("test project should be creatable with the bootstrap scene");

    validate(
        &scene_schema,
        &json_file(&temp.0.join("scenes/scene_001/scene.json")),
        "bootstrap scene.json",
    );
    for shot_number in 1..=2 {
        let shot_json = temp.0.join(format!(
            "scenes/scene_001/shots/shot_{shot_number:03}/shot.json"
        ));
        validate(&shot_schema, &json_file(&shot_json), "bootstrap shot.json");

        // A canonical write keeps the intent and still matches the schema.
        let mut shot = ShotWorkflow::load(&project, shot_number).expect("shot should load");
        shot.generate(&project)
            .expect("bootstrap shot should accept a generation");
        let written = json_file(&shot_json);
        validate(&shot_schema, &written, "bootstrap shot.json after a write");
        assert!(
            written["action"]
                .as_str()
                .is_some_and(|action| !action.is_empty())
        );
    }
}

#[test]
fn shot_video_records_match_committed_schemas() {
    let artifact_schema = schema("artifact.schema.json");
    let shot_schema = schema("shot.schema.json");
    let temp = TempDir::new();
    let project = CanonicalProject::create(&temp.0, manifest(), b"schema contract story\n")
        .expect("test project should be creatable");
    let mut shot = ShotWorkflow::load(&project, 1).expect("shot should load");
    shot.generate(&project)
        .expect("shot generation should persist");
    shot.select(&project, 0)
        .expect("a candidate should be selectable");
    shot.lock(&project).expect("the selection should lock");

    let plan = shot
        .plan_video(
            &project,
            kineto_project::shot::VideoGenerationSpec {
                provider: "schema-contract-video",
                model: "clip-1",
                canonical_params: b"duration=6",
                compiler_version: "video-prompt/1",
            },
        )
        .expect("a video should be plannable from the locked keyframe");
    let media_path = kineto_project::shot::video_media_path(1, plan.artifact_id())
        .expect("video path should be valid");
    let bytes = b"\0\0\0\x18ftypisomschema";
    project
        .root()
        .write_atomic(&media_path, bytes)
        .expect("video bytes should be written");
    shot.attach_video(
        &project,
        &plan,
        &kineto_project::shot::VideoOutput {
            artifact_id: plan.artifact_id().clone(),
            content_hash: kineto_project::ContentHash::of(bytes),
            provider: "schema-contract-video".to_owned(),
            model: "clip-1".to_owned(),
            media_path,
            provenance: None,
            keyframe: plan.keyframe().expect("plan names its keyframe").0.clone(),
        },
    )
    .expect("the video should attach");
    shot.lock_video(&project).expect("the video should lock");

    let shot_root = temp.0.join("scenes/scene_001/shots/shot_001");
    validate(
        &shot_schema,
        &json_file(&shot_root.join("shot.json")),
        "shot.json with a video revision",
    );
    validate_artifact_array(&shot_root.join("artifacts.json"), &artifact_schema);
}

#[test]
fn the_source_schema_rejects_an_empty_revision_list() {
    let source_schema = schema("source.schema.json");
    let empty = serde_json::json!({"schema_version": 1, "revisions": []});
    assert!(validate_value(&source_schema, &empty, "$").is_err());
}

#[test]
fn imported_source_revisions_match_committed_schema() {
    let source_schema = schema("source.schema.json");
    let temp = TempDir::new();
    let project = CanonicalProject::create(&temp.0, manifest(), b"schema contract story\n")
        .expect("test project should be creatable");
    kineto_project::source::import(
        &project,
        b"# Imported\r\n\r\nA second draft.\r\n",
        kineto_project::source::SourceFormat::Markdown,
        Some("draft.md"),
    )
    .expect("the source should import");
    validate(
        &source_schema,
        &json_file(&temp.0.join("source/source.json")),
        "source/source.json",
    );
}

#[test]
fn generated_story_analysis_and_its_revision_log_match_committed_schemas() {
    let analysis_schema = schema("story-analysis.schema.json");
    let revisions_schema = schema("revisions.schema.json");
    let temp = TempDir::new();
    let project = CanonicalProject::create(&temp.0, manifest(), b"schema contract story\n")
        .expect("test project should be creatable");
    validate(
        &analysis_schema,
        &serde_json::from_str(kineto_project::story::SAMPLE_ANALYSIS_JSON)
            .expect("the sample is JSON"),
        "sample story analysis",
    );
    let record = kineto_core::story::analyze(
        &project,
        &kineto_llm::fake::FakeLlm::answering([kineto_project::story::SAMPLE_ANALYSIS_JSON]),
    )
    .expect("the analysis should be generated");
    validate(
        &analysis_schema,
        &json_file(&temp.0.join(&record.path)),
        "generated story analysis",
    );
    validate(
        &revisions_schema,
        &json_file(&temp.0.join("story/analysis/revisions.json")),
        "story analysis revisions",
    );
}

#[test]
fn the_story_analysis_schema_rejects_unknown_fields() {
    let analysis_schema = schema("story-analysis.schema.json");
    let mut sample: Value = serde_json::from_str(kineto_project::story::SAMPLE_ANALYSIS_JSON)
        .expect("the sample is JSON");
    sample["characters"][0]["age"] = 40.into();
    assert!(validate_value(&analysis_schema, &sample, "$").is_err());
}

#[test]
fn generated_screenplays_and_their_revision_log_match_committed_schemas() {
    let screenplay_schema = schema("screenplay.schema.json");
    let scene_schema = schema("screenplay-scene.schema.json");
    let revisions_schema = schema("revisions.schema.json");
    let temp = TempDir::new();
    let project = CanonicalProject::create(&temp.0, manifest(), b"schema contract story\n")
        .expect("test project should be creatable");
    kineto_core::story::analyze(
        &project,
        &kineto_llm::fake::FakeLlm::answering([kineto_project::story::SAMPLE_ANALYSIS_JSON]),
    )
    .expect("the analysis should be generated");
    let written = kineto_core::screenplay::write(
        &project,
        &kineto_llm::fake::FakeLlm::answering([kineto_project::screenplay::SAMPLE_SCREENPLAY_JSON]),
    )
    .expect("the screenplay should be written");
    let sample: Value = serde_json::from_str(kineto_project::screenplay::SAMPLE_SCREENPLAY_JSON)
        .expect("the sample is JSON");
    validate(&scene_schema, &sample["scenes"][0], "sample scene");
    let mut scene = sample["scenes"][0].clone();
    scene["blocks"][0]["text"] = "Two chairs, one empty.".into();
    let rewritten = kineto_core::screenplay::rewrite_scene(
        &project,
        &kineto_llm::fake::FakeLlm::answering([scene.to_string()]),
        "office_night",
        None,
    )
    .expect("the scene should be rewritten");
    for record in [&written, &rewritten] {
        validate(
            &screenplay_schema,
            &json_file(&temp.0.join(&record.path)),
            &record.path,
        );
    }
    validate(
        &revisions_schema,
        &json_file(&temp.0.join("screenplay/revisions.json")),
        "screenplay revisions",
    );
}

#[test]
fn a_screenplay_scene_is_the_same_shape_alone_and_in_a_screenplay() {
    let strip = |mut value: Value| {
        let object = value.as_object_mut().expect("schemas are objects");
        for key in ["$schema", "$id", "title", "description"] {
            object.remove(key);
        }
        value
    };
    let screenplay_schema = schema("screenplay.schema.json");
    assert_eq!(
        strip(schema("screenplay-scene.schema.json")),
        strip(screenplay_schema["properties"]["scenes"]["items"].clone()),
        "a scene rewritten on its own must fit back into the screenplay"
    );
}

#[test]
fn the_screenplay_schema_rejects_drift() {
    let screenplay_schema = schema("screenplay.schema.json");
    let sample: Value = serde_json::from_str(kineto_project::screenplay::SAMPLE_SCREENPLAY_JSON)
        .expect("the sample is JSON");
    type Edit = fn(&mut Value);
    let drift: [Edit; 5] = [
        |value| value["scenes"][0]["mood"] = "dark".into(),
        |value| value["scenes"][0]["blocks"][0]["kind"] = "montage".into(),
        |value| value["scenes"][0]["setting"] = "indoors".into(),
        |value| value["scenes"] = serde_json::json!([]),
        |value| value["scenes"][0]["id"] = "s".repeat(65).into(),
    ];
    for edit in drift {
        let mut value = sample.clone();
        edit(&mut value);
        assert!(validate_value(&screenplay_schema, &value, "$").is_err());
    }
}

#[test]
fn real_job_runtime_intent_matches_committed_schema() {
    let job_schema = schema("job.schema.json");
    let temp = TempDir::new();
    let project = CanonicalProject::create(&temp.0, manifest(), b"schema contract story\n")
        .expect("test project should be creatable");
    let runtime = JobRuntime::new(
        &project,
        ProviderKey::new("schema-contract-provider").expect("provider key should be valid"),
        runtime_policy(),
    );
    let job_id = JobId::new("schema_contract_job").expect("job id should be valid");

    runtime
        .prepare(
            job_id.clone(),
            "image.generate",
            InputHash::new(format!("sha256:{}", "a".repeat(64)))
                .expect("input hash should be valid"),
            ArtifactId::new("schema_contract_candidate").expect("artifact id should be valid"),
        )
        .expect("prepare should persist the write-ahead intent");

    validate(
        &job_schema,
        &json_file(
            &temp
                .0
                .join(".kineto/jobs/intents")
                .join(format!("{}.json", job_id.as_str())),
        ),
        "production job intent",
    );
}

#[test]
fn production_generation_jobs_and_candidates_match_committed_schemas() {
    let job_schema = schema("job.schema.json");
    let artifact_schema = schema("artifact.schema.json");
    let temp = TempDir::new();
    let project = CanonicalProject::create(&temp.0, manifest(), b"schema contract story\n")
        .expect("test project should be creatable");
    let mut shot = ShotWorkflow::load(&project, 1).expect("shot should load");

    generation::generate(&project, &mut shot, &ImageProvider::LocalDemo, &mut NoHooks)
        .expect("generation should run as write-ahead jobs");

    let mut intents = fs::read_dir(temp.0.join(".kineto/jobs/intents"))
        .expect("generation should record intents")
        .map(|entry| entry.expect("intent entry should be readable").path())
        .collect::<Vec<_>>();
    intents.sort();
    assert_eq!(intents.len(), 3, "one intent per candidate");
    for intent in &intents {
        let value = json_file(intent);
        validate(&job_schema, &value, "production generation job intent");
        assert_eq!(value["state"], "reconciled");
        assert_eq!(value["operation"], "image.generate");
    }

    let artifacts_path = temp
        .0
        .join("scenes/scene_001/shots/shot_001/artifacts.json");
    validate_artifact_array(&artifacts_path, &artifact_schema);
    for artifact in json_file(&artifacts_path)
        .as_array()
        .expect("artifacts.json is an array")
    {
        assert_eq!(artifact["provider"], "local-demo");
        assert_eq!(artifact["model"], "deterministic-demo-v1");
    }
}

/// Answers every MiniMax image request with a tiny JPEG; never touches the
/// network.
struct RecordedMinimax;

impl kineto_provider_minimax::Transport for RecordedMinimax {
    fn post_json(
        &self,
        _url: &str,
        _api_key: &kineto_providers::secret::SecretString,
        _body: &[u8],
    ) -> Result<kineto_provider_minimax::HttpResponse, kineto_provider_minimax::TransportFailure>
    {
        Ok(kineto_provider_minimax::HttpResponse {
            status: 200,
            // base64 of ff d8 ff e0 00 10
            body: br#"{"id":"r","data":{"image_base64":["/9j/4AAQ"]},"base_resp":{"status_code":0,"status_msg":"success"}}"#
                .to_vec(),
        })
    }

    fn get(
        &self,
        _url: &str,
        _api_key: &kineto_providers::secret::SecretString,
    ) -> Result<kineto_provider_minimax::HttpResponse, kineto_provider_minimax::TransportFailure>
    {
        unreachable!("image generation makes no GET request")
    }

    fn download(
        &self,
        _url: &str,
        _max_bytes: u64,
    ) -> Result<kineto_provider_minimax::HttpResponse, kineto_provider_minimax::TransportFailure>
    {
        unreachable!("image generation downloads nothing")
    }
}

#[test]
fn provider_generated_keyframe_candidates_match_committed_schemas() {
    let artifact_schema = schema("artifact.schema.json");
    let temp = TempDir::new();
    let project = CanonicalProject::create_with_files(
        &temp.0,
        manifest(),
        b"schema contract story\n",
        &kineto_core::bootstrap::scene_files(),
    )
    .expect("test project should be creatable with the bootstrap scene");
    let client = kineto_provider_minimax::MinimaxClient::with_transport(
        RecordedMinimax,
        kineto_providers::secret::SecretString::new("sk-contract".to_owned())
            .expect("test key is not blank"),
        "https://example.test",
    );
    let mut shot = ShotWorkflow::load(&project, 1).expect("shot should load");

    generation::generate_with(
        &project,
        &mut shot,
        kineto_core::minimax::MinimaxKeyframes::new(&project, Some(&client)),
        &mut NoHooks,
    )
    .expect("generation should attach provider keyframes");

    let artifacts_path = temp
        .0
        .join("scenes/scene_001/shots/shot_001/artifacts.json");
    validate_artifact_array(&artifacts_path, &artifact_schema);
    for artifact in json_file(&artifacts_path)
        .as_array()
        .expect("artifacts.json is an array")
    {
        for field in [
            "media_path",
            "media_state",
            "compiler_version",
            "compiled_prompt",
            "prompt_hash",
            "applied_constraints",
            "dropped_constraints",
            "degradation",
        ] {
            assert!(!artifact[field].is_null(), "{field} is recorded");
        }
    }
}

#[test]
fn contract_harness_fails_closed_on_unknown_schema_keywords_and_patterns() {
    let unsupported_keyword = serde_json::json!({
        "type": "object",
        "oneOf": [{"type": "object"}]
    });
    assert!(ensure_supported_schema(&unsupported_keyword, "$").is_err());

    let unsupported_pattern = serde_json::json!({
        "type": "string",
        "pattern": "^new-pattern$"
    });
    assert!(ensure_supported_schema(&unsupported_pattern, "$").is_err());
}

#[test]
fn committed_schemas_reject_representative_model_drift() {
    let project_schema = schema("project.schema.json");
    let mut project =
        toml_file_as_json(&repo_root().join("fixtures/projects/minimal/project.toml"));
    project
        .as_object_mut()
        .expect("project fixture is an object")
        .remove("title");
    assert!(validate_value(&project_schema, &project, "$").is_err());

    let mut invalid_timestamp =
        toml_file_as_json(&repo_root().join("fixtures/projects/minimal/project.toml"));
    invalid_timestamp["created_at"] = Value::String("2026-02-30T25:61:61+24:60".to_owned());
    assert!(validate_value(&project_schema, &invalid_timestamp, "$").is_err());

    let artifact_schema = schema("artifact.schema.json");
    let mut artifact =
        json_file(&repo_root().join("fixtures/projects/minimal/characters/alice/artifacts.json"))
            .as_array()
            .expect("artifact fixture is an array")[0]
            .clone();
    artifact["content_hash"] = Value::String("not-a-canonical-hash".to_owned());
    assert!(validate_value(&artifact_schema, &artifact, "$").is_err());

    let selection_schema = schema("selection.schema.json");
    let duplicate_selection = serde_json::json!({
        "selected_artifact_id": "candidate_a",
        "candidate_artifact_ids": ["candidate_a", "candidate_a"]
    });
    assert!(validate_value(&selection_schema, &duplicate_selection, "$").is_err());

    let job_schema = schema("job.schema.json");
    let invalid_job = serde_json::json!({
        "schema_version": 1,
        "provider": "provider with spaces",
        "job_id": "job_001",
        "operation": "image.generate",
        "state": "prepared",
        "idempotency_key": "image.generate:key",
        "input_hash": format!("sha256:{}", "a".repeat(64)),
        "attempts": 0
    });
    assert!(validate_value(&job_schema, &invalid_job, "$").is_err());
}
