# Rustle

[![Rust](https://img.shields.io/badge/rust-stable%20%7C%20edition%202021-000000?style=flat&logo=rust)](https://www.rust-lang.org)
[![Build](https://github.com/kearns2000/rustle/actions/workflows/ci.yml/badge.svg)](https://github.com/kearns2000/rustle/actions/workflows/ci.yml)
[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20Linux-0078D4?style=flat)](https://github.com/kearns2000/rustle/actions/workflows/ci.yml)
[![Licence: MIT](https://img.shields.io/badge/Licence-MIT-yellow.svg)](LICENSE)
[![Tests](https://img.shields.io/badge/tests-cargo-000000?style=flat&logo=rust)](tests)

Find the signal in your logs.

Rustle is a small command-line tool for filtering large log files, following a trace or correlation id, merging several services into one timeline, and tailing logs as they grow. It reads newline-delimited JSON and plain text, including a mix of both in the same file.

Serilog compact JSON and the JSON shapes used by Microsoft.Extensions.Logging are recognised directly. Plain lines keep their original text.

## Install

Install a stable Rust toolchain, then build from this directory:

```powershell
cargo build --release
.\target\release\rustle.exe --help
```

Or install the binary onto your PATH:

```powershell
cargo install --path . --locked
rustle --help
```

The commands below also work in Unix shells. Quote glob patterns everywhere so the shell does not expand them before Rustle sees them. PowerShell does not expand globs for native programs; Rustle expands `*`, `?`, and `[...]` itself.

## Examples

```powershell
rustle api.log
rustle api.log --level error
rustle api.log --min-level warning
rustle ./logs --trace abc123
rustle "*.json" --merge
rustle api.log --follow
rustle api.log --search "connection failed"
rustle api.log --highlight "connection failed"
rustle api.log --since "2026-11-14T10:00:00Z"
rustle api.log --output json
Get-Content api.log | rustle -
```

Passing the file path keeps the original bytes. `Get-Content` decodes the file as text before the pipe, so prefer `rustle api.log` when the encoding matters. `-` reads standard input. With no inputs, Rustle reads standard input when it is redirected, and prints help when it is a terminal. `rustle -h` and `rustle --help` print that help with the ASCII logo at the top.

Sample logs live in `examples/`:

- `serilog.jsonl` — Serilog compact events, including a template-only event and an offset timestamp
- `microsoft.jsonl` — Microsoft.Extensions.Logging JSON, including nested `State` and `properties`
- `plain.log` — plain text with a timestamp, a bracketed level, a `LEVEL:` prefix, and an unstructured line

```powershell
rustle .\examples\serilog.jsonl --level error
rustle .\examples --trace abc123
```

## Inputs

Rustle accepts files, directories, and glob patterns, in the order you write them.

- Explicit file paths may use any extension and are read in the order given.
- A directory is scanned one level deep. `--recursive` (`-r`) includes subdirectories. Nested directory symlinks are skipped.
- Directory scans include `.log`, `.json`, and `.jsonl`, compared case-insensitively.
- An empty directory prints a warning and contributes no files.
- Glob matches are sorted by path. A glob that matches nothing is an error. A match that is a directory is scanned with the directory rules.
- The same canonical path is read once. The first path you wrote is the one shown in the output.
- `-` reads standard input and can be combined with files, except with `--follow`.

## Formats

Each line is parsed on its own. A line that trims to a JSON object is structured. Malformed JSON, JSON arrays, and every other line become plain-text entries. Reading continues after a bad line.

Blank lines are skipped. They still occupy a line number, so later numbers match the file.

### JSON fields

Values are read from the first location that has a non-empty string. Locations, in order:

1. Top-level object
2. `Properties`
3. `properties`
4. `State`, when it is an object

Inside a location, names are tried in the order below. Nulls, empty strings, and non-strings are skipped.

| Field | Names |
| --- | --- |
| Timestamp | `@t`, `Timestamp`, `timestamp`, `time` |
| Level | `@l`, `Level`, `level`, `LogLevel` |
| Message | `@m`, `Message`, `message` |
| Template fallback | `@mt`, `MessageTemplate` |
| Exception | `@x`, `Exception`, `exception` |
| Trace | `@tr`, `TraceId`, `traceId`, `trace_id` |
| Correlation | `CorrelationId`, `correlationId`, `correlation_id` |
| Logger | `SourceContext`, `Category`, `category` |

A rendered message wins over a template. When only `@mt` or `MessageTemplate` is present, that template is the message and `message_is_template` is true. Placeholders are left as written.

For a Serilog compact event, a top-level `@t` with no level string anywhere is treated as `info`. An unrecognised level string stays unrecognised.

The original JSON object is preserved on output, including properties that Rustle does not interpret.

### Timestamps

Timestamps must be RFC 3339 with `Z` or a numeric offset such as `+01:00`, and may include a fractional second. They are converted to UTC for comparisons and display. Extra fractional digits beyond nanoseconds are truncated. A value with no offset is left unparsed; Rustle does not guess a time zone.

### Plain text

Plain text keeps the complete original line for search and for the JSON `original` field.

A leading RFC 3339 timestamp is recognised. A level token is recognised only when it is:

- wrapped in brackets, such as `[ERROR]`
- followed by a colon, such as `INFO: started`
- the first token after a recognised timestamp, such as `2026-11-14T10:00:00Z ERROR failed`

`error handling is hard` stays unstructured. The displayed message is the line with a recognised timestamp and level prefix removed.

### Levels

| Normalised | Aliases | Text label |
| --- | --- | --- |
| `trace` | `trace`, `verbose` | `TRACE` |
| `debug` | `debug` | `DEBUG` |
| `info` | `info`, `information` | `INFO` |
| `warn` | `warn`, `warning` | `WARN` |
| `error` | `error` | `ERROR` |
| `fatal` | `fatal`, `critical` | `FATAL` |

Severity increases from `trace` to `fatal`.

## Filtering

Every active filter is combined with AND.

- `--level` matches one normalised level.
- `--min-level` matches that severity and anything more severe.
- `--search` is a case-insensitive literal search of the original line. It is not a regular expression.
- `--trace` matches a JSON trace id or correlation id exactly, including the nested fields above. On a plain-text line it is a case-sensitive substring of the original line.
- `--since` and `--until` are inclusive UTC bounds.

An entry with no recognised level does not match `--level` or `--min-level`. An entry with no valid timestamp does not match `--since` or `--until`.

## Output

The default text layout is:

```text
2026-11-14T09:00:01Z  ERROR  api.log  connection failed
  System.Net.Sockets.SocketException (10061): connection refused
```

The columns are the UTC timestamp, the level, the input path, and the message. Exception lines, and extra message lines, are indented.

Severity colours are applied to the level column when stdout is a terminal. `--highlight` keeps every line and marks each case-insensitive match as yellow text on a red background. `--no-color` disables both. A non-empty `NO_COLOR` environment variable disables them as well. Redirected stdout has no ANSI codes, so a highlight is not painted there. On Windows, Rustle switches the console to UTF-8 and enables virtual-terminal processing before it uses colour.

`--output json` writes one JSON object per line:

```json
{"source":"api.log","line":2,"timestamp":"2026-11-14T09:00:01Z","level":"error","message":"connection failed","message_is_template":false,"trace_id":"abc123","correlation_id":"corr-9","exception":"...","logger":null,"original":{}}
```

`source` is the input path (`-` for standard input). `logger` is the `SourceContext` or `Category`. `original` is the JSON object, or the original plain-text line. Missing values are `null`. Warnings and errors go to stderr.

A closed stdout pipe is a normal end. The process exits 0.

## Ordering

Without `--merge`, files are streamed in the resolved order with buffered reads. A whole file is not loaded into memory.

`--merge` reads every entry, sorts them, then prints. Entries with a timestamp come first, oldest to newest. Entries without a timestamp follow them. Equal timestamps keep the input order and then the line number. This mode holds the entries in memory. That is a deliberate limit of this version.

## Follow

`--follow` (`-f`) prints the lines already in the resolved files, then polls those same paths for appended data. The poll interval is 200ms. Filters and formatters are unchanged.

- A trailing partial line is held until a newline arrives.
- If the file shrinks, its starting bytes change, or it is replaced by a new file at the same path, reading starts over. Rename-and-recreate rotation is detected from the file identity.
- On Windows the read handle allows other processes to read, write, and delete the file, so rotation is not blocked while Rustle has it open.
- A path that disappears produces one stderr warning and is retried. A second warning is written when it can be read again.
- Ctrl+C stops the loop and exits 0.
- Lines are emitted in the order they are observed. Follow does not claim a global timestamp order.
- The file list is fixed at startup. A new file that appears later, including one that matches the original glob or directory, is not added.

`--follow` cannot be combined with `--merge` or with standard input.

## Encoding

Files and standard input are decoded as UTF-8. A leading UTF-8 BOM is removed. CRLF and a final line without a newline are accepted. Invalid UTF-8 bytes are replaced with U+FFFD and reading continues.

## Exit codes

| Code | When |
| --- | --- |
| 0 | Success, including no matching lines, an empty file, a closed pipe, or Ctrl+C during `--follow` |
| 1 | A requested path is missing, a glob matches nothing, or a file cannot be read |
| 2 | Invalid arguments, such as an unknown level, a timestamp without an offset, or `--follow` with `--merge` or stdin |

## Limitations

- `--merge` stores every parsed entry in memory before sorting.
- `--follow` does not discover files created after startup and does not globally sort new lines.
- Message templates are not rendered.
- Timestamps without an offset are not assigned a time zone.
- Plain-text levels are recognised only in the positions described above.
- `--search` and `--highlight` are literals. `--highlight` is not painted in JSON output or when colour is off. `--trace` does not scan arbitrary JSON properties beyond the fields listed above.
- Directory discovery does not include other extensions. Pass those files explicitly or with a glob.

## Development

From the repository root:

```text
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo build --release
```

GitHub Actions runs the same checks on Windows and Linux.

## Contributing

Contributions are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md) for setup, project layout, and pull request expectations. This project follows the [Code of Conduct](CODE_OF_CONDUCT.md). Security reports go through [SECURITY.md](SECURITY.md).

## Licence

MIT. See [LICENSE](LICENSE). Copyright (c) 2026 PKSoftLibs.
