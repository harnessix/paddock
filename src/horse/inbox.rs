//! The inbox of a session: entries that wait for the next safe point.
//!
//! `paddock horse append` queues an entry. The custodian of a live run
//! (live.rs) delivers it when the session is idle, and records the
//! delivery: a blob node in the DAG whose parent is the session head at
//! that moment, and a row in `deliveries/<session>.json`. The entry leaves
//! the inbox only after dsh has accepted it.

use super::store::{Hash, Meta, Store};
use anyhow::{bail, Context as _, Result};
use serde::{Deserialize, Serialize};

/// The roles that an append can have.
pub const ROLES: [&str; 4] = ["user", "system", "developer", "note"];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub session: String,
    pub role: String,
    pub text: String,
    pub queued_ms: u64,
    /// Urgent: a custody that can stop the running turn (the native
    /// interrupt key of a TUI) does so, and delivers at that safe point.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub interrupt: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mail_seq: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delivery {
    pub entry: Entry,
    /// The record node in the DAG.
    pub node: Hash,
    /// The session head when paddock delivered the entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at_head: Option<Hash>,
    /// The id of the queued user message (`SessionPromptResult.messageId`).
    pub message_id: String,
    pub delivered_ms: u64,
    /// The seq of the row of the message, when it came in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub landed_seq: Option<u64>,
}

impl Entry {
    /// The text that dsh gets. The SDK admits user messages only, so an
    /// entry of another role names its role in a short tag.
    pub fn wire_text(&self) -> String {
        if self.role == "user" {
            self.text.clone()
        } else {
            format!(
                "<paddock-append role=\"{}\">\n{}\n</paddock-append>",
                self.role, self.text
            )
        }
    }
}

/// Nanoseconds since the epoch: entry ids sort in queue order.
fn now_ns() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

fn dir(session: &str) -> String {
    format!("inbox/{session}")
}

impl Store {
    /// Queue an entry for a session of the store.
    pub fn queue_append(&self, session: &str, role: &str, text: &str) -> Result<Entry> {
        self.queue_entry(session, role, text, false)
    }

    /// Queue an entry; with `interrupt` it is urgent ([`Entry::interrupt`]).
    pub fn queue_entry(
        &self,
        session: &str,
        role: &str,
        text: &str,
        interrupt: bool,
    ) -> Result<Entry> {
        if !ROLES.contains(&role) {
            bail!("role {role} is not known: use one of {}", ROLES.join(", "));
        }
        if text.is_empty() {
            bail!("the text of an append must not be empty");
        }
        self.session(session)?;
        self.write_entry(session, role, text, interrupt)
    }

    /// Queue one mailbox message, preserving its sequence for acknowledgement.
    pub fn queue_mail(
        &self,
        session: &str,
        text: &str,
        interrupt: bool,
        seq: u64,
    ) -> Result<Entry> {
        let _l = self.lock()?;
        let pending = self.inbox(session)?;
        if let Some(e) = pending.iter().find(|e| e.mail_seq == Some(seq)).cloned() {
            return Ok(e);
        }
        if let Some(e) = self
            .deliveries(session)?
            .into_iter()
            .find(|d| d.entry.mail_seq == Some(seq))
            .map(|d| d.entry)
        {
            return Ok(e);
        }
        self.session(session)?;
        // Keep mailbox entries in sequence even if the wall clock moves back.
        let last = pending
            .iter()
            .filter_map(|e| e.id.get(..20)?.parse::<u128>().ok())
            .max()
            .unwrap_or(0);
        let timestamp = now_ns().max(last.saturating_add(1));
        let e = Entry {
            id: format!(
                "{:020}-{}",
                timestamp,
                super::hex(&super::random_bytes::<3>()?)
            ),
            session: session.into(),
            role: "user".into(),
            text: text.into(),
            queued_ms: super::now_ms(),
            interrupt,
            mail_seq: Some(seq),
        };
        self.for_session(session)?.write_ref(
            &format!("{}/{}.json", dir(session), e.id),
            &serde_json::to_vec_pretty(&e)?,
        )?;
        Ok(e)
    }

