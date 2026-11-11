//! The sandbox side of the mailbox: read the `down` box, write the `up`
//! box. The devices `/dev/paddock/mail/in` and `out` (device.rs) and the
//! host request `mail` of `paddock-host mail` (through the gateway device
//! or the host socket) both end here, so they behave the same. The mail
//! hooks of a harness ask here too (op `deliver`).
//!
//! The sandbox id always comes from the caller's channel (the mount or the
//! socket), never from the request. A request that names another run in
//! `to` is refused: an agent writes only its own `up` box.

use super::deliver::{self, Via};
use super::{Dir, Kind, Message, SendError, AGENT, FROM_AGENT};
use crate::env::HostEnv;
use std::time::{Duration, Instant};

/// The most message bytes that one read returns. A read returns at least
/// one message, and the rest comes with the next read.
pub const MAX_READ: usize = 512 * 1024;

/// Read the unread `down` messages of `id`, waiting until `end` for the
/// first one, and move the agent's cursor past them (a `delivered` event
/// records the read). None: the time passed first (still waiting).
pub fn read(
    env: &HostEnv,
    id: &str,
    end: Instant,
    stop: &dyn Fn() -> bool,
) -> Option<Vec<Message>> {
    loop {
        let after = super::cursor(env, id, Dir::Down, AGENT);
        super::wait(env, id, Dir::Down, after, end, stop)?;
        // A hook can take the same messages first: then wait again.
        match deliver::take(env, id, Via::Read, MAX_READ) {
            Ok(t) if !t.messages.is_empty() => {
                log::info!(
                    "sandbox {id}: the agent read {} mail message(s)",
                    t.messages.len()
                );
                return Some(t.messages);
            }
            Ok(_) => {
                if Instant::now() >= end || stop() {
                    return None;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => {
                log::warn!("sandbox {id}: cannot read the mail: {e:#}");
                return None;
            }
        }
    }
}

/// A hook asks for the mail that it gives the agent (deliver.rs): `stop`
/// takes every unread message, `tool` only up to the last interrupt.
fn hook_deliver(env: &HostEnv, id: &str, via: &str) -> crate::broker::Response {
    use crate::broker::{Response, Status};
    let via = match Via::parse(via) {
        Some(v @ (Via::Stop | Via::Tool)) => v,
        _ => return Response::new(Status::Invalid, "via is stop or tool"),
    };
    match deliver::take(env, id, via, deliver::MAX_HOOK_TEXT) {
        Ok(t) => {
            let text = (!t.messages.is_empty()).then(|| deliver::hook_text(via, &t));
            Response::done(serde_json::json!({
                "messages": t.messages,
                "left": t.left,
                "text": text,
            }))
        }
        Err(e) => Response::new(Status::Failed, format!("{e:#}")),
    }
}

/// The fields that an agent may set. Unknown fields are refused, so a typo
/// does not pass unseen.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Outgoing {
    #[serde(default)]
    pub kind: Option<String>,
    pub text: String,
    #[serde(default)]
    pub refs: Vec<String>,
    #[serde(default)]
    pub reply_to: Option<u64>,
    /// Must be this run when it is set.
    #[serde(default)]
    pub to: Option<String>,
}

/// Plain text is one `say`; a JSON object is an `Outgoing`.
pub fn parse_outgoing(body: &[u8]) -> Result<Outgoing, SendError> {
    let raw = std::str::from_utf8(body)
        .map_err(|_| SendError::Invalid("the message is not UTF-8 text".into()))?;
    if raw.trim_start().starts_with('{') {
        return serde_json::from_str(raw.trim())
            .map_err(|e| SendError::Invalid(format!("bad message JSON: {e}")));
    }
    Ok(Outgoing {
        text: raw.strip_suffix('\n').unwrap_or(raw).to_string(),
        ..Default::default()
    })
}

/// Write an agent's message into its own `up` box.
pub fn send(env: &HostEnv, id: &str, o: Outgoing) -> Result<Message, SendError> {
    if let Some(to) =
        o.to.as_deref()
            .filter(|t| *t != id && *t != crate::ids::short(id))
    {
        log::info!("sandbox {id}: refused mail to another run");
        return Err(SendError::Denied(format!(
            "a sandbox writes only to its own mailbox, not to {to:?}"
        )));
    }
    let kind = match o.kind.as_deref() {
        None => Kind::Say,
        Some(k) => Kind::parse(k)
            .ok_or_else(|| SendError::Invalid(format!("kind {k:?}: use say, ask or status")))?,
    };
    if kind == Kind::Status {
        super::text::check_status(&o.text).map_err(SendError::Invalid)?;
    }
    if kind == Kind::Status && o.text.trim_start().starts_with("done:") {
        match crate::trainer::completion::ready(env, id, &o.text) {
            Ok(true) => {},
            Ok(false) => return Err(SendError::Invalid("a trainer is reviewing this final report; wait for its advice and then submit done again".into())),
            Err(error) => return Err(SendError::Io(error)),
        }
    }
    let mut m = Message::new(FROM_AGENT, kind, o.text);
    m.refs = o.refs;
    m.reply_to = o.reply_to;
    let m = super::append(env, id, Dir::Up, m)?;
    log::info!(
        "sandbox {id}: the agent sent mail #{} ({} bytes)",
        m.seq,
        m.text.len()
    );
    Ok(m)
}

/// The host request `mail` (`paddock-host mail`): `op` is `read` or `send`.
pub fn handle(env: &HostEnv, id: &str, req: &crate::broker::Request) -> crate::broker::Response {
    use crate::broker::{Response, Status};
    if req.v != crate::broker::PROTOCOL {
        return Response::new(Status::Invalid, "unsupported protocol version");
    }
    if !super::exists(env, id) {
        return Response::new(Status::Disabled, "this run has no mailbox");
    }
    match req.args.get("op").and_then(|v| v.as_str()) {
        Some("read") => {
            let asked = req.args.get("timeout").and_then(|v| v.as_u64());
            let timeout = match crate::waiting::read_timeout(asked) {
                Ok(t) => t,
                Err(e) => return Response::new(Status::Invalid, format!("{e:#}")),
            };
            let end = Instant::now() + Duration::from_secs(timeout);
            match read(env, id, end, &|| false) {
                Some(v) => Response::done(serde_json::json!({ "messages": v })),
                None => Response::new(
                    Status::Waiting,
                    format!(
                        "still waiting: no mail in {timeout} seconds; run the same command again"
                    ),
                ),
            }
        }
        Some("send") => {
            let body = req.args.get("message").cloned().unwrap_or_default();
            let o = match serde_json::from_value::<Outgoing>(body) {
                Ok(o) => o,
                Err(e) => return Response::new(Status::Invalid, format!("bad message: {e}")),
            };
            match send(env, id, o) {
                Ok(m) => Response::done(serde_json::json!({ "seq": m.seq })),
                Err(e) => Response::new(status_of(&e), e.to_string()),
            }
        }
        Some("deliver") => hook_deliver(
            env,
            id,
            req.args.get("via").and_then(|v| v.as_str()).unwrap_or(""),
        ),
        Some("status") => Response::done(serde_json::json!({
            "unread": super::unread_down(env, id).len(),
            "last_seq": super::last_seq(env, id),
        })),
        _ => Response::new(Status::Invalid, "op is read, send, deliver or status"),
    }
}

fn status_of(e: &SendError) -> crate::broker::Status {
    use crate::broker::Status;
    match e {
        SendError::Denied(_) => Status::Denied,
        SendError::Rate | SendError::Ended | SendError::NoMailbox(_) => Status::Disabled,
        SendError::Io(_) => Status::Failed,
        SendError::TooLarge(_) | SendError::Invalid(_) => Status::Invalid,
    }
}
