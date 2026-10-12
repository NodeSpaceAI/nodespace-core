//! Bounded newline-delimited line buffering for streaming HTTP responses.
//!
//! Both NDJSON and SSE (`text/event-stream`) bodies arrive as newline-delimited
//! records. A stream chunk may split a line at an arbitrary byte boundary —
//! including mid-way through a multi-byte UTF-8 sequence — so bytes must be
//! accumulated raw and decoded only once a complete line is in hand. The buffer
//! is capped so a malfunctioning or hostile server emitting an unterminated line
//! cannot grow memory without bound.

/// Maximum bytes buffered between newlines before the stream is rejected.
///
/// A single line is a small JSON object (e.g. a chat delta); 1 MiB is orders of
/// magnitude past any legitimate line.
pub const MAX_LINE_BYTES: usize = 1024 * 1024;

/// Reason a [`NdjsonLineBuffer`] rejected input.
#[derive(Debug, PartialEq, Eq)]
pub enum NdjsonError {
    /// The inter-newline buffer exceeded [`MAX_LINE_BYTES`].
    LineTooLong { limit: usize },
    /// A complete line was not valid UTF-8.
    InvalidUtf8,
}

impl std::fmt::Display for NdjsonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NdjsonError::LineTooLong { limit } => {
                write!(f, "stream line exceeded {limit} bytes without a newline")
            }
            NdjsonError::InvalidUtf8 => {
                write!(f, "stream line was not valid UTF-8")
            }
        }
    }
}

/// Accumulates raw stream bytes and yields complete, UTF-8-decoded lines.
///
/// Bytes are buffered until a `\n` is seen; each complete line is decoded with a
/// strict UTF-8 check (safe because the line is complete, unlike a per-chunk
/// lossy decode that can straddle a multi-byte boundary). Every line is bounded
/// by [`MAX_LINE_BYTES`]: a completed line whose length exceeds the cap, or an
/// unterminated tail that grows past it, makes [`push`](Self::push) return
/// [`NdjsonError::LineTooLong`] before the oversized line is allocated.
///
/// Each `push` searches only bytes not already searched: the unterminated tail
/// is known to be newline-free up to `scanned`, so a line drip-fed in many small
/// chunks costs time linear in its length rather than quadratic.
#[derive(Default)]
pub struct NdjsonLineBuffer {
    buffer: Vec<u8>,
    /// Length of the prefix of `buffer` already searched and known to contain
    /// no newline.
    scanned: usize,
}

