//! Groups: the four groups of variables of an agent in a project (`view`).

use super::*;

/// The basics that every sandbox gets from the host (backend::base_env).
pub const BASICS: &[&str] = &[
    "TERM",
    "COLORTERM",
    "LANG",
    "LANGUAGE",
    "TZ",
    "NO_COLOR",
    "FORCE_COLOR",
    "TERM_PROGRAM",
];

pub fn is_basic(name: &str) -> bool {
    BASICS.contains(&name) || name.starts_with("LC_")
}

/// Tool variables that a screen suggests to pass, when the host has them.
pub const SUGGESTED: &[&str] = &[
    "JAVA_HOME",
    "JDK_HOME",
    "ANDROID_HOME",
    "ANDROID_SDK_ROOT",
    "GOFLAGS",
    "GOPRIVATE",
    "GOPROXY",
    "GONOSUMDB",
    "NODE_OPTIONS",
    "NODE_EXTRA_CA_CERTS",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "REQUESTS_CA_BUNDLE",
    "CURL_CA_BUNDLE",
    "EDITOR",
    "VISUAL",
    "PAGER",
    "RUST_LOG",
    "RUST_BACKTRACE",
    "MAVEN_OPTS",
    "GRADLE_OPTS",
    "CC",
    "CXX",
    "PKG_CONFIG_PATH",
];

/// Whether a host variable is a tool variable that a screen suggests.
pub fn suggested(name: &str) -> bool {
    SUGGESTED.contains(&name)
        || (name.ends_with("_HOME")
            && name != "HOME"
            && !policy::CACHE_ENV.contains(&name)
            && !name.starts_with("XDG_"))
}

/// One variable in a group.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub name: String,
    /// agent: "basics", "proxy", "profile", "login". yours: "pass" or
    /// "set" is in `kind`. A run record: the origin of the spec.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub origin: String,
    /// yours: "pass" (a host value by name) or "set" (a fixed value).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// yours: "user", "agent", "project" (`[env.project]`), "file" (the
    /// project's paddock.toml).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Whether it reaches the agent at the next start.
    pub passes: bool,
    /// The value, with `--values`. Never of a key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// A login variable of the profile: its value is handed to the agent.
    /// The screens offer "Keep on this computer" (`action`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub handed_over: bool,
    /// What a screen offers for the row: "keep_on_host" (add it as a key).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    /// Why it does not pass: "not_set" (the host has no value), "held".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// One key of `[credentials]`, as the screens show it. Never a value.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyItem {
    pub name: String,
    /// The variable in the sandbox.
    pub var: String,
    pub via: String,
    /// "kept_on_host", "handed_over", "agent", "not_available".
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    /// "keychain", "env VAR", "command PROG", "ssh agent", ...
    pub store: String,
    pub ready: bool,
    /// Whether this agent in this project gets it without a question.
    pub granted: bool,
    /// "any" (every project), "project" (`projects`), "standing" (an
    /// answer "Always in this project"), "ask" (one question at the next
    /// start), "agent" (not for this agent), "none" (no agent chosen).
    pub grant: String,
    /// `harnesses`: empty means every agent.
    pub agents: Vec<String>,
    /// `projects`: `*` means any project.
    pub projects: Vec<String>,
    /// Uses for this agent and project (all, without them): requests for
    /// a gateway key, starts for a handed-over key.
    pub uses: u64,
    pub unit: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used: Option<u64>,
}

/// A name that does not pass, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Held {
    pub name: String,
    /// "never" (no switch), "key" (only as a key), "hidden" (`[env]
    /// hide`), "account" (the login folder of the harness).
    pub rule: String,
    /// never: "socket", "sandbox", "loader", "history". key: "secret",
    /// "cloud", "registered".
    pub reason: String,
    /// The sentence of paddock, with the safe way.
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
    /// "agent", "yours", "keys", "host".
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<Item>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<KeyItem>,
    /// host: the number of variables in the group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<usize>,
    /// host: whether they pass (`inherit`) or stay (`listed`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passes: Option<bool>,
    /// host: tool variables that a screen suggests.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suggested: Vec<String>,
    /// host: the other names, without values (unless `--values`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub names: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    /// Variables that pass: groups agent and yours (and host in inherit).
    pub variables: usize,
    /// Keys that this agent gets without a question.
    pub keys: usize,
    /// Host variables that stay on this computer, held names included.
    pub stay: usize,
    pub held: usize,
}

