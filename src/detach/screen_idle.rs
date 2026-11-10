//! Track visible pane changes rather than terminal control traffic.

#[derive(Default)]
pub(super) struct ScreenIdle {
    screen: Option<String>,
    since: u64,
}

impl ScreenIdle {
    pub(super) fn observe(&mut self, screen: &str, now: u64) -> u64 {
        let screen = without_clocks(screen);
        if self.screen.as_ref() != Some(&screen) {
            self.screen = Some(screen);
            self.since = now;
        }
        self.since
    }
}

// Ignore time-of-day fields, including seconds, which are common in TUI bars.
fn without_clocks(screen: &str) -> String {
    let mut chars = screen.chars().peekable();
    let mut out = String::new();
    while let Some(c) = chars.next() {
        if c.is_ascii_digit() {
            let mut word = String::from(c);
            while chars
                .peek()
                .is_some_and(|c| c.is_ascii_digit() || *c == ':')
            {
                word.push(chars.next().unwrap());
            }
            let parts: Vec<_> = word.split(':').collect();
            if (parts.len() == 2 || parts.len() == 3)
                && parts
                    .iter()
                    .all(|p| p.len() == 2 && p.bytes().all(|b| b.is_ascii_digit()))
            {
                out.push_str("<clock>");
            } else {
                out.push_str(&word);
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clock_changes_do_not_reset_the_timer_but_other_changes_do() {
        let mut d = ScreenIdle::default();
        assert_eq!(d.observe("ready 12:30:01", 10), 10);
        assert_eq!(d.observe("ready 12:30:02", 20), 10);
        assert_eq!(d.observe("working 12:30:03", 30), 30);
    }
}
