use std::cmp::Ordering;

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::level::Level;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Json,
    Text,
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub source: String,
    pub source_index: usize,
    pub line: usize,
    pub raw: String,
    pub timestamp: Option<DateTime<Utc>>,
    pub level: Option<Level>,
    pub message: String,
    pub message_is_template: bool,
    pub trace_id: Option<String>,
    pub correlation_id: Option<String>,
    pub exception: Option<String>,
    pub logger: Option<String>,
    pub original: Value,
    pub kind: Kind,
}

/// Timestamp order, with missing timestamps last. Equal timestamps keep input
/// order and then line number.
pub fn cmp_merge(left: &Entry, right: &Entry) -> Ordering {
    timestamp_rank(left.timestamp)
        .cmp(&timestamp_rank(right.timestamp))
        .then_with(|| match (left.timestamp, right.timestamp) {
            (Some(left_time), Some(right_time)) => left_time.cmp(&right_time),
            _ => Ordering::Equal,
        })
        .then(left.source_index.cmp(&right.source_index))
        .then(left.line.cmp(&right.line))
}

fn timestamp_rank(timestamp: Option<DateTime<Utc>>) -> u8 {
    if timestamp.is_some() {
        0
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeutil::parse_timestamp;

    fn entry(index: usize, line: usize, timestamp: Option<&str>) -> Entry {
        Entry {
            source: format!("f{index}"),
            source_index: index,
            line,
            raw: String::new(),
            timestamp: timestamp.and_then(parse_timestamp),
            level: None,
            message: format!("{index}:{line}"),
            message_is_template: false,
            trace_id: None,
            correlation_id: None,
            exception: None,
            logger: None,
            original: Value::Null,
            kind: Kind::Text,
        }
    }

    #[test]
    fn merge_order_is_total_and_stable() {
        let mut entries = [
            entry(0, 2, None),
            entry(1, 1, Some("2026-11-14T10:00:00Z")),
            entry(0, 1, Some("2026-11-14T10:00:00Z")),
            entry(0, 3, Some("2026-11-14T09:00:00Z")),
            entry(1, 2, None),
        ];
        entries.sort_by(cmp_merge);
        let order: Vec<_> = entries
            .iter()
            .map(|entry| (entry.source_index, entry.line))
            .collect();
        assert_eq!(order, vec![(0, 3), (0, 1), (1, 1), (0, 2), (1, 2)]);
    }
}
