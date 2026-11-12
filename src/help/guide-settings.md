## Project settings

`paddock init` writes a commented `paddock.toml` into the project. The project directory is writable from inside the sandbox, so an agent can edit this file. Any setting that widens access (extra mounts, host network, host display, GPU, passed variables, setup commands, image changes) needs a one-time approval on the host. paddock asks on the terminal, or you run `paddock approve`.

paddock finds the folders in `mounts.rw`, `mounts.ro` and `secrets.hide` again at each start. The agent can create symlinks in the project and in other folders that it can write. So paddock refuses such an entry if its path goes through a symlink in one of those folders, and the start stops with a message. Name the real folder instead. The same rule applies to the workspace folders of an app.
