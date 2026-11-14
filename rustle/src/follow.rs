use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crate::decode::{read_retry, LineSplitter};
use crate::error::Result;

pub(crate) const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(200);
const FINGERPRINT_LEN: usize = 64;

/// A file observed by `--follow`.
///
/// Discovery stays fixed for the lifetime of the tailer. Each poll reopens the
/// path with shared read access, emits complete lines, and keeps a trailing
/// partial line. Truncation, a changed starting prefix, or a new file identity
/// (rename-and-recreate) resets the read to the start. A missing file produces
/// one warning until it can be opened again.
pub struct Tailer {
    files: Vec<TailedFile>,
}

#[derive(Debug)]
struct TailedFile {
    index: usize,
    display: String,
    path: PathBuf,
    offset: u64,
    splitter: LineSplitter,
    identity: Option<FileId>,
    fingerprint: Vec<u8>,
    warned: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileId {
    dev: u64,
    ino: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FollowLine {
    pub index: usize,
    pub display: String,
    pub line_no: usize,
    pub text: String,
}

impl Tailer {
    pub fn new(files: impl IntoIterator<Item = (usize, String, PathBuf)>) -> Self {
        Self {
            files: files
                .into_iter()
                .map(|(index, display, path)| TailedFile {
                    index,
                    display,
                    path,
                    offset: 0,
                    splitter: LineSplitter::new(),
                    identity: None,
                    fingerprint: Vec::new(),
                    warned: false,
                })
                .collect(),
        }
    }

    pub fn poll(&mut self, err: &mut dyn Write) -> Result<Vec<FollowLine>> {
        let mut lines = Vec::new();
        for file in &mut self.files {
            match read_available(file, &mut lines) {
                Ok(()) => {
                    if file.warned {
                        writeln!(err, "warning: {} is available again", file.display)?;
                        file.warned = false;
                    }
                }
                Err(io_err) => {
                    if !file.warned {
                        writeln!(
                            err,
                            "warning: {} is temporarily unavailable ({io_err}); retrying",
                            file.display
                        )?;
                        file.warned = true;
                    }
                }
            }
        }
        Ok(lines)
    }

    pub fn run<F, H>(
        &mut self,
        err: &mut dyn Write,
        interval: Duration,
        stop: &AtomicBool,
        mut on_line: F,
        mut hook: H,
    ) -> Result<()>
    where
        F: FnMut(FollowLine) -> Result<()>,
        H: FnMut(),
    {
        loop {
            for line in self.poll(err)? {
                if stop.load(Ordering::Relaxed) {
                    return Ok(());
                }
                on_line(line)?;
            }
            hook();
            if stop.load(Ordering::Relaxed) {
                return Ok(());
            }
            wait(interval, stop);
            if stop.load(Ordering::Relaxed) {
                return Ok(());
            }
        }
    }
}

pub(crate) fn open_shared(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE.
        // Rust already defaults to this mode; set it explicitly so a reader
        // does not block a log writer from appending or rotating the file.
        options.share_mode(0x0000_0001 | 0x0000_0002 | 0x0000_0004);
    }
    options.open(path)
}

pub(crate) fn wait(interval: Duration, stop: &AtomicBool) {
    let slice = Duration::from_millis(50);
    let started = Instant::now();
    while started.elapsed() < interval {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let remaining = interval.saturating_sub(started.elapsed());
        thread::sleep(slice.min(remaining));
    }
}

fn read_available(file: &mut TailedFile, out: &mut Vec<FollowLine>) -> std::io::Result<()> {
    let mut handle = open_shared(&file.path)?;
    let identity = file_identity(&handle)?;
    let len = handle.metadata()?.len();
    let replaced = file
        .identity
        .as_ref()
        .is_some_and(|previous| previous != &identity);
    let shrunk = len < file.offset;
    let rewritten = prefix_differs(&mut handle, &file.fingerprint)?;
    if replaced || shrunk || rewritten {
        file.offset = 0;
        file.fingerprint.clear();
        file.splitter.reset();
    }
    let started_at_zero = file.offset == 0;
    handle.seek(SeekFrom::Start(file.offset))?;
    let mut captured = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        let read = read_retry(&mut handle, &mut buf)?;
        if read == 0 {
            break;
        }
        if started_at_zero && captured.len() < FINGERPRINT_LEN {
            let room = FINGERPRINT_LEN - captured.len();
            captured.extend_from_slice(&buf[..read.min(room)]);
        }
        file.offset += read as u64;
        for line in file.splitter.push(&buf[..read], false) {
            out.push(FollowLine {
                index: file.index,
                display: file.display.clone(),
                line_no: line.number,
                text: line.text,
            });
        }
    }
    if started_at_zero && file.fingerprint.is_empty() && !captured.is_empty() {
        file.fingerprint = captured;
    }
    file.identity = Some(identity);
    Ok(())
}

