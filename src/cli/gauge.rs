use anyhow::{Context as _, Result};
use clap::Args;
use paddock::env::HostEnv;
use paddock::gauge::{self, Opts};
use std::path::PathBuf;

#[derive(Args)]
pub(crate) struct GaugeArgs {
    /// A feature folder in specs/. Default: the active feature.
    pub feature: Option<String>,
    /// The project. Default: the current folder.
    #[arg(long)]
    pub dir: Option<PathBuf>,
    /// The seed of the forecast. One seed and one history give one forecast.
    #[arg(long, default_value_t = gauge::forecast::SEED)]
    pub seed: u64,
    /// The days of history that the rates come from.
    #[arg(long, default_value_t = gauge::WINDOW_DAYS)]
    pub window: usize,
    /// Count only tasks with evidence as finished.
    #[arg(long)]
    pub strict: bool,
    /// Measure as at a past time (RFC 3339, such as 2026-11-08T09:00:00Z, or Unix seconds): what the gauge said then.
    #[arg(long)]
    pub at: Option<String>,
    #[arg(long)]
    pub json: bool,
}

pub(crate) fn run(env: &HostEnv, a: GaugeArgs) -> Result<i32> {
    // In a sandbox the loft history is out of sight: the host computes it.
    if env.var("PADDOCK") == Some("1") {
        anyhow::bail!("in a paddock sandbox, run `paddock-host gauge`: the host reads the history of tasks.md, which the sandbox cannot see");
    }
    let dir = match a.dir {
        Some(d) => d,
        None => std::env::current_dir()?,
    };
    let project =
        std::fs::canonicalize(&dir).with_context(|| format!("no folder {}", dir.display()))?;
    anyhow::ensure!(a.window >= 1, "--window is at least 1 day");
    let o = Opts {
        seed: a.seed,
        window_days: a.window,
        strict: a.strict,
        at: match a.at.as_deref() {
            None => None,
            Some(t) => Some(match t.parse::<u64>() {
                Ok(n) => n,
                Err(_) => paddock::waiting::parse_rfc3339(t)?,
            }),
        },
        ..Opts::default()
    };
    let r = gauge::gauge(env, &project, a.feature.as_deref(), &o)?;
    if a.json {
        println!("{}", serde_json::to_string_pretty(&r)?);
    } else {
        print!("{}", gauge::render::text(&r));
    }
    Ok(0)
}
