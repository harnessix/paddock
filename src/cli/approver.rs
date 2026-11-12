use anyhow::{bail, Result};
use clap::Subcommand;
use paddock::env::HostEnv;
use std::os::unix::fs::PermissionsExt;

#[derive(Subcommand)]
pub(crate) enum ApproverCmd {
    /// Show the configured approver and whether it is trusted.
    Show,
    /// Turn the approver on. Without --system it is protected by a password.
    Enable {
        /// Capabilities it may answer, comma-separated, for example screenshot,open-url.
        #[arg(long, value_delimiter = ',', required = true)]
        capabilities: Vec<String>,
        /// Seconds before it counts as abstaining.
        #[arg(long, default_value_t = 60)]
        timeout: u64,
        /// Write the root-owned system file (run with sudo).
        #[arg(long)]
        system: bool,
        /// With --system: the user it applies to (default: the user who ran sudo).
        #[arg(long)]
        user: Option<String>,
        /// The command, with an absolute path, after --.
        #[arg(last = true, required = true)]
        command: Vec<String>,
    },
    /// Turn the approver off.
    Disable {
        #[arg(long)]
        system: bool,
        #[arg(long)]
        user: Option<String>,
    },
    /// Send a sample request to the approver and print its answer.
    Test {
        /// The capability of the sample request.
        #[arg(long, default_value = "screenshot")]
        capability: String,
    },
}

pub(crate) const APPROVER_HELP: &str =
    "The agent approver is a command on the host, usually a reviewing agent,
that answers host requests (paddock-host) in your place. It reads one request
as JSON on stdin:
  {\"api_version\":\"paddock.approval-request/v1\",\"request\":{id, harness, project,
   capability, summary, detail, reason, session_allowed, ...}}
and prints one JSON line on stdout:
  {\"answer\":\"once|session|deny|abstain\",\"note\":\"why\"}
abstain, a crash, other output or a timeout pass the request to you. The fixed
rules still hold: exec is never allowed in advance, session covers only the
identical command, and protected paths are refused before anybody is asked.

Turning it on needs one of:
  sudo paddock approver enable --system --capabilities screenshot,open-url -- /usr/local/bin/review
      writes /etc/paddock/approver.d/USER.toml, owned by root. Processes of your
      own user cannot change it.
  paddock approver enable --capabilities screenshot,open-url -- /usr/local/bin/review
      signs ~/.config/paddock/approver.toml with an application password. paddock
      asks for the password when a sandbox starts; Enter skips the approver for
      that run. A changed file or a wrong password turns it off.

Every answer of the approver is in the audit log (paddock log) and in
~/.local/state/paddock/approver-decisions.jsonl with its note.";

#[derive(Subcommand)]
pub(crate) enum ApprovalsCmd {
    /// Answer one request: once, session (same request again in this sandbox),
    /// deny, or deny-session (same request again in this sandbox is denied
    /// without a card). Answer a question from /dev/paddock/ask with --text
    /// TEXT, or dismiss it with deny.
    Answer {
        id: String,
        #[arg(required_unless_present_any = ["text", "connect"])]
        answer: Option<String>,
        /// The answer to a question (/dev/paddock/ask), or with deny and
        /// deny-session a note for the agent (one line, at most 300
        /// characters). It goes to the agent. `-` reads it from standard
        /// input, so it never shows in the process list or the shell history.
        #[arg(long, allow_hyphen_values = true)]
        text: Option<String>,
        /// A connect question of the firewall of paddock's macOS guest:
        /// allow this connection.
        #[arg(long, group = "connect", conflicts_with_all = ["answer", "text"])]
        allow_once: bool,
        /// Always allow the site and its subdomains.
        #[arg(long, group = "connect", conflicts_with_all = ["answer", "text"])]
        allow_site: bool,
        /// Allow all hosts for ten minutes.
        #[arg(long, group = "connect", conflicts_with_all = ["answer", "text"])]
        allow_everything: bool,
        /// Allow this app, host and port for the rest of the session.
        #[arg(long, group = "connect", conflicts_with_all = ["answer", "text"])]
        allow_session: bool,
        /// Allow this app, host and port from now on: a rule in the rules
        /// file of the profile (paddock app rules NAME list).
        #[arg(long, group = "connect", conflicts_with_all = ["answer", "text"])]
        allow_always: bool,
        /// Deny this connection.
        #[arg(long, group = "connect", conflicts_with_all = ["answer", "text"])]
        deny: bool,
        /// Deny this app, host and port from now on: a rule in the rules file.
        #[arg(long, group = "connect", conflicts_with_all = ["answer", "text"])]
        deny_always: bool,
    },
}

pub(crate) const APPROVALS_HELP: &str =
    "A sandbox asks the host for help with paddock-host (screenshots, one host
command, one host file, the clipboard, a URL). Requests in \"ask\" mode wait
here, in the bar panel and in a desktop notification, until you answer one of
them. The first answer wins.

