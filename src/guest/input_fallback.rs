//! The choice of the guest's input source for `input.set`, platform-neutral
//! so the tests run on Linux. The host's source can be missing in the
//! guest, or be a mode of an input method that the guest has not enabled
//! (`com.apple.inputmethod.SCIM.ITABC`, Pinyin). The agent then enables the
//! input method of the mode and tries again, and when the source still
//! cannot be selected, it selects the ABC layout (or US) instead. The
//! Text Input Sources calls come in as closures (`input.rs`).

use std::sync::Mutex;

/// The layouts that the guest falls back to, in order.
pub const FALLBACK: [&str; 2] = ["com.apple.keylayout.ABC", "com.apple.keylayout.US"];

/// What the choice did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    /// The host's source is selected.
    Selected,
    /// The host's source could not be selected (`status` is its OSStatus,
    /// or None when the guest does not have it): `layout` is selected.
    Fallback { layout: String, status: Option<i32> },
    /// Nothing could be selected: the OSStatus of the host's source, or
    /// None when the guest does not have it.
    Failed(Option<i32>),
}

/// The input method that holds a mode: `com.apple.inputmethod.SCIM.ITABC`
/// gives `com.apple.inputmethod.SCIM`. A layout or a bare input method has
/// none.
pub fn parent_method(id: &str) -> Option<&str> {
    let at = id.find(".inputmethod.")?;
    let after = &id[at + ".inputmethod.".len()..];
    // The bundle name and at least one mode component after it.
    if after.split('.').filter(|c| !c.is_empty()).count() < 2 {
        return None;
    }
    id.rfind('.').map(|dot| &id[..dot])
}

/// Select `current`, or a fallback layout. `enable` and `select` give the
/// OSStatus of the call, or None for an id that the guest does not have.
pub fn choose(
    current: &str,
    mut enable: impl FnMut(&str) -> Option<i32>,
    mut select: impl FnMut(&str) -> Option<i32>,
) -> Choice {
    let mut status = select(current);
    if status != Some(0) {
        if let Some(parent) = parent_method(current) {
            if enable(parent) == Some(0) {
                let again = select(current);
                if again.is_some() {
                    status = again;
                }
            }
        }
    }
    if status == Some(0) {
        return Choice::Selected;
    }
    for layout in FALLBACK {
        if layout == current {
            continue;
        }
        if select(layout) == Some(0) {
            return Choice::Fallback {
                layout: layout.to_string(),
                status,
            };
        }
    }
    Choice::Failed(status)
}

/// True the first time an id is seen: the info line of a fallback is
/// written once per source, not at every `input.set`.
pub fn first_time(seen: &Mutex<Vec<String>>, id: &str) -> bool {
    let mut seen = seen.lock().unwrap_or_else(|e| e.into_inner());
    if seen.iter().any(|s| s == id) {
        return false;
    }
    seen.push(id.to_string());
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A fake guest: the sources it has, the OSStatus of each select, and
    /// which input methods make their modes selectable once enabled.
    struct Guest {
        select: HashMap<&'static str, i32>,
        after_enable: HashMap<&'static str, &'static str>,
        enabled: Vec<String>,
        calls: Vec<String>,
    }

    impl Guest {
        fn new(select: &[(&'static str, i32)]) -> Self {
            Self {
                select: select.iter().copied().collect(),
                after_enable: HashMap::new(),
                enabled: Vec::new(),
                calls: Vec::new(),
            }
        }

        fn run(&mut self, current: &str) -> Choice {
            let g = std::cell::RefCell::new(self);
            choose(
                current,
                |id| {
                    let mut g = g.borrow_mut();
                    g.calls.push(format!("enable {id}")); // not user text
                    g.enabled.push(id.to_string());
                    g.after_enable.contains_key(id).then_some(0)
                },
                |id| {
                    let mut g = g.borrow_mut();
                    g.calls.push(format!("select {id}")); // not user text
                    let unlocked = g
                        .after_enable
                        .iter()
                        .any(|(m, mode)| *mode == id && g.enabled.iter().any(|e| e == m));
                    if unlocked {
                        return Some(0);
                    }
                    g.select.get(id).copied()
                },
            )
        }
    }

    const PINYIN: &str = "com.apple.inputmethod.SCIM.ITABC";
    const ABC: &str = "com.apple.keylayout.ABC";

    #[test]
    fn parent_of_a_mode() {
        assert_eq!(parent_method(PINYIN), Some("com.apple.inputmethod.SCIM"));
        assert_eq!(
            parent_method("com.sogou.inputmethod.sogou.pinyin"),
            Some("com.sogou.inputmethod.sogou")
        );
        assert_eq!(parent_method("com.apple.inputmethod.SCIM"), None);
        assert_eq!(parent_method(ABC), None);
        assert_eq!(parent_method(""), None);
    }

    #[test]
    fn host_source_selected() {
        let mut g = Guest::new(&[("com.apple.keylayout.German", 0)]);
        assert_eq!(g.run("com.apple.keylayout.German"), Choice::Selected);
        assert_eq!(g.calls, ["select com.apple.keylayout.German"]);
    }

    #[test]
    fn mode_selectable_after_its_method_is_enabled() {
        let mut g = Guest::new(&[(PINYIN, -50), (ABC, 0)]);
        g.after_enable.insert("com.apple.inputmethod.SCIM", PINYIN);
        assert_eq!(g.run(PINYIN), Choice::Selected);
        assert!(g
            .calls
            .contains(&"enable com.apple.inputmethod.SCIM".into()));
        assert!(!g.calls.iter().any(|c| c.contains(ABC)));
    }

    #[test]
    fn missing_or_refused_source_falls_back_to_abc() {
        // The guest refuses the mode, and its method does not help.
        let mut g = Guest::new(&[(PINYIN, -50), (ABC, 0)]);
        assert_eq!(
            g.run(PINYIN),
            Choice::Fallback {
                layout: ABC.into(),
                status: Some(-50)
            }
        );
        // The guest does not have the source at all.
        let mut g = Guest::new(&[(ABC, 0)]);
        assert_eq!(
            g.run("com.apple.keylayout.Dvorak"),
            Choice::Fallback {
                layout: ABC.into(),
                status: None
            }
        );
        // No ABC in the guest: US.
        let mut g = Guest::new(&[("com.apple.keylayout.US", 0)]);
        assert_eq!(
            g.run(PINYIN),
            Choice::Fallback {
                layout: "com.apple.keylayout.US".into(),
                status: None
            }
        );
    }

    #[test]
    fn nothing_selectable_fails_with_the_status() {
        let mut g = Guest::new(&[(PINYIN, -50)]);
        assert_eq!(g.run(PINYIN), Choice::Failed(Some(-50)));
        let mut g = Guest::new(&[]);
        assert_eq!(g.run(ABC), Choice::Failed(None));
        // ABC itself is not tried twice.
        assert_eq!(g.calls.iter().filter(|c| c.contains(ABC)).count(), 1);
    }

    #[test]
    fn fallback_line_once_per_source() {
        let seen = Mutex::new(Vec::new());
        assert!(first_time(&seen, PINYIN));
        assert!(!first_time(&seen, PINYIN));
        assert!(first_time(&seen, "com.apple.keylayout.Dvorak"));
    }
}
