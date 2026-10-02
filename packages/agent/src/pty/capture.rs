//! Opt-in output capture for PTY agent sessions.
//!
//! [`SessionCapture`] maintains a circular byte buffer of PTY output chunks.
//! On session end, [`SessionCapture::transcript`] assembles the raw scrollback
//! for the ai-chat node, and [`SessionCapture::plain_text`] the text a summary
//! of the session is derived from.

use std::collections::VecDeque;

use crate::pty::plain_text::strip_terminal_sequences;
use crate::pty::session::OutputChunk;

/// Maximum total bytes kept in the ring buffer (1 MiB). Older chunks are
/// evicted when the buffer is full.
pub const MAX_BUFFER_BYTES: usize = 1024 * 1024;

/// Accumulates PTY output chunks in a bounded circular buffer.
///
/// The buffer evicts the *oldest* chunks when `max_bytes` is exceeded, so
/// the most recent output is always retained.
#[derive(Clone)]
pub struct SessionCapture {
    buffer: VecDeque<OutputChunk>,
    max_bytes: usize,
    current_bytes: usize,
    /// Whether output has been dropped to stay within `max_bytes`. The buffer
    /// then no longer starts where the session did.
    overflowed: bool,
}

impl SessionCapture {
    pub fn new() -> Self {
        Self::with_max_bytes(MAX_BUFFER_BYTES)
    }

    pub fn with_max_bytes(max_bytes: usize) -> Self {
        Self {
            buffer: VecDeque::new(),
            max_bytes,
            current_bytes: 0,
            overflowed: false,
        }
    }

    /// Append a chunk, evicting the oldest chunks if the buffer would exceed
    /// `max_bytes`.
    pub fn push(&mut self, chunk: OutputChunk) {
        let chunk_len = chunk.data.len();

        // If a single chunk is larger than the entire buffer, just store it
        // alone (truncated to max_bytes).
        if chunk_len >= self.max_bytes {
            self.overflowed |= !self.buffer.is_empty() || chunk_len > self.max_bytes;
            self.buffer.clear();
            self.current_bytes = 0;
            let truncated = OutputChunk {
                data: chunk.data[..self.max_bytes].to_vec(),
                timestamp: chunk.timestamp,
            };
            self.current_bytes = truncated.data.len();
            self.buffer.push_back(truncated);
            return;
        }

        // Evict oldest chunks until there is room.
        while self.current_bytes + chunk_len > self.max_bytes {
            if let Some(oldest) = self.buffer.pop_front() {
                self.current_bytes -= oldest.data.len();
                self.overflowed = true;
            } else {
                break;
            }
        }

        self.current_bytes += chunk_len;
        self.buffer.push_back(chunk);
    }

    /// Concatenate all buffered chunks into a single UTF-8 string.
    /// Non-UTF-8 bytes are replaced with the Unicode replacement character.
    pub fn transcript(&self) -> String {
        let mut bytes = Vec::with_capacity(self.current_bytes);
        for chunk in &self.buffer {
            bytes.extend_from_slice(&chunk.data);
        }
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// The buffered output as plain text: no escape sequences, no control
    /// characters, and a repainted screen kept once (see
    /// [`strip_terminal_sequences`]).
    ///
    /// Once the buffer has overflowed it starts at an arbitrary point in the
    /// stream, possibly inside an escape sequence or a line. What is left of
    /// that first line cannot be told apart from text, so it is dropped.
    pub fn plain_text(&self) -> String {
        let transcript = self.transcript();
        let whole_lines = if self.overflowed {
            transcript.split_once('\n').map_or("", |(_, rest)| rest)
        } else {
            transcript.as_str()
        };
        strip_terminal_sequences(whole_lines)
    }

    /// Whether output has been dropped to keep the buffer within its bound.
    pub fn overflowed(&self) -> bool {
        self.overflowed
    }

    /// Total bytes currently in the buffer.
    pub fn current_bytes(&self) -> usize {
        self.current_bytes
    }

    /// Number of chunks currently buffered.
    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }
}

impl Default for SessionCapture {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn chunk(data: &[u8]) -> OutputChunk {
        OutputChunk {
            data: data.to_vec(),
            timestamp: Utc::now(),
        }
    }