    /// Queue the task of a spawned child as a `user` entry that goes out
    /// right after the start entry of the session, before every entry
    /// that another process appended in between. The session of a child
    /// can be seen before its run queues the task (a hitched child gets
    /// its session when its parent spawns it), and its custody delivers
    /// nothing before the task is queued. So the task takes the place
    /// after the first pending entry when that is the start entry
    /// (`system`), else the first place.
    pub fn queue_task(&self, session: &str, text: &str) -> Result<Entry> {
        if text.is_empty() {
            bail!("the text of an append must not be empty");
        }
        let _l = self.lock()?;
        self.session(session)?;
        let pending = self.inbox(session)?;
        let tag = super::hex(&super::random_bytes::<3>()?);
        let id = match pending.first() {
            // A longer id with the same start sorts right after it, and
            // before every id with a later time.
            Some(first) if first.role == "system" => format!("{}_task-{tag}", first.id),
            _ => format!("{:020}-task-{tag}", 0),
        };
        let e = Entry {
            id,
            session: session.into(),
            role: "user".into(),
            text: text.into(),
            queued_ms: super::now_ms(),
            interrupt: false,
            mail_seq: None,
        };
        self.for_session(session)?.write_ref(
            &format!("{}/{}.json", dir(session), e.id),
            &serde_json::to_vec_pretty(&e)?,
        )?;
        Ok(e)
    }

    /// Write an entry with no check that its session exists: the first
    /// entry of a new session goes in before the session can be seen
    /// (session/locked.rs), so no other append comes before it.
    pub(crate) fn write_entry(
        &self,
        session: &str,
        role: &str,
        text: &str,
        interrupt: bool,
    ) -> Result<Entry> {
        let now = super::now_ms();
        let e = Entry {
            id: format!(
                "{:020}-{}",
                now_ns(),
                super::hex(&super::random_bytes::<3>()?)
            ),
            session: session.into(),
            role: role.into(),
            text: text.into(),
            queued_ms: now,
            interrupt,
            mail_seq: None,
        };
        self.for_session(session)?.write_ref(
            &format!("{}/{}.json", dir(session), e.id),
            &serde_json::to_vec_pretty(&e)?,
        )?;
        Ok(e)
    }

    /// The entries of a session, oldest first.
    pub fn inbox(&self, session: &str) -> Result<Vec<Entry>> {
        let d = dir(session);
        if !self.root().join(&d).is_dir() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for n in self.list_refs(&d)? {
            if !n.ends_with(".json") {
                continue;
            }
            if let Some(b) = self.read_ref(&format!("{d}/{n}"))? {
                out.push(
                    serde_json::from_slice::<Entry>(&b)
                        .with_context(|| format!("inbox entry {n} is damaged"))?,
                );
            }
        }
        Ok(out)
    }