Modes come only from [host] in ~/.config/paddock/config.toml:
  [host]
  screenshot = \"ask\"       # deny | ask | allow
  exec = \"ask\"             # deny | ask (allow is read as ask)
  fetch = \"ask\"
  clipboard-read = \"ask\"
  clipboard-write = \"ask\"
  open-url = \"ask\"
  notify = \"allow\"
  wait = 120                 # seconds before a request times out

Questions from /dev/paddock/ask ([devices] ask = true) wait here too. Answer
one with --text: the text goes to the agent, and nothing runs on the host.
--text - reads the answer from standard input, so it never shows in the
process list or the shell history. deny dismisses it.

A denial can carry a note for the agent: deny --text TEXT (or --text -). The
agent gets it in the answer, so it knows why and does not ask again. The note
is one line of at most 300 characters. The audit log keeps only its length.

deny-session denies the request and every same request of that sandbox for
the rest of its session, at once and without a new card, with the same note.
\"The same request\" is what \"session\" (allow for this session) covers. It
never covers a child or a parent sandbox.

A connect question of the firewall of paddock's macOS guest (a desktop app
in network mode ask wants to reach a host) takes one of --allow-once,
--allow-session, --allow-always, --deny or --deny-always. The marker shows
the same question as a panel next to the app's window; the first answer
wins. --allow-always and --deny-always write a rule into the rules file of
the app (paddock app rules NAME list).

Examples:
  paddock approvals
  paddock approvals answer r-1793321234-1a2b3c4d once
  paddock approvals answer r-1793321234-1a2b3c4d deny --text \"do not ask for host exec\"
  paddock approvals answer r-1793321234-1a2b3c4d --allow-always
  paddock approvals answer r-1793321234-1a2b3c4d deny-session --text -
  paddock approvals answer r-1793321234-9f8e7d6c --text -
  paddock approvals --json";

