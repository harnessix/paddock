//! A `DeviceDef` back as TOML: what `paddock device add` writes and
//! `paddock device show` prints. The text parses to the same `DeviceDef`
//! (config_tests.rs proves the round trip). Keys of 0.2.1 come out in the
//! spelling of today: `shape = "exec"` for `kind = "command"`,
//! `max_request` and `max_response` for `max_body`.

use super::config::{render_size, DeviceDef, Mode, Run, SandboxProfile, Scope, Shape, Start};
use super::config::{DEFAULT_IDLE, DEFAULT_READY};
use crate::execbox::Home;
use toml::{Table, Value};

fn strings(v: &[String]) -> Value {
    Value::Array(v.iter().map(|s| Value::String(s.clone())).collect())
}

fn paths(v: &[std::path::PathBuf]) -> Value {
    Value::Array(
        v.iter()
            .map(|p| Value::String(p.display().to_string()))
            .collect(),
    )
}

impl DeviceDef {
    /// The table as `toml::Table`, with `sandbox` as a nested table.
    pub fn to_table(&self) -> Table {
        let mut t = Table::new();
        match self.shape {
            Shape::HttpProxy => {
                t.insert("kind".into(), Value::String("http".into()));
            }
            Shape::Exec | Shape::Serve => {
                t.insert("shape".into(), Value::String(self.shape.name().into()));
                t.insert("run".into(), Value::String(self.run.name().into()));
            }
        }
        t.insert("enabled".into(), Value::Boolean(self.enabled));
        if let Some(p) = &self.http {
            t.insert("target".into(), Value::String(p.target.to_string()));
            t.insert("methods".into(), strings(&p.methods));
            t.insert("paths".into(), strings(&p.paths));
        } else if !self.command.is_empty() {
            t.insert("command".into(), strings(&self.command));
        }
        match &self.run {
            Run::Host { cwd, env } => {
                if let Some(c) = cwd {
                    t.insert("cwd".into(), Value::String(c.display().to_string()));
                }
                if !env.is_empty() {
                    let mut e = toml::Table::new();
                    for (k, v) in env {
                        e.insert(k.clone(), Value::String(v.clone()));
                    }
                    t.insert("env".into(), Value::Table(e));
                }
            }
            Run::Remote { target, device } => {
                t.insert("target".into(), Value::String(target.clone()));
                t.insert("device".into(), Value::String(device.clone()));
            }
            Run::Sandbox | Run::Broker => {}
        }
        if let Some(g) = &self.grant {
            t.insert(
                "grant".into(),
                Value::Array(g.iter().map(|g| Value::String(g.render())).collect()),
            );
        }
        if self.mode != Mode::Ask {
            t.insert("mode".into(), Value::String(self.mode.name().into()));
        }
        if self.shape != Shape::HttpProxy {
            if self.scope != Scope::Run {
                t.insert("scope".into(), Value::String(self.scope.name().into()));
            }
            if self.start != Start::FirstRequest {
                t.insert("start".into(), Value::String(self.start.name().into()));
            }
            if self.stream {
                t.insert("stream".into(), Value::Boolean(true));
            }
        }
        t.insert(
            "max_request".into(),
            Value::String(render_size(self.limits.max_request)),
        );
        t.insert(
            "max_response".into(),
            Value::String(render_size(self.limits.max_response)),
        );
        t.insert("timeout".into(), Value::Integer(self.limits.timeout as i64));
        if self.shape != Shape::HttpProxy {
            let default = if self.shape == Shape::Serve { 4 } else { 1 };
            if self.limits.concurrency != default {
                t.insert(
                    "concurrency".into(),
                    Value::Integer(self.limits.concurrency as i64),
                );
            }
            if self.idle != DEFAULT_IDLE {
                t.insert("idle".into(), Value::Integer(self.idle as i64));
            }
            if self.ready != DEFAULT_READY {
                t.insert("ready".into(), Value::Integer(self.ready as i64));
            }
        }
        if self.cost > 0 {
            t.insert("cost".into(), Value::Float(as_float(self.cost)));
        }
        if let Some(h) = &self.cost_header {
            t.insert("cost_header".into(), Value::String(h.clone()));
        }
        if let Some(b) = self.budget {
            t.insert("budget".into(), Value::Float(as_float(b)));
        }
        if let Some(p) = &self.sandbox {
            if p.exec_base {
                // The spelling of 0.2.1, kept so that the base stays the
                // `[exec]` keys. `network` is its only key.
                t.remove("shape");
                t.remove("run");
                t.insert("kind".into(), Value::String("command".into()));
                if p.network != super::config::Net::None {
                    t.insert("network".into(), Value::Boolean(true));
                }
            } else {
                t.insert("sandbox".into(), Value::Table(profile_table(p)));
            }
        }
        t
    }

