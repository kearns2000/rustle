use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub index: usize,
    pub display: String,
    pub source: Source,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Stdin,
    File(PathBuf),
}

/// Resolve files, directories, and glob patterns.
///
/// Explicit paths keep the order they were written. A directory or glob is
/// expanded in lexicographic path order. Canonical paths are read once.
/// Directory scans include `.log`, `.json`, and `.jsonl` only. `--recursive`
/// descends into real subdirectories and skips directory symlinks.
pub fn resolve(inputs: &[String], recursive: bool, err: &mut dyn Write) -> Result<Vec<Resolved>> {
    let mut seen = HashSet::new();
    let mut seen_stdin = false;
    let mut resolved = Vec::new();
    for input in inputs {
        if input == "-" {
            if seen_stdin {
                return Err(Error::usage("stdin can only be read once"));
            }
            seen_stdin = true;
            let index = resolved.len();
            resolved.push(Resolved {
                index,
                display: "-".to_string(),
                source: Source::Stdin,
            });
            continue;
        }
        for path in expand(input, recursive, err)? {
            let key = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            if !seen.insert(key) {
                continue;
            }
            let index = resolved.len();
            let display = path.display().to_string();
            resolved.push(Resolved {
                index,
                display,
                source: Source::File(path),
            });
        }
    }
    Ok(resolved)
}

fn expand(input: &str, recursive: bool, err: &mut dyn Write) -> Result<Vec<PathBuf>> {
    let path = Path::new(input);
    if path.is_dir() {
        return scan_dir(path, recursive, err);
    }
    if path.is_file() {
        return Ok(vec![path.to_path_buf()]);
    }
    if has_glob_meta(input) {
        return expand_glob(input, recursive, err);
    }
    if fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink()) {
        return Err(Error::runtime(format!(
            "cannot read {input}: symbolic link is broken or does not point at a file"
        )));
    }
    Err(Error::runtime(format!(
        "no such file or directory: {input}"
    )))
}

fn expand_glob(pattern: &str, recursive: bool, err: &mut dyn Write) -> Result<Vec<PathBuf>> {
    let paths = glob::glob(pattern)
        .map_err(|err| Error::usage(format!("invalid glob pattern '{pattern}': {err}")))?;
    let mut files = Vec::new();
    for entry in paths {
        let path = entry
            .map_err(|err| Error::runtime(format!("cannot read a match for '{pattern}': {err}")))?;
        if path.is_dir() {
            files.extend(scan_dir(&path, recursive, err)?);
        } else if path.is_file() {
            files.push(path);
        }
    }
    if files.is_empty() {
        return Err(Error::runtime(format!("no files matched '{pattern}'")));
    }
    files.sort();
    files.dedup();
    Ok(files)
}

fn scan_dir(dir: &Path, recursive: bool, err: &mut dyn Write) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    visit(dir, recursive, &mut found)?;
    if found.is_empty() {
        writeln!(
            err,
            "warning: no .log, .json, or .jsonl files in {}",
            dir.display()
        )?;
    }
    found.sort();
    Ok(found)
}

fn visit(dir: &Path, recursive: bool, found: &mut Vec<PathBuf>) -> Result<()> {
    let entries = fs::read_dir(dir)
        .map_err(|err| Error::runtime(format!("cannot read directory {}: {err}", dir.display())))?;
    for entry in entries {
        let entry = entry.map_err(|err| {
            Error::runtime(format!("cannot read directory {}: {err}", dir.display()))
        })?;
        let path = entry.path();
        let meta = fs::symlink_metadata(&path)
            .map_err(|err| Error::runtime(format!("cannot inspect {}: {err}", path.display())))?;
        let file_type = meta.file_type();
        if file_type.is_symlink() {
            if path.is_file() && allowed_extension(&path) {
                found.push(path);
            }
            continue;
        }
        if file_type.is_dir() {
            if recursive {
                visit(&path, true, found)?;
            }
            continue;
        }
        if file_type.is_file() && allowed_extension(&path) {
            found.push(path);
        }
    }
    Ok(())
}

