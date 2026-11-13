//! The rules of the running session from `paddock app rules NAME add
//! --session|--minutes` and `remove`: the `grant rule ...` lines of the
//! control socket. They change the session rules in memory and the
//! snapshot `session-rules.json`. The rules file stays as it is. The CLI
//! publishes the stream event.

use super::*;
use crate::macvm::grants_live::{Answer, Op, RuleAdd};
use crate::macvm::text;

impl Firewall {
    /// The answer of a `grant rule ...` line, or `None` for another op.
    pub fn rule_op(&self, op: &Op) -> Option<Result<Answer, String>> {
        match op {
            Op::RuleAdd(add) => Some(self.cli_add(add)),
            Op::RuleRemove { id } => Some(self.cli_remove(id)),
            _ => None,
        }
    }

    fn cli_add(&self, add: &RuleAdd) -> Result<Answer, String> {
        if add.rule.scope != Scope::Session {
            return Err(text::rule_not_session(&add.rule.id));
        }
        let rule = {
            let mut rules = self.rules();
            rules
                .add(add.rule.clone(), add.yes_all_hosts)
                .map_err(|e| format!("{e:#}"))?
                .clone()
        };
        self.cli_changed(&rule, "add");
        Ok(Answer {
            rule: Some(rule),
            ..Answer::live()
        })
    }

    fn cli_remove(&self, id: &str) -> Result<Answer, String> {
        let rule = {
            let mut rules = self.rules();
            match rules.session.iter().position(|r| r.id == id) {
                Some(i) => rules.session.remove(i),
                None => return Err(text::no_session_rule(id)),
            }
        };
        self.cli_changed(&rule, "remove");
        Ok(Answer {
            rule: Some(rule),
            ..Answer::live()
        })
    }

    fn cli_changed(&self, rule: &Rule, action: &str) {
        self.note_rule(rule, action, "cli", crate::state::now());
        self.write_session_rules();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::macvm::firewall::{new_rule, Port, Verdict};

    fn session_rule(host: &str, verdict: Verdict, expires: Option<u64>) -> Rule {
        let mut r = new_rule(
            "*",
            None,
            host,
            Port::Num(443),
            verdict,
            Scope::Session,
            BY_USER,
            crate::state::now(),
        );
        r.expires = expires;
        r
    }

    #[test]
    fn session_rules_from_the_cli_decide_and_go() {
        let t = tempfile::tempdir().unwrap();
        let log = t.path().join("logs/connections.jsonl");
        std::fs::create_dir_all(log.parent().unwrap()).unwrap();
        let rules = Rules::load_at(t.path().join("rules.toml")).unwrap();
        let fw = Firewall::new(Mode::Ask, None, rules, None, log.clone());
        let conn = Conn::new("com.example.notes", "api.example.com", 443);
        assert_eq!(
            firewall::decide(fw.mode(), None, &fw.rules().all(), &conn),
            Decision::Ask
        );
        // A deny of this session decides at once and lands in the snapshot.
        let deny = session_rule("api.example.com", Verdict::Deny, None);
        let op = Op::parse(
            Op::RuleAdd(RuleAdd {
                rule: deny.clone(),
                yes_all_hosts: false,
            })
            .line()
            .strip_prefix("grant ")
            .unwrap(),
        )
        .unwrap();
        let a = fw.rule_op(&op).unwrap().unwrap();
        assert_eq!(a.rule.as_ref().unwrap().id, deny.id);
        assert_eq!(
            firewall::decide(fw.mode(), None, &fw.rules().all(), &conn),
            Decision::Deny(Some(deny.id.clone()))
        );
        let snap: Vec<Rule> = serde_json::from_slice(
            &std::fs::read(log.with_file_name("session-rules.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(snap, vec![deny.clone()]);
        assert!(!t.path().join("rules.toml").exists(), "the file stays");
        // A rule with an end time stops deciding after it.
        let past = session_rule("old.example.com", Verdict::Allow, Some(1));
        fw.rule_op(&Op::RuleAdd(RuleAdd {
            rule: past,
            yes_all_hosts: false,
        }))
        .unwrap()
        .unwrap();
        assert_eq!(
            firewall::decide(
                fw.mode(),
                None,
                &fw.rules().all(),
                &Conn::new("x", "old.example.com", 443)
            ),
            Decision::Ask
        );
        // Every host needs the confirmation; an always rule is refused.
        let all = session_rule("*", Verdict::Allow, None);
        let e = fw
            .rule_op(&Op::RuleAdd(RuleAdd {
                rule: all.clone(),
                yes_all_hosts: false,
            }))
            .unwrap()
            .unwrap_err();
        assert!(e.contains("--yes-all-hosts"), "{e}");
        let mut always = all;
        always.scope = Scope::Always;
        assert!(fw
            .rule_op(&Op::RuleAdd(RuleAdd {
                rule: always,
                yes_all_hosts: true,
            }))
            .unwrap()
            .is_err());
        // Remove: the rule goes; an unknown id is refused.
        let a = fw
            .rule_op(&Op::RuleRemove {
                id: deny.id.clone(),
            })
            .unwrap()
            .unwrap();
        assert_eq!(a.rule.unwrap().host, "api.example.com");
        assert_eq!(
            firewall::decide(fw.mode(), None, &fw.rules().all(), &conn),
            Decision::Ask
        );
        let e = fw
            .rule_op(&Op::RuleRemove { id: deny.id })
            .unwrap()
            .unwrap_err();
        assert!(e.contains("no rule"), "{e}");
        assert!(fw.rule_op(&Op::List).is_none());
    }
}
