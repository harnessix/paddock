//! `paddock doctor --json`: the prerequisites of app mode on macOS for the
//! checklist of the Mac launcher. Pure over [`Facts`], so the Linux tests
//! build every case by hand.
//!
//! Each check has an `id`, a `level` (`ok`, `warn` or `fail`) and one
//! line. `ok` at the top is false when any check fails; a warning keeps
//! it true.

use super::View;
use crate::macvm::golden;
use crate::macvm::limits::MIN_MACOS_MAJOR;
use crate::macvm::lock;
use crate::macvm::manage::GoldenView;
use crate::macvm::text::{self, Holder};
use serde_json::{json, Value};
use std::path::PathBuf;

/// The version of this output, its `v`.
pub const V: u32 = 1;
/// Below this memory the launcher warns: apps get the minimum.
pub const MEMORY_WARN_BYTES: u64 = 16 << 30;

/// What the JSON is built from, besides the [`View`] of the `macvm` block.
#[derive(Debug, Clone, PartialEq)]
pub struct Facts {
    /// `machdep.cpu.brand_string`: `Apple M2 Pro`.
    pub chip: Option<String>,
    /// `hw.memsize`.
    pub memory: Option<u64>,
    /// The `macvm` folder.
    pub dir: PathBuf,
    /// The path of `paddock-vm`, when paddock found it.
    pub helper: Option<PathBuf>,
    pub paddock_version: String,
}

/// The free space that the next action needs: a golden build without a
/// golden, else the clone of one profile.
pub fn space_need(golden: &GoldenView) -> u64 {
    match golden {
        GoldenView::Sealed { .. } => crate::macvm::session::CLONE_NEEDS,
        GoldenView::Unsealed(m) => golden::space_needed(m.state, m.ipsw_size),
        _ => golden::space_needed(golden::State::Fetch, None),
    }
}

fn check(id: &str, level: &str, line: String) -> Value {
    json!({ "id": id, "level": level, "text": line })
}

fn holder(h: &Option<Holder>) -> Value {
    match h {
        None => Value::Null,
        Some(h) => {
            let (kind, app, pid, user) = match h {
                Holder::Mine { app, pid } => ("mine", Some(app.clone()), Some(*pid), None),
                Holder::User { name } => ("user", None, None, Some(name.clone())),
                Holder::OtherProgram => ("other_program", None, None, None),
            };
            json!({ "kind": kind, "app": app, "pid": pid, "user": user, "text": text::holder_short(h) })
        }
    }
}

fn golden_state(g: &GoldenView) -> Value {
    match g {
        GoldenView::Missing => json!({ "state": "missing" }),
        GoldenView::Unreadable(e) => json!({ "state": "unreadable", "error": e }),
        GoldenView::Unsealed(m) => json!({ "state": m.state.name(), "version": m.version }),
        GoldenView::Sealed { manifest, size, .. } => {
            json!({ "state": "sealed", "version": manifest.version_text(), "bytes": size })
        }
    }
}

/// Read this Mac: the [`View`] of the `macvm` block and the [`Facts`]. Off
/// macOS the Mac fields are empty and the checks fail with their lines.
pub fn collect(env: &crate::env::HostEnv) -> (View, Facts) {
    let host = crate::macvm::limits::host_facts();
    let version = host
        .as_ref()
        .map(|f| f.macos.as_str())
        .filter(|v| !v.is_empty());
    let mut view = View::read(env, version);
    if let Some(f) = &host {
        view.apple_silicon = f.apple_silicon;
    }
    let facts = Facts {
        chip: sysctl("machdep.cpu.brand_string"),
        memory: sysctl("hw.memsize").and_then(|m| m.parse().ok()),
        dir: crate::macvm::layout::Layout::new(env).root,
        helper: crate::vm::host::helper_path(env).ok(),
        paddock_version: crate::macvm::version::paddock_version().into(),
    };
    (view, facts)
}

/// `sysctl -n NAME` on macOS, nothing elsewhere.
fn sysctl(name: &str) -> Option<String> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let o = std::process::Command::new("/usr/sbin/sysctl")
        .args(["-n", name])
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
    (o.status.success() && !s.is_empty()).then_some(s)
}

/// The JSON of `paddock doctor --json`.
pub fn build(v: &View, f: &Facts) -> Value {
    let mut checks = Vec::new();
    checks.push(if v.apple_silicon {
        check(
            "apple_silicon",
            "ok",
            text::doctor_chip_ok(f.chip.as_deref()),
        )
    } else {
        check("apple_silicon", "fail", text::not_apple_silicon())
    });
    let major = v
        .host_version
        .as_deref()
        .and_then(crate::macvm::limits::macos_major);
    let macos_ok = major.is_some_and(|m| m >= MIN_MACOS_MAJOR);
    checks.push(match (&v.host_version, macos_ok) {
        (Some(ver), true) => check("macos", "ok", text::doctor_macos_ok(ver)),
        (Some(ver), false) => check("macos", "fail", text::host_too_old(ver)),
        (None, _) => check("macos", "fail", text::doctor_macos_unknown()),
    });
    let need = space_need(&v.golden);
    let space_ok = v.free >= need;
    checks.push(check(
        "free_space",
        if space_ok { "ok" } else { "fail" },
        text::doctor_space(v.free, need),
    ));
    let per_app_max = f.memory.map(|m| (m / 2) >> 20);
    checks.push(match f.memory {
        Some(m) if m >= MEMORY_WARN_BYTES => check("memory", "ok", text::doctor_memory(m, false)),
        Some(m) => check("memory", "warn", text::doctor_memory(m, true)),
        None => check("memory", "warn", text::doctor_memory_unknown()),
    });
    checks.push(match v.entitlement {
        Some(true) => check("entitlement", "ok", text::doctor_entitlement(Some(true))),
        other => check("entitlement", "fail", text::doctor_entitlement(other)),
    });
    let used = v.slots.iter().flatten().count();
    let total = v.slots.len().max(lock::SLOTS);
    checks.push(check(
        "slots",
        if used >= total { "warn" } else { "ok" },
        text::doctor_slots(used, total),
    ));
    let ok = checks.iter().all(|c| c["level"] != "fail");
    json!({
        "v": V,
        "ok": ok,
        "paddock": { "version": f.paddock_version },
        "chip": { "name": f.chip, "apple_silicon": v.apple_silicon },
        "macos": { "version": v.host_version, "min_major": MIN_MACOS_MAJOR, "ok": macos_ok },
        "disk": { "dir": f.dir, "free": v.free, "need": need, "ok": space_ok },
        "memory": {
            "bytes": f.memory,
            "default_mib": crate::macvm::host_app::DEFAULT_MEMORY_MIB,
            "per_app_max_mib": per_app_max,
        },
        "entitlement": { "present": v.entitlement, "helper": f.helper },
        "slots": { "total": total, "used": used, "holders": v.slots.iter().map(holder).collect::<Vec<_>>() },
        "golden": golden_state(&v.golden),
        "checks": checks,
    })
}

#[cfg(test)]
#[path = "macvm_json_tests.rs"]
mod tests;