fn prefix_differs(handle: &mut File, fingerprint: &[u8]) -> std::io::Result<bool> {
    if fingerprint.is_empty() {
        return Ok(false);
    }
    let mut current = vec![0u8; fingerprint.len()];
    let mut filled = 0;
    while filled < current.len() {
        match handle.read(&mut current[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        }
    }
    current.truncate(filled);
    Ok(current != fingerprint)
}

#[cfg(unix)]
fn file_identity(file: &File) -> std::io::Result<FileId> {
    use std::os::unix::fs::MetadataExt;
    let meta = file.metadata()?;
    Ok(FileId {
        dev: meta.dev(),
        ino: meta.ino(),
    })
}

#[cfg(windows)]
fn file_identity(file: &File) -> std::io::Result<FileId> {
    use std::os::windows::io::AsRawHandle;

    #[repr(C)]
    struct FileTime {
        low: u32,
        high: u32,
    }

    #[repr(C)]
    struct ByHandleFileInformation {
        attributes: u32,
        creation: FileTime,
        last_access: FileTime,
        last_write: FileTime,
        volume_serial: u32,
        size_high: u32,
        size_low: u32,
        links: u32,
        index_high: u32,
        index_low: u32,
    }

    const _: () = assert!(std::mem::size_of::<ByHandleFileInformation>() == 52);

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFileInformationByHandle(
            file: *mut std::ffi::c_void,
            info: *mut ByHandleFileInformation,
        ) -> i32;
    }

    let mut info = ByHandleFileInformation {
        attributes: 0,
        creation: FileTime { low: 0, high: 0 },
        last_access: FileTime { low: 0, high: 0 },
        last_write: FileTime { low: 0, high: 0 },
        volume_serial: 0,
        size_high: 0,
        size_low: 0,
        links: 0,
        index_high: 0,
        index_low: 0,
    };
    // SAFETY: `handle` is an open file and `info` is a writable struct with the
    // Windows BY_HANDLE_FILE_INFORMATION layout. The size assertion above guards
    // that layout.
    let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(FileId {
        dev: u64::from(info.volume_serial),
        ino: (u64::from(info.index_high) << 32) | u64::from(info.index_low),
    })
}

