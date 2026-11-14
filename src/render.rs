use std::io::Write;

use serde::Serialize;
use serde_json::Value;

use crate::cli::OutputFormat;
use crate::entry::Entry;
use crate::error::Result;
use crate::level::Level;
use crate::timeutil::format_timestamp;

pub struct Renderer<'a> {
    format: OutputFormat,
    color: bool,
    highlight: Option<&'a str>,
}

impl<'a> Renderer<'a> {
    pub fn new(format: OutputFormat, color: bool, highlight: Option<&'a str>) -> Self {
        Self {
            format,
            color,
            highlight,
        }
    }

    pub fn write(&self, entry: &Entry, out: &mut dyn Write) -> Result<()> {
        match self.format {
            OutputFormat::Text => write_text(entry, self.color, self.highlight, out),
            OutputFormat::Json => write_json(entry, out),
        }
    }
}

fn write_text(
    entry: &Entry,
    color: bool,
    highlight: Option<&str>,
    out: &mut dyn Write,
) -> Result<()> {
    let timestamp = entry
        .timestamp
        .map(format_timestamp)
        .unwrap_or_else(|| "-".to_string());
    let timestamp = style_text(&timestamp, highlight, None, color);
    let label = entry.level.map(Level::as_label).unwrap_or("-");
    let label = format!("{label:<5}");
    let label = style_text(&label, highlight, severity_code(entry.level, color), color);
    let source = style_text(&entry.source, highlight, None, color);
    let mut lines = entry.message.split('\n');
    let first = style_text(strip_cr(lines.next().unwrap_or("")), highlight, None, color);
    if entry.message.is_empty() {
        writeln!(out, "{timestamp}  {label}  {source}")?;
    } else {
        writeln!(out, "{timestamp}  {label}  {source}  {first}")?;
    }
    for extra in lines {
        let extra = style_text(strip_cr(extra), highlight, None, color);
        writeln!(out, "  {extra}")?;
    }
    if let Some(exception) = &entry.exception {
        for line in exception.split('\n') {
            let line = style_text(strip_cr(line), highlight, None, color);
            writeln!(out, "  {line}")?;
        }
    }
    Ok(())
}

fn strip_cr(line: &str) -> &str {
    line.strip_suffix('\r').unwrap_or(line)
}

/// Yellow text on a red background.
const HIGHLIGHT_CODE: &str = "93;41";

fn severity_code(level: Option<Level>, color: bool) -> Option<&'static str> {
    if !color {
        return None;
    }
    level.map(|level| match level {
        Level::Trace => "2",
        Level::Debug => "36",
        Level::Info => "32",
        Level::Warn => "33",
        Level::Error => "31",
        Level::Fatal => "1;31",
    })
}

fn style_text(text: &str, needle: Option<&str>, base: Option<&str>, color: bool) -> String {
    if !color {
        return text.to_string();
    }
    let Some(needle) = needle.filter(|needle| !needle.is_empty()) else {
        return paint(text, base);
    };
    let mut out = String::new();
    for (matched, slice) in split_matches(text, needle) {
        if slice.is_empty() {
            continue;
        }
        if matched {
            out.push_str(&paint(slice, Some(HIGHLIGHT_CODE)));
        } else {
            out.push_str(&paint(slice, base));
        }
    }
    out
}

fn paint(text: &str, code: Option<&str>) -> String {
    match code {
        Some(code) => format!("\u{1b}[{code}m{text}\u{1b}[0m"),
        None => text.to_string(),
    }
}

fn split_matches<'a>(text: &'a str, needle: &str) -> Vec<(bool, &'a str)> {
    let needle_lower = needle.to_lowercase();
    let width = needle.chars().count();
    if width == 0 || needle_lower.is_empty() {
        return vec![(false, text)];
    }
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut parts = Vec::new();
    let mut index = 0;
    let mut plain_start = 0;
    while index < chars.len() {
        let end_char = index + width;
        if end_char <= chars.len() {
            let start = chars[index].0;
            let end = chars
                .get(end_char)
                .map(|(byte, _)| *byte)
                .unwrap_or(text.len());
            let candidate = &text[start..end];
            if candidate.to_lowercase() == needle_lower {
                if start > plain_start {
                    parts.push((false, &text[plain_start..start]));
                }
                parts.push((true, candidate));
                plain_start = end;
                index = end_char;
                continue;
            }
        }
        index += 1;
    }
    if plain_start < text.len() {
        parts.push((false, &text[plain_start..]));
    }
    parts
}