    /// The table as text: `[devices.NAME]`, then `[devices.NAME.sandbox]`.
    pub fn to_toml(&self) -> String {
        let mut root = Table::new();
        let mut devices = Table::new();
        devices.insert(self.name.clone(), Value::Table(self.to_table()));
        root.insert("devices".into(), Value::Table(devices));
        toml::to_string(&root).unwrap_or_default()
    }
}

fn profile_table(p: &SandboxProfile) -> Table {
    let mut t = Table::new();
    if p.network != super::config::Net::None {
        t.insert("network".into(), Value::String(p.network.name().into()));
    }
    if !p.allow.is_empty() {
        t.insert("allow".into(), strings(&p.allow));
    }
    if !p.connect.is_empty() {
        t.insert(
            "connect".into(),
            Value::Array(
                p.connect
                    .iter()
                    .map(|c| Value::String(c.render()))
                    .collect(),
            ),
        );
    }
    if !p.ro.is_empty() {
        t.insert("ro".into(), paths(&p.ro));
    }
    if !p.rw.is_empty() {
        t.insert("rw".into(), paths(&p.rw));
    }
    let home = match p.home {
        Home::Hidden => None,
        Home::ReadOnly => Some("read-only"),
        Home::Full => Some("full"),
    };
    if let Some(h) = home {
        t.insert("home".into(), Value::String(h.into()));
    }
    if !p.env.is_empty() {
        let mut e = Table::new();
        for (k, v) in &p.env {
            e.insert(k.clone(), Value::String(v.clone()));
        }
        t.insert("env".into(), Value::Table(e));
    }
    if !p.env_pass.is_empty() {
        t.insert("env_pass".into(), strings(&p.env_pass));
    }
    if !p.credentials.is_empty() {
        t.insert("credentials".into(), strings(&p.credentials));
    }
    if !p.caller.is_empty() {
        t.insert(
            "caller".into(),
            Value::Array(
                p.caller
                    .iter()
                    .map(|c| Value::String(c.name().into()))
                    .collect(),
            ),
        );
    }
    if let Some(m) = p.mem {
        t.insert("mem".into(), Value::String(render_bytes(m)));
    }
    if let Some(c) = p.cpu_percent {
        t.insert(
            "cpus".into(),
            if c.is_multiple_of(100) {
                Value::Integer((c / 100) as i64)
            } else {
                Value::Float(c as f64 / 100.0)
            },
        );
    }
    t
}

/// Micro units of the ledger as the table writes them.
fn as_float(micros: u64) -> f64 {
    micros as f64 / super::ledger::UNIT as f64
}

/// Bytes as the table writes them, without loss: 512M, 1536K, or bytes.
fn render_bytes(b: u64) -> String {
    for (unit, size) in [
        ("T", 1u64 << 40),
        ("G", 1 << 30),
        ("M", 1 << 20),
        ("K", 1 << 10),
    ] {
        if b >= size && b.is_multiple_of(size) {
            return format!("{}{unit}", b / size);
        }
    }
    b.to_string()
}
