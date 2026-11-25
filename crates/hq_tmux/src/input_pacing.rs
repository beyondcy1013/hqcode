//! Paced terminal input delivery for ink-based Agent TUIs.
//!
//! Agent TUIs treat a single stdin `read()` returning multiple bytes as a paste
//! event and drop embedded erase bytes (`0x7f` DEL / `0x08` BS). Splitting each
//! erase byte into its own write separated by 5ms prevents batched backspaces
//! from being swallowed.

use std::{io, thread, time::Duration};

pub const ERASE_KEY_WRITE_GAP: Duration = Duration::from_millis(5);

fn is_erase_key_char(ch: char) -> bool {
    ch == '\u{7f}' || ch == '\u{8}'
}

pub fn erase_key_segments(data: &str) -> Vec<&str> {
    let mut segments = Vec::new();
    let mut start = 0;
    for (index, ch) in data.char_indices() {
        if !is_erase_key_char(ch) {
            continue;
        }
        if start < index {
            segments.push(&data[start..index]);
        }
        segments.push(&data[index..index + 1]);
        start = index + 1;
    }
    if start < data.len() {
        segments.push(&data[start..]);
    }
    segments
}

pub fn write_input_paced<W: io::Write + ?Sized>(writer: &mut W, data: &str) -> io::Result<()> {
    for (index, segment) in erase_key_segments(data).into_iter().enumerate() {
        if index > 0 {
            thread::sleep(ERASE_KEY_WRITE_GAP);
        }
        writer.write_all(segment.as_bytes())?;
        writer.flush()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct RecordingWriter {
        writes: Vec<String>,
    }

    impl io::Write for RecordingWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.writes.push(String::from_utf8_lossy(buf).to_string());
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn isolates_every_erase_key() {
        let mut writer = RecordingWriter::default();
        write_input_paced(&mut writer, "abc\u{7f}\u{7f}d").unwrap();
        assert_eq!(writer.writes, vec!["abc", "\u{7f}", "\u{7f}", "d"]);
    }
}
