//! Bounded, replayable output queues for one sandbox's peer jobs.
use anyhow::{bail, Result};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

const MAX_BYTES: usize = 1024 * 1024;
const MAX_JOBS: usize = 8;
const RETAIN: Duration = Duration::from_secs(600);

#[derive(Clone, Debug)]
pub enum Event {
    Output {
        stream: String,
        bytes: Vec<u8>,
    },
    Exit(i32),
    /// Remote dispatch did not happen; the sandbox may run locally.
    NotStarted(String),
    /// The peer may have started the command. Never run it again locally.
    Unknown(String),
}
impl Event {
    fn size(&self) -> usize {
        match self {
            Self::Output { bytes, .. } => bytes.len(),
            _ => 0,
        }
    }
    fn terminal(&self) -> bool {
        !matches!(self, Self::Output { .. })
    }
}
struct State {
    events: VecDeque<(u64, Event)>,
    next: u64,
    acked: u64,
    bytes: usize,
    done_at: Option<Instant>,
}
struct Job {
    state: Mutex<State>,
    changed: Condvar,
}

#[derive(Clone)]
pub struct Sink(Arc<Job>);
impl Sink {
    pub fn send(&self, event: Event) -> Result<()> {
        let size = event.size();
        if size > MAX_BYTES {
            bail!("exclave output frame too large");
        }
        let mut state = self
            .0
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("job lock failed"))?;
        while state.bytes + size > MAX_BYTES {
            let (next, timeout) = self
                .0
                .changed
                .wait_timeout(state, Duration::from_secs(30))
                .map_err(|_| anyhow::anyhow!("job lock failed"))?;
            state = next;
            if timeout.timed_out() {
                bail!("exclave output reader stopped");
            }
        }
        if state.done_at.is_some() {
            bail!("exclave job already ended");
        }
        let terminal = event.terminal();
        let seq = state.next;
        state.next += 1;
        state.bytes += size;
        state.events.push_back((seq, event));
        if terminal {
            state.done_at = Some(Instant::now());
        }
        self.0.changed.notify_all();
        Ok(())
    }
}

#[derive(Default)]
pub struct Jobs(Mutex<HashMap<String, Arc<Job>>>);
impl Jobs {
    /// Registers the operation before it can run. IDs are caller-generated and
    /// must be unpredictable, so identical commands cannot be coalesced.
    pub fn start(&self, id: &str) -> Result<Sink> {
        if id.len() != 64
            || !id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            bail!("invalid operation id");
        }
        let mut jobs = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("job table lock failed"))?;
        jobs.retain(|_, job| {
            job.state
                .lock()
                .ok()
                .is_none_or(|s| s.done_at.is_none_or(|t| t.elapsed() < RETAIN))
        });
        if jobs.contains_key(id) {
            bail!("operation already exists");
        }
        if jobs.len() >= MAX_JOBS {
            bail!("too many exclave jobs");
        }
        let job = Arc::new(Job {
            state: Mutex::new(State {
                events: VecDeque::new(),
                next: 1,
                acked: 0,
                bytes: 0,
                done_at: None,
            }),
            changed: Condvar::new(),
        });
        jobs.insert(id.to_owned(), job.clone());
        Ok(Sink(job))
    }

    /// Release an operation only after its terminal event was received.
    pub fn finish(&self, id: &str, terminal_seq: u64) -> Result<()> {
        let mut jobs = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("job table lock failed"))?;
        let job = jobs
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("unknown exclave operation"))?;
        {
            let state = job
                .state
                .lock()
                .map_err(|_| anyhow::anyhow!("job lock failed"))?;
            if state.done_at.is_none()
                || state
                    .events
                    .back()
                    .is_none_or(|(seq, event)| *seq != terminal_seq || !event.terminal())
            {
                bail!("invalid terminal acknowledgement");
            }
        }
        jobs.remove(id);
        Ok(())
    }

    /// Ack events through `after` and return the next event, retaining it
    /// until acknowledged. A retry at the same cursor receives the same event.
    pub fn poll(&self, id: &str, after: u64, timeout: Duration) -> Result<Option<(u64, Event)>> {
        let job = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("job table lock failed"))?
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown exclave operation"))?;
        let mut state = job
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("job lock failed"))?;
        if after >= state.next || after < state.acked {
            bail!("invalid output cursor");
        }
        state.acked = after;
        while state.events.front().is_some_and(|(seq, _)| *seq <= after) {
            if let Some((_, event)) = state.events.pop_front() {
                state.bytes -= event.size();
            }
        }
        job.changed.notify_all();
        if state.events.is_empty() && state.done_at.is_none() {
            let result = job
                .changed
                .wait_timeout(state, timeout)
                .map_err(|_| anyhow::anyhow!("job lock failed"))?;
            state = result.0;
        }
        Ok(state.events.front().cloned())
    }
}

/// Converts writes into bounded output events, preserving the stream name.
pub struct EventWriter {
    pub sink: Sink,
    pub stream: &'static str,
}
impl std::io::Write for EventWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let n = bytes.len().min(16 * 1024);
        if n == 0 {
            return Ok(0);
        }
        self.sink
            .send(Event::Output {
                stream: self.stream.into(),
                bytes: bytes[..n].to_vec(),
            })
            .map_err(std::io::Error::other)?;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retry_returns_the_same_event_until_acknowledged() {
        let jobs = Jobs::default();
        let id = "a".repeat(64);
        let sink = jobs.start(&id).unwrap();
        sink.send(Event::Output {
            stream: "stdout".into(),
            bytes: b"hello".to_vec(),
        })
        .unwrap();
        sink.send(Event::Exit(7)).unwrap();
        let first = jobs.poll(&id, 0, Duration::ZERO).unwrap().unwrap();
        let retry = jobs.poll(&id, 0, Duration::ZERO).unwrap().unwrap();
        assert_eq!(first.0, retry.0);
        assert!(matches!(retry.1, Event::Output { .. }));
        let second = jobs.poll(&id, first.0, Duration::ZERO).unwrap().unwrap();
        assert!(matches!(second.1, Event::Exit(7)));
        assert!(jobs.poll(&id, 0, Duration::ZERO).is_err());
    }
}
