## Logs and bug reports

paddock prints warnings and errors to the terminal. Other messages stay hidden.

- `-v` shows more: `-v` adds info, `-vv` debug, `-vvv` trace. `-q` shows errors only.
- `PADDOCK_LOG` sets the level too. `PADDOCK_LOG=debug` applies to everything. `PADDOCK_LOG=broker=debug,proxy=trace` applies to single parts.
- The parts that run for the whole session also write to the systemd journal, under the name `paddock`. These are the host request broker, the proxy, the login callback forwarder and the window border. Read the warnings with `journalctl -t paddock -p warning`. Without systemd, paddock skips the journal.
- The bar plugin is silent. It writes a warning to the shell log when it cannot read paddock's output. To see more, start the shell with `PADDOCK_DEBUG=true`. Read the shell log with `journalctl -t omarchy-shell`.

To report a bug, run `paddock doctor --report`. It prints the version, the `paddock doctor` output, the running sandboxes and recent paddock warnings from the journal, and on a Mac the `MACVM` section of the macOS guest (its logs, stamps and rules files). `--output FILE` writes it to a file. The report replaces your user name, host name, home path, variable values and tokens with placeholders; `--no-hosts` also replaces every host name of the guest's rules files and connection logs with `host-N`. Read it before you share it.
