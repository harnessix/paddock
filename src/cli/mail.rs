//! `paddock mail`: the starter's side of a run's mailbox (src/mail.rs).

use anyhow::{bail, Result};
use clap::Subcommand;
use paddock::env::HostEnv;
use paddock::mail::{self, events, text, Dir, Kind, Message};
use std::io::{Read as _, Write as _};
use std::time::{Duration, Instant};

/// How often `watch` looks for a dead run process.
const GONE_EVERY: Duration = Duration::from_secs(1);
/// The exit code of `watch` when its --timeout passed (as `paddock wait`).
const EXIT_TIMEOUT: i32 = 124;

#[derive(Subcommand)]
pub(crate) enum MailCmd {
    /// Send a message to the agent of a run. The text comes from the
    /// arguments, or from stdin with - or without text.
    Send {
        /// A sandbox id, or its short form such as 1a2b3c.
        run: String,
        text: Vec<String>,
        /// A question: the agent answers with a reply to its number.
        #[arg(long)]
        ask: bool,
        /// Mark the message as urgent for the agent.
        #[arg(long)]
        interrupt: bool,
        /// Answer the agent's message with this number.
        #[arg(long, value_name = "SEQ")]
        reply_to: Option<u64>,
        /// A path in the project or its loft (.paddock/) that the message
        /// is about. Repeat it for more.
        #[arg(long = "ref", value_name = "PATH")]
        refs: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// Print the agent's messages and paddock's events of a run, one line
    /// each, as they come. It ends after the end of the run (exit 0). Each
    /// line is printed once per reader: a new watch goes on where the last
    /// one stopped.
    Watch {
        /// A sandbox id or its short form. Or --mine.
        #[arg(required_unless_present = "mine", conflicts_with = "mine")]
        run: Option<String>,
        /// Every run that this caller started from the host
        /// ($PADDOCK_CALLER, or `user`), also runs that start later.
        #[arg(long)]
        mine: bool,
        /// One JSON object per line.
        #[arg(long)]
        json: bool,
        /// The reader whose place the watch keeps. Two watchers with
        /// different names each get every line.
        #[arg(long, default_value = mail::STARTER)]
        reader: String,
        /// Print from the first message, and keep the reader's place.
        #[arg(long, conflicts_with = "from")]
        all: bool,
        /// Print from this message number, and keep the reader's place.
        #[arg(long, value_name = "SEQ")]
        from: Option<u64>,
        /// The longest watch, for example 90s or 30m. Exit code 124 then.
        #[arg(long, value_name = "DURATION")]
        timeout: Option<String>,
    },
    /// List the runs that have a mailbox, or the messages of one run.
    Ls {
        run: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Print one message in full.
    Show {
        run: String,
        seq: u64,
        #[arg(long)]
        json: bool,
    },
}

pub(crate) fn run(env: &HostEnv, cmd: MailCmd) -> Result<i32> {
    match cmd {
        MailCmd::Send {
            run,
            text,
            ask,
            interrupt,
            reply_to,
            refs,
            json,
        } => {
            let id = resolve(env, &run)?;
            let body = if text.is_empty() || text == ["-"] {
                let mut s = String::new();
                std::io::stdin()
                    .take(mail::MAX_TEXT as u64 * 2)
                    .read_to_string(&mut s)?;
                s
            } else {
                text.join(" ")
            };
            let mut m = Message::new(mail::HOST, if ask { Kind::Ask } else { Kind::Say }, body);
            m.interrupt = interrupt;
            m.reply_to = reply_to;
            m.refs = refs;
            events::check_gone(env, &id);
            let m = mail::send_down(env, &id, m).map_err(|e| anyhow::anyhow!("{e}"))?;
            let when = mail::delivery(env, &id).when(&m);
            if json {
                println!(
                    "{}",
                    serde_json::json!({ "run": id, "seq": m.seq, "delivery": when })
                );
            } else {
                println!("sent as #{}; {when}", m.seq);
            }
            Ok(0)
        }
        MailCmd::Watch {
            run,
            mine: _,
            json,
            reader,
            all,
            from,
            timeout,
        } => {
            if !mail::valid_name(&reader) {
                bail!("--reader is 1 to 64 letters, digits, - _ . or :");
            }
            let end = match timeout {
                Some(t) => {
                    Some(Instant::now() + Duration::from_secs(paddock::timer::parse_duration(&t)?))
                }
                None => None,
            };
            let start = if all {
                Some(0)
            } else {
                from.map(|f| f.saturating_sub(1))
            };
            let w = Watch {
                json,
                reader,
                start,
                end,
            };
            match run {
                Some(r) => w.one(env, &resolve(env, &r)?),
                None => w.mine(env),
            }
        }
        MailCmd::Ls { run: None, json } => list_runs(env, json),
        MailCmd::Ls { run: Some(r), json } => list_messages(env, &resolve(env, &r)?, json),
        MailCmd::Show { run, seq, json } => {
            let id = resolve(env, &run)?;
            let Some((dir, m)) = mail::find(env, &id, seq) else {
                bail!(
                    "{id} has no message #{seq}: `paddock mail ls {}` lists them",
                    paddock::ids::short(&id)
                );
            };
            if json {
                println!("{}", serde_json::to_string_pretty(&m)?);
                return Ok(0);
            }
            println!(
                "#{} {} {} {}",
                m.seq,
                dir.name(),
                paddock::view::local_time(m.ts),
                text::sender(&m, &id)
            );
            println!(
                "kind: {}",
                m.event
                    .as_deref()
                    .map_or(m.kind.name().to_string(), |e| format!("event {e}"))
            );
            if let Some(r) = m.reply_to {
                println!("reply to: #{r}");
            }
            if !m.refs.is_empty() {
                println!("refs: {}", text::clean(&m.refs.join(", ")));
            }
            println!("\n{}", text::clean(&m.text).trim_end());
            Ok(0)
        }
    }
}

/// The full id of a run with a mailbox.
fn resolve(env: &HostEnv, input: &str) -> Result<String> {
    let ids = mail::list(env);
    match paddock::ids::resolve_from(input, ids.iter().map(String::as_str)) {
        Ok(id) => Ok(id),
        Err(_) => bail!("no mailbox for {input}: `paddock mail ls` lists the runs that have one"),
    }
}

struct Watch {
    json: bool,
    reader: String,
    /// Replay from after this seq instead of the reader's place.
    start: Option<u64>,
    end: Option<Instant>,
}

impl Watch {
    fn after(&self, env: &HostEnv, id: &str) -> u64 {
        self.start
            .unwrap_or_else(|| mail::cursor(env, id, Dir::Up, &self.reader))
    }

    /// Print one message and keep the reader's place. False when stdout
    /// is gone: the place stays, so the next watch prints it again.
    fn emit(&self, env: &HostEnv, id: &str, m: &Message) -> bool {
        let line = if self.json {
            text::watch_json(m, id)
        } else {
            text::watch_line(m, id)
        };
        let mut out = std::io::stdout().lock();
        if writeln!(out, "{line}").and_then(|()| out.flush()).is_err() {
            return false;
        }
        if let Err(e) = mail::set_cursor(env, id, Dir::Up, &self.reader, m.seq) {
            eprintln!(
                "paddock: cannot keep the place of reader {}: {e:#}",
                self.reader
            );
        }
        true
    }

    fn timed_out(&self) -> bool {
        self.end.is_some_and(|e| Instant::now() >= e)
    }

    /// One run, until its end event.
    fn one(&self, env: &HostEnv, id: &str) -> Result<i32> {
        let mut after = self.after(env, id);
        let mut checked = Instant::now() - GONE_EVERY;
        loop {
            if checked.elapsed() >= GONE_EVERY {
                events::check_gone(env, id);
                checked = Instant::now();
            }
            let wait_end =
                (Instant::now() + GONE_EVERY).min(self.end.unwrap_or(Instant::now() + GONE_EVERY));
            for m in mail::wait(env, id, Dir::Up, after, wait_end, &|| false).unwrap_or_default() {
                if !self.emit(env, id, &m) {
                    return Ok(0);
                }
                after = m.seq;
                if m.is_end() {
                    return Ok(0);
                }
            }
            if mail::end(env, id).is_some_and(|e| e.seq <= after) {
                eprintln!("paddock: {id} ended; its mailbox has nothing new");
                return Ok(0);
            }
            if self.timed_out() {
                return Ok(EXIT_TIMEOUT);
            }
        }
    }

    /// Every run of this caller, the ones that start later too.
    fn mine(&self, env: &HostEnv) -> Result<i32> {
        let me = mail::caller_key();
        let mut places: std::collections::BTreeMap<String, u64> = Default::default();
        loop {
            for id in mail::list(env) {
                let ours =
                    mail::starter(env, &id).is_some_and(|s| s.kind == "host" && s.caller == me);
                if !ours {
                    continue;
                }
                let after = *places
                    .entry(id.clone())
                    .or_insert_with(|| self.after(env, &id));
                if mail::end(env, &id).is_some_and(|e| e.seq <= after) {
                    continue;
                }
                events::check_gone(env, &id);
                for m in mail::read_after(env, &id, Dir::Up, after) {
                    if !self.emit(env, &id, &m) {
                        return Ok(0);
                    }
                    places.insert(id.clone(), m.seq);
                }
            }
            if self.timed_out() {
                return Ok(EXIT_TIMEOUT);
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }
}

/// The state of a run, for `ls`.
fn state(env: &HostEnv, id: &str) -> String {
    events::check_gone(env, id);
    match mail::end(env, id) {
        Some(e) if e.event.as_deref() == Some("gone") => "gone".into(),
        Some(e) => match e.code {
            Some(c) => format!("ended ({c})"),
            None => "ended".into(),
        },
        None if mail::run_info(env, id).is_some() => "running".into(),
        None => "starting".into(),
    }
}

fn list_runs(env: &HostEnv, json: bool) -> Result<i32> {
    let mut rows = Vec::new();
    for id in mail::list(env) {
        let s = mail::starter(env, &id);
        let up = mail::read(env, &id, Dir::Up);
        let unread_up = up
            .iter()
            .filter(|m| m.seq > mail::cursor(env, &id, Dir::Up, mail::STARTER))
            .count();
        rows.push(serde_json::json!({
            "id": id,
            "short": paddock::ids::short(&id),
            "state": state(env, &id),
            "starter": s.as_ref().map(|s| s.caller.clone()),
            "parent": s.and_then(|s| s.parent),
            "up": up.len(),
            "unread_up": unread_up,
            "unread_down": mail::unread_down(env, &id).len(),
            "last": up.last().map(|m| m.ts),
        }));
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(0);
    }
    if rows.is_empty() {
        println!("no run has a mailbox yet");
        return Ok(0);
    }
    println!(
        "{:<8} {:<12} {:<16} {:>9} {:>11}  RUN",
        "SHORT", "STATE", "STARTER", "UNREAD UP", "UNREAD DOWN"
    );
    for r in rows {
        println!(
            "{:<8} {:<12} {:<16} {:>9} {:>11}  {}",
            r["short"].as_str().unwrap_or(""),
            r["state"].as_str().unwrap_or(""),
            r["starter"].as_str().unwrap_or(""),
            r["unread_up"],
            r["unread_down"],
            r["id"].as_str().unwrap_or("")
        );
    }
    Ok(0)
}

fn list_messages(env: &HostEnv, id: &str, json: bool) -> Result<i32> {
    events::check_gone(env, id);
    let read_by_agent = mail::cursor(env, id, Dir::Down, mail::AGENT);
    let delivered = mail::deliver::delivered(env, id);
    let state_of = |d: Dir, m: &Message| {
        (d == Dir::Down && m.kind != Kind::Event)
            .then(|| mail::deliver::state(m, read_by_agent, &delivered))
    };
    let mut all: Vec<(Dir, Message)> = [Dir::Down, Dir::Up]
        .into_iter()
        .flat_map(|d| mail::read(env, id, d).into_iter().map(move |m| (d, m)))
        .collect();
    all.sort_by_key(|(_, m)| m.seq);
    if json {
        let v: Vec<_> = all
            .iter()
            .map(|(d, m)| {
                let mut v = serde_json::to_value(m).unwrap_or_default();
                v["dir"] = d.name().into();
                if let Some((state, via)) = state_of(*d, m) {
                    v["state"] = state.into();
                    v["delivered_via"] = via.into();
                }
                v
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(0);
    }
    println!("mailbox of {id}: {}", state(env, id));
    for (d, m) in &all {
        let mut line = text::one_line(m, id);
        if line.chars().count() > 100 {
            line = line.chars().take(99).collect::<String>() + "…";
        }
        let unread = match state_of(*d, m) {
            Some((_, Some(via))) => format!(" ({})", mail::deliver::Via::describe_name(&via)),
            Some((state, None)) => format!(" ({state})"),
            None => String::new(),
        };
        let kind = m
            .event
            .as_deref()
            .map_or(m.kind.name().to_string(), |e| format!("event {e}"));
        println!(
            "#{:<4} {} {:<4} {:<18} {:<14} {line}{unread}",
            m.seq,
            text::clock(m.ts),
            d.name(),
            kind,
            m.from
        );
    }
    Ok(0)
}