pub(crate) fn approver_cmd(env: &HostEnv, cmd: ApproverCmd) -> Result<i32> {
    use paddock::approver::{self as ap, HookConfig};
    let target_user = |user: Option<String>| -> Result<String> {
        let u = user
            .or_else(|| std::env::var("SUDO_USER").ok())
            .unwrap_or_else(|| env.user.clone());
        if u.is_empty()
            || !u
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        {
            bail!("invalid user name");
        }
        Ok(u)
    };
    match cmd {
        ApproverCmd::Show => {
            let user = target_user(None)?;
            let sys = ap::system_file(&user);
            match ap::load_system(&user) {
                Ok(Some(h)) => println!(
                    "system: {} (trusted)\n  command: {}\n  capabilities: {}",
                    sys.display(),
                    h.command.join(" "),
                    h.capabilities.join(", ")
                ),
                Ok(None) => println!("system: none ({})", sys.display()),
                Err(e) => println!("system: {} is ignored: {e:#}", sys.display()),
            }
            match ap::load_signed(env)? {
                Some(s) => println!(
                    "password: {} (checked with the password when a sandbox starts)\n  command: {}\n  capabilities: {}",
                    ap::user_file(env).display(),
                    s.hook.command.join(" "),
                    s.hook.capabilities.join(", ")
                ),
                None => println!("password: none"),
            }
            println!("The system file wins when both exist.");
            Ok(0)
        }
        ApproverCmd::Enable {
            capabilities,
            timeout,
            system,
            user,
            command,
        } => {
            let hook = HookConfig {
                command,
                capabilities,
                timeout,
            };
            hook.validate()?;
            if system {
                if unsafe { libc::geteuid() } != 0 {
                    bail!("--system writes a root-owned file: run it with sudo");
                }
                let user = target_user(user)?;
                let f = ap::system_file(&user);
                for d in [
                    std::path::Path::new("/etc/paddock"),
                    std::path::Path::new(ap::SYSTEM_DIR),
                ] {
                    std::fs::create_dir_all(d)?;
                    std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o755))?;
                }
                std::fs::write(&f, toml::to_string(&hook)?)?;
                std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644))?;
                ap::root_owned_chain(&f)?;
                println!(
                    "Wrote {}. It applies to {user} from the next sandbox start.",
                    f.display()
                );
            } else {
                let p1 = rpassword::prompt_password(
                    "New password for the agent approver (at least 8 characters): ",
                )?;
                if p1.chars().count() < 8 {
                    bail!("the password needs at least 8 characters");
                }
                if rpassword::prompt_password("Again: ")? != p1 {
                    bail!("the passwords differ");
                }
                let signed = ap::sign(&hook, &p1)?;
                let f = ap::user_file(env);
                std::fs::create_dir_all(f.parent().unwrap())?;
                std::fs::write(&f, toml::to_string(&signed)?)?;
                std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o600))?;
                println!(
                    "Wrote {}. paddock asks for this password when a sandbox starts.",
                    f.display()
                );
            }
            Ok(0)
        }
        ApproverCmd::Disable { system, user } => {
            let f = if system {
                ap::system_file(&target_user(user)?)
            } else {
                ap::user_file(env)
            };
            match std::fs::remove_file(&f) {
                Ok(()) => println!("Removed {}", f.display()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    println!("{} does not exist", f.display())
                }
                Err(e) if system => bail!("cannot remove {}: {e}; run it with sudo", f.display()),
                Err(e) => return Err(e.into()),
            }
            Ok(0)
        }
        ApproverCmd::Test { capability } => {
            let Some((hook, _)) = ap::resolve(env, true)? else {
                bail!("no trusted approver is configured")
            };
            let p = paddock::broker::Pending {
                id: "r-test".into(),
                sandbox: "test".into(),
                harness: "test".into(),
                project: env.home.join("example-project"),
                capability: capability.clone(),
                summary: format!("sample {capability} request"),
                detail: format!("A sample {capability} request from paddock approver test"),
                reason: "testing the approver".into(),
                created: paddock::state::now(),
                expires: paddock::state::now() + 60,
                session_allowed: true,
                choices: Vec::new(),
                started_by: vec![],
                connect: None,
            };
            let (a, note) = ap::run_hook(&hook, &p);
            match a {
                Some(a) => println!("answer: {a:?}  note: {note}"),
                None => println!("abstained or failed, so the user would decide: {note}"),
            }
            Ok(0)
        }
    }
}

pub(crate) fn run(env: &HostEnv, cmd: ApproverCmd) -> Result<i32> {
    approver_cmd(env, cmd)
}

