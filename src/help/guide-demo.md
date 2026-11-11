## See it

Inside the paddock, the agent cannot see the SSH key on the machine, the `.env` in the project is empty for it, and it still runs commands and writes code without approval prompts.

<p align="center">
  <img src="docs/assets/demo.gif" alt="A terminal recording: paddock claude opens Claude Code's TUI with bypass permissions on. The agent runs a shell command and writes src/greet.rs without approval prompts, and reports that the SSH key does not exist and .env is empty. Then paddock explain lists the mounts" width="100%">
</p>
