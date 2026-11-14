use std::io::{BufRead, IsTerminal, Write};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use clap::{CommandFactory, Parser};

use crate::cli::{prepare, Cli, Settings};
use crate::console;
use crate::decode::for_each_line;
use crate::discover::{self, Resolved, Source};
use crate::entry::{cmp_merge, Entry};
use crate::error::{Error, Result};
use crate::filter::Filter;
use crate::follow::{open_shared, Tailer, DEFAULT_POLL_INTERVAL};
use crate::parse::parse_line;
use crate::render::Renderer;

#[must_use]
pub fn run() -> ExitCode {
    console::ignore_sigpipe();
    let cli = match Cli::try_parse_from(std::env::args_os()) {
        Ok(cli) => cli,
        Err(err) => return clap_status(err),
    };
    let stdin_is_terminal = std::io::stdin().is_terminal();
    let stdout_is_terminal = std::io::stdout().is_terminal();
    let settings = match prepare(&cli, stdin_is_terminal) {
        Ok(Some(settings)) => settings,
        Ok(None) => {
            if let Err(err) = Cli::command().print_long_help() {
                if err.kind() == std::io::ErrorKind::BrokenPipe {
                    return ExitCode::SUCCESS;
                }
                eprintln!("error: {err}");
                return ExitCode::from(1);
            }
            println!();
            return ExitCode::SUCCESS;
        }
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::from(err.code());
        }
    };
    let color = console::prepare_ansi(console::color_enabled(
        settings.no_color,
        std::env::var_os("NO_COLOR").as_deref(),
        stdout_is_terminal,
    ));
    let stop = Arc::new(AtomicBool::new(false));
    if settings.follow {
        let flag = Arc::clone(&stop);
        if let Err(err) = ctrlc::set_handler(move || {
            flag.store(true, Ordering::SeqCst);
        }) {
            eprintln!("error: cannot install Ctrl+C handler: {err}");
            return ExitCode::from(1);
        }
    }

    let stdin = std::io::stdin();
    let mut stdin = stdin.lock();
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();
    let stderr = std::io::stderr();
    let mut stderr = stderr.lock();
    match execute(
        &settings,
        &mut stdin,
        &mut stdout,
        &mut stderr,
        color,
        &stop,
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(Error::BrokenPipe) => ExitCode::SUCCESS,
        Err(err) => {
            let _ = writeln!(stderr, "error: {err}");
            ExitCode::from(err.code())
        }
    }
}

fn clap_status(err: clap::Error) -> ExitCode {
    let code = u8::try_from(err.exit_code()).unwrap_or(2);
    match err.print() {
        Ok(()) => ExitCode::from(code),
        Err(print_err) if print_err.kind() == std::io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(print_err) => {
            eprintln!("error: {print_err}");
            ExitCode::from(1)
        }
    }
}

fn execute(
    settings: &Settings,
    stdin: &mut dyn BufRead,
    out: &mut dyn Write,
    err: &mut dyn Write,
    color: bool,
    stop: &AtomicBool,
) -> Result<()> {
    let resolved = discover::resolve(&settings.inputs, settings.recursive, err)?;
    let renderer = Renderer::new(settings.output, color, settings.highlight.as_deref());
    if settings.follow {
        if !resolved
            .iter()
            .any(|item| matches!(item.source, Source::File(_)))
        {
            return Err(Error::runtime("--follow requires at least one file"));
        }
        drive_follow(
            &resolved,
            &settings.filter,
            &renderer,
            &mut Outputs { out, err },
            stop,
            DEFAULT_POLL_INTERVAL,
            || {},
        )?;
    } else if settings.merge {
        merge_all(&resolved, &settings.filter, &renderer, stdin, out)?;
    } else {
        stream_all(&resolved, &settings.filter, &renderer, stdin, out)?;
    }
    out.flush()?;
    Ok(())
}

fn stream_all(
    inputs: &[Resolved],
    filter: &Filter,
    renderer: &Renderer,
    stdin: &mut dyn BufRead,
    out: &mut dyn Write,
) -> Result<()> {
    for input in inputs {
        read_source(input, stdin, |entry| {
            if filter.matches(&entry) {
                renderer.write(&entry, out)?;
            }
            Ok(())
        })?;
    }
    Ok(())
}

fn merge_all(
    inputs: &[Resolved],
    filter: &Filter,
    renderer: &Renderer,
    stdin: &mut dyn BufRead,
    out: &mut dyn Write,
) -> Result<()> {
    let mut entries = Vec::new();
    for input in inputs {
        read_source(input, stdin, |entry| {
            entries.push(entry);
            Ok(())
        })?;
    }
    entries.sort_by(cmp_merge);
    for entry in &entries {
        if filter.matches(entry) {
            renderer.write(entry, out)?;
        }
    }
    Ok(())
}

fn read_source(
    input: &Resolved,
    stdin: &mut dyn BufRead,
    mut on_entry: impl FnMut(Entry) -> Result<()>,
) -> Result<()> {
    match &input.source {
        Source::Stdin => for_each_line(stdin, |line| {
            on_entry(parse_line(
                &input.display,
                input.index,
                line.number,
                &line.text,
            ))
        }),
        Source::File(path) => {
            let file = open_shared(path)
                .map_err(|err| Error::runtime(format!("cannot read {}: {err}", input.display)))?;
            for_each_line(file, |line| {
                on_entry(parse_line(
                    &input.display,
                    input.index,
                    line.number,
                    &line.text,
                ))
            })
        }
    }
}

