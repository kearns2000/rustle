use clap::{Parser, ValueEnum};

use crate::error::{Error, Result};
use crate::filter::Filter;
use crate::level::{Level, LEVEL_ALIASES};
use crate::timeutil::parse_timestamp;

const LOGO: &str = include_str!("logo.txt");

const AFTER_HELP: &str = "\
Notes:
  All active filters are combined with AND.
  Entries with no recognised level do not match --level or --min-level.
  Entries with no valid timestamp do not match --since or --until.
  Timestamps are compared in UTC. Values without a numeric offset are not parsed.
  --search is a case-insensitive literal. --trace is an exact id match for JSON
  and a literal substring for plain text.
  --highlight keeps every line and marks a case-insensitive literal in text
  output. Matches are yellow text on a red background when colour is enabled.
  --merge loads every entry into memory, then sorts by timestamp.
  Entries with no timestamp sort after timestamped entries.
  --follow prints the current contents, then polls for appends in observation order.
  It watches only the files resolved at startup.
  Invalid UTF-8 is replaced with U+FFFD and reading continues.

Examples:
  rustle api.log --level error
  rustle ./logs --trace abc123
  rustle \"*.json\" --merge
  rustle api.log --follow
  rustle api.log --highlight \"connection failed\"
  Get-Content api.log | rustle -

Level aliases:
  trace, verbose, debug, info, information, warn, warning, error, fatal, critical";

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Default)]
pub enum OutputFormat {
    #[default]
    Text,
    Json,
}

#[derive(Debug, Parser)]
#[command(
    name = "rustle",
    version,
    about = "Find the signal in your logs.",
    long_about = "Find the signal in your logs.\n\nRustle filters log files, follows correlation ids, merges multi-service logs into a timeline, and tails live logs.",
    before_help = LOGO,
    after_help = AFTER_HELP
)]
pub struct Cli {
    /// Log files, directories, or glob patterns. `-` reads standard input.
    #[arg(value_name = "INPUT")]
    pub inputs: Vec<String>,

    /// Exact normalised level.
    #[arg(long, value_name = "LEVEL")]
    pub level: Option<String>,

    /// Inclusive minimum severity.
    #[arg(long, value_name = "LEVEL")]
    pub min_level: Option<String>,

    /// Match a trace id or correlation id.
    #[arg(long, value_name = "ID")]
    pub trace: Option<String>,

    /// Case-insensitive literal search over the original line.
    #[arg(long, value_name = "TEXT")]
    pub search: Option<String>,

    /// Mark a case-insensitive literal in text output without hiding other lines.
    #[arg(long, value_name = "TEXT")]
    pub highlight: Option<String>,

    /// Inclusive lower timestamp bound, as RFC 3339 with an offset.
    #[arg(long, value_name = "TIMESTAMP")]
    pub since: Option<String>,

    /// Inclusive upper timestamp bound, as RFC 3339 with an offset.
    #[arg(long, value_name = "TIMESTAMP")]
    pub until: Option<String>,

    /// Buffer every entry, then sort them into one timeline.
    #[arg(long)]
    pub merge: bool,

    /// Print the current entries, then watch the resolved files for new lines.
    #[arg(long, short = 'f')]
    pub follow: bool,

    /// Include subdirectories when scanning a directory.
    #[arg(long, short = 'r')]
    pub recursive: bool,

    /// Output format: text or json.
    #[arg(long, value_enum, default_value = "text")]
    pub output: OutputFormat,

    /// Disable severity colours.
    #[arg(long)]
    pub no_color: bool,
}

#[derive(Debug, Clone)]
pub struct Settings {
    pub inputs: Vec<String>,
    pub filter: Filter,
    pub merge: bool,
    pub follow: bool,
    pub recursive: bool,
    pub output: OutputFormat,
    pub no_color: bool,
    pub highlight: Option<String>,
}

