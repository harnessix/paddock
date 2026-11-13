//! The paddock.toml that `paddock setup` writes into a project.

pub const TEMPLATE: &str = r#"# paddock.toml: sandbox settings for this project.
# Fields that widen access need a one-time approval on the host.

# default_harness = "claude"
# backend = "auto"            # auto | bwrap | docker

[mounts]
# rw = []                     # extra writable host paths
# ro = ["../shared-protos"]   # extra read-only host paths

[secrets]
# hide = ["secrets/"]         # hide more project paths
# reveal = [".env.test"]      # do not hide these

[network]
# mode = "egress"             # none | api | allowlist | egress | host
# publish = [5173]            # ports reachable from the host
# allow = ["crates.io"]       # extra domains for modes "api" and "allowlist"
# lan = true                # reach private networks: routers, NAS, other machines on the LAN
# host_ports = [8765]         # host loopback ports reachable from the sandbox (local MCP servers)

[run]
# approvals = "auto"          # auto: the agent approves its own steps in the sandbox
#                             # manual: you approve each step in the agent's interface

[auth]
# mode = "shared"             # shared: your login | isolated | isolated:NAME: a separate login

[mcp]
# allow = ["my-server"]       # MCP servers whose host files may be mounted read-only

[gui]
# mode = "headless"           # off | headless | window | host
#                             # headless: private display inside the sandbox (default for desktop projects)
#                             # window:   a nested compositor window on your desktop
#                             # host:     your real desktop; the agent can read the screen and type
# gpu = false                 # bind /dev/dri render nodes
# width = 1600
# height = 1000

[env]
# mode = "listed"             # listed: only the lists pass | inherit: the host environment passes
# pass = []                   # host variables to pass through
# set = { RUST_LOG = "info" }

[setup]
# mise_install = true
# shared_build_cache = true   # worktree runs build in a cargo cache outside the worktree
# run = []

[docker]
# image = "debian:stable"
# packages = ["libssl-dev"]

[devices]
# use = ["mac"]               # ask for devices of the user configuration (paddock device ls)
"#;
