//! `/dev/paddock/mail/in` and `/dev/paddock/mail/out`: the mailbox as
//! files.
//!
//! - `in`: a read returns the unread `down` messages, one JSON object per
//!   line, and moves the agent's cursor past them. It blocks until the
//!   first one comes: at most the `timeout` of the request (default 100
//!   seconds), then it fails with ETIME (still waiting), like `wait`.
//!   `cat /dev/paddock/mail/in` works: an empty request is the default.
//! - `out`: one write is one message. Plain text is a `say`; a JSON object
//!   sets `kind` (say, ask, status), `refs` and `reply_to`. `echo TEXT >
//!   /dev/paddock/mail/out` sends it when the file closes; a client that
//!   reads after the write gets `{"seq": N}`.

use super::serve;
use crate::devices::{Call, DevError, Device, Info, Limits};
use crate::env::HostEnv;
use crate::waiting;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const IN: &str = "mail/in";
pub const OUT: &str = "mail/out";

/// The mail devices of one sandbox, when its mailbox exists.
pub fn devices(env: &HostEnv, id: &str) -> Vec<Arc<dyn Device>> {
    if !super::exists(env, id) {
        return Vec::new();
    }
    vec![Arc::new(In::new(env)), Arc::new(Out::new(env))]
}

pub struct In {
    env: HostEnv,
}

impl In {
    pub fn new(env: &HostEnv) -> In {
        In { env: env.clone() }
    }
}

impl Device for In {
    fn name(&self) -> &str {
        IN
    }
    fn info(&self) -> Info {
        Info {
            shape: "builtin",
            stream: false,
            request: format!(
                "nothing (cat /dev/paddock/mail/in), or JSON {{\"timeout\": SECONDS}}. The read waits for mail from the starter of this run at most timeout seconds (default {}, at most {}), then fails with ETIME (still waiting): read again",
                waiting::DEFAULT_READ_TIMEOUT,
                waiting::MAX_READ_TIMEOUT
            ),
            response: "the unread messages, one JSON object per line: seq, ts, from, kind (say, ask, status), text, refs, reply_to, interrupt. The read marks them as read. Answer an ask with reply_to set to its seq".into(),
            limits: Limits {
                max_request: 1024,
                max_response: serve::MAX_READ + super::MAX_TEXT * 6 + 4096,
                deadline_seconds: waiting::MAX_READ_TIMEOUT + 5,
            },
        }
    }
    fn call(&self, call: &Call, request: &[u8]) -> Result<Vec<u8>, DevError> {
        let raw = std::str::from_utf8(request)
            .map_err(|_| DevError::Invalid)?
            .trim();
        let asked = if raw.is_empty() {
            None
        } else {
            let v: serde_json::Value = serde_json::from_str(raw).map_err(|_| DevError::Invalid)?;
            let obj = v.as_object().ok_or(DevError::Invalid)?;
            if obj.keys().any(|k| k != "timeout") {
                return Err(DevError::Invalid);
            }
            obj.get("timeout")
                .map(|t| t.as_u64().ok_or(DevError::Invalid))
                .transpose()?
        };
        let timeout = waiting::read_timeout(asked).map_err(|_| DevError::Invalid)?;
        let end = (Instant::now() + Duration::from_secs(timeout))
            .min(call.deadline - Duration::from_secs(2));
        match serve::read(&self.env, &call.sandbox, end, &|| call.cancelled()) {
            Some(v) => {
                let mut out = Vec::new();
                for m in v {
                    out.extend(serde_json::to_vec(&m).map_err(|_| DevError::Failed)?);
                    out.push(b'\n');
                }
                Ok(out)
            }
            None if call.cancelled() => Err(DevError::Cancelled),
            None => Err(DevError::Waiting),
        }
    }
}

pub struct Out {
    env: HostEnv,
}

impl Out {
    pub fn new(env: &HostEnv) -> Out {
        Out { env: env.clone() }
    }
}

impl Device for Out {
    fn name(&self) -> &str {
        OUT
    }
    fn info(&self) -> Info {
        Info {
            shape: "builtin",
            stream: false,
            request: format!(
                "one message to the starter of this run: plain text (a say), or JSON {{\"kind\": \"say|ask|status\", \"text\": TEXT, \"refs\": [PATHS], \"reply_to\": SEQ}}. A status is working, blocked: REASON or done: SUMMARY. At most {} bytes of text: put longer text in a file in .paddock/ and name it in refs. echo TEXT > /dev/paddock/mail/out sends at close",
                super::MAX_TEXT
            ),
            response: "{\"seq\": N}. EMSGSIZE: the text is too long. EACCES: not this run's mailbox. EDQUOT: too many messages this minute".into(),
            limits: Limits {
                max_request: super::MAX_TEXT * 6 + 4096,
                max_response: 256,
                deadline_seconds: 10,
            },
        }
    }
    fn call(&self, call: &Call, request: &[u8]) -> Result<Vec<u8>, DevError> {
        let o = serve::parse_outgoing(request).map_err(|_| DevError::Invalid)?;
        match serve::send(&self.env, &call.sandbox, o) {
            Ok(m) => Ok(format!("{{\"seq\":{}}}\n", m.seq).into_bytes()),
            Err(super::SendError::TooLarge(_)) => Err(DevError::TooLarge),
            Err(super::SendError::Denied(_)) => Err(DevError::Denied),
            Err(super::SendError::Rate) => Err(DevError::BudgetUsed),
            Err(super::SendError::Io(_)) => Err(DevError::Failed),
            Err(_) => Err(DevError::Invalid),
        }
    }
    fn sends_on_close(&self) -> bool {
        true
    }
}
