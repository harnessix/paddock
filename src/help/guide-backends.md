## Backends

| Backend | Use it when |
| :--- | :--- |
| bwrap (default) | You want every host tool, and start in milliseconds. |
| podman, docker | The project needs system packages that the host does not have. The base image follows the host distribution. |
| seatbelt (macOS) | You run an agent on a Mac. `--backend auto` picks it for most runs. |
| vm (macOS) | You need a Linux-only feature on a Mac. See The VM backend on macOS (`paddock help macos`). |

Prefer rootless podman over docker. paddock runs it with `--userns=keep-id`, so files in the project keep your user as owner. The default seccomp profile of podman allows nested user namespaces, a common start of kernel exploits, so paddock derives a profile that blocks only that.
