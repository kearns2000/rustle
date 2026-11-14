use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let nonce = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "rustle-cli-{}-{}-{}",
            std::process::id(),
            nanos,
            nonce
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Captured {
    code: i32,
    stdout: String,
    stderr: String,
}

fn rustle(dir: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rustle"));
    command.current_dir(dir);
    command.env_remove("NO_COLOR");
    command
}

fn run(command: &mut Command) -> Captured {
    let output = command.output().unwrap();
    Captured {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

fn messages(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let value: Value = serde_json::from_str(line)
                .unwrap_or_else(|err| panic!("stdout was not JSONL ({err}): {line}"));
            value["message"].as_str().unwrap().to_string()
        })
        .collect()
}

fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&path, body).unwrap();
    path
}

#[test]
fn help_and_version_describe_the_tool() {
    let dir = TempDir::new();
    let help = run(rustle(dir.path()).arg("--help"));
    assert_eq!(help.code, 0);
    assert!(help.stdout.contains("(`-').->(`-')"));
    assert!(help.stdout.contains("Find the signal in your logs."));

    let short = run(rustle(dir.path()).arg("-h"));
    assert_eq!(short.code, 0);
    assert!(short.stdout.contains("(`-').->(`-')"));
    assert!(short.stdout.contains("Print help"));
    assert!(help.stdout.contains("no recognised level"));
    assert!(help.stdout.contains("no valid timestamp"));
    assert!(help.stdout.contains("Get-Content api.log | rustle -"));

    let version = run(rustle(dir.path()).arg("--version"));
    assert_eq!(version.code, 0);
    assert!(version.stdout.contains("rustle 0.1.0"));
}

#[test]
fn json_formats_nested_fields_and_default_serilog_level() {
    let dir = TempDir::new();
    let path = write(
        dir.path(),
        "api.jsonl",
        concat!(
            r#"{"@t":"2026-11-14T10:00:00Z","@mt":"User {UserId} logged in","UserId":"42","@tr":"abc123"}"#,
            "\n",
            r#"{"Timestamp":"2026-11-14T10:05:00Z","LogLevel":"Warning","Category":"App.Weather","Message":"cache miss","State":{"Message":"ignored","TraceId":"trace-state","CorrelationId":"corr-state"}}"#,
            "\n",
            r#"{"timestamp":"2026-11-14T10:06:00Z","level":"debug","message":"loaded","properties":{"trace_id":"prop-trace"}}"#,
            "\n",
        ),
    );
    let output = run(rustle(dir.path()).arg(&path).args(["--output", "json"]));
    assert_eq!(output.code, 0, "{}", output.stderr);
    assert!(!output.stdout.contains('\u{1b}'));
    let lines: Vec<Value> = output
        .stdout
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines[0]["level"], "info");
    assert_eq!(lines[0]["message"], "User {UserId} logged in");
    assert_eq!(lines[0]["message_is_template"], true);
    assert_eq!(lines[0]["original"]["UserId"], "42");
    assert_eq!(lines[0]["trace_id"], "abc123");
    assert_eq!(lines[1]["level"], "warn");
    assert_eq!(lines[1]["message"], "cache miss");
    assert_eq!(lines[1]["trace_id"], "trace-state");
    assert_eq!(lines[1]["correlation_id"], "corr-state");
    assert_eq!(lines[1]["logger"], "App.Weather");
    assert_eq!(lines[2]["level"], "debug");
    assert_eq!(lines[2]["trace_id"], "prop-trace");
    assert!(output.stderr.is_empty());
}

