# Contributing to Rustle

Thanks for your interest in contributing. Rustle is a small command-line tool for filtering, merging, and tailing logs. Contributions that keep that focus are welcome.

## Before you start

- Search [existing issues](https://github.com/kearns2000/rustle/issues) to avoid duplicate work.
- For large changes (new log formats, output modes, architecture), open an issue first to discuss approach.
- Keep pull requests focused. One feature or fix per PR is easier to review.

## Prerequisites

- A stable [Rust](https://rustup.rs/) toolchain, including rustfmt and Clippy
- Any editor

## Getting started

```bash
git clone https://github.com/kearns2000/rustle.git
cd rustle
cargo test
```

## Project layout

```text
src/                 # CLI library
  cli.rs             # Arguments
  discover.rs        # Files, directories, and globs
  parse.rs           # JSON and plain-text lines
  filter.rs          # Combined filters
  render.rs          # Text and JSON output
  follow.rs          # Live tail
tests/cli.rs         # Command-line integration tests
examples/            # Sample logs
.github/workflows/   # Windows and Linux CI
```

## Making changes

- Match existing naming and module structure.
- Use UK English in comments and documentation.
- Do not use em dashes in comments or documentation.
- Add or update tests for behaviour changes.
- Keep the tool a single binary. Avoid async runtimes and plugin systems.

## Running checks

From the repository root:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

## Pull requests

1. Fork the repository and create a branch from `main`.
2. Make your changes with tests.
3. Ensure the checks above pass.
4. Open a pull request with a clear description of the change and why it helps.

Please read the [Code of Conduct](CODE_OF_CONDUCT.md) before participating.

## Licence

By contributing, you agree that your contributions will be licensed under the MIT Licence.

## Questions

Open a [GitHub issue](https://github.com/kearns2000/rustle/issues) for questions or ideas.
