//! The subset of JSON Schema draft 2020-12 Kineto's committed schemas use,
//! executed.
//!
//! The committed schemas under `schemas/` are the contract for canonical
//! files and for what a language model is asked to answer. This crate
//! checks a JSON value against one: the schema contract tests use it on
//! project files, and `llm.complete` uses it on a model's answer before the
//! answer becomes typed state. It fails closed: a schema keyword, format or
//! pattern it does not implement is an error, never silently ignored, so
//! extending a schema cannot weaken validation.

use std::collections::BTreeSet;

use serde_json::Value;

/// Check that `schema` uses only what this crate implements and that
/// `instance` satisfies it.
pub fn validate(schema: &Value, instance: &Value) -> Result<(), String> {
    ensure_supported_schema(schema, "$")?;
    validate_value(schema, instance, "$")
}

/// Check that `schema` uses only what this crate implements; `path` names
/// it in errors, as `$` for a whole schema.
pub fn ensure_supported_schema(schema: &Value, path: &str) -> Result<(), String> {
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
        "array" | "boolean" | "integer" | "null" | "number" | "object" | "string" => Ok(()),
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

/// Check `instance` against `schema`; `path` names it in errors, as `$`
/// for a whole document. The first violation found is returned.
pub fn validate_value(schema: &Value, instance: &Value, path: &str) -> Result<(), String> {
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
            && value.chars().count() < usize::try_from(minimum).unwrap_or(usize::MAX)
        {
            return Err(format!(
                "{path}: string is shorter than minLength {minimum}"
            ));
        }
        if let Some(maximum) = object.get("maxLength").and_then(Value::as_u64)
            && value.chars().count() > usize::try_from(maximum).unwrap_or(usize::MAX)
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
        "boolean" => instance.is_boolean(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_value_is_checked_against_every_constraint_it_meets() {
        let schema = json!({
            "type": "object",
            "required": ["id", "tags"],
            "properties": {
                "id": {"type": "string", "pattern": "^[A-Za-z0-9_.-]{1,64}$"},
                "tags": {"type": "array", "uniqueItems": true, "items": {"type": "string", "minLength": 1}}
            },
            "additionalProperties": false
        });
        validate(&schema, &json!({"id": "office_night", "tags": ["rain"]})).unwrap();
        for (instance, expected) in [
            (
                json!({"id": "has space", "tags": []}),
                "$.id: \"has space\" does not match",
            ),
            (
                json!({"id": "a", "tags": [""]}),
                "$.tags[0]: string is shorter than minLength 1",
            ),
            (
                json!({"id": "a", "tags": ["x", "x"]}),
                "$.tags[1]: duplicate item",
            ),
            (
                json!({"id": "a", "tags": [], "mood": 1}),
                "$: unexpected property \"mood\"",
            ),
            (json!({"tags": []}), "$: missing required property \"id\""),
        ] {
            let error = validate(&schema, &instance).unwrap_err();
            assert!(
                error.starts_with(expected),
                "{error:?} does not start with {expected:?}"
            );
        }
    }

    #[test]
    fn a_boolean_is_only_a_boolean() {
        let schema = json!({"type": "object", "properties": {"approval": {"type": "boolean"}}});
        ensure_supported_schema(&schema, "$").unwrap();
        validate(&schema, &json!({"approval": false})).unwrap();
        for wrong in [json!(0), json!("true"), json!(null)] {
            assert!(
                validate(&schema, &json!({"approval": wrong})).is_err(),
                "{wrong}"
            );
        }
    }

    #[test]
    fn what_is_not_implemented_fails_closed() {
        for schema in [
            json!({"type": "object", "oneOf": []}),
            json!({"type": "string", "pattern": "^[a-z]+$"}),
            json!({"type": "string", "format": "email"}),
            json!({"type": "tuple"}),
        ] {
            assert!(
                validate(&schema, &json!("x")).is_err(),
                "{schema} was accepted"
            );
        }
    }
}