#[test]
fn malformed_json_bom_crlf_and_a_missing_final_newline() {
    let dir = TempDir::new();
    let mut body = vec![0xEF, 0xBB, 0xBF];
    body.extend_from_slice(br#"{"@t":"2026-11-14T10:00:00Z","@m":"first"}"#);
    body.extend_from_slice(b"\r\n{not-json}\r\n");
    body.extend_from_slice(br#"{"@t":"2026-11-14T10:00:01Z","@m":"second"}"#);
    let path = dir.path().join("mixed.log");
    fs::write(&path, body).unwrap();

    let output = run(rustle(dir.path()).arg(&path).args(["--output", "json"]));
    assert_eq!(output.code, 0, "{}", output.stderr);
    assert_eq!(
        messages(&output.stdout),
        vec!["first", "{not-json}", "second"]
    );
    let lines: Vec<Value> = output
        .stdout
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines[0]["level"], "info");
    assert_eq!(lines[0]["line"], 1);
    assert_eq!(lines[1]["level"], Value::Null);
    assert_eq!(lines[1]["line"], 2);
    assert_eq!(lines[2]["line"], 3);

    let lossy = dir.path().join("lossy.log");
    fs::write(&lossy, b"2026-11-14T10:00:00Z INFO caf\xffe\n").unwrap();
    let output = run(rustle(dir.path()).arg(&lossy));
    assert_eq!(output.code, 0, "{}", output.stderr);
    assert!(output.stdout.contains("caf\u{FFFD}e"));
    assert!(!output.stdout.contains('\u{1b}'));
}

#[test]
fn filters_combine_and_trace_matching_is_exact_for_json() {
    let dir = TempDir::new();
    let path = write(
        dir.path(),
        "api.log",
        concat!(
            "2026-11-14T10:00:00Z ERROR alpha trace=abc123\n",
            "2026-11-14T10:00:00Z INFO alpha trace=abc123\n",
            "2026-11-14T09:00:00Z ERROR alpha trace=abc123\n",
            "2026-11-14T10:00:00Z ERROR beta trace=zzz\n",
            "this prose mentions error and abc123\n",
            r#"{"@t":"2026-11-14T10:00:00Z","@m":"mentions abc123","@l":"Error"}"#,
            "\n",
            r#"{"@t":"2026-11-14T10:00:01Z","@m":"exact","@l":"Error","@tr":"abc123"}"#,
            "\n",
            r#"{"@t":"2026-11-14T10:00:02Z","@m":"prefix","@l":"Error","@tr":"abc123-extra","CorrelationId":"other"}"#,
            "\n",
            r#"{"@t":"2026-11-14T10:00:03Z","@m":"corr","@l":"Warning","CorrelationId":"abc123"}"#,
            "\n",
        ),
    );

    let level = run(rustle(dir.path())
        .args(["--level", "warning", "--output", "json"])
        .arg(&path));
    assert_eq!(level.code, 0, "{}", level.stderr);
    assert_eq!(messages(&level.stdout), vec!["corr"]);

    let minimum = run(rustle(dir.path())
        .args(["--min-level", "trace", "--output", "json"])
        .arg(&path));
    assert!(!messages(&minimum.stdout)
        .iter()
        .any(|message| message.contains("prose")));

    let combined = run(rustle(dir.path())
        .args([
            "--level",
            "error",
            "--search",
            "ALPHA",
            "--trace",
            "abc123",
            "--since",
            "2026-11-14T10:00:00Z",
            "--output",
            "json",
        ])
        .arg(&path));
    assert_eq!(combined.code, 0, "{}", combined.stderr);
    assert_eq!(messages(&combined.stdout), vec!["alpha trace=abc123"]);

    let trace = run(rustle(dir.path())
        .args(["--trace", "abc123", "--output", "json"])
        .arg(&path));
    let traced = messages(&trace.stdout);
    assert!(traced.iter().any(|message| message.starts_with("alpha")));
    assert!(traced.iter().any(|message| message == "exact"));
    assert!(traced.iter().any(|message| message == "corr"));
    assert!(!traced.iter().any(|message| message == "mentions abc123"));
    assert!(!traced.iter().any(|message| message == "prefix"));
    assert!(traced.iter().any(|message| message.contains("prose")));
}

#[test]
fn timestamp_offsets_are_normalised_and_missing_times_drop_out() {
    let dir = TempDir::new();
    let path = write(
        dir.path(),
        "times.log",
        concat!(
            "2026-11-14T12:00:00+02:00 INFO shifted\n",
            "2026-11-14T10:00:00Z INFO exact\n",
            "2026-11-14T07:59:59Z INFO before\n",
            "INFO: no clock\n",
            "2026-11-14T10:00:00 INFO local-without-offset\n",
        ),
    );
    let output = run(rustle(dir.path())
        .args([
            "--since",
            "2026-11-14T08:00:00Z",
            "--until",
            "2026-11-14T10:00:00Z",
            "--output",
            "json",
        ])
        .arg(&path));
    assert_eq!(output.code, 0, "{}", output.stderr);
    assert_eq!(messages(&output.stdout), vec!["shifted", "exact"]);
    let lines: Vec<Value> = output
        .stdout
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines[0]["timestamp"], "2026-11-14T10:00:00Z");
}