/// `paddock env show --json`.
#[derive(Debug, Clone, Serialize)]
pub struct View {
    pub api_version: &'static str,
    /// None: all agents.
    pub agent: Option<String>,
    pub project: PathBuf,
    pub mode: Mode,
    pub mode_origin: &'static str,
    /// The review of current users, while it is open.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review: Option<Review>,
    /// The first line of each screen.
    pub summary: String,
    pub counts: Counts,
    pub groups: Vec<Group>,
    pub held: Vec<Held>,
    /// "Applies at the next start."
    pub note: &'static str,
    /// Whether a keychain tool is there, for the "store not there" state.
    pub keychain: bool,
}

/// The title of the host group.
pub fn host_title() -> &'static str {
    if cfg!(target_os = "macos") {
        "From this Mac"
    } else {
        "From this computer"
    }
}

pub(super) fn group(id: &str, title: String) -> Group {
    Group {
        id: id.into(),
        title,
        ..Default::default()
    }
}

/// The scopes of `[env]` that apply to an agent in a project, in their
/// order: the agent, then the project.
pub fn scopes<'a>(
    env: &HostEnv,
    m: &'a Manifest,
    agent: Option<&str>,
    project: &Path,
) -> Vec<(&'static str, String, &'a ScopeCfg)> {
    let mut v = Vec::new();
    if let Some(a) = agent {
        if let Some(s) = m.env.agent.get(a) {
            v.push(("agent", a.to_string(), s));
        }
    }
    let here = std::fs::canonicalize(project).unwrap_or_else(|_| project.to_path_buf());
    for (k, s) in &m.env.project {
        let p = env.expand(k, &env.home);
        let p = std::fs::canonicalize(&p).unwrap_or(p);
        if p == here {
            v.push(("project", k.clone(), s));
        }
    }
    v
}

/// The input of `view`.
pub struct Input<'a> {
    pub env: &'a HostEnv,
    pub project: &'a Path,
    /// None: all agents.
    pub profile: Option<&'a crate::profile::Profile>,
    /// `--env` of the command line.
    pub cli: Option<bool>,
    /// Add the values of plain variables.
    pub values: bool,
}

