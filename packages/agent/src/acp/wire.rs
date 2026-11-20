//! Framing, limits and errors for the stdio JSON-RPC wire.
//!
//! Lines are bounded while they are read, before they are buffered whole: a line
//! over the message limit is discarded as it streams past and never held.

use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Everything the agent's output and behaviour are bounded by. The defaults are
/// for production; tests shrink them.
#[derive(Debug, Clone, Copy)]
pub struct AcpLimits {
    /// Largest one line the agent may send. A larger line is dropped unread.
    pub message_bytes: usize,
    /// Most oversized lines tolerated in one session.
    pub max_oversize: u32,
    /// Most bytes read in one prompt turn.
    pub turn_bytes: u64,
    /// Most bytes read in one session.
    pub session_bytes: u64,
    /// Most update messages in one prompt turn.
    pub turn_events: u32,
    /// Lines that are not JSON-RPC: how many, and how many bytes, in a session.
    pub noise_lines: u32,
    pub noise_bytes: usize,
    /// Most requests the client refuses, and most stray responses or
    /// notifications, before the session is ended (per session, and reset each turn).
    pub agent_requests: u32,
    /// Most permission requests waiting for an answer at once; one more is
    /// answered as cancelled.
    pub pending_permissions: usize,
    /// Most permission requests in one turn before the session is ended.
    pub turn_permissions: u32,
    /// Longest text of a prompt.
    pub prompt_bytes: usize,
    /// `initialize` and `session/new` together, credential lookup included.
    pub startup: Duration,
    /// One prompt turn, start to stop reason.
    pub turn: Duration,
    /// No message from the agent during a turn (not counted while a permission
    /// request waits for the user).
    pub idle: Duration,
    /// How long a permission request waits for the user before it is cancelled.
    pub permission: Duration,
    /// How long an agent has to finish a cancelled turn before it is stopped.
    pub cancel_grace: Duration,
    /// How long one write to the agent may block.
    pub write: Duration,
    /// How long a full event channel may stay full before the session is ended.
    pub consumer_stall: Duration,
}

impl Default for AcpLimits {
    fn default() -> Self {
        Self {
            message_bytes: 4 * 1024 * 1024,
            max_oversize: 8,
            turn_bytes: 64 * 1024 * 1024,
            session_bytes: 1024 * 1024 * 1024,
            turn_events: 200_000,
            noise_lines: 256,
            noise_bytes: 256 * 1024,
            agent_requests: 256,
            pending_permissions: 8,
            turn_permissions: 256,
            prompt_bytes: 256 * 1024,
            startup: Duration::from_secs(60),
            turn: Duration::from_secs(2 * 60 * 60),
            idle: Duration::from_secs(10 * 60),
            permission: Duration::from_secs(5 * 60),
            cancel_grace: Duration::from_secs(10),
            write: Duration::from_secs(5),
            consumer_stall: Duration::from_secs(60),
        }
    }
}

/// What can go wrong with a session. Text that came from the agent has had
/// credentials removed and is cut.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AcpError {
    #[error("the agent could not be started: {0}")]
    Launch(String),
    #[error("{0}")]
    Credential(String),
    #[error("the agent did not finish {0} within {1} seconds")]
    Timeout(&'static str, u64),
    #[error("the agent broke the protocol: {0}")]
    Protocol(String),
    #[error("the agent needs a sign-in: {0}")]
    AuthRequired(String),
    #[error("the agent exited")]
    AgentExited,
    #[error("the agent went over a limit: {0}")]
    Limit(String),
    #[error("the session has ended")]
    Closed,
    #[error("a turn is already running")]
    Busy,
    #[error("there is no such permission request")]
    UnknownPermission,
    #[error("that is not one of the offered choices")]
    InvalidOption,
    #[error("the prompt is larger than the limit")]
    PromptTooLarge,
}

/// One unit read from the agent.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Frame {
    Line(Vec<u8>),
    /// A line over the message limit, dropped without being held.
    Oversize,
    Eof,
}

/// Reads newline-delimited lines with a bound. Cancel-safe: the only await is a
/// read, and all state is kept in the struct.
pub(super) struct LineReader<R> {
    source: R,
    buf: Vec<u8>,
    scanned: usize,
    discarding: bool,
    message_limit: usize,
    bytes_read: u64,
    ceiling: u64,
}

/// More bytes were read than the budget allows.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct OverBudget;

const READ_CHUNK: usize = 8192;

