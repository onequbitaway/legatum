//! One agent as a child process: start it, send it [`ToAgent`] lines, wait a bounded time for
//! each [`FromAgent`] line, and stop it. Nothing here blocks past a deadline, and a broken or
//! hostile agent can only cost its own seat its orders.

use crate::protocol::{FromAgent, MAX_AGENT_LINE, ToAgent, decode_from_agent, encode};
use legatum_core::Order;
use std::collections::VecDeque;
use std::fmt;
use std::io::{self, BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Lines of standard error kept for the log, and the longest line kept.
const STDERR_LINES: usize = 20;
const STDERR_LINE_CHARS: usize = 500;

/// How to start an agent: a program and its arguments, run with no shell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentCommand {
    pub program: String,
    pub args: Vec<String>,
    /// The working directory, or the caller's when None.
    pub dir: Option<PathBuf>,
}

impl AgentCommand {
    /// Splits `words` on whitespace: the first word is the program. There is no quoting; use
    /// [`AgentCommand::program`] and [`AgentCommand::args`] for arguments with spaces.
    pub fn from_words(words: &str) -> Result<Self, String> {
        let mut parts = words.split_whitespace().map(str::to_owned);
        let program = parts.next().ok_or("an agent command needs a program")?;
        Ok(Self {
            program,
            args: parts.collect(),
            dir: None,
        })
    }
}

impl fmt::Display for AgentCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.program)?;
        self.args.iter().try_for_each(|a| write!(f, " {a}"))
    }
}

/// Why an agent gave no answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentError {
    /// No answer before the deadline. The agent may still answer late; a late answer is dropped.
    Timeout,
    /// The agent closed its output or exited. It gives no more answers.
    Gone,
    /// The agent sent a line that is not a valid answer here.
    Protocol(String),
    /// Starting the agent, or writing to it, failed.
    Io(String),
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout => write!(f, "no answer in time"),
            Self::Gone => write!(f, "the agent exited"),
            Self::Protocol(why) => write!(f, "protocol error: {why}"),
            Self::Io(why) => write!(f, "I/O error: {why}"),
        }
    }
}

impl std::error::Error for AgentError {}

/// A running agent. Dropping it kills the process if it still runs.
pub struct AgentProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<Result<String, String>>,
    stderr: Arc<Mutex<VecDeque<String>>>,
    gone: bool,
}

impl AgentProcess {
    pub fn spawn(command: &AgentCommand) -> Result<Self, AgentError> {
        let mut cmd = Command::new(&command.program);
        cmd.args(&command.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(dir) = &command.dir {
            cmd.current_dir(dir);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| AgentError::Io(format!("cannot start `{command}`: {e}")))?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("stdout is piped");
        let stderr_pipe = child.stderr.take().expect("stderr is piped");
        let (sender, lines) = mpsc::channel();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                match read_line_limited(&mut reader, MAX_AGENT_LINE) {
                    Ok(Some(line)) => {
                        if sender.send(Ok(line)).is_err() {
                            return;
                        }
                    }
                    Ok(None) => return,
                    Err(e) => {
                        let _ = sender.send(Err(e.to_string()));
                        return;
                    }
                }
            }
        });
        let stderr = Arc::new(Mutex::new(VecDeque::new()));
        let tail = Arc::clone(&stderr);
        thread::spawn(move || {
            for line in BufReader::new(stderr_pipe).lines() {
                let Ok(line) = line else { return };
                let mut tail = tail.lock().expect("stderr tail lock");
                if tail.len() == STDERR_LINES {
                    tail.pop_front();
                }
                tail.push_back(line.chars().take(STDERR_LINE_CHARS).collect());
            }
        });
        Ok(Self {
            child,
            stdin,
            lines,
            stderr,
            gone: false,
        })
    }

    /// Writes one message as one line.
    pub fn send(&mut self, message: &ToAgent) -> Result<(), AgentError> {
        if self.gone {
            return Err(AgentError::Gone);
        }
        let stdin = self.stdin.as_mut().ok_or(AgentError::Gone)?;
        let mut line = encode(message);
        line.push('\n');
        stdin
            .write_all(line.as_bytes())
            .and_then(|()| stdin.flush())
            .map_err(|e| {
                self.gone = true;
                AgentError::Io(e.to_string())
            })
    }

    /// The next answer, waiting until `deadline` at most.
    pub fn receive(&mut self, deadline: Instant) -> Result<FromAgent, AgentError> {
        if self.gone {
            return Err(AgentError::Gone);
        }
        let wait = deadline.saturating_duration_since(Instant::now());
        match self.lines.recv_timeout(wait) {
            Ok(Ok(line)) => decode_from_agent(&line).map_err(AgentError::Protocol),
            Ok(Err(why)) => {
                self.gone = true;
                Err(AgentError::Protocol(why))
            }
            Err(RecvTimeoutError::Timeout) => Err(AgentError::Timeout),
            Err(RecvTimeoutError::Disconnected) => {
                self.gone = true;
                Err(AgentError::Gone)
            }
        }
    }

    /// Sends `hello` and returns the agent's name.
    pub fn hello(&mut self, hello: &ToAgent, limit: Duration) -> Result<String, AgentError> {
        let deadline = Instant::now() + limit;
        self.send(hello)?;
        match self.receive(deadline)? {
            FromAgent::Hello { name, .. } => Ok(name),
            FromAgent::Orders { .. } => {
                Err(AgentError::Protocol("expected hello, got orders".into()))
            }
        }
    }

    /// Sends one `turn` message and returns the orders for `turn`. Answers for earlier turns
    /// (late answers) are dropped; they never count for this turn.
    pub fn turn(
        &mut self,
        message: &ToAgent,
        turn: u32,
        limit: Duration,
    ) -> Result<Vec<Order>, AgentError> {
        let deadline = Instant::now() + limit;
        self.send(message)?;
        loop {
            match self.receive(deadline)? {
                FromAgent::Orders { turn: t, orders } if t == turn => return Ok(orders),
                FromAgent::Orders { turn: t, .. } if t < turn => {}
                FromAgent::Orders { turn: t, .. } => {
                    return Err(AgentError::Protocol(format!(
                        "orders for turn {t} during turn {turn}"
                    )));
                }
                FromAgent::Hello { .. } => {
                    return Err(AgentError::Protocol("hello during a turn".into()));
                }
            }
        }
    }

    /// Sends `end`, closes the agent's input and waits up to `grace` for it to exit, then kills
    /// it. Returns its exit status when it exited by itself.
    pub fn finish(mut self, end: &ToAgent, grace: Duration) -> Option<ExitStatus> {
        let _ = self.send(end);
        self.stdin = None;
        let deadline = Instant::now() + grace;
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => return Some(status),
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                _ => return None,
            }
        }
    }

    /// The last lines the agent wrote to standard error, oldest first.
    pub fn stderr_tail(&self) -> Vec<String> {
        self.stderr
            .lock()
            .expect("stderr tail lock")
            .iter()
            .cloned()
            .collect()
    }

    /// The agent closed its output, broke the protocol beyond repair, or could not be written to.
    pub fn is_gone(&self) -> bool {
        self.gone
    }
}

