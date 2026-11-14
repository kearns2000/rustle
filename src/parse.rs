use serde_json::{Map, Value};

use crate::entry::{Entry, Kind};
use crate::level::Level;
use crate::timeutil::parse_timestamp;

const TIMESTAMP_FIELDS: &[&str] = &["@t", "Timestamp", "timestamp", "time"];
const LEVEL_FIELDS: &[&str] = &["@l", "Level", "level", "LogLevel"];
const MESSAGE_FIELDS: &[&str] = &["@m", "Message", "message"];
const TEMPLATE_FIELDS: &[&str] = &["@mt", "MessageTemplate"];
const EXCEPTION_FIELDS: &[&str] = &["@x", "Exception", "exception"];
const TRACE_FIELDS: &[&str] = &["@tr", "TraceId", "traceId", "trace_id"];
const CORRELATION_FIELDS: &[&str] = &["CorrelationId", "correlationId", "correlation_id"];
const LOGGER_FIELDS: &[&str] = &["SourceContext", "Category", "category"];

pub fn parse_line(source: &str, source_index: usize, line: usize, raw: &str) -> Entry {
    if let Some(value) = json_object(raw) {
        return from_json(source, source_index, line, raw, value);
    }
    from_text(source, source_index, line, raw)
}

fn json_object(raw: &str) -> Option<Value> {
    let trimmed = raw.trim();
    if !trimmed.starts_with('{') {
        return None;
    }
    match serde_json::from_str::<Value>(trimmed) {
        Ok(value) if value.is_object() => Some(value),
        _ => None,
    }
}

fn from_json(source: &str, source_index: usize, line: usize, raw: &str, value: Value) -> Entry {
    let Some(map) = value.as_object() else {
        return from_text(source, source_index, line, raw);
    };
    let timestamp = lookup(map, TIMESTAMP_FIELDS).and_then(parse_timestamp);
    let level = match lookup(map, LEVEL_FIELDS) {
        Some(text) => Level::parse(text),
        None if map.get("@t").is_some_and(|item| !item.is_null()) => Some(Level::Info),
        None => None,
    };
    let rendered = lookup(map, MESSAGE_FIELDS);
    let template = lookup(map, TEMPLATE_FIELDS);
    let (message, message_is_template) = match (rendered, template) {
        (Some(message), _) => (message.to_string(), false),
        (None, Some(template)) => (template.to_string(), true),
        (None, None) => (String::new(), false),
    };
    let trace_id = lookup(map, TRACE_FIELDS).map(str::to_string);
    let correlation_id = lookup(map, CORRELATION_FIELDS).map(str::to_string);
    let exception = lookup(map, EXCEPTION_FIELDS).map(str::to_string);
    let logger = lookup(map, LOGGER_FIELDS).map(str::to_string);
    Entry {
        source: source.to_string(),
        source_index,
        line,
        raw: raw.to_string(),
        timestamp,
        level,
        message,
        message_is_template,
        trace_id,
        correlation_id,
        exception,
        logger,
        original: value,
        kind: Kind::Json,
    }
}

/// First match wins. Locations are searched in order, and inside each location
/// the names are searched in order. Nulls, empty strings, and non-strings are
/// skipped so a later supported string can still be used.
fn lookup<'a>(root: &'a Map<String, Value>, names: &[&str]) -> Option<&'a str> {
    for location in locations(root) {
        for name in names {
            let Some(value) = location.get(*name) else {
                continue;
            };
            if value.is_null() {
                continue;
            }
            if let Some(text) = value.as_str().filter(|text| !text.is_empty()) {
                return Some(text);
            }
        }
    }
    None
}

fn locations(root: &Map<String, Value>) -> Vec<&Map<String, Value>> {
    let mut found = Vec::with_capacity(4);
    found.push(root);
    for key in ["Properties", "properties", "State"] {
        if let Some(nested) = root.get(key).and_then(Value::as_object) {
            found.push(nested);
        }
    }
    found
}

fn from_text(source: &str, source_index: usize, line: usize, raw: &str) -> Entry {
    let (timestamp, body) = split_timestamp(raw);
    let (level, message) = split_level(body, timestamp.is_some());
    let owned = raw.to_string();
    Entry {
        source: source.to_string(),
        source_index,
        line,
        raw: owned.clone(),
        timestamp,
        level,
        message,
        message_is_template: false,
        trace_id: None,
        correlation_id: None,
        exception: None,
        logger: None,
        original: Value::String(owned),
        kind: Kind::Text,
    }
}