impl<R: AsyncRead + Unpin> LineReader<R> {
    pub(super) fn new(source: R, message_limit: usize) -> Self {
        Self {
            source,
            buf: Vec::new(),
            scanned: 0,
            discarding: false,
            message_limit,
            bytes_read: 0,
            ceiling: u64::MAX,
        }
    }

    pub(super) fn bytes_read(&self) -> u64 {
        self.bytes_read
    }

    /// No more than this many bytes in total may have been read.
    pub(super) fn set_ceiling(&mut self, ceiling: u64) {
        self.ceiling = ceiling;
    }

    pub(super) async fn next(&mut self) -> Result<Frame, OverBudget> {
        loop {
            if !self.discarding {
                if let Some(offset) = self.buf[self.scanned..].iter().position(|b| *b == b'\n') {
                    let end = self.scanned + offset;
                    let mut line: Vec<u8> = self.buf.drain(..=end).collect();
                    self.scanned = 0;
                    line.pop();
                    if line.last() == Some(&b'\r') {
                        line.pop();
                    }
                    return Ok(Frame::Line(line));
                }
                self.scanned = self.buf.len();
                if self.buf.len() > self.message_limit {
                    self.buf = Vec::new();
                    self.scanned = 0;
                    self.discarding = true;
                }
            }
            if self.bytes_read >= self.ceiling {
                return Err(OverBudget);
            }
            let mut chunk = [0u8; READ_CHUNK];
            let n = self.source.read(&mut chunk).await.unwrap_or(0);
            if n == 0 {
                return Ok(Frame::Eof);
            }
            self.bytes_read += n as u64;
            if self.bytes_read > self.ceiling {
                return Err(OverBudget);
            }
            if self.discarding {
                if let Some(p) = chunk[..n].iter().position(|b| *b == b'\n') {
                    self.discarding = false;
                    self.buf.extend_from_slice(&chunk[p + 1..n]);
                    return Ok(Frame::Oversize);
                }
            } else {
                self.buf.extend_from_slice(&chunk[..n]);
            }
        }
    }
}

/// Writes one message as one line, within the write bound.
pub(super) async fn write_message<W: AsyncWrite + Unpin>(
    sink: &mut W,
    message: &Value,
    bound: Duration,
) -> Result<(), AcpError> {
    let mut line = serde_json::to_vec(message).unwrap_or_default();
    line.push(b'\n');
    let write = async {
        sink.write_all(&line).await?;
        sink.flush().await
    };
    match tokio::time::timeout(bound, write).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(_)) => Err(AcpError::AgentExited),
        Err(_) => Err(AcpError::Timeout("reading its input", bound.as_secs())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn lines_are_split_and_crlf_is_trimmed() {
        let (mut w, r) = tokio::io::duplex(64);
        w.write_all(b"one\r\ntwo\n\nthree").await.unwrap();
        drop(w);
        let mut reader = LineReader::new(r, 100);
        assert_eq!(reader.next().await, Ok(Frame::Line(b"one".to_vec())));
        assert_eq!(reader.next().await, Ok(Frame::Line(b"two".to_vec())));
        assert_eq!(reader.next().await, Ok(Frame::Line(Vec::new())));
        assert_eq!(reader.next().await, Ok(Frame::Eof));
    }

    #[tokio::test]
    async fn an_oversize_line_is_dropped_without_being_held_and_reading_continues() {
        let (mut w, r) = tokio::io::duplex(1024);
        let writer = tokio::spawn(async move {
            w.write_all(&vec![b'x'; 200_000]).await.unwrap();
            w.write_all(b"\nok\n").await.unwrap();
        });
        let mut reader = LineReader::new(r, 1000);
        assert_eq!(reader.next().await, Ok(Frame::Oversize));
        assert!(reader.buf.len() <= READ_CHUNK);
        assert_eq!(reader.next().await, Ok(Frame::Line(b"ok".to_vec())));
        writer.await.unwrap();
    }

    #[tokio::test]
    async fn a_stream_with_no_newline_hits_the_byte_budget() {
        let (mut w, r) = tokio::io::duplex(1024);
        tokio::spawn(async move {
            let chunk = vec![b'x'; 4096];
            while w.write_all(&chunk).await.is_ok() {}
        });
        let mut reader = LineReader::new(r, 1000);
        reader.set_ceiling(50_000);
        assert_eq!(reader.next().await, Err(OverBudget));
        assert!(reader.bytes_read() <= 50_000 + READ_CHUNK as u64);
    }
}