struct Outputs<'a> {
    out: &'a mut dyn Write,
    err: &'a mut dyn Write,
}

fn drive_follow<H>(
    inputs: &[Resolved],
    filter: &Filter,
    renderer: &Renderer,
    outputs: &mut Outputs<'_>,
    stop: &AtomicBool,
    interval: Duration,
    hook: H,
) -> Result<()>
where
    H: FnMut(),
{
    let files = inputs.iter().filter_map(|input| match &input.source {
        Source::File(path) => Some((input.index, input.display.clone(), path.clone())),
        Source::Stdin => None,
    });
    let mut tailer = Tailer::new(files);
    tailer.run(
        outputs.err,
        interval,
        stop,
        |line| {
            let entry = parse_line(&line.display, line.index, line.line_no, &line.text);
            if filter.matches(&entry) {
                renderer.write(&entry, outputs.out)?;
                outputs.out.flush()?;
            }
            Ok(())
        },
        hook,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::prepare;
    use crate::test_support::TempDir;
    use clap::Parser;
    use std::fs;
    use std::io::{self, Read};
    use std::time::Instant;

    struct BrokenPipe;

    impl Write for BrokenPipe {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "broken pipe"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "broken pipe"))
        }
    }

    #[test]
    fn a_closed_stdout_is_successful_completion() {
        let dir = TempDir::new();
        let path = dir.path().join("api.log");
        fs::write(&path, "2026-11-14T10:00:00Z INFO hello\n").unwrap();
        let cli = Cli::try_parse_from(["rustle", path.to_str().unwrap()]).unwrap();
        let settings = prepare(&cli, true).unwrap().unwrap();
        let err = execute(
            &settings,
            &mut io::empty(),
            &mut BrokenPipe,
            &mut Vec::new(),
            false,
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert!(matches!(err, Error::BrokenPipe));
        assert_eq!(err.code(), 0);
    }

    #[test]
    fn follow_applies_filters_and_stops_without_sleeping() {
        let dir = TempDir::new();
        let path = dir.path().join("api.log");
        fs::write(
            &path,
            "2026-11-14T10:00:00Z INFO skip\n2026-11-14T10:00:01Z ERROR keep\n",
        )
        .unwrap();
        let cli = Cli::try_parse_from([
            "rustle",
            "--follow",
            "--min-level",
            "error",
            path.to_str().unwrap(),
        ])
        .unwrap();
        let settings = prepare(&cli, true).unwrap().unwrap();
        let mut err = Vec::new();
        let resolved = discover::resolve(&settings.inputs, false, &mut err).unwrap();
        let renderer = Renderer::new(settings.output, false, settings.highlight.as_deref());
        let stop = AtomicBool::new(false);
        let mut out = Vec::new();
        let started = Instant::now();
        drive_follow(
            &resolved,
            &settings.filter,
            &renderer,
            &mut Outputs {
                out: &mut out,
                err: &mut err,
            },
            &stop,
            Duration::from_secs(30),
            || stop.store(true, Ordering::Relaxed),
        )
        .unwrap();
        assert!(started.elapsed() < Duration::from_millis(500));
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("keep"));
        assert!(!text.contains("skip"));
    }

    #[test]
    fn merge_orders_entries_across_files() {
        let dir = TempDir::new();
        let first = dir.path().join("a.log");
        let second = dir.path().join("b.log");
        fs::write(
            &first,
            "2026-11-14T10:00:00Z INFO a-late\nuntimed a\n2026-11-14T09:00:00Z INFO a-early\n",
        )
        .unwrap();
        fs::write(
            &second,
            "2026-11-14T10:00:00Z INFO b-same\n2026-11-14T08:00:00Z INFO b-early\nuntimed b\n",
        )
        .unwrap();
        let cli = Cli::try_parse_from([
            "rustle",
            "--merge",
            "--output",
            "json",
            first.to_str().unwrap(),
            second.to_str().unwrap(),
        ])
        .unwrap();
        let settings = prepare(&cli, true).unwrap().unwrap();
        let mut out = Vec::new();
        execute(
            &settings,
            &mut io::empty(),
            &mut out,
            &mut Vec::new(),
            false,
            &AtomicBool::new(false),
        )
        .unwrap();
        let messages = json_messages(&out);
        assert_eq!(
            messages,
            vec![
                "b-early",
                "a-early",
                "a-late",
                "b-same",
                "untimed a",
                "untimed b"
            ]
        );
    }

    fn json_messages(bytes: &[u8]) -> Vec<String> {
        String::from_utf8(bytes.to_vec())
            .unwrap()
            .lines()
            .map(|line| {
                let value: serde_json::Value = serde_json::from_str(line).unwrap();
                value["message"].as_str().unwrap().to_string()
            })
            .collect()
    }

    #[test]
    fn stream_reads_stdin_when_requested() {
        let cli = Cli::try_parse_from(["rustle", "-"]).unwrap();
        let settings = prepare(&cli, true).unwrap().unwrap();
        let mut input = io::Cursor::new(b"INFO: from stdin\n");
        let mut out = Vec::new();
        execute(
            &settings,
            &mut input,
            &mut out,
            &mut Vec::new(),
            false,
            &AtomicBool::new(false),
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("from stdin"));
        assert!(text.contains("-"));
        assert_eq!(input.read(&mut [0]).unwrap(), 0);
    }
}
