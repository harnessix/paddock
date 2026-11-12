## Support scope

paddock is built and tested on Omarchy: Arch Linux with Hyprland. That is the supported platform. Every release passes `bin/test` and `paddock doctor --escape` there.

- Other distributions and compositors are maintained by the community. The bwrap backend, the network isolation and the terminal harnesses work wherever bubblewrap, passt and user namespaces work, but nobody tests each release on them. The apt and dnf paths in `bin/install` are best effort.
- Window focus and the border on protected app windows need Hyprland. The bar widget and the approval card need Omarchy.
- Pull requests for other platforms are welcome when they meet the bar in [CONTRIBUTING.md](CONTRIBUTING.md). Issues about platforms outside Omarchy can be closed without a fix.
- Every issue needs the output of `paddock doctor --report`. The issue form asks for it.