#[test]
fn merge_order_is_stable_and_streaming_keeps_input_order() {
    let dir = TempDir::new();
    let first = write(
        dir.path(),
        "a.log",
        "2026-11-14T10:00:00Z INFO a-late\nuntimed a\n2026-11-14T09:00:00Z INFO a-early\n",
    );
    let second = write(
        dir.path(),
        "b.log",
        "2026-11-14T10:00:00Z INFO b-same\n2026-11-14T08:00:00Z INFO b-early\nuntimed b\n",
    );
    let streamed = run(rustle(dir.path())
        .args(["--output", "json"])
        .arg(&first)
        .arg(&second));
    assert_eq!(
        messages(&streamed.stdout),
        vec![
            "a-late",
            "untimed a",
            "a-early",
            "b-same",
            "b-early",
            "untimed b"
        ]
    );

    let merged = run(rustle(dir.path())
        .args(["--merge", "--output", "json"])
        .arg(&second)
        .arg(&first));
    assert_eq!(
        messages(&merged.stdout),
        vec![
            "b-early",
            "a-early",
            "b-same",
            "a-late",
            "untimed b",
            "untimed a"
        ]
    );
}

#[test]
fn globs_duplicates_and_directories_follow_the_discovery_rules() {
    let dir = TempDir::new();
    write(dir.path(), "b.log", "2026-11-14T10:00:00Z INFO from-b\n");
    write(dir.path(), "a.log", "2026-11-14T10:00:00Z INFO from-a\n");
    write(
        dir.path(),
        "note.txt",
        "2026-11-14T10:00:00Z INFO from-txt\n",
    );
    write(
        dir.path(),
        "logs/nested/c.json",
        "2026-11-14T10:00:00Z INFO from-c\n",
    );
    write(
        dir.path(),
        "logs/top.log",
        "2026-11-14T10:00:00Z INFO from-top\n",
    );
    write(
        dir.path(),
        "logs/skip.txt",
        "2026-11-14T10:00:00Z INFO from-skip\n",
    );

    let listed = run(rustle(dir.path()).args(["--output", "json", "b.log", "a.log", "b.log"]));
    assert_eq!(listed.code, 0, "{}", listed.stderr);
    assert_eq!(messages(&listed.stdout), vec!["from-b", "from-a"]);

    let globbed = run(rustle(dir.path()).args(["--output", "json", "a.log", "*.log"]));
    assert_eq!(globbed.code, 0, "{}", globbed.stderr);
    assert_eq!(messages(&globbed.stdout), vec!["from-a", "from-b"]);

    let directory = run(rustle(dir.path()).args(["--output", "json", "logs"]));
    assert_eq!(messages(&directory.stdout), vec!["from-top"]);

    let recursive = run(rustle(dir.path()).args(["--recursive", "--output", "json", "logs"]));
    assert_eq!(messages(&recursive.stdout), vec!["from-c", "from-top"]);

    let explicit = run(rustle(dir.path()).args(["--output", "json", "logs/skip.txt"]));
    assert_eq!(messages(&explicit.stdout), vec!["from-skip"]);

    let missing_glob = run(rustle(dir.path()).arg("*.nope"));
    assert_eq!(missing_glob.code, 1);
    assert!(missing_glob.stderr.contains("no files matched"));
    assert!(missing_glob.stdout.is_empty());
}