pub fn prepare(cli: &Cli, stdin_is_terminal: bool) -> Result<Option<Settings>> {
    if cli.follow && cli.merge {
        return Err(Error::usage("--follow cannot be combined with --merge"));
    }
    let level = parse_level_arg("--level", cli.level.as_deref())?;
    let min_level = parse_level_arg("--min-level", cli.min_level.as_deref())?;
    let since = parse_time_arg("--since", cli.since.as_deref())?;
    let until = parse_time_arg("--until", cli.until.as_deref())?;
    if let (Some(since), Some(until)) = (since, until) {
        if since > until {
            return Err(Error::usage("--since is later than --until"));
        }
    }
    if cli.highlight.as_ref().is_some_and(|value| value.is_empty()) {
        return Err(Error::usage("--highlight requires text to mark"));
    }
    let mut inputs = cli.inputs.clone();
    if inputs.is_empty() {
        if stdin_is_terminal {
            return Ok(None);
        }
        inputs.push("-".to_string());
    }
    if cli.follow && inputs.iter().any(|input| input == "-") {
        return Err(Error::usage("--follow cannot read from stdin"));
    }
    Ok(Some(Settings {
        inputs,
        filter: Filter {
            level,
            min_level,
            search: cli.search.as_ref().map(|value| value.to_lowercase()),
            trace: cli.trace.clone(),
            since,
            until,
        },
        merge: cli.merge,
        follow: cli.follow,
        recursive: cli.recursive,
        output: cli.output,
        no_color: cli.no_color,
        highlight: cli.highlight.clone(),
    }))
}

fn parse_level_arg(flag: &str, value: Option<&str>) -> Result<Option<Level>> {
    let Some(value) = value else {
        return Ok(None);
    };
    Level::parse(value).map(Some).ok_or_else(|| {
        Error::usage(format!(
            "unknown level '{value}' for {flag}; expected one of: {LEVEL_ALIASES}"
        ))
    })
}

fn parse_time_arg(
    flag: &str,
    value: Option<&str>,
) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
    let Some(value) = value else {
        return Ok(None);
    };
    parse_timestamp(value).map(Some).ok_or_else(|| {
        Error::usage(format!(
            "invalid RFC 3339 timestamp for {flag}: '{value}'. Include a numeric offset such as Z or +00:00; time zones are not guessed"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(args).unwrap()
    }

    #[test]
    fn hyphen_is_an_input_and_flags_validate() {
        let cli = parse(&["rustle", "-", "api.log", "--level", "Warning"]);
        assert_eq!(cli.inputs, vec!["-", "api.log"]);
        let settings = prepare(&cli, true).unwrap().unwrap();
        assert_eq!(settings.filter.level, Some(Level::Warn));

        let help = parse(&["rustle"]);
        assert!(prepare(&help, true).unwrap().is_none());
        let piped = prepare(&help, false).unwrap().unwrap();
        assert_eq!(piped.inputs, vec!["-"]);
    }

    #[test]
    fn incompatible_and_invalid_arguments_fail() {
        let follow_merge = parse(&["rustle", "--follow", "--merge", "api.log"]);
        let err = prepare(&follow_merge, true).unwrap_err();
        assert_eq!(err.code(), 2);
        assert!(err.to_string().contains("cannot be combined"));

        let follow_stdin = parse(&["rustle", "--follow"]);
        let err = prepare(&follow_stdin, false).unwrap_err();
        assert!(err.to_string().contains("cannot read from stdin"));

        let level = parse(&["rustle", "--level", "nope", "api.log"]);
        assert!(prepare(&level, true)
            .unwrap_err()
            .to_string()
            .contains("unknown level"));

        let since = parse(&["rustle", "--since", "2026-11-14T10:00:00", "api.log"]);
        assert!(prepare(&since, true)
            .unwrap_err()
            .to_string()
            .contains("numeric offset"));

        let order = parse(&[
            "rustle",
            "--since",
            "2026-11-14T12:00:00Z",
            "--until",
            "2026-11-14T11:00:00Z",
            "api.log",
        ]);
        assert!(prepare(&order, true)
            .unwrap_err()
            .to_string()
            .contains("later than"));

        let highlight = parse(&["rustle", "--highlight", "", "api.log"]);
        assert!(prepare(&highlight, true)
            .unwrap_err()
            .to_string()
            .contains("requires text"));
    }
}
