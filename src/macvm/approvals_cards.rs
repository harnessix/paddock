//! Approval card rendering and event publication.

use super::*;
use crate::broker::{Connect, Pending, CONNECT_CAPABILITY};
use crate::eventstream::schema::Question as EventQuestion;

impl Asker {
    /// A `question` line on the launcher event stream.
    pub(super) fn publish(
        &self,
        state: QuestionState,
        id: &str,
        about: Option<(&Triple, Option<String>)>,
        answer: Option<(&str, &str)>,
        open: Option<u64>,
    ) {
        let (host, port, process) = match about {
            Some((t, p)) => (t.host.clone(), t.port, p),
            None => (String::new(), 0, None),
        };
        let event = EventQuestion {
            state,
            id: id.to_string(),
            app: self.profile.clone(),
            host,
            port,
            process,
            answer: answer.map(|a| a.0.to_string()),
            by: answer.map(|a| a.1.to_string()),
            open,
        };
        #[cfg(test)]
        {
            // Tests never fall through to the process-global event bus,
            // whose shortened macOS socket path can be outside the TempDir.
            EVENT_HOOK.with(|hook| {
                if let Some(hook) = hook.borrow().as_ref() {
                    hook(event);
                }
            });
        }
        #[cfg(not(test))]
        crate::eventstream::publish(&self.env, crate::eventstream::Event::Question(event));
    }

    /// The card of a question on the screen.
    pub fn card(&self, q: &Question, t: &Triple, more: u64) -> Pending {
        let why_path = PathBuf::from(q.path.clone().unwrap_or_else(|| t.app.clone()));
        let mut detail = text::ask_panel(&self.label, &t.host, t.port, &why_path);
        if !self.wake_order_fenced() {
            detail.push('\n');
            detail.push_str(&text::ask_allow_unfenced(&self.profile, &t.host, t.port));
        }
        // One read: the detail text and the count field must agree.
        let waiting = q.count();
        for line in [text::ask_count(waiting), text::ask_more(more)] {
            if !line.is_empty() {
                detail.push('\n');
                detail.push_str(&line);
            }
        }
        Pending {
            id: q.card_id(),
            sandbox: format!("macvm-{}", self.profile), // not user text
            harness: self.profile.clone(),
            project: self.profile_dir.clone(),
            capability: CONNECT_CAPABILITY.into(),
            summary: text::ask_summary(&self.label, &t.host, t.port),
            detail,
            created: q.asked_at(),
            session_allowed: true,
            choices: ConnectAnswer::ALL
                .iter()
                .map(|a| a.word().to_string())
                .collect(),
            connect: Some(Connect {
                app: t.app.clone(),
                process_app: Some(q.who.app.clone()),
                parent_app: q.who.parent_app.clone(),
                team: q.who.team.clone(),
                host: t.host.clone(),
                port: t.port,
                count: waiting,
                asked_at: q.asked_at(),
                pid: self.pid,
                path: q.path.clone(),
                profile: self.profile.clone(),
                label: self.label.clone(),
                more,
                notice: false,
                icon: self.icon.clone(),
            }),
            ..Default::default()
        }
    }

    /// The notice card of a paused session.
    pub(super) fn notice_card(&self, id: &str, open: usize, now: u64) -> Pending {
        Pending {
            id: id.to_string(),
            sandbox: format!("macvm-{}", self.profile), // not user text
            harness: self.profile.clone(),
            project: self.profile_dir.clone(),
            capability: CONNECT_CAPABILITY.into(),
            summary: text::asks_notice_summary(&self.label, open),
            detail: text::asks_notice_detail(&self.label, &self.profile, open),
            created: now,
            choices: [ConnectAnswer::AllowOnce, ConnectAnswer::Deny]
                .iter()
                .map(|a| a.word().to_string())
                .collect(),
            connect: Some(Connect {
                app: self.label.clone(),
                asked_at: now,
                pid: self.pid,
                profile: self.profile.clone(),
                label: self.label.clone(),
                more: open as u64,
                notice: true,
                icon: self.icon.clone(),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    pub(super) fn write_card(&self, p: &Pending, published: bool) -> bool {
        #[cfg(test)]
        if let Err(e) =
            WRITE_HOOK.with(|hook| hook.borrow().as_ref().map_or(Ok(()), |hook| hook(p)))
        {
            log::warn!("macvm: cannot write the connect question {}: {e}", p.id);
            return false;
        }
        let result = if published {
            broker::update_pending(&self.env, p)
        } else {
            broker::write_pending(&self.env, p).map(|_| ())
        };
        if let Err(e) = result {
            log::warn!("macvm: cannot write the connect question {}: {e}", p.id);
            return false;
        }
        true
    }

    /// A malformed answer is unknown, not a user denial.
    pub(super) fn parse_answer(text: &str) -> Option<(ConnectAnswer, &'static str)> {
        let source = serde_json::from_str::<serde_json::Value>(text)
            .ok()
            .and_then(|v| v.get("source").and_then(|v| v.as_str()).map(str::to_owned));
        let who = match source.as_deref() {
            Some("cli") => "cli",
            Some("approver") => "approver",
            _ => "user",
        };
        Some((ConnectAnswer::parse_file(text)?, who))
    }
}