fn allowed_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            ext.eq_ignore_ascii_case("log")
                || ext.eq_ignore_ascii_case("json")
                || ext.eq_ignore_ascii_case("jsonl")
        })
}

fn has_glob_meta(input: &str) -> bool {
    // On Windows `\` is a path separator. Elsewhere it escapes the next character.
    #[cfg(windows)]
    {
        input.chars().any(|ch| matches!(ch, '*' | '?' | '['))
    }
    #[cfg(not(windows))]
    {
        let mut chars = input.chars();
        while let Some(ch) = chars.next() {
            match ch {
                '*' | '?' | '[' => return true,
                '\\' => {
                    let _ = chars.next();
                }
                _ => {}
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use std::fs;

    #[test]
    fn directories_globs_and_duplicates_are_deterministic() {
        let dir = TempDir::new();
        let root = dir.path();
        fs::create_dir(root.join("nested")).unwrap();
        fs::write(root.join("b.log"), "b").unwrap();
        fs::write(root.join("a.JSON"), "a").unwrap();
        fs::write(root.join("note.txt"), "t").unwrap();
        fs::write(root.join("nested").join("c.jsonl"), "c").unwrap();

        let mut err = Vec::new();
        let top = resolve(&[root.display().to_string()], false, &mut err).unwrap();
        let names = names_of(&top);
        assert_eq!(names, vec!["a.JSON", "b.log"]);
        assert!(err.is_empty());

        err.clear();
        let recursive = resolve(&[root.display().to_string()], true, &mut err).unwrap();
        assert_eq!(names_of(&recursive), vec!["a.JSON", "b.log", "c.jsonl"]);

        let explicit = root.join("note.txt");
        let with_text = resolve(&[explicit.display().to_string()], false, &mut Vec::new()).unwrap();
        assert_eq!(names_of(&with_text), vec!["note.txt"]);

        let pattern = root.join("*.log");
        fs::write(root.join("c.log"), "c").unwrap();
        let globbed = resolve(
            &[
                root.join("b.log").display().to_string(),
                pattern.display().to_string(),
            ],
            false,
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(names_of(&globbed), vec!["b.log", "c.log"]);

        let doubled = resolve(
            &[
                root.join("b.log").display().to_string(),
                root.join("nested")
                    .join("..")
                    .join("b.log")
                    .display()
                    .to_string(),
            ],
            false,
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(doubled.len(), 1);
    }

    #[test]
    fn missing_paths_and_empty_directories_are_reported() {
        let dir = TempDir::new();
        let err = resolve(&["missing.log".into()], false, &mut Vec::new()).unwrap_err();
        assert!(err.to_string().contains("no such file or directory"));
        assert_eq!(err.code(), 1);

        let pattern = dir.path().join("*.log");
        let err = resolve(&[pattern.display().to_string()], false, &mut Vec::new()).unwrap_err();
        assert!(err.to_string().contains("no files matched"));

        let mut warnings = Vec::new();
        let empty = dir.path().join("empty");
        fs::create_dir(&empty).unwrap();
        let resolved = resolve(&[empty.display().to_string()], false, &mut warnings).unwrap();
        assert!(resolved.is_empty());
        let warning = String::from_utf8(warnings).unwrap();
        assert!(warning.contains("warning:"));
        assert!(warning.contains("empty"));

        let err = resolve(&["[".into()], false, &mut Vec::new()).unwrap_err();
        assert!(err.to_string().contains("invalid glob pattern"));
        assert_eq!(err.code(), 2);

        let err = resolve(&["-".into(), "-".into()], false, &mut Vec::new()).unwrap_err();
        assert!(err.to_string().contains("stdin can only be read once"));
    }

    fn names_of(resolved: &[Resolved]) -> Vec<String> {
        resolved
            .iter()
            .map(|item| {
                let Source::File(path) = &item.source else {
                    panic!("expected a file");
                };
                path.file_name().unwrap().to_string_lossy().into_owned()
            })
            .collect()
    }
}