fn split_timestamp(raw: &str) -> (Option<chrono::DateTime<chrono::Utc>>, &str) {
    let body = raw.trim_start();
    if body.is_empty() {
        return (None, raw);
    }
    let token_end = body.find(char::is_whitespace).unwrap_or(body.len());
    let token = &body[..token_end];
    if let Some(timestamp) = parse_timestamp(token) {
        let rest = body[token_end..].trim_start();
        (Some(timestamp), rest)
    } else {
        (None, raw)
    }
}

fn split_level(text: &str, had_timestamp: bool) -> (Option<Level>, String) {
    let body = text.trim_start();
    if body.is_empty() {
        return (None, text.to_string());
    }
    if let Some(parsed) = bracket_level(body) {
        return parsed;
    }
    if let Some((level, message, explicit)) = word_level(body) {
        if explicit || had_timestamp {
            return (Some(level), message);
        }
    }
    (None, text.to_string())
}

fn bracket_level(body: &str) -> Option<(Option<Level>, String)> {
    let inner = body.strip_prefix('[')?;
    let end = inner.find(']')?;
    let level = Level::parse(inner[..end].trim())?;
    let mut rest = inner[end + 1..].trim_start();
    if let Some(stripped) = rest.strip_prefix(':') {
        rest = stripped.trim_start();
    }
    Some((Some(level), rest.to_string()))
}