pub(crate) fn approvals(env: &HostEnv, json: bool, cmd: Option<ApprovalsCmd>) -> Result<i32> {
    use paddock::broker;
    if let Some(ApprovalsCmd::Answer {
        id,
        answer,
        text,
        allow_once,
        allow_site,
        allow_everything,
        allow_session,
        allow_always,
        deny,
        deny_always,
    }) = cmd
    {
        let flags = [
            (allow_once, broker::ConnectAnswer::AllowOnce),
            (allow_site, broker::ConnectAnswer::AllowSite),
            (allow_everything, broker::ConnectAnswer::AllowEverything),
            (allow_session, broker::ConnectAnswer::AllowSession),
            (allow_always, broker::ConnectAnswer::AllowAlways),
            (deny, broker::ConnectAnswer::Deny),
            (deny_always, broker::ConnectAnswer::DenyAlways),
        ];
        // The card says whether its session applies an allow; read it
        // before the answer hides it.
        let card = broker::pending(env).into_iter().find(|p| p.id == id);
        let note = |a: broker::ConnectAnswer| {
            if let Some(n) = card
                .as_ref()
                .and_then(|c| paddock::macvm::approvals::cli_answer_note(c, a))
            {
                println!("{n}");
            }
        };
        if let Some((_, a)) = flags.iter().find(|(set, _)| *set) {
            broker::answer_connect_cli(env, &id, *a)?;
            println!("{}", paddock::macvm::text::connect_answered(&id, a.word()));
            note(*a);
            return Ok(0);
        }
        match (answer, text) {
            (Some(a), Some(t)) => {
                let a: broker::Answer = a.parse()?;
                if !a.denies() {
                    bail!("--text goes with deny or deny-session, or alone for a question");
                }
                let t = if t == "-" {
                    let mut s = String::new();
                    std::io::Read::read_to_string(
                        &mut std::io::Read::take(std::io::stdin(), 64 * 1024),
                        &mut s,
                    )?;
                    s
                } else {
                    t
                };
                let note = broker::clean_note(&t);
                broker::answer_with_note_cli(env, &id, a, note.as_deref())?;
                match note {
                    Some(n) => println!(
                        "Recorded selection for {id}: {} with a note of {} characters. Delivery and completion are not confirmed.",
                        serde_json::to_value(a)?.as_str().unwrap_or("deny"),
                        n.chars().count()
                    ),
                    None => println!("Recorded selection for {id}: deny. Delivery and completion are not confirmed."),
                }
            }
            (None, Some(t)) => {
                let t = if t == "-" {
                    let mut s = String::new();
                    std::io::Read::read_to_string(
                        &mut std::io::Read::take(std::io::stdin(), 64 * 1024),
                        &mut s,
                    )?;
                    s.trim_end_matches(['\n', '\r']).to_string()
                } else {
                    t
                };
                if t.trim().is_empty() {
                    bail!("the answer is empty");
                }
                broker::answer_reply_cli(env, &id, &t)?;
                println!("Recorded answer for {id} with {} characters. Delivery and completion are not confirmed.", t.chars().count());
            }
            (Some(a), None) => {
                let parsed: broker::Answer = a.parse()?;
                broker::answer_with_note_cli(env, &id, parsed, None)?;
                println!(
                    "Recorded selection for {id}: {a}. Delivery and completion are not confirmed."
                );
                if card
                    .as_ref()
                    .is_some_and(|c| c.capability == broker::CONNECT_CAPABILITY)
                {
                    if let Ok(connect) = broker::ConnectAnswer::from_answer(parsed) {
                        note(connect);
                    }
                }
            }
            (None, None) => bail!("give an answer, or --text TEXT for a question"),
        }
        return Ok(0);
    }
    let report = broker::pending_report(env)?;
    let list = report.pending;
    let complete = report.diagnostics.is_empty();
    if json {
        println!(
            "{}",
            approvals_json(
                &list,
                &report.diagnostics,
                &report.outcomes,
                &report.unresolved,
                paddock::state::now()
            )
        );
    } else if list.is_empty() && complete {
        println!("No requests wait for an answer.");
    } else {
        for p in list {
            // Files of an older paddock can hold raw sandbox text.
            let p = p.cleaned();
            println!("{}  {} in {}", p.id, p.harness, p.project.display());
            if let Some(c) = paddock::spawn::chain_text(&p.started_by) {
                println!("    {c}");
            }
            for l in p.detail.lines() {
                println!("    {l}");
            }
            if !p.reason.is_empty() {
                println!("    reason given by the agent: {}", p.reason);
            }
            if p.capability != broker::ASK_CAPABILITY && !p.choices.is_empty() {
                println!("    choices: {}", p.choices.join(" | "));
                println!(
                    "    answer: paddock approvals answer {} --text {}, or deny\n",
                    p.id,
                    p.choices.join("|")
                );
                continue;
            }
            if p.capability == broker::ASK_CAPABILITY {
                if !p.choices.is_empty() {
                    println!("    choices: {}", p.choices.join(" | "));
                }
                println!(
                            "    answer: paddock approvals answer {} --text -   (type the answer, then Ctrl-D), or deny\n",
                            p.id
                        );
                continue;
            }
            println!(
                "    answer: paddock approvals answer {} once|{}deny{}  [--text NOTE]\n",
                p.id,
                if p.session_allowed { "session|" } else { "" },
                if p.session_allowed {
                    "|deny-session"
                } else {
                    ""
                }
            );
        }
    }
    if !json {
        for outcome in &report.outcomes {
            println!("Local application {}: {:?}, scope {:?}, selection {}, rule {} (not a delivery or admission confirmation).",
                outcome.id, outcome.state, outcome.scope, outcome.selected, outcome.rule);
            for affected in &outcome.affected {
                println!(
                    "    affected decision {}: {} {}:{}; settlement unconfirmed",
                    affected.id, affected.app, affected.host, affected.port
                );
            }
        }
        for diagnostic in &report.diagnostics {
            eprintln!("Incomplete approval listing: {diagnostic}");
        }
    }
    Ok(if complete { 0 } else { 2 })
}