    #[test]
    fn push_and_transcript_basic() {
        let mut cap = SessionCapture::new();
        cap.push(chunk(b"hello "));
        cap.push(chunk(b"world"));
        assert_eq!(cap.transcript(), "hello world");
        assert_eq!(cap.current_bytes(), 11);
        assert_eq!(cap.len(), 2);
    }

    #[test]
    fn evicts_oldest_on_overflow() {
        let mut cap = SessionCapture::with_max_bytes(10);
        cap.push(chunk(b"aaaaa")); // 5 bytes
        cap.push(chunk(b"bbbbb")); // 5 bytes — total 10, exactly full
        assert_eq!(cap.current_bytes(), 10);

        cap.push(chunk(b"ccccc")); // 5 bytes — should evict "aaaaa"
        assert_eq!(cap.current_bytes(), 10);
        assert_eq!(cap.transcript(), "bbbbbccccc");
    }

    #[test]
    fn oversized_chunk_replaces_entire_buffer() {
        let mut cap = SessionCapture::with_max_bytes(5);
        cap.push(chunk(b"xx"));
        cap.push(chunk(b"yyyyyy")); // 6 bytes > max_bytes=5, truncated to 5
        assert_eq!(cap.current_bytes(), 5);
        assert_eq!(cap.transcript(), "yyyyy");
    }

    /// The transcript keeps the stream as it was read; the plain text is what
    /// a reader saw.
    #[test]
    fn plain_text_has_no_escape_sequences_and_the_transcript_keeps_them() {
        let mut cap = SessionCapture::new();
        cap.push(chunk(b"\x1b]0;claude\x07\x1b[1;32mBuild passed\x1b[0m\r\n"));
        // A sequence split across two reads of the PTY.
        cap.push(chunk(b"\x1b[38;5"));
        cap.push(chunk(b";208mAll 42 tests pass\x1b[0m\r\n"));

        assert_eq!(cap.plain_text(), "Build passed\nAll 42 tests pass");
        assert!(cap.transcript().contains("\x1b[1;32m"));
        assert!(!cap.overflowed());
    }

    /// A buffer that has overflowed starts mid-stream. Here it starts in the
    /// middle of a colour sequence, whose tail (`8;5;208m`) reads as text;
    /// the partial first line goes, and nothing of the sequence is left.
    #[test]
    fn plain_text_of_an_overflowed_buffer_drops_the_partial_first_line() {
        let mut cap = SessionCapture::with_max_bytes(48);
        cap.push(chunk(b"early output, long gone\r\n\x1b[3"));
        cap.push(chunk(b"8;5;208mtail of a line\x1b[0m\r\n"));
        cap.push(chunk(b"\x1b[32mlast line\x1b[0m\r\n"));

        assert!(cap.overflowed());
        assert!(cap.transcript().starts_with("8;5;208m"));
        assert_eq!(cap.plain_text(), "last line");
    }

    #[test]
    fn an_oversized_chunk_counts_as_overflow() {
        let mut cap = SessionCapture::with_max_bytes(5);
        cap.push(chunk(b"yyyyy"));
        assert!(!cap.overflowed(), "exactly full, nothing dropped");
        cap.push(chunk(b"zzzzzz"));
        assert!(cap.overflowed());
        assert_eq!(cap.plain_text(), "", "one partial line, and it is dropped");
    }

    #[test]
    fn empty_capture_returns_empty_strings() {
        let cap = SessionCapture::new();
        assert_eq!(cap.transcript(), "");
        assert_eq!(cap.plain_text(), "");
        assert!(cap.is_empty());
    }

    #[test]
    fn multiple_evictions_maintain_correct_byte_count() {
        let mut cap = SessionCapture::with_max_bytes(15);
        for i in 0..10 {
            cap.push(chunk(format!("{:05}", i).as_bytes())); // 5 bytes each
        }
        // Buffer should hold exactly the last 3 chunks (15 bytes).
        assert_eq!(cap.current_bytes(), 15);
        assert_eq!(cap.len(), 3);
    }
}