    /// Record a delivery and take the entry out of the inbox.
    pub fn record_delivery(
        &self,
        e: &Entry,
        at_head: Option<&Hash>,
        message_id: &str,
    ) -> Result<Delivery> {
        let meta = Meta {
            kind: "blob".into(),
            harness: None,
            container: None,
            version: None,
            parent: at_head.cloned(),
            key: Some(format!("append:{}", e.id)),
            media: Some("horse-append".into()),
        };
        let scoped = self.for_horse(&self.session(&e.session)?.horse)?;
        let node = scoped.put(&meta, &serde_json::to_vec(e)?)?;
        let d = Delivery {
            entry: e.clone(),
            node,
            at_head: at_head.cloned(),
            message_id: message_id.into(),
            delivered_ms: super::now_ms(),
            landed_seq: None,
        };
        let _l = self.lock()?;
        let mut all = self.deliveries(&e.session)?;
        all.push(d.clone());
        self.save_deliveries(&e.session, &all)?;
        let path = self
            .root()
            .join(format!("{}/{}.json", dir(&e.session), e.id));
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(x) if x.kind() == std::io::ErrorKind::NotFound => {}
            Err(x) => return Err(x).with_context(|| format!("cannot remove {}", path.display())),
        }
        Ok(d)
    }

    /// Note the seq of the row that holds a delivered message.
    pub fn delivery_landed(&self, session: &str, message_id: &str, seq: u64) -> Result<bool> {
        let _l = self.lock()?;
        let mut all = self.deliveries(session)?;
        let Some(d) = all
            .iter_mut()
            .find(|d| d.message_id == message_id && d.landed_seq.is_none())
        else {
            return Ok(false);
        };
        d.landed_seq = Some(seq);
        self.save_deliveries(session, &all)?;
        Ok(true)
    }

    /// Take back the delivery of entry `id` whose row never came: the
    /// entry goes back into the inbox under its own id, so it keeps its
    /// place in the queue, and its delivery record goes. The record node
    /// stays in the DAG. `None` when no such delivery waits to land.
    ///
    /// The inbox file comes first: after a crash between the two writes
    /// the entry is in the inbox and still recorded, and a recorded entry
    /// never goes out again.
    pub fn retract_delivery(&self, session: &str, id: &str) -> Result<Option<Entry>> {
        let _l = self.lock()?;
        let mut all = self.deliveries(session)?;
        let Some(at) = all
            .iter()
            .position(|d| d.entry.id == id && d.landed_seq.is_none())
        else {
            return Ok(None);
        };
        let d = all.remove(at);
        self.for_session(session)?.write_ref(
            &format!("{}/{}.json", dir(session), d.entry.id),
            &serde_json::to_vec_pretty(&d.entry)?,
        )?;
        self.save_deliveries(session, &all)?;
        Ok(Some(d.entry))
    }

    pub fn deliveries(&self, session: &str) -> Result<Vec<Delivery>> {
        match self.read_ref(&format!("deliveries/{session}.json"))? {
            Some(b) => serde_json::from_slice(&b)
                .with_context(|| format!("the deliveries of {session} are damaged")),
            None => Ok(Vec::new()),
        }
    }

    fn save_deliveries(&self, session: &str, all: &[Delivery]) -> Result<()> {
        self.for_session(session)?.write_ref(
            &format!("deliveries/{session}.json"),
            &serde_json::to_vec_pretty(all)?,
        )
    }
}

mod drop;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::horse::Binding;

    #[test]
    fn an_entry_waits_until_it_is_delivered() {
        let t = tempfile::tempdir().unwrap();
        let st = Store::open(t.path()).unwrap();
        let h = st.new_default_horse().unwrap();
        let b = Binding {
            harness: "dsh".into(),
            native_id: Some("session-1".into()),
            version: "1".into(),
            container: "c".into(),
        };
        let s = st.new_session(&h.id, b).unwrap();
        assert!(st.queue_append(&s.id, "boss", "x").is_err());
        assert!(st.queue_append(&s.id, "user", "").is_err());
        assert!(st.queue_append("s-000000000000", "user", "x").is_err());
        let a = st.queue_append(&s.id, "user", "first").unwrap();
        let n = st.queue_append(&s.id, "note", "second").unwrap();
        assert_eq!(st.inbox(&s.id).unwrap(), vec![a.clone(), n.clone()]);
        assert_eq!(a.wire_text(), "first");
        assert!(n
            .wire_text()
            .starts_with("<paddock-append role=\"note\">\nsecond"));

        let head = Hash::of(b"head");
        let d = st.record_delivery(&a, Some(&head), "msg-1").unwrap();
        assert_eq!(st.inbox(&s.id).unwrap(), vec![n.clone()]);
        let node = st.get(&d.node).unwrap();
        assert_eq!(node.meta.parent, Some(head));
        assert_eq!(node.meta.media.as_deref(), Some("horse-append"));
        assert!(st.delivery_landed(&s.id, "msg-1", 7).unwrap());
        assert!(!st.delivery_landed(&s.id, "msg-1", 8).unwrap());
        assert_eq!(st.deliveries(&s.id).unwrap()[0].landed_seq, Some(7));

        // A landed delivery is never taken back; one that waits goes back
        // into the inbox in its place.
        assert_eq!(st.retract_delivery(&s.id, &a.id).unwrap(), None);
        st.record_delivery(&n, None, &n.id).unwrap();
        assert!(st.inbox(&s.id).unwrap().is_empty());
        assert_eq!(st.retract_delivery(&s.id, &n.id).unwrap(), Some(n.clone()));
        assert_eq!(st.inbox(&s.id).unwrap(), vec![n.clone()]);
        assert_eq!(st.deliveries(&s.id).unwrap().len(), 1);
        assert_eq!(st.retract_delivery(&s.id, &n.id).unwrap(), None);
    }
}
