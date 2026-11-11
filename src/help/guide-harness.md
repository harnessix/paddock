## Add a harness

A harness is a TOML profile. Put a file in `$XDG_CONFIG_HOME/paddock/harness.d/`. A file with the name of a built-in profile replaces it. The built-in profiles are in [profiles/](profiles/).

```toml
name = "myagent"
description = "My agent"

[locate]
commands = ["myagent"]

[launch]
yolo_args = ["--yes-to-everything"]

[state]
auth = [".myagent/token.json"]
import_ro = [".myagent/config.toml"]
env_pass = ["MYAGENT_API_KEY"]
scratch = [".myagent/cache"]      # pure caches: a tmpfs in the sandbox
data = [".myagent"]               # what --profile default copies into its snapshot

# Telemetry switches, set only inside the sandbox. Cite the vendor's documentation.
[privacy]
env = { MYAGENT_TELEMETRY = "0" }

[[privacy.settings]]
file = ".myagent/settings.json"   # in the sandbox home; json or toml (format = "toml")
path = ["telemetry", "enabled"]
value = false

[privacy.training]
setting = "Settings, Data, Use my data for training"
url = "https://example.com/settings/data"

# The one-shot mode for `paddock-host spawn`. Optional.
[oneshot]
args = ["run", "--yes-to-everything", "--task-file", "{prompt_file}"]
prompt = "file"          # or "stdin"
probe_args = ["run", "--help"]

# The first prompt of the interactive interface, for `paddock run --detach`. Optional.
[detach]
args = ["Read the file {prompt_file} and do the task that it describes."]
```

More entries, for harnesses that need them:

```toml
[launch]
# paddock wrap runs the harness on the host, with this notice.
wrap_on_host = "it keeps its own sandbox, which cannot start inside paddock"

[state]
# An entry that ends with / is a folder: paddock copies every file in it.
auth = [".myagent/credentials/"]
# JSON keys that only go from false to true, such as a finished onboarding. paddock carries a true value between the login store and the sandbox, in both directions, and never writes false over true.
done_flags = { ".myagent/onboarding.json" = ["onboardingComplete"] }

# The folder that an account variable replaces, for paddock wrap.
[state.account]
env = "MYAGENT_HOME"
dir = ".myagent"

# Values written into the sandbox copy of a settings file, for example to
# turn off a self-update that cannot work on the read-only install.
[[state.settings]]
file = ".myagent/settings.json"
path = ["auto_update"]
value = false

[network]
# A loopback port of the sandbox that the host browser reaches on 127.0.0.1.
publish = [3080]
```

Before each start, paddock makes sure that the help text of the harness still mentions every flag in `yolo_args`. If a flag is renamed, paddock stops with an error and does not start a harness that waits for approvals. A spawn makes the same check for the flags in `[oneshot] args`, with the help text of `probe_args`. The task never goes into `args`: with `prompt = "stdin"` the harness reads it on its standard input, and with `prompt = "file"` paddock replaces `{prompt_file}` with the path of a private file in the sandbox home.
