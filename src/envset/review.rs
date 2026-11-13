//! The review of current users: `paddock env migrate`.

use super::*;

#[derive(Debug, Clone, Serialize)]
pub struct ReviewList {
    pub api_version: &'static str,
    pub mode: Mode,
    /// Whether the review is open.
    pub open: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub migrated: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forced_from: Option<String>,
    /// "Until now, 73 variables of this computer passed to every agent."
    pub summary: String,
    /// The variables that pass unseen in inherit mode. `suggested`: on
    /// by default.
    pub items: Vec<ReviewItem>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReviewItem {
    pub name: String,
    pub suggested: bool,
    /// Already in `[env] pass`.
    pub listed: bool,
}

/// The host variables that pass unseen in `inherit` mode, for the review.
pub fn review(env: &HostEnv, project: &Path) -> Result<ReviewList> {
    let user = Manifest::load_user(env)?;
    let r = resolve(env, &user);
    let mut v = view(&Input {
        env,
        project,
        profile: None,
        cli: Some(true),
        values: false,
    })?;
    let host = v.groups.pop().unwrap_or_default();
    let items: Vec<ReviewItem> = host
        .names
        .iter()
        .map(|n| ReviewItem {
            name: n.clone(),
            suggested: suggested(n),
            listed: user.env.pass.contains(n),
        })
        .collect();
    let place = if cfg!(target_os = "macos") {
        "this Mac"
    } else {
        "this computer"
    };
    Ok(ReviewList {
        api_version: REVIEW_API,
        mode: r.mode,
        open: r.mode == Mode::Inherit && r.review.is_some(),
        migrated: r.review.as_ref().map(|x| x.migrated.clone()),
        forced_from: r.review.as_ref().map(|x| x.forced_from.clone()),
        summary: format!(
            "Until now, {} of {place} passed to every agent.",
            plural(items.len(), "variable", "variables")
        ),
        items,
    })
}

/// "Pass Only These": the list goes into `[env] pass`, and the mode
/// becomes `listed`. The review closes.
pub fn finish_review(env: &HostEnv, pass: &[String]) -> Result<()> {
    for n in pass {
        if !valid_name(n) {
            bail!("'{n}' is not an environment variable name");
        }
        if let Some(r) = refusal(n, env.var(n)) {
            return Err(r.into());
        }
    }
    edit(env, |t| {
        let arr = t
            .entry("pass")
            .or_insert(toml_edit::value(toml_edit::Array::new()))
            .as_array_mut()
            .context("[env] pass is not a list")?;
        for n in pass {
            if !arr.iter().any(|v| v.as_str() == Some(n)) {
                arr.push(n.as_str());
            }
        }
        t.insert("mode", toml_edit::value("listed"));
        t.remove("migrated");
        t.remove("inherit");
        Ok(())
    })
}
