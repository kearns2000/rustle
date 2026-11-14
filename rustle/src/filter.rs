use chrono::{DateTime, Utc};

use crate::entry::{Entry, Kind};
use crate::level::Level;

#[derive(Debug, Clone)]
pub struct Filter {
    pub level: Option<Level>,
    pub min_level: Option<Level>,
    pub search: Option<String>,
    pub trace: Option<String>,
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
}

impl Filter {
    pub fn matches(&self, entry: &Entry) -> bool {
        if let Some(expected) = self.level {
            match entry.level {
                Some(level) if level == expected => {}
                _ => return false,
            }
        }
        if let Some(minimum) = self.min_level {
            match entry.level {
                Some(level) if level >= minimum => {}
                _ => return false,
            }
        }
        if let Some(search) = &self.search {
            if !entry.raw.to_lowercase().contains(search) {
                return false;
            }
        }
        if let Some(trace) = &self.trace {
            if !trace_matches(entry, trace) {
                return false;
            }
        }
        if self.since.is_some() || self.until.is_some() {
            let Some(timestamp) = entry.timestamp else {
                return false;
            };
            if self.since.is_some_and(|since| timestamp < since) {
                return false;
            }
            if self.until.is_some_and(|until| timestamp > until) {
                return false;
            }
        }
        true
    }
}

fn trace_matches(entry: &Entry, trace: &str) -> bool {
    match entry.kind {
        Kind::Json => {
            entry.trace_id.as_deref() == Some(trace)
                || entry.correlation_id.as_deref() == Some(trace)
        }
        Kind::Text => entry.raw.contains(trace),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_line;

    fn filter() -> Filter {
        Filter {
            level: None,
            min_level: None,
            search: None,
            trace: None,
            since: None,
            until: None,
        }
    }

    fn line(raw: &str) -> Entry {
        parse_line("api.log", 0, 1, raw)
    }

    #[test]
    fn level_filters_skip_unrecognised_levels_and_combine_with_and() {
        let error = line("2026-11-14T10:00:00Z ERROR alpha failed");
        let info = line("2026-11-14T10:00:00Z INFO alpha failed");
        let prose = line("alpha failed without a level");

        let mut exact = filter();
        exact.level = Some(Level::Error);
        assert!(exact.matches(&error));
        assert!(!exact.matches(&info));
        assert!(!exact.matches(&prose));

        let mut minimum = filter();
        minimum.min_level = Some(Level::Warn);
        assert!(minimum.matches(&error));
        assert!(!minimum.matches(&info));
        assert!(!minimum.matches(&prose));

        let mut combined = filter();
        combined.level = Some(Level::Error);
        combined.search = Some("alpha".to_string());
        combined.trace = Some("missing".to_string());
        assert!(!combined.matches(&error));
        combined.trace = Some("alpha".to_string());
        assert!(combined.matches(&error));
    }

    #[test]
    fn search_is_literal_and_trace_rules_differ_by_format() {
        let mut search = filter();
        search.search = Some("a.b".to_string());
        assert!(search.matches(&line("value A.B is here")));
        search.search = Some(".*".to_string());
        assert!(!search.matches(&line("value A.B is here")));

        let exact =
            line(r#"{"@t":"2026-11-14T10:00:00Z","@m":"mentions abc123","@tr":"abc123-extra"}"#);
        let message_only =
            line(r#"{"@t":"2026-11-14T10:00:00Z","@m":"mentions abc123","@l":"Info"}"#);
        let correlation =
            line(r#"{"@t":"2026-11-14T10:00:00Z","@m":"ok","CorrelationId":"abc123"}"#);
        let mut trace = filter();
        trace.trace = Some("abc123".to_string());
        assert!(!trace.matches(&exact));
        assert!(!trace.matches(&message_only));
        assert!(trace.matches(&correlation));
        assert!(trace.matches(&line("plain text abc123 tail")));
        assert!(!trace.matches(&line("plain text ABC123 tail")));
    }

    #[test]
    fn time_bounds_are_inclusive_and_require_a_timestamp() {
        let at = line("2026-11-14T10:00:00Z INFO at");
        let before = line("2026-11-14T09:59:59Z INFO before");
        let shifted = line("2026-11-14T12:00:00+02:00 INFO shifted");
        let untimed = line("INFO: no clock");

        let mut window = filter();
        window.since = crate::timeutil::parse_timestamp("2026-11-14T10:00:00Z");
        window.until = crate::timeutil::parse_timestamp("2026-11-14T10:00:00Z");
        assert!(window.matches(&at));
        assert!(window.matches(&shifted));
        assert!(!window.matches(&before));
        assert!(!window.matches(&untimed));
    }
}
