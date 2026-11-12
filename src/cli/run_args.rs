use clap::{Args, Parser};
use paddock::spec::{GuiMode, Network};
use std::path::PathBuf;

#[derive(Args, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct RunArgs {
    /// Harness name, for example claude or codex. Defaults to default_harness in paddock.toml.
    pub(super) harness: Option<String>,
    /// Project directory. Defaults to the current directory, widened to its git root.
    pub(super) path: Option<PathBuf>,
    /// auto, bwrap, docker, podman, srt, seatbelt or vm (macOS).
    #[arg(long, short)]
    pub(super) backend: Option<String>,
    /// none, api, egress or host. api: only the model API hosts of the
    /// profile and the domains of --allow, over HTTPS through the paddock proxy.
    #[arg(long, short)]
    pub(super) network: Option<Network>,
    /// off, headless, window or host.
    #[arg(long)]
    pub(super) gui: Option<GuiMode>,
    /// Bind GPU render nodes.
    #[arg(long)]
    pub(super) gpu: bool,
    /// inherit (default) passes the host environment; clean passes only the allowlist.
    #[arg(long, value_parser = ["inherit", "clean"])]
    pub(super) env: Option<String>,
    /// shared (default) reuses your login; isolated or isolated:NAME uses a separate one.
    #[arg(long)]
    pub(super) auth: Option<String>,
    /// A domain the sandbox can reach in network modes api and allowlist
    /// (repeatable). HOST, HOST:PORT, or *.DOMAIN for its subdomains.
    #[arg(long)]
    pub(super) allow: Vec<String>,
    /// Work in a new git worktree on its own branch, and review the diff afterwards.
    #[arg(long)]
    pub(super) worktree: bool,
    /// Join this pasture of the project: [pastures.NAME] of the user
    /// configuration. Its members share the pasture scope of paddock kv.
    /// Without it, the sandbox shares paddock kv with no other.
    #[arg(long, value_name = "NAME")]
    pub(super) pasture: Option<String>,
    /// Give this run a device of the user configuration (repeatable):
    /// [devices.NAME] of ~/.config/paddock/config.toml, whatever its grant
    /// says. `paddock device ls` lists them.
    #[arg(long, value_name = "NAME")]
    #[serde(default)]
    pub(super) device: Vec<String>,
    /// default: start the harness with a private snapshot of its own host
    /// data (conversations, settings, plugins) in the sandbox home. The
    /// run changes only the snapshot, and paddock removes it at the end.
    /// bwrap only.
    #[arg(long, value_name = "PROFILE", value_parser = ["default"])]
    pub(super) profile: Option<String>,
    /// Manual approvals: you approve the agent's steps in its own
    /// interface. paddock adds no auto-approval flags and does not answer
    /// the agent's trust question. The sandbox stays the same.
    /// `--no-yolo` is the old name. The default comes from `[run]
    /// approvals` (paddock.toml, then the user configuration), else auto.
    #[arg(long, visible_alias = "no-yolo", conflicts_with = "auto")]
    #[serde(default, alias = "no_yolo")]
    pub(super) manual: bool,
    /// Auto approvals for this run, over a manual default: the agent
    /// approves its own steps inside the sandbox.
    #[arg(long)]
    #[serde(default)]
    pub(super) auto: bool,
    /// Approve the project's paddock.toml without asking.
    #[arg(long, short = 'y')]
    pub(super) yes: bool,
    /// Start the run in the background, in a session of paddock's own tmux
    /// server: the harness runs its interactive interface with the task as
    /// its first prompt. `paddock view 1a2b3c --write` takes over, `paddock
    /// wait 1a2b3c` waits for the end. Needs --prompt-file or --prompt.
    #[arg(long)]
    #[serde(skip)]
    pub(super) detach: bool,
    /// With --detach: the file with the task.
    #[arg(
        long,
        value_name = "FILE",
        requires = "detach",
        conflicts_with = "prompt"
    )]
    #[serde(skip)]
    pub(super) prompt_file: Option<PathBuf>,
    /// With --detach: the task as text.
    #[arg(long, value_name = "TEXT", requires = "detach")]
    #[serde(skip)]
    pub(super) prompt: Option<String>,
    /// Record this run in a new horse (dsh, Claude, or Codex).
    /// Also enabled by `[horse] enabled`.
    #[arg(long)]
    #[serde(default)]
    pub(super) horse: bool,
    /// Continue the horse ID in horse mode: dsh resumes its newest session.
    #[arg(long, value_name = "ID")]
    #[serde(default)]
    pub(super) horse_id: Option<String>,
    /// Arguments for the harness, after `--`.
    #[arg(last = true)]
    pub(super) args: Vec<String>,
}

#[derive(Parser)]
#[command(name = "paddock")]
pub(crate) struct RunArgsCli {
    #[command(flatten)]
    pub(super) run: RunArgs,
}
