//! The grants of the running session next to the saved ones, for the
//! launcher's privacy page: `paddock app clipboard NAME --session` (read,
//! or change the session only) and `paddock app share NAME keep PATH`
//! (a folder that only this session shares becomes a grant).

use super::*;

/// `paddock app clipboard NAME [--session] --json`: the saved directions
/// and, while the app runs, the ones of the session. `to_app` and
/// `from_app` repeat the saved ones for older readers.
#[derive(Debug, Serialize)]
pub struct ClipboardState {
    pub v: u32,
    pub app: String,
    pub running: bool,
    pub profile: Clipboard,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<Clipboard>,
    pub to_app: bool,
    pub from_app: bool,
    /// Why `session` is missing while the app runs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

/// The clipboard of the profile and of its session.
pub fn clipboard_state(env: &HostEnv, name: &str) -> Result<ClipboardState> {
    let t = Target::of(env, name)?;
    let g = grants::load(&t.profile)?;
    let mut warning = None;
    let session = match &t.running {
        None => None,
        Some(s) => match ask(&s.control, &Op::List) {
            Ok(a) => a.clipboard,
            Err(e) => {
                warning = Some(text::grant_list_from_record(&format!("{e:#}")));
                None
            }
        },
    };
    Ok(ClipboardState {
        v: 1,
        app: name.to_string(),
        running: t.running.is_some(),
        profile: g.clipboard,
        session,
        to_app: g.clipboard.to_app,
        from_app: g.clipboard.from_app,
        warning,
    })
}

/// `paddock app clipboard NAME --session [--json]`: print both.
pub fn clipboard_show(env: &HostEnv, name: &str, json: bool) -> Result<i32> {
    let s = clipboard_state(env, name)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&s)?);
        return Ok(0);
    }
    if let Some(w) = &s.warning {
        eprintln!("{w}");
    }
    println!("{}", text::grant_clipboard_both(name, s.profile, s.session));
    Ok(0)
}

/// The answer of `--session` with a direction.
#[derive(Debug, Serialize)]
pub struct SessionOutcome {
    pub v: u32,
    pub ok: bool,
    pub applies: Applies,
    pub profile: Clipboard,
    pub session: Clipboard,
}

/// `paddock app clipboard NAME --session --to-app on|off --from-app
/// on|off`: the running session only. The profile keeps its directions,
/// and the next start uses them.
pub fn clipboard_session(
    env: &HostEnv,
    name: &str,
    to_app: Option<bool>,
    from_app: Option<bool>,
    json: bool,
) -> Result<i32> {
    let t = Target::of(env, name)?;
    let Some(s) = &t.running else {
        bail!(text::grant_session_not_running(name));
    };
    let live = ask(&s.control, &Op::List)?
        .clipboard
        .context(text::grant_missing_live_setting("clipboard settings"))?;
    let new = Clipboard {
        to_app: to_app.unwrap_or(live.to_app),
        from_app: from_app.unwrap_or(live.from_app),
    };
    let a = ask(&s.control, &Op::Clipboard(new))?;
    let profile = grants::load(&t.profile)?.clipboard;
    let o = SessionOutcome {
        v: 1,
        ok: true,
        applies: a.applies,
        profile,
        session: a.clipboard.unwrap_or(new),
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&o)?);
    } else {
        println!(
            "{}",
            text::grant_clipboard_both(name, o.profile, Some(o.session))
        );
    }
    Ok(0)
}

/// `paddock app share NAME keep PATH`: a folder that the running session
/// shares, but the profile does not, becomes a grant with its mode now.
/// The session does not change.
pub fn share_keep(env: &HostEnv, name: &str, path: &Path, json: bool) -> Result<i32> {
    let t = Target::of(env, name)?;
    if t.running.is_none() {
        bail!(text::grant_keep_not_running(name));
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| env.home.clone());
    let p = named_folder(path, &cwd);
    let unprotected = crate::app::unprotected_workspaces(env, name).unwrap_or(false);
    let now = crate::state::now();
    let change = update_live(&t, |g, step| {
        if g.shares.iter().any(|f| f.path == p) {
            bail!(text::grant_already_kept(name, &p));
        }
        let Some(ls) = live_share(step, &p, false)? else {
            bail!(text::grant_not_shared(name, &p));
        };
        grants::check_folder(
            name,
            &p,
            &cwd,
            &env.home,
            &grants::paddock_dirs(env),
            unprotected,
        )?;
        g.shares.push(FolderGrant {
            path: p.clone(),
            read_only: ls.read_only,
            unprotected,
            added: now,
        });
        let c = Change::new(
            now,
            What::Folder,
            "keep",
            &p.display().to_string(),
            mode_word(ls.read_only),
            Applies::NextStart,
            Source::You,
        );
        g.changes.push(c.clone());
        Ok(c)
    })?;
    grants::event(&t.profile, &change, &env.home);
    let o = Outcome::new(change, None, Vec::new());
    print(env, &t, &o, json)
}
