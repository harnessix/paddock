## Why

| Problem | What paddock does |
| :--- | :--- |
| An agent with full auto-approval can read your home and your other projects. | The sandbox contains only the project, the harness, a private home and package caches. See What the sandbox contains (`paddock help sandbox`). |
| The agent sometimes needs the host: a push, the browser, a screenshot. | It asks with `paddock-host`, and you approve each request on the host. See Host requests (`paddock help host-requests`). |
| Agents leave design notes and instruction files in the repository, and a push publishes them. | The loft keeps these files in a private store. See The loft (`paddock help private-notes`). |
| An unattended run stops at a start screen, and nobody answers. | paddock answers known screens with a safe choice and reports stuck runs. See Start screens and stuck runs (`paddock help host-requests`). |
| A context compaction makes the agent forget its role and its task. | After a compaction, the agent gets a short note. See After a context compaction (`paddock help usage-limits`). |
| You want several agents to work in parallel. | Agents spawn other agents, share state in pastures, and claim Spec Kit tasks with leases. See Spawn other agents (`paddock help host-requests`), Pastures (`paddock help shared-state`) and Spec Kit projects (`paddock help spec-kit`). |
| You do not know how much of your usage limits is left. | `paddock forage` and the Omarchy agents panel show the limits of each account. See Forage (`paddock help usage-limits`) and Usage statistics (`paddock help defaults`). |
| A run breaks project files. | paddock takes a snapshot before each start, and `paddock undo` restores it. See Undo a run (`paddock help undo-runs`). |

The boundary is "which files exist", not "which commands can run". The agent has every tool and approves itself, and kernel namespaces keep the rest of the host out of the file system.