#[test]
fn stdin_dash_and_redirected_output() {
    let dir = TempDir::new();
    let mut piped = rustle(dir.path());
    piped
        .arg("-")
        .args(["--output", "json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = piped.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"INFO: from stdin\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(0));
    assert!(!stdout.contains('\u{1b}'));
    let value: Value = serde_json::from_str(stdout.lines().next().unwrap()).unwrap();
    assert_eq!(value["source"], "-");
    assert_eq!(value["message"], "from stdin");
    assert_eq!(value["level"], "info");

    let mut implicit = rustle(dir.path());
    implicit
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = implicit.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"INFO: implicit\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert!(stdout.contains("implicit"));
    assert!(!stdout.contains('\u{1b}'));
}

#[test]
fn invalid_arguments_and_missing_files_use_distinct_exit_codes() {
    let dir = TempDir::new();
    let path = write(dir.path(), "api.log", "INFO: hello\n");

    let missing = run(rustle(dir.path()).arg("missing.log"));
    assert_eq!(missing.code, 1);
    assert!(missing.stderr.contains("no such file or directory"));
    assert!(missing.stdout.is_empty());

    let follow_merge = run(rustle(dir.path()).args(["--follow", "--merge"]).arg(&path));
    assert_eq!(follow_merge.code, 2);
    assert!(follow_merge.stderr.contains("cannot be combined"));

    let follow_stdin = run(rustle(dir.path()).args(["--follow", "-"]));
    assert_eq!(follow_stdin.code, 2);
    assert!(follow_stdin.stderr.contains("cannot read from stdin"));

    let mut piped_follow = rustle(dir.path());
    piped_follow.arg("--follow").stdin(Stdio::null());
    let piped_follow = run(&mut piped_follow);
    assert_eq!(piped_follow.code, 2);
    assert!(piped_follow.stderr.contains("cannot read from stdin"));

    let level = run(rustle(dir.path()).args(["--level", "nope"]).arg(&path));
    assert_eq!(level.code, 2);
    assert!(level.stderr.contains("unknown level"));

    let since = run(rustle(dir.path())
        .args(["--since", "2026-11-14T10:00:00"])
        .arg(&path));
    assert_eq!(since.code, 2);
    assert!(since.stderr.contains("numeric offset"));

    let unknown = run(rustle(dir.path()).arg("--not-a-flag"));
    assert_eq!(unknown.code, 2);

    let empty = dir.path().join("empty.log");
    fs::write(&empty, "").unwrap();
    let empty_output = run(rustle(dir.path()).arg(&empty));
    assert_eq!(empty_output.code, 0);
    assert!(empty_output.stdout.is_empty());

    let empty_dir = dir.path().join("empty-dir");
    fs::create_dir(&empty_dir).unwrap();
    let warned = run(rustle(dir.path()).arg(&empty_dir));
    assert_eq!(warned.code, 0);
    assert!(warned.stderr.contains("warning:"));
    assert!(warned.stdout.is_empty());
}

#[test]
fn broken_pipe_exits_successfully() {
    let dir = TempDir::new();
    write(dir.path(), "api.log", "2026-11-14T10:00:00Z INFO hello\n");
    let mut child = rustle(dir.path())
        .arg("api.log")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    let output = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    assert!(!stderr.contains("error:"));
}

#[test]
fn search_is_a_case_insensitive_literal() {
    let dir = TempDir::new();
    let path = write(
        dir.path(),
        "api.log",
        "2026-11-14T10:00:00Z INFO Connection Failed.\n2026-11-14T10:00:00Z INFO other\n",
    );
    let matched = run(rustle(dir.path())
        .args(["--search", "connection failed", "--output", "json"])
        .arg(&path));
    assert_eq!(messages(&matched.stdout), vec!["Connection Failed."]);

    let literal = run(rustle(dir.path())
        .args(["--search", ".*", "--output", "json"])
        .arg(&path));
    assert!(messages(&literal.stdout).is_empty());
}

#[test]
fn highlight_keeps_lines_that_do_not_contain_the_text() {
    let dir = TempDir::new();
    let path = write(
        dir.path(),
        "api.log",
        "2026-11-14T10:00:00Z INFO alpha stays\n2026-11-14T10:00:01Z INFO beta stays\n",
    );
    let highlighted = run(rustle(dir.path())
        .arg("--highlight")
        .arg("alpha")
        .arg(&path));
    assert_eq!(highlighted.code, 0, "{}", highlighted.stderr);
    assert!(highlighted.stdout.contains("alpha stays"));
    assert!(highlighted.stdout.contains("beta stays"));
    assert!(!highlighted.stdout.contains('\u{1b}'));

    let empty = run(rustle(dir.path()).args(["--highlight", ""]).arg(&path));
    assert_eq!(empty.code, 2);
    assert!(empty.stderr.contains("requires text"));
}

#[test]
fn bundled_examples_filter_from_the_cli() {
    let examples = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples");
    let output = run(Command::new(env!("CARGO_BIN_EXE_rustle"))
        .arg(examples.join("serilog.jsonl"))
        .args(["--level", "error", "--output", "json"]));
    assert_eq!(output.code, 0, "{}", output.stderr);
    assert_eq!(messages(&output.stdout), vec!["connection failed"]);

    let plain = run(Command::new(env!("CARGO_BIN_EXE_rustle"))
        .arg(examples.join("plain.log"))
        .args(["--trace", "abc123"]));
    assert_eq!(plain.code, 0, "{}", plain.stderr);
    assert!(plain.stdout.contains("connection failed"));
    assert!(!plain.stdout.contains("server listening"));
}
