use anyhow::{bail, Context as _, Result};
use clap::Args;
use paddock::env::HostEnv;

#[derive(Args)]
pub(crate) struct TrainArgs {
    /// Live run to review, or `stats` / `feedback`.
    run: String,
    /// Run ID when using `stats` or `feedback`.
    subject: Option<String>,
    /// Advice ID when using `feedback`.
    advice: Option<String>,
    /// `adopted` or `not-adopted` when using `feedback`.
    outcome: Option<String>,
    /// Optional explanation for feedback.
    #[arg(long, default_value = "")]
    note: String,
    /// Read more of the target through the scoped peek grant.
    #[arg(long)]
    deep: bool,
    /// Harness for the trainer; defaults to the target's harness.
    #[arg(long)]
    harness: Option<String>,
}

pub(crate) fn run(env: &HostEnv, args: TrainArgs) -> Result<i32> {
    if env.var("PADDOCK") == Some("1") {
        bail!("use paddock-host train inside a sandbox");
    }
    if args.run == "stats" || args.run == "feedback" {
        if args.deep || args.harness.is_some() {
            bail!("--deep and --harness apply only when starting a review");
        }
        let store = paddock::horse::Store::open_host(env)?;
        if args.run == "stats" {
            if args.advice.is_some() || args.outcome.is_some() || !args.note.is_empty() {
                bail!("usage: paddock train stats [RUN]");
            }
            let output = if let Some(run) = args.subject.as_deref() {
                serde_json::to_value(paddock::trainer::learning::stats(env, &store, run)?)?
            } else {
                serde_json::to_value(paddock::trainer::learning::all_stats(env, &store)?)?
            };
            println!("{}", serde_json::to_string_pretty(&output)?);
        } else {
            let run = args.subject.as_deref().context("expected a run ID")?;
            let id = args.advice.as_deref().context("expected an advice ID")?;
            let adopted = match args.outcome.as_deref() {
                Some("adopted") => true,
                Some("not-adopted") => false,
                _ => bail!("expected adopted or not-adopted"),
            };
            paddock::trainer::learning::feedback(&store, run, id, adopted, &args.note)?;
            println!("trainer feedback recorded");
        }
        return Ok(0);
    }
    if args.subject.is_some()
        || args.advice.is_some()
        || args.outcome.is_some()
        || !args.note.is_empty()
    {
        bail!("usage: paddock train RUN [--deep] [--harness HARNESS]");
    }
    let status =
        paddock::trainer::sandbox::run(env, &args.run, args.harness.as_deref(), args.deep)?;
    println!("trainer {}: {:?}", status.sandbox, status.state);
    Ok(status.exit_code.unwrap_or(1))
}
