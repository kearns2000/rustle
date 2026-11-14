use chrono::{DateTime, SecondsFormat, Utc};

/// Parse an RFC 3339 timestamp and convert it to UTC.
///
/// Fractional seconds may be longer than nine digits; extra digits are truncated.
/// Values without a numeric offset, including bare dates and local times, are
/// rejected so a time zone is never guessed.
pub fn parse_timestamp(value: &str) -> Option<DateTime<Utc>> {
    let trimmed = value.trim();
    // RFC 3339 separates the date and time with `T`. A space is not accepted,
    // so a local time is never inferred from a date-only token.
    let bytes = trimmed.as_bytes();
    if bytes.len() <= 10 || (bytes[10] != b'T' && bytes[10] != b't') {
        return None;
    }
    let normalised = normalise_fraction(trimmed);
    DateTime::parse_from_rfc3339(&normalised)
        .ok()
        .map(|timestamp| timestamp.with_timezone(&Utc))
}

pub fn format_timestamp(timestamp: DateTime<Utc>) -> String {
    trim_fraction_zeros(&timestamp.to_rfc3339_opts(SecondsFormat::Nanos, true))
}

fn trim_fraction_zeros(value: &str) -> String {
    let Some(dot) = value.find('.') else {
        return value.to_string();
    };
    let suffix_at = value[dot + 1..]
        .find(|ch: char| !ch.is_ascii_digit())
        .map(|index| dot + 1 + index)
        .unwrap_or(value.len());
    let digits = &value[dot + 1..suffix_at];
    let trimmed = digits.trim_end_matches('0');
    if trimmed.is_empty() {
        let mut out = String::with_capacity(value.len());
        out.push_str(&value[..dot]);
        out.push_str(&value[suffix_at..]);
        return out;
    }
    let mut out = String::with_capacity(value.len());
    out.push_str(&value[..=dot]);
    out.push_str(trimmed);
    out.push_str(&value[suffix_at..]);
    out
}

fn normalise_fraction(input: &str) -> String {
    let Some(dot) = input.find('.') else {
        return input.to_string();
    };
    let after = &input[dot + 1..];
    let digits = after.chars().take_while(char::is_ascii_digit).count();
    if digits <= 9 {
        return input.to_string();
    }
    let mut normalised = String::with_capacity(input.len() - (digits - 9));
    normalised.push_str(&input[..=dot]);
    normalised.push_str(&input[dot + 1..dot + 1 + 9]);
    normalised.push_str(&input[dot + 1 + digits..]);
    normalised
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_and_fractions_become_utc() {
        let utc = parse_timestamp("2026-11-14T10:00:00Z").unwrap();
        assert_eq!(format_timestamp(utc), "2026-11-14T10:00:00Z");

        let shifted = parse_timestamp("2026-11-14T10:00:01+01:00").unwrap();
        assert_eq!(format_timestamp(shifted), "2026-11-14T09:00:01Z");

        let behind = parse_timestamp("2026-11-14T10:00:00-05:00").unwrap();
        assert_eq!(format_timestamp(behind), "2026-11-14T15:00:00Z");

        let fraction = parse_timestamp("2026-11-14T10:00:00.1234567Z").unwrap();
        assert_eq!(format_timestamp(fraction), "2026-11-14T10:00:00.1234567Z");

        let truncated = parse_timestamp("2026-11-14T10:00:00.123456789999Z").unwrap();
        assert_eq!(
            format_timestamp(truncated),
            "2026-11-14T10:00:00.123456789Z"
        );
    }

    #[test]
    fn timestamps_without_an_offset_are_rejected() {
        assert!(parse_timestamp("2026-11-14T10:00:00").is_none());
        assert!(parse_timestamp("2026-11-14").is_none());
        assert!(parse_timestamp("2026-11-14 10:00:00Z").is_none());
        assert!(parse_timestamp("yesterday").is_none());
        assert!(parse_timestamp("").is_none());
    }
}
