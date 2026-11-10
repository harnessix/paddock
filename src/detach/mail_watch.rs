//! Mail status handling for detached runs.
use crate::env::HostEnv;
use crate::mail::{self, Dir, Kind, Message};

#[derive(Default)]
pub(super) struct MailWatch {
    seen: u64,
    pending: Option<(u64, u64, u64)>,
}

impl MailWatch {
    pub(super) fn poll(&mut self, env: &HostEnv, id: &str, now: u64, attached: bool) -> bool {
        let statuses = mail::read(env, id, Dir::Up);
        for m in statuses
            .iter()
            .filter(|m| m.kind == Kind::Status && m.seq > self.seen)
            .collect::<Vec<_>>()
        {
            self.seen = m.seq;
            if m.text.starts_with("done:") {
                self.pending = Some((m.seq, m.ts, m.seq));
                let _ = mail::append(
                    env,
                    id,
                    Dir::Up,
                    Message::event("done", format!("done, still running: {}", m.text)),
                );
            } else {
                self.pending = None;
            }
        }
        if let Some((_, at, down)) = self.pending {
            if mail::read(env, id, Dir::Down)
                .last()
                .is_some_and(|m| m.seq > down)
            {
                self.pending = None;
                return false;
            }
            let grace = std::fs::read_to_string(env.paddock_config().join("config.toml"))
                .ok()
                .and_then(|s| s.parse::<toml::Value>().ok())
                .and_then(|v| {
                    v.get("mail")?
                        .get("end_after_done")?
                        .as_str()
                        .and_then(|s| crate::timer::parse_duration(s).ok())
                })
                .unwrap_or(120);
            return !attached && now.saturating_sub(at) >= grace;
        }
        false
    }
}

pub(super) fn idle(env: &HostEnv, id: &str, quiet: u64, notice: &str) {
    let status = mail::read(env, id, Dir::Up)
        .into_iter()
        .rev()
        .find(|m| m.kind == Kind::Status)
        .map_or_else(|| "none".to_string(), |m| m.text);
    let _ = mail::append(
        env,
        id,
        Dir::Up,
        Message::event(
            "idle",
            format!("{notice}; quiet {quiet} seconds; last status: {status}"),
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detach::tests::env_in;
    use crate::mail::{Starter, FROM_AGENT};

    #[test]
    fn done_grace_holds_for_views_and_new_mail_cancels_it() {
        let t = tempfile::tempdir().unwrap();
        let env = env_in(t.path());
        let id = "mail-done";
        mail::create(&env, id, &Starter::host()).unwrap();
        let done = mail::append(
            &env,
            id,
            Dir::Up,
            Message::new(FROM_AGENT, Kind::Status, "done: finished"),
        )
        .unwrap();
        let mut watch = MailWatch::default();
        assert!(!watch.poll(&env, id, done.ts + 119, false));
        assert!(!watch.poll(&env, id, done.ts + 121, true));
        assert!(watch.poll(&env, id, done.ts + 121, false));
        mail::append(
            &env,
            id,
            Dir::Down,
            Message::new("starter", Kind::Say, "continue"),
        )
        .unwrap();
        assert!(!watch.poll(&env, id, done.ts + 122, false));
        assert_eq!(
            mail::read(&env, id, Dir::Up)
                .iter()
                .filter(|m| m.event.as_deref() == Some("done"))
                .count(),
            1
        );
    }
}