impl Drop for AgentProcess {
    fn drop(&mut self) {
        self.stdin = None;
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

/// Reads one line without its line end. `Ok(None)` at the end of the stream. A line longer than
/// `limit` bytes is an error, and the reader does not hold more than `limit` bytes of it.
fn read_line_limited<R: BufRead>(reader: &mut R, limit: usize) -> io::Result<Option<String>> {
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return if line.is_empty() {
                Ok(None)
            } else {
                finish_line(line).map(Some)
            };
        }
        let (taken, done) = match available.iter().position(|&b| b == b'\n') {
            Some(end) => (end + 1, true),
            None => (available.len(), false),
        };
        if line.len() + taken > limit + 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("line over {limit} bytes"),
            ));
        }
        line.extend_from_slice(&available[..taken]);
        reader.consume(taken);
        if done {
            line.pop();
            return finish_line(line).map(Some);
        }
    }
}

fn finish_line(mut line: Vec<u8>) -> io::Result<String> {
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    String::from_utf8(line).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "not UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::read_line_limited;
    use std::io::BufReader;

    fn lines(bytes: &[u8], limit: usize) -> Vec<Result<String, String>> {
        // A one-byte buffer makes lines arrive in pieces, as from a slow pipe.
        let mut reader = BufReader::with_capacity(1, bytes);
        let mut out = Vec::new();
        loop {
            match read_line_limited(&mut reader, limit) {
                Ok(Some(line)) => out.push(Ok(line)),
                Ok(None) => return out,
                Err(e) => {
                    out.push(Err(e.to_string()));
                    return out;
                }
            }
        }
    }

    #[test]
    fn lines_split_on_newlines_and_drop_carriage_returns() {
        assert_eq!(
            lines(b"one\r\ntwo\nthree", 10),
            [Ok("one".into()), Ok("two".into()), Ok("three".into())]
        );
        assert!(lines(b"", 10).is_empty());
    }

    #[test]
    fn a_line_at_the_limit_passes_and_a_longer_one_fails() {
        assert_eq!(lines(b"12345\n", 5), [Ok("12345".into())]);
        assert_eq!(
            lines(b"123456\nnext\n", 5),
            [Err("line over 5 bytes".into())]
        );
    }

    #[test]
    fn bytes_that_are_not_utf8_fail() {
        assert_eq!(lines(b"\xff\n", 5), [Err("not UTF-8".into())]);
    }
}