impl NdjsonLineBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a stream chunk and drain every newly-completed line.
    ///
    /// Empty and whitespace-only lines are skipped. Returns the trimmed lines in
    /// order, or an error if the buffer overflowed or a completed line was not
    /// valid UTF-8.
    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<String>, NdjsonError> {
        self.buffer.extend_from_slice(chunk);

        let mut lines = Vec::new();
        // Start of the current (not yet completed) line within `buffer`.
        let mut line_start = 0;
        let mut search_from = self.scanned;
        while let Some(offset) = self.buffer[search_from..].iter().position(|&b| b == b'\n') {
            let newline = search_from + offset;
            // Reject an oversized completed line before allocating it. The
            // length excludes the terminating newline.
            if newline - line_start > MAX_LINE_BYTES {
                return Err(NdjsonError::LineTooLong {
                    limit: MAX_LINE_BYTES,
                });
            }
            let line = std::str::from_utf8(&self.buffer[line_start..newline])
                .map_err(|_| NdjsonError::InvalidUtf8)?
                .trim();
            if !line.is_empty() {
                lines.push(line.to_string());
            }
            line_start = newline + 1;
            search_from = line_start;
        }

        // Drop every completed line in one shift; what remains is the
        // unterminated tail, all of which has now been searched.
        self.buffer.drain(..line_start);
        self.scanned = self.buffer.len();

        // A newline resets the count, so only an unterminated tail remains here.
        if self.buffer.len() > MAX_LINE_BYTES {
            return Err(NdjsonError::LineTooLong {
                limit: MAX_LINE_BYTES,
            });
        }

        Ok(lines)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_multiple_lines_in_one_chunk() {
        let mut buf = NdjsonLineBuffer::new();
        let lines = buf.push(b"{\"a\":1}\n{\"b\":2}\n").unwrap();
        assert_eq!(lines, vec!["{\"a\":1}", "{\"b\":2}"]);
    }

    #[test]
    fn buffers_partial_line_across_chunks() {
        let mut buf = NdjsonLineBuffer::new();
        assert!(buf.push(b"{\"a\":").unwrap().is_empty());
        let lines = buf.push(b"1}\n").unwrap();
        assert_eq!(lines, vec!["{\"a\":1}"]);
    }

    #[test]
    fn decodes_multibyte_utf8_split_across_chunks() {
        // "é" is 0xC3 0xA9 in UTF-8; split it across two chunks.
        let mut buf = NdjsonLineBuffer::new();
        assert!(buf.push(&[b'"', 0xC3]).unwrap().is_empty());
        let lines = buf.push(&[0xA9, b'"', b'\n']).unwrap();
        assert_eq!(lines, vec!["\"é\""]);
    }

    #[test]
    fn skips_empty_and_whitespace_lines() {
        let mut buf = NdjsonLineBuffer::new();
        let lines = buf.push(b"\n   \n{\"a\":1}\n").unwrap();
        assert_eq!(lines, vec!["{\"a\":1}"]);
    }

    #[test]
    fn errors_when_unterminated_line_exceeds_cap() {
        let mut buf = NdjsonLineBuffer::new();
        let huge = vec![b'x'; MAX_LINE_BYTES + 1];
        assert_eq!(
            buf.push(&huge),
            Err(NdjsonError::LineTooLong {
                limit: MAX_LINE_BYTES
            })
        );
    }

    #[test]
    fn errors_when_completed_line_exceeds_cap() {
        // A newline-terminated line longer than the cap must be rejected before
        // it is allocated, not just an unterminated tail.
        let mut buf = NdjsonLineBuffer::new();
        let mut line = vec![b'x'; MAX_LINE_BYTES + 1];
        line.push(b'\n');
        assert_eq!(
            buf.push(&line),
            Err(NdjsonError::LineTooLong {
                limit: MAX_LINE_BYTES
            })
        );
    }

    #[test]
    fn errors_when_tail_grows_past_cap_across_chunks() {
        // Overflow must trip even when the oversized tail accumulates over
        // several push calls rather than arriving in one.
        let mut buf = NdjsonLineBuffer::new();
        let half = vec![b'x'; MAX_LINE_BYTES / 2 + 1];
        assert!(buf.push(&half).unwrap().is_empty());
        assert_eq!(
            buf.push(&half),
            Err(NdjsonError::LineTooLong {
                limit: MAX_LINE_BYTES
            })
        );
    }

    #[test]
    fn drip_fed_line_is_not_rescanned() {
        // A line delivered one byte per chunk must cost linear time. Rescanning
        // the buffered prefix on every push makes this ~5 * 10^11 comparisons
        // at the cap, far past any reasonable test timeout.
        let mut buf = NdjsonLineBuffer::new();
        for _ in 0..MAX_LINE_BYTES {
            assert!(buf.push(b"x").unwrap().is_empty());
        }
        let lines = buf.push(b"\n").unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].len(), MAX_LINE_BYTES);
    }

    #[test]
    fn line_completed_after_partial_scan_keeps_later_lines() {
        // The resume offset must not skip a newline arriving in the same chunk
        // as the end of a partially-scanned line, nor lines after it.
        let mut buf = NdjsonLineBuffer::new();
        assert!(buf.push(b"ab").unwrap().is_empty());
        assert!(buf.push(b"c").unwrap().is_empty());
        let lines = buf.push(b"d\ne\nf").unwrap();
        assert_eq!(lines, vec!["abcd", "e"]);
        assert_eq!(buf.push(b"g\n").unwrap(), vec!["fg"]);
    }

    #[test]
    fn newline_resets_overflow_count() {
        // A large but newline-terminated stream must not trip the cap: the
        // buffer never holds more than one incomplete line's worth at a time.
        let mut buf = NdjsonLineBuffer::new();
        let line = format!("{}\n", "x".repeat(MAX_LINE_BYTES - 1));
        for _ in 0..4 {
            buf.push(line.as_bytes()).unwrap();
        }
        // Buffer is empty after each terminated line; another chunk still works.
        assert_eq!(buf.push(b"ok\n").unwrap(), vec!["ok"]);
    }
}
