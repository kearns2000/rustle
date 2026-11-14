use std::io::{BufReader, Read};

use crate::error::Result;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedLine {
    pub number: usize,
    pub text: String,
}

/// Splits a byte stream into logical lines.
///
/// A UTF-8 BOM is removed from the start of the stream. CRLF is treated as one
/// line ending. Incomplete trailing bytes stay buffered until `finish` is set,
/// which is how follow mode retains a partial line. Invalid UTF-8 becomes
/// U+FFFD and does not stop the split.
#[derive(Debug)]
pub struct LineSplitter {
    pending: Vec<u8>,
    next_number: usize,
    check_bom: bool,
}

impl Default for LineSplitter {
    fn default() -> Self {
        Self::new()
    }
}

impl LineSplitter {
    pub fn new() -> Self {
        Self {
            pending: Vec::new(),
            next_number: 1,
            check_bom: true,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    pub fn push(&mut self, bytes: &[u8], finish: bool) -> Vec<DecodedLine> {
        self.pending.extend_from_slice(bytes);
        self.drain_lines(finish)
    }

    fn drain_lines(&mut self, finish: bool) -> Vec<DecodedLine> {
        self.strip_bom(finish);
        let mut lines = Vec::new();
        let mut start = 0;
        while let Some(offset) = self.pending[start..].iter().position(|byte| *byte == b'\n') {
            let end = start + offset;
            let mut line = self.pending[start..end].to_vec();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            self.push_line(&line, &mut lines);
            start = end + 1;
        }
        if start > 0 {
            self.pending.drain(..start);
        }
        if finish && !self.pending.is_empty() {
            if self.pending.last() == Some(&b'\r') {
                self.pending.pop();
            }
            if !self.pending.is_empty() {
                let line = std::mem::take(&mut self.pending);
                self.push_line(&line, &mut lines);
            }
        }
        lines
    }

    fn push_line(&mut self, line: &[u8], lines: &mut Vec<DecodedLine>) {
        let number = self.next_number;
        self.next_number += 1;
        if line.is_empty() {
            return;
        }
        lines.push(DecodedLine {
            number,
            text: String::from_utf8_lossy(line).into_owned(),
        });
    }

    fn strip_bom(&mut self, finish: bool) {
        if !self.check_bom {
            return;
        }
        const BOM: &[u8] = &[0xEF, 0xBB, 0xBF];
        if self.pending.starts_with(BOM) {
            self.pending.drain(..BOM.len());
            self.check_bom = false;
            return;
        }
        let still_a_prefix = self.pending.len() < BOM.len() && BOM.starts_with(&self.pending);
        if finish || !still_a_prefix {
            self.check_bom = false;
        }
    }
}

pub fn for_each_line<R, F>(reader: R, mut on_line: F) -> Result<()>
where
    R: Read,
    F: FnMut(DecodedLine) -> Result<()>,
{
    let mut reader = BufReader::new(reader);
    let mut splitter = LineSplitter::new();
    let mut buf = [0u8; 8192];
    loop {
        let read = read_retry(&mut reader, &mut buf)?;
        let finish = read == 0;
        for line in splitter.push(&buf[..read], finish) {
            on_line(line)?;
        }
        if finish {
            break;
        }
    }
    Ok(())
}

pub fn read_retry(reader: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    loop {
        match reader.read(buf) {
            Ok(read) => return Ok(read),
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn splitter_keeps_partial_lines_until_finished() {
        let mut splitter = LineSplitter::new();
        assert!(splitter.push(b"hel", false).is_empty());
        let lines = splitter.push(b"lo\nwor", false);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "hello");
        assert_eq!(lines[0].number, 1);
        assert!(splitter.push(&[], false).is_empty());
        let finished = splitter.push(&[], true);
        assert_eq!(finished[0].text, "wor");
        assert_eq!(finished[0].number, 2);
    }

    #[test]
    fn byte_chunks_handle_bom_crlf_blank_lines_and_invalid_utf8() {
        let data = b"\xEF\xBB\xBFone\r\n\ncaf\xffe";
        let mut splitter = LineSplitter::new();
        let mut lines = Vec::new();
        for byte in data {
            lines.extend(splitter.push(std::slice::from_ref(byte), false));
        }
        lines.extend(splitter.push(&[], true));
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].number, 1);
        assert_eq!(lines[0].text, "one");
        assert_eq!(lines[1].number, 3);
        assert_eq!(lines[1].text, "caf\u{FFFD}e");
    }

    #[test]
    fn reader_streams_without_requiring_a_final_newline() {
        let mut lines = Vec::new();
        for_each_line(Cursor::new(&b"\xEF\xBB\xBFalpha\r\nbeta"[..]), |line| {
            lines.push((line.number, line.text));
            Ok(())
        })
        .unwrap();
        assert_eq!(
            lines,
            vec![(1, "alpha".to_string()), (2, "beta".to_string())]
        );
    }

    #[test]
    fn empty_input_yields_no_lines() {
        let mut lines = Vec::new();
        for_each_line(Cursor::new(&b""[..]), |line| {
            lines.push(line);
            Ok(())
        })
        .unwrap();
        assert!(lines.is_empty());
    }
}
