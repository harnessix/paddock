pub struct Dep {
    pub cmd: &'static str,
    /// Package names for Arch, Debian/Ubuntu and Fedora.
    pub pkg: [&'static str; 3],
    pub required: bool,
    pub why: &'static str,
}

/// Every host dependency, in one place. `bin/install`, the PKGBUILD and the
/// README list the same packages.
pub const DEPS: &[Dep] = &[
    Dep {
        cmd: "git",
        pkg: ["git", "git", "git"],
        required: true,
        why: "project and worktree detection",
    },
    Dep {
        cmd: "bwrap",
        pkg: ["bubblewrap", "bubblewrap", "bubblewrap"],
        required: true,
        why: "the default backend",
    },
    Dep {
        cmd: "pasta",
        pkg: ["passt", "passt", "passt"],
        required: true,
        why: "network mode 'egress' on the bwrap backend",
    },
    Dep {
        cmd: "ip",
        pkg: ["iproute2", "iproute2", "iproute"],
        required: true,
        why: "isolating host addresses in network mode 'egress'",
    },
    Dep {
        cmd: "weston",
        pkg: ["weston", "weston", "weston"],
        required: false,
        why: "displays for desktop projects (gui headless and window)",
    },
    Dep {
        cmd: "weston-screenshooter",
        pkg: ["weston", "weston", "weston"],
        required: false,
        why: "paddock-screenshot",
    },
    Dep {
        cmd: "Xwayland",
        pkg: ["xorg-xwayland", "xwayland", "xorg-x11-server-Xwayland"],
        required: false,
        why: "X11 apps on the private display",
    },
    Dep {
        cmd: "dbus-run-session",
        pkg: ["dbus", "dbus", "dbus-daemon"],
        required: false,
        why: "a private session bus for desktop apps",
    },
    Dep {
        cmd: "gnome-keyring-daemon",
        pkg: ["gnome-keyring", "gnome-keyring", "gnome-keyring"],
        required: false,
        why: "a private keyring for app sandboxes and harnesses that need one (copilot)",
    },
    Dep {
        cmd: "docker",
        pkg: ["docker", "docker.io", "moby-engine"],
        required: false,
        why: "the container backend",
    },
    Dep {
        cmd: "podman",
        pkg: ["podman", "podman", "podman"],
        required: false,
        why: "the rootless container backend",
    },
    Dep {
        cmd: "srt",
        pkg: [
            "sandbox-runtime (npm)",
            "sandbox-runtime (npm)",
            "sandbox-runtime (npm)",
        ],
        required: false,
        why: "the srt backend and network mode 'allowlist' (also needs socat and ripgrep)",
    },
    Dep {
        cmd: "socat",
        pkg: ["socat", "socat", "socat"],
        required: false,
        why: "the srt backend",
    },
    Dep {
        cmd: "rg",
        pkg: ["ripgrep", "ripgrep", "ripgrep"],
        required: false,
        why: "the srt backend",
    },
    Dep {
        cmd: "mise",
        pkg: ["mise", "mise", "mise"],
        required: false,
        why: "per-project toolchains from mise.toml",
    },
];