fn word_level(body: &str) -> Option<(Level, String, bool)> {
    let word_end = body.find(char::is_whitespace).unwrap_or(body.len());
    let word_end = body[..word_end].find(':').unwrap_or(word_end);
    if word_end == 0 {
        return None;
    }
    let level = Level::parse(&body[..word_end])?;
    let mut rest = body[word_end..].trim_start();
    let explicit = rest.starts_with(':');
    if explicit {
        rest = rest[1..].trim_start();
    }
    Some((level, rest.to_string(), explicit))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::for_each_line;
    use crate::timeutil::format_timestamp;
    use std::fs::File;
    use std::path::PathBuf;

    fn parse(raw: &str) -> Entry {
        parse_line("api.log", 0, 1, raw)
    }

    #[test]
    fn serilog_fields_nested_values_and_default_information() {
        let inferred = parse(
            r#"{"@t":"2026-11-14T10:00:00.1234567Z","@mt":"User {UserId} logged in","UserId":"42","@tr":"abc123","SourceContext":"Auth.Service"}"#,
        );
        assert_eq!(inferred.kind, Kind::Json);
        assert_eq!(inferred.level, Some(Level::Info));
        assert!(inferred.message_is_template);
        assert_eq!(inferred.message, "User {UserId} logged in");
        assert_eq!(inferred.trace_id.as_deref(), Some("abc123"));
        assert_eq!(inferred.logger.as_deref(), Some("Auth.Service"));
        assert_eq!(inferred.original["UserId"], "42");
        assert_eq!(
            format_timestamp(inferred.timestamp.unwrap()),
            "2026-11-14T10:00:00.1234567Z"
        );

        let rendered = parse(
            r#"{"@t":"2026-11-14T10:00:00Z","@m":"rendered","@mt":"template {Name}","@l":"Error"}"#,
        );
        assert_eq!(rendered.message, "rendered");
        assert!(!rendered.message_is_template);
        assert_eq!(rendered.level, Some(Level::Error));

        let invalid_level = parse(r#"{"@t":"2026-11-14T10:00:00Z","@l":"nope","@m":"x"}"#);
        assert_eq!(invalid_level.level, None);

        let nested = parse(
            r#"{"Timestamp":"2026-11-14T11:00:00Z","@m":"top","Properties":{"@t":"2026-11-14T10:00:00Z","trace_id":"from-props"},"State":{"TraceId":"from-state","CorrelationId":"corr"}}"#,
        );
        assert_eq!(
            format_timestamp(nested.timestamp.unwrap()),
            "2026-11-14T11:00:00Z"
        );
        assert_eq!(nested.trace_id.as_deref(), Some("from-props"));
        assert_eq!(nested.correlation_id.as_deref(), Some("corr"));

        let name_order =
            parse(r#"{"@t":"2026-11-14T10:00:00Z","Timestamp":"2026-11-14T12:00:00Z","@m":"x"}"#);
        assert_eq!(
            format_timestamp(name_order.timestamp.unwrap()),
            "2026-11-14T10:00:00Z"
        );

        let state_trace = parse(
            r#"{"@t":"2026-11-14T10:00:00Z","@m":"nested","State":{"TraceId":"state-id","Message":"ignored"}}"#,
        );
        assert_eq!(state_trace.message, "nested");
        assert_eq!(state_trace.trace_id.as_deref(), Some("state-id"));

        let string_state =
            parse(r#"{"Timestamp":"2026-11-14T10:00:00Z","Message":"ok","State":"text"}"#);
        assert_eq!(string_state.message, "ok");
        assert_eq!(string_state.level, None);
    }

    #[test]
    fn malformed_json_and_plain_text_stay_intact() {
        let malformed = parse("{not json");
        assert_eq!(malformed.kind, Kind::Text);
        assert_eq!(malformed.raw, "{not json");
        assert_eq!(malformed.level, None);

        let array = parse("[1, 2]");
        assert_eq!(array.kind, Kind::Text);

        let timed = parse("2026-11-14T10:00:00Z ERROR connection failed");
        assert_eq!(timed.level, Some(Level::Error));
        assert_eq!(timed.message, "connection failed");
        assert_eq!(timed.raw, "2026-11-14T10:00:00Z ERROR connection failed");
        assert_eq!(
            format_timestamp(timed.timestamp.unwrap()),
            "2026-11-14T10:00:00Z"
        );

        let offset = parse("2026-11-14T10:00:00+02:00 [WARN] disk low");
        assert_eq!(offset.level, Some(Level::Warn));
        assert_eq!(offset.message, "disk low");
        assert_eq!(
            format_timestamp(offset.timestamp.unwrap()),
            "2026-11-14T08:00:00Z"
        );

        let colon = parse("INFO: startup complete");
        assert_eq!(colon.level, Some(Level::Info));
        assert_eq!(colon.message, "startup complete");
        assert!(colon.timestamp.is_none());

        let prose = parse("error handling is hard");
        assert_eq!(prose.level, None);
        assert_eq!(prose.message, "error handling is hard");

        let bare = parse("INFO server started");
        assert_eq!(bare.level, None);
        assert_eq!(bare.raw, "INFO server started");

        let no_offset = parse("2026-11-14T10:00:00 INFO local");
        assert!(no_offset.timestamp.is_none());
        assert_eq!(no_offset.level, None);
    }

    #[test]
    fn bundled_examples_match_the_documented_shapes() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples");
        let serilog = read_entries(&root.join("serilog.jsonl"));
        assert_eq!(serilog[0].level, Some(Level::Info));
        assert!(serilog[0].message_is_template);
        assert_eq!(serilog[0].message, "User {UserId} logged in");
        assert_eq!(serilog[1].level, Some(Level::Error));
        assert_eq!(serilog[1].message, "connection failed");
        assert!(serilog[1]
            .exception
            .as_ref()
            .unwrap()
            .contains("SocketException"));
        assert_eq!(
            format_timestamp(serilog[1].timestamp.unwrap()),
            "2026-11-14T09:00:01Z"
        );
        assert_eq!(serilog[2].level, Some(Level::Trace));
        assert_eq!(serilog[2].trace_id.as_deref(), Some("from-props"));

        let microsoft = read_entries(&root.join("microsoft.jsonl"));
        assert_eq!(microsoft[0].level, Some(Level::Warn));
        assert_eq!(microsoft[0].message, "cache miss for Seattle");
        assert_eq!(microsoft[0].trace_id.as_deref(), Some("trace-state"));
        assert_eq!(microsoft[0].correlation_id.as_deref(), Some("corr-state"));
        assert_eq!(microsoft[0].logger.as_deref(), Some("App.Weather"));
        assert_eq!(microsoft[1].level, Some(Level::Info));
        assert_eq!(microsoft[1].correlation_id.as_deref(), Some("corr-props"));
        assert_eq!(microsoft[1].logger.as_deref(), Some("App.Config"));
        assert_eq!(microsoft[2].level, Some(Level::Error));
        assert_eq!(microsoft[2].message, "state was a string");

        let plain = read_entries(&root.join("plain.log"));
        assert_eq!(plain[0].level, Some(Level::Info));
        assert_eq!(plain[0].message, "server listening");
        assert_eq!(plain[1].level, Some(Level::Error));
        assert!(plain[1].raw.contains("abc123"));
        assert_eq!(plain[2].message, "startup complete");
        assert_eq!(plain[3].level, None);
        assert_eq!(plain[3].message, "this line has no structure at all");
    }

    fn read_entries(path: &std::path::Path) -> Vec<Entry> {
        let file = File::open(path).unwrap();
        let mut entries = Vec::new();
        let source = path.display().to_string();
        for_each_line(file, |line| {
            entries.push(parse_line(&source, 0, line.number, &line.text));
            Ok(())
        })
        .unwrap();
        entries
    }
}