/// The variables of an agent in a project, in the four groups.
pub fn view(i: &Input) -> Result<View> {
    let env = i.env;
    let loaded = Manifest::load(env, i.project)?;
    let user = Manifest::load_user(env)?;
    let m = &loaded.merged;
    let resolved = run_mode(env, m, i.cli);
    let agent = i.profile.map(|p| p.name.clone());
    let creds = crate::credentials::load(env)?;
    let key_vars: BTreeSet<String> = creds
        .iter()
        .flat_map(|(n, c)| [c.host_var(n).to_string(), c.sandbox_var(n).to_string()])
        .collect();
    let value = |k: &str| {
        if i.values {
            env.var(k).map(str::to_string)
        } else {
            None
        }
    };
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut held: Vec<Held> = Vec::new();
    let hold = |held: &mut Vec<Held>, r: Refusal| {
        if !held.iter().any(|h| h.name == r.name) {
            held.push(Held {
                name: r.name,
                rule: r.rule.into(),
                reason: r.reason.into(),
                text: r.sentence,
            });
        }
    };

    // 1. Needed by the agent.
    let mut g1 = group(
        "agent",
        match &agent {
            Some(a) => format!("Needed by {a}"),
            None => "Needed by the agent".into(),
        },
    );
    for (k, _) in env.vars.iter().filter(|(k, _)| is_basic(k)) {
        g1.items.push(Item {
            name: k.clone(),
            origin: "basics".into(),
            passes: true,
            value: value(k),
            ..Default::default()
        });
        seen.insert(k.clone());
    }
    for k in crate::proxy::PROXY_VARS {
        if env.var(k).is_some_and(|v| !v.is_empty()) && seen.insert(k.to_string()) {
            g1.items.push(Item {
                name: k.to_string(),
                origin: "proxy".into(),
                passes: true,
                value: value(k),
                ..Default::default()
            });
        }
    }
    let login: Vec<String> = i
        .profile
        .map(|p| p.state.env_pass.clone())
        .unwrap_or_default();
    if let Some(p) = i.profile {
        for (k, v) in &p.launch.env {
            seen.insert(k.clone());
            g1.items.push(Item {
                name: k.clone(),
                origin: "profile".into(),
                passes: true,
                value: i.values.then(|| v.clone()),
                ..Default::default()
            });
        }
        for k in &login {
            if env.var(k).is_none() || key_vars.contains(k) || !seen.insert(k.clone()) {
                continue;
            }
            g1.items.push(Item {
                name: k.clone(),
                origin: "login".into(),
                passes: true,
                handed_over: policy::looks_secret(k),
                action: policy::looks_secret(k).then(|| "keep_on_host".to_string()),
                ..Default::default()
            });
        }
    }

    // 2. Your variables.
    let mut g2 = group("yours", "Your variables".into());
    let mut named: BTreeSet<String> = login.iter().cloned().collect();
    let file = loaded.project.as_ref().map(|(_, p)| p);
    let mut lists: Vec<(String, &Vec<String>, &BTreeMap<String, String>)> =
        vec![("user".into(), &user.env.pass, &user.env.set)];
    if let Some(f) = file {
        lists.push(("file".into(), &f.env.pass, &f.env.set));
    }
    for (kind, _, s) in scopes(env, &user, agent.as_deref(), i.project) {
        lists.push((kind.into(), &s.pass, &s.set));
    }
    for (scope, pass, set) in lists {
        for k in pass {
            named.insert(k.clone());
            let refused = policy::never_pass(k, env.var(k)).map(|w| (w, k));
            let (passes, reason) = if let Some((w, k)) = refused {
                hold(
                    &mut held,
                    Refusal {
                        name: k.clone(),
                        rule: "never",
                        reason: w.name(),
                        sentence: policy::never_sentence(k, w),
                    },
                );
                (false, Some("held"))
            } else if key_vars.contains(k) {
                (false, Some("key"))
            } else if env.var(k).is_none() {
                (false, Some("not_set"))
            } else {
                (true, None)
            };
            g2.items.retain(|it| it.name != *k);
            g2.items.push(Item {
                name: k.clone(),
                kind: Some("pass".into()),
                scope: Some(scope.clone()),
                passes,
                value: if passes { value(k) } else { None },
                reason: reason.map(str::to_string),
                ..Default::default()
            });
            seen.insert(k.clone());
        }
        for (k, v) in set {
            g2.items.retain(|it| it.name != *k);
            g2.items.push(Item {
                name: k.clone(),
                kind: Some("set".into()),
                scope: Some(scope.clone()),
                passes: true,
                value: i.values.then(|| v.clone()),
                ..Default::default()
            });
            seen.insert(k.clone());
        }
    }

    // 3. Keys.
    let mut g3 = group("keys", "Keys".into());
    let uses = crate::credentials::keys::uses(env);
    let canon = std::fs::canonicalize(i.project).unwrap_or_else(|_| i.project.to_path_buf());
    for (name, c) in &creds {
        let for_agent = agent.as_deref().is_none_or(|a| c.applies_to(a));
        let grant = match agent.as_deref() {
            None => "none".to_string(),
            Some(_) if !for_agent => "agent".to_string(),
            Some(a) => crate::credentials::keys::grant(env, name, c, a, &canon)
                .name()
                .to_string(),
        };
        let granted = matches!(grant.as_str(), "any" | "project" | "standing");
        let (n, last) =
            crate::credentials::keys::count(&uses, name, agent.as_deref(), Some(&canon));
        seen.insert(c.host_var(name).to_string());
        seen.insert(c.sandbox_var(name).to_string());
        g3.keys.push(KeyItem {
            name: name.clone(),
            var: c.sandbox_var(name).to_string(),
            via: c.via.name().into(),
            state: crate::credentials::keys::state(c).into(),
            preset: c.preset.clone(),
            store: crate::credentials::keys::store_name(name, c),
            ready: crate::credentials::source_ready(env, name, c),
            granted,
            grant,
            agents: c.harnesses.clone(),
            projects: c.projects.clone(),
            uses: n,
            unit: crate::credentials::keys::unit(c).into(),
            last_used: last,
        });
    }

    // 4. From this computer, and the held names.
    let account = i
        .profile
        .and_then(|p| p.state.account.as_ref())
        .map(|a| a.env.clone());
    let mut g4 = group("host", host_title().into());
    let mut host_count = 0;
    for (k, v) in &env.vars {
        if seen.contains(k) {
            continue;
        }
        if let Some(w) = policy::never_pass(k, Some(v)) {
            if w != Never::Sandbox {
                hold(
                    &mut held,
                    Refusal {
                        name: k.clone(),
                        rule: "never",
                        reason: w.name(),
                        sentence: policy::never_sentence(k, w),
                    },
                );
            }
            continue;
        }
        if account.as_deref() == Some(k.as_str()) {
            held.push(Held {
                name: k.clone(),
                rule: "account".into(),
                reason: "login folder".into(),
                text: format!("{k} stays on this computer: it names the login folder of the agent, which paddock gives the agent itself."),
            });
            continue;
        }
        if m.env.hide.contains(k) {
            held.push(Held {
                name: k.clone(),
                rule: "hidden".into(),
                reason: "env hide".into(),
                text: format!("{k} stays on this computer: [env] hide names it. To pass it again: paddock env unhide {k}"),
            });
            continue;
        }
        if !named.contains(k) {
            if let Some(r) = refusal(k, Some(v)) {
                hold(&mut held, r);
                continue;
            }
        }
        if policy::CACHE_ENV.contains(&k.as_str()) {
            continue;
        }
        host_count += 1;
        if suggested(k) {
            g4.suggested.push(k.clone());
        } else {
            g4.names.push(k.clone());
        }
    }
    let inherit = resolved.mode == Mode::Inherit;
    g4.count = Some(host_count);
    g4.passes = Some(inherit);
    if inherit {
        // In inherit mode the screens show them as passing, with values
        // only on request.
        g4.names.append(&mut g4.suggested);
        g4.names.sort();
    }
    held.sort_by(|a, b| a.name.cmp(&b.name));

    let variables = g1.items.iter().filter(|i| i.passes).count()
        + g2.items.iter().filter(|i| i.passes).count()
        + if inherit { host_count } else { 0 };
    let keys = g3.keys.iter().filter(|k| k.granted).count();
    let stay = held.len() + if inherit { 0 } else { host_count };
    let counts = Counts {
        variables,
        keys,
        stay,
        held: held.len(),
    };
    let summary = summary(agent.as_deref(), &counts);
    Ok(View {
        api_version: API,
        agent,
        project: canon,
        mode: resolved.mode,
        mode_origin: resolved.origin,
        review: resolved.review,
        summary,
        counts,
        groups: vec![g1, g2, g3, g4],
        held,
        note: NEXT_START,
        keychain: crate::credentials::keys::keychain_tool(env),
    })
}

pub(super) fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// "claude gets 14 variables and 2 keys. 61 variables stay on this computer."
pub fn summary(agent: Option<&str>, c: &Counts) -> String {
    let who = agent.unwrap_or("Each agent");
    let place = if cfg!(target_os = "macos") {
        "this Mac"
    } else {
        "this computer"
    };
    format!(
        "{who} gets {} and {}. {} on {place}.",
        plural(c.variables, "variable", "variables"),
        plural(c.keys, "key", "keys"),
        if c.stay == 1 {
            "1 variable stays".to_string()
        } else {
            format!("{} variables stay", c.stay)
        }
    )
}
