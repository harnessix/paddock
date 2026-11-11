//! Host-side run-end notification. The configured array is argv, never shell text.
use crate::{
    env::HostEnv,
    mail::{self, Dir, Kind},
};
use std::process::{Command, Stdio};

pub(crate) fn start(env: &HostEnv, id: &str, code: Option<i32>) {
    let args = crate::hooks::Cfg::load(env).run_end;
    let Some((program, rest)) = args.split_first().filter(|(p, _)| !p.is_empty()) else {
        return;
    };
    let status = mail::read(env, id, Dir::Up)
        .into_iter()
        .rev()
        .find(|m| m.kind == Kind::Status)
        .map(|m| m.text)
        .or_else(|| {
            crate::detach::Record::load(env, id)
                .and_then(|r| r.done)
                .map(|d| format!("{}: {}", d.status.name(), d.text))
        })
        .unwrap_or_else(|| match code {
            Some(c) => format!("exit: {c}"),
            None => "ended: unknown".into(),
        });
    match Command::new(program)
        .args(rest)
        .env("PADDOCK_RUN_ID", id)
        .env("PADDOCK_RUN_STATUS", status)
        .stdin(Stdio::null())
        .spawn()
    {
        Ok(mut child) => {
            std::thread::spawn(move || match child.wait() {
                Ok(s) if s.success() => (),
                Ok(s) => log::warn!("run-end hook exited with {s}"),
                Err(e) => log::warn!("cannot wait for run-end hook: {e}"),
            });
        }
        Err(e) => log::warn!("cannot start run-end hook: {e}"),
    }
}