fn write_json(entry: &Entry, out: &mut dyn Write) -> Result<()> {
    let view = JsonOut {
        source: &entry.source,
        line: entry.line,
        timestamp: entry.timestamp.map(format_timestamp),
        level: entry.level.map(Level::as_str),
        message: &entry.message,
        message_is_template: entry.message_is_template,
        trace_id: entry.trace_id.as_deref(),
        correlation_id: entry.correlation_id.as_deref(),
        exception: entry.exception.as_deref(),
        logger: entry.logger.as_deref(),
        original: &entry.original,
    };
    let json = serde_json::to_string(&view)
        .map_err(|err| crate::error::Error::runtime(format!("cannot encode JSON: {err}")))?;
    writeln!(out, "{json}")?;
    Ok(())
}

#[derive(Serialize)]
struct JsonOut<'a> {
    source: &'a str,
    line: usize,
    timestamp: Option<String>,
    level: Option<&'static str>,
    message: &'a str,
    message_is_template: bool,
    trace_id: Option<&'a str>,
    correlation_id: Option<&'a str>,
    exception: Option<&'a str>,
    logger: Option<&'a str>,
    original: &'a Value,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_line;
    use serde_json::Value;

    #[test]
    fn text_uses_severity_colour_only_when_requested() {
        let entry = parse_line(
            "api.log",
            0,
            4,
            "2026-11-14T10:00:00Z ERROR connection failed",
        );
        let mut plain = Vec::new();
        Renderer::new(OutputFormat::Text, false, None)
            .write(&entry, &mut plain)
            .unwrap();
        let text = String::from_utf8(plain).unwrap();
        assert!(text.contains("2026-11-14T10:00:00Z"));
        assert!(text.contains("ERROR"));
        assert!(text.contains("api.log"));
        assert!(text.contains("connection failed"));
        assert!(!text.contains('\u{1b}'));

        let mut coloured = Vec::new();
        let fatal = parse_line("api.log", 0, 1, "FATAL: disk gone");
        Renderer::new(OutputFormat::Text, true, None)
            .write(&fatal, &mut coloured)
            .unwrap();
        let coloured = String::from_utf8(coloured).unwrap();
        assert!(coloured.contains("\u{1b}[1;31m"));
        assert!(coloured.contains("disk gone"));
    }

    #[test]
    fn json_output_is_one_object_per_line() {
        let entry = parse_line(
            "api.log",
            0,
            2,
            r#"{"@t":"2026-11-14T10:00:00Z","@mt":"Hello {Name}","Name":"Ada","@x":"boom"}"#,
        );
        let mut out = Vec::new();
        Renderer::new(OutputFormat::Json, true, Some("Ada"))
            .write(&entry, &mut out)
            .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(!text.contains('\u{1b}'));
        assert_eq!(text.lines().count(), 1);
        let value: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        assert_eq!(value["source"], "api.log");
        assert_eq!(value["line"], 2);
        assert_eq!(value["level"], "info");
        assert_eq!(value["message"], "Hello {Name}");
        assert_eq!(value["message_is_template"], true);
        assert_eq!(value["exception"], "boom");
        assert_eq!(value["original"]["Name"], "Ada");
        assert!(value["trace_id"].is_null());
    }

    #[test]
    fn highlight_marks_every_match_and_keeps_the_other_text() {
        let entry = parse_line(
            "api.log",
            0,
            1,
            "2026-11-14T10:00:00Z INFO alpha then ALPHA stays",
        );
        let mut out = Vec::new();
        Renderer::new(OutputFormat::Text, true, Some("alpha"))
            .write(&entry, &mut out)
            .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("\u{1b}[93;41malpha\u{1b}[0m"));
        assert!(text.contains("\u{1b}[93;41mALPHA\u{1b}[0m"));
        assert!(text.contains("then"));
        assert!(text.contains("stays"));
        assert!(text.contains("INFO"));

        let mut plain = Vec::new();
        Renderer::new(OutputFormat::Text, false, Some("alpha"))
            .write(&entry, &mut plain)
            .unwrap();
        let plain = String::from_utf8(plain).unwrap();
        assert!(plain.contains("alpha then ALPHA stays"));
        assert!(!plain.contains('\u{1b}'));
    }
}