#[cfg(not(any(unix, windows)))]
fn file_identity(file: &File) -> std::io::Result<FileId> {
    let _ = file;
    Ok(FileId { dev: 0, ino: 0 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use std::fs;
    use std::io::Write;
    use std::sync::Arc;

    fn texts(lines: &[FollowLine]) -> Vec<&str> {
        lines.iter().map(|line| line.text.as_str()).collect()
    }

    #[test]
    fn appends_partial_lines_truncation_and_rotation() {
        let dir = TempDir::new();
        let path = dir.path().join("app.log");
        fs::write(&path, b"one\n").unwrap();
        let mut tailer = Tailer::new([(0, "app.log".into(), path.clone())]);
        let mut err = Vec::new();

        assert_eq!(texts(&tailer.poll(&mut err).unwrap()), vec!["one"]);

        let mut handle = OpenOptions::new().append(true).open(&path).unwrap();
        handle.write_all(b"two\n").unwrap();
        assert_eq!(texts(&tailer.poll(&mut err).unwrap()), vec!["two"]);

        fs::write(&path, b"hel").unwrap();
        assert!(tailer.poll(&mut err).unwrap().is_empty());
        let mut handle = OpenOptions::new().append(true).open(&path).unwrap();
        handle.write_all(b"lo\r").unwrap();
        assert!(tailer.poll(&mut err).unwrap().is_empty());
        let mut handle = OpenOptions::new().append(true).open(&path).unwrap();
        handle.write_all(b"\n").unwrap();
        let lines = tailer.poll(&mut err).unwrap();
        assert_eq!(texts(&lines), vec!["hello"]);

        fs::write(&path, b"one\ntwo\n").unwrap();
        assert_eq!(texts(&tailer.poll(&mut err).unwrap()), vec!["one", "two"]);
        fs::write(&path, b"three\n").unwrap();
        assert_eq!(texts(&tailer.poll(&mut err).unwrap()), vec!["three"]);

        fs::write(&path, b"same\n").unwrap();
        assert_eq!(texts(&tailer.poll(&mut err).unwrap()), vec!["same"]);
        fs::rename(&path, dir.path().join("app.log.1")).unwrap();
        fs::write(&path, b"same\n").unwrap();
        let rotated = tailer.poll(&mut err).unwrap();
        assert_eq!(texts(&rotated), vec!["same"]);
        assert_eq!(rotated[0].line_no, 1);
    }

    #[test]
    fn missing_files_warn_once_and_new_paths_are_not_added() {
        let dir = TempDir::new();
        let path = dir.path().join("app.log");
        fs::write(&path, b"one\n").unwrap();
        let mut tailer = Tailer::new([(0, "app.log".into(), path.clone())]);
        let mut err = Vec::new();
        assert_eq!(texts(&tailer.poll(&mut err).unwrap()), vec!["one"]);

        fs::remove_file(&path).unwrap();
        tailer.poll(&mut err).unwrap();
        tailer.poll(&mut err).unwrap();
        let warning = String::from_utf8(err).unwrap();
        assert_eq!(warning.matches("temporarily unavailable").count(), 1);

        fs::write(dir.path().join("other.log"), b"hidden\n").unwrap();
        fs::write(&path, b"two\n").unwrap();
        let mut err = Vec::new();
        let lines = tailer.poll(&mut err).unwrap();
        assert_eq!(texts(&lines), vec!["two"]);
        let warning = String::from_utf8(err).unwrap();
        assert!(warning.contains("available again"));
        assert!(!warning.contains("hidden"));
    }

    #[test]
    fn run_emits_the_current_file_and_stops_without_waiting() {
        let dir = TempDir::new();
        let path = dir.path().join("app.log");
        fs::write(&path, b"ready\n").unwrap();
        let mut tailer = Tailer::new([(0, "app.log".into(), path)]);
        let stop = AtomicBool::new(false);
        let mut seen = Vec::new();
        let started = Instant::now();
        tailer
            .run(
                &mut Vec::new(),
                Duration::from_secs(30),
                &stop,
                |line| {
                    seen.push(line.text);
                    Ok(())
                },
                || stop.store(true, Ordering::Relaxed),
            )
            .unwrap();
        assert_eq!(seen, vec!["ready".to_string()]);
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn wait_honours_the_stop_flag() {
        let stop = AtomicBool::new(true);
        let started = Instant::now();
        wait(Duration::from_secs(5), &stop);
        assert!(started.elapsed() < Duration::from_millis(200));

        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(40));
            flag.store(true, Ordering::Relaxed);
        });
        let started = Instant::now();
        wait(Duration::from_secs(5), &stop);
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