fn approvals_json(
    list: &[paddock::broker::Pending],
    diagnostics: &[String],
    outcomes: &[paddock::broker::ConnectOutcome],
    unresolved: &[paddock::broker::ApprovalUnresolved],
    now: u64,
) -> serde_json::Value {
    let pending: Vec<_> = list
        .iter()
        .map(|p| {
            let mut value = serde_json::to_value(p.cleaned()).expect("pending request serializes");
            let item = value.as_object_mut().expect("pending request is an object");
            let since = p.connect.as_ref().map_or(p.created, |c| c.asked_at);
            item.insert("age_s".into(), now.saturating_sub(since).into());
            let process = p.connect.as_ref().and_then(|c| c.path.as_ref()).cloned();
            item.insert("process".into(), serde_json::json!(process));
            value
        })
        .collect();
    serde_json::json!({ "v": 1, "api_version": "paddock.approvals/v1", "pending": pending,
        "complete": diagnostics.is_empty(), "diagnostics": diagnostics,
        "local_outcomes": outcomes, "unresolved": unresolved })
}

pub(crate) fn log(env: &HostEnv, json: bool, lines: usize) -> Result<i32> {
    let text = std::fs::read_to_string(paddock::broker::audit_file(env)).unwrap_or_default();
    let all: Vec<&str> = text.lines().collect();
    for l in &all[all.len().saturating_sub(lines)..] {
        if json {
            println!("{l}");
        } else if let Ok(v) = serde_json::from_str::<serde_json::Value>(l) {
            println!(
                "{}  {:<8} {:<15} {:<26} {}",
                v["time"],
                v["harness"].as_str().unwrap_or(""),
                v["capability"].as_str().unwrap_or(""),
                v["decision"].as_str().unwrap_or(""),
                v["detail"]
                    .as_str()
                    .unwrap_or("")
                    .lines()
                    .last()
                    .unwrap_or("")
            );
        }
    }
    Ok(0)
}

#[cfg(test)]
mod launcher_approval_tests {
    #[test]
    fn json_listing_returns_an_error_for_a_malformed_card() {
        let t = tempfile::tempdir().unwrap();
        let mut vars = std::collections::BTreeMap::new();
        vars.insert(
            "XDG_RUNTIME_DIR".to_string(),
            t.path().join("run").display().to_string(),
        );
        let env = paddock::env::HostEnv::from_parts(
            t.path().join("home"),
            "test".into(),
            1000,
            1000,
            vars,
        );
        let dir = paddock::broker::approvals_dir(&env);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("bad.json"), "{broken").unwrap();
        assert_eq!(super::approvals(&env, true, None).unwrap(), 2);
    }

    #[test]
    fn json_has_version_process_and_saturating_age() {
        let p = paddock::broker::Pending {
            created: 20,
            connect: Some(paddock::broker::Connect {
                asked_at: 30,
                path: Some("/Applications/App".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let encoded = super::approvals_json(&[p], &[], &[], &[], 25).to_string();
        let v: serde_json::Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(v["v"], 1);
        assert_eq!(v["pending"][0]["age_s"], 0);
        assert_eq!(v["pending"][0]["process"], "/Applications/App");
    }

    #[test]
    fn json_exposes_local_failure_and_unresolved_selection() {
        use paddock::broker::{
            ApplicationScope, ApprovalUnresolved, ConnectOutcome, LocalApplication, Pending,
        };
        let request = Pending {
            id: "r-example".into(),
            capability: "connect".into(),
            ..Default::default()
        };
        let outcome = ConnectOutcome {
            id: request.id.clone(),
            request: request.clone(),
            selected: "deny-always".into(),
            local_allowed: None,
            rule: "stale".into(),
            by: "cli".into(),
            state: LocalApplication::Failed,
            scope: ApplicationScope::Unknown,
            reason: Some("application failed".into()),
            recorded_at: Some(1),
            affected: vec![],
        };
        let unresolved = ApprovalUnresolved {
            id: request.id.clone(),
            reason: "application failed".into(),
            selected: Some("deny-always".into()),
            request: Some(request),
        };
        let json = super::approvals_json(
            &[],
            &["application failed".into()],
            &[outcome],
            &[unresolved],
            2,
        );
        assert_eq!(json["complete"], false);
        assert_eq!(json["local_outcomes"][0]["state"], "failed");
        assert!(json["local_outcomes"][0]["local_allowed"].is_null());
        assert_eq!(json["unresolved"][0]["selected"], "deny-always");
        assert_eq!(json["unresolved"][0]["request"]["id"], "r-example");
    }
}
