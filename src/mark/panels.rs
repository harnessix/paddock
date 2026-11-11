//! The pure part of the firewall panels of the marker: which question of
//! the approvals store gets a panel, which panel closes, and what a panel
//! says. The AppKit drawing is in `ask_panel.rs`, macOS only.
//!
//! The rules:
//! - One panel per session (the session helper pid and the app profile of
//!   the card). The asker files one card per session anyway; this is the
//!   second fence, for an old helper or a hand-made card.
//! - A panel keeps its question while the question is open: the panel of
//!   a question goes only when its card is gone from the store (answered,
//!   withdrawn, or its helper died). A card that did not read once (a
//!   read in the middle of a rename) does not close it.
//! - In a session with no panel: the notice of a paused session first,
//!   else the oldest question.

use crate::broker::{ConnectAnswer, Pending};
use crate::macvm::text::{self, AskButton};
use std::collections::BTreeSet;

/// The session of a card: its helper pid and its app profile.
pub type SessionKey = (u32, String);

pub fn session_of(p: &Pending) -> Option<SessionKey> {
    p.connect.as_ref().map(|c| (c.pid, c.profile.clone()))
}

/// The cards that get a panel. `has_panel`: the ids that have one now.
pub fn pick<'a>(cards: &'a [Pending], has_panel: &BTreeSet<String>) -> Vec<&'a Pending> {
    let mut out: Vec<&Pending> = Vec::new();
    let mut by_key: Vec<(SessionKey, Vec<&Pending>)> = Vec::new();
    for p in cards {
        let Some(k) = session_of(p) else {
            continue;
        };
        match by_key.iter_mut().find(|(key, _)| *key == k) {
            Some((_, v)) => v.push(p),
            None => by_key.push((k, vec![p])),
        }
    }
    for (_, group) in by_key {
        let kept = group.iter().find(|p| has_panel.contains(&p.id));
        let notice = group
            .iter()
            .find(|p| p.connect.as_ref().is_some_and(|c| c.notice));
        let oldest = group.iter().min_by_key(|p| (p.created, p.id.clone()));
        if let Some(p) = kept.or(notice).or(oldest) {
            out.push(p);
        }
    }
    out
}

/// The panels to close. `panels`: the id and session of each panel;
/// `picked`: the cards that get a panel; `card_exists`: the card file of
/// an id is still in the store.
pub fn to_close(
    panels: &[(String, SessionKey)],
    picked: &[&Pending],
    card_exists: impl Fn(&str) -> bool,
) -> Vec<String> {
    let picked_ids: BTreeSet<&str> = picked.iter().map(|p| p.id.as_str()).collect();
    let picked_keys: Vec<SessionKey> = picked.iter().filter_map(|p| session_of(p)).collect();
    panels
        .iter()
        .filter(|(id, key)| {
            !picked_ids.contains(id.as_str()) && (!card_exists(id) || picked_keys.contains(key))
        })
        .map(|(id, _)| id.clone())
        .collect()
}

/// The role of a button of the panel, after the system alerts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Esc, the leftmost of the row.
    Cancel,
    Plain,
    /// Return, the rightmost of the row, drawn with the accent color.
    Default,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Button {
    pub title: String,
    /// What VoiceOver says: the button and the host.
    pub label: String,
    pub answer: ConnectAnswer,
    pub role: Role,
}

/// What one panel shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Content {
    pub title: String,
    pub detail: String,
    /// `and 3 more waiting`, empty for none.
    pub more: String,
    /// The row at the bottom right, in order from left to right.
    pub buttons: Vec<Button>,
    /// The answers of the `More…` menu.
    pub menu: Vec<Button>,
}

/// The answer of a button of the text table.
pub fn answer_of(b: AskButton) -> ConnectAnswer {
    match b {
        AskButton::AllowOnce => ConnectAnswer::AllowOnce,
        AskButton::AllowSession => ConnectAnswer::AllowSession,
        AskButton::AlwaysAllow => ConnectAnswer::AllowAlways,
        AskButton::Deny => ConnectAnswer::Deny,
        AskButton::AlwaysDeny => ConnectAnswer::DenyAlways,
    }
}

/// The three buttons of the row (the macOS order: cancel left, default
/// right) and the rest of the table for the menu. Return is `Allow
/// once`: a reflex
/// Return must not make a permanent rule, so `Always allow` takes a
/// click or Command-A.
const ROW: [(AskButton, Role); 3] = [
    (AskButton::Deny, Role::Cancel),
    (AskButton::AllowOnce, Role::Plain),
    (AskButton::AlwaysAllow, Role::Default),
];

/// The key of a button besides its role: Command-A for `Always allow`.
/// `None` for the others.
pub fn command_key(answer: ConnectAnswer) -> Option<&'static str> {
    match answer {
        ConnectAnswer::AllowAlways => Some("a"),
        _ => None,
    }
}

/// The content of the panel of a card. `None` for a card that is no
/// `connect` question.
pub fn content(p: &Pending) -> Option<Content> {
    let c = p.connect.as_ref()?;
    let app = if c.label.is_empty() {
        c.app.as_str()
    } else {
        c.label.as_str()
    };
    if c.notice {
        let open = c.more as usize;
        let b = |deny_all: bool, answer, role| Button {
            title: text::asks_notice_button(deny_all).to_string(),
            label: text::asks_notice_button(deny_all).to_string(),
            answer,
            role,
        };
        return Some(Content {
            title: text::asks_notice_summary(app, open),
            detail: text::asks_notice_detail(app, &c.profile, open),
            more: String::new(),
            buttons: vec![
                b(true, ConnectAnswer::Deny, Role::Cancel),
                b(false, ConnectAnswer::AllowOnce, Role::Default),
            ],
            menu: Vec::new(),
        });
    }
    let button = |ab: AskButton, role: Role| {
        let title = text::ask_button(ab).to_string();
        Button {
            label: text::ask_button_label(&title, &c.host),
            title,
            answer: answer_of(ab),
            role,
        }
    };
    let site = crate::macvm::firewall::site::registrable_site(&c.host);
    let target = site.as_deref().unwrap_or(&c.host);
    let mut buttons: Vec<Button> = ROW.iter().map(|(b, r)| button(*b, *r)).collect();
    buttons[2] = Button {
        title: format!("Allow {target}"),
        label: if site.is_some() {
            format!("Allow {target} and all its subdomains")
        } else {
            format!("Always allow {target}")
        },
        answer: ConnectAnswer::AllowSite,
        role: Role::Default,
    };
    let mut menu: Vec<Button> = AskButton::ALL
        .iter()
        .filter(|b| !ROW.iter().any(|(r, _)| r == *b))
        .map(|b| button(*b, Role::Plain))
        .collect();
    menu.push(button(AskButton::AlwaysAllow, Role::Plain));
    menu.push(Button {
        title: "Allow everything for 10 minutes".into(),
        label: "Allow everything for 10 minutes".into(),
        answer: ConnectAnswer::AllowEverything,
        role: Role::Plain,
    });
    Some(Content {
        title: text::ask_title(app, &c.host),
        detail: if site.is_some() {
            format!(
                "{} · {target} and all its subdomains",
                text::ask_detail(c.port)
            )
        } else {
            text::ask_detail(c.port)
        },
        more: text::ask_more(c.more),
        buttons,
        menu,
    })
}

// ---- The layout --------------------------------------------------------------

/// The measures of the panel, after the system alert: 360 pt wide (380 at
/// most when the buttons need it), 16 pt padding, a 32 pt icon, buttons
/// 28 pt high.
pub const WIDTH: f64 = 360.0;
pub const WIDTH_MAX: f64 = 380.0;
pub const PAD: f64 = 16.0;
pub const ICON: f64 = 32.0;
pub const ICON_GAP: f64 = 10.0;
pub const BUTTON_H: f64 = 28.0;
pub const BUTTON_GAP: f64 = 8.0;
pub const BUTTON_MIN_W: f64 = 64.0;
pub const LINE_GAP: f64 = 3.0;

/// The width of the text column of a panel of width `w`.
pub fn text_width(w: f64) -> f64 {
    w - PAD - ICON - ICON_GAP - PAD
}

/// Where each part of a panel goes, in AppKit coordinates (the origin at
/// the bottom left).
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub w: f64,
    pub h: f64,
    pub icon: super::Rect,
    pub title: super::Rect,
    pub detail: super::Rect,
    pub more: Option<super::Rect>,
    /// The buttons of the row, in the order of `Content::buttons`.
    pub buttons: Vec<super::Rect>,
    pub more_button: Option<super::Rect>,
}

/// The width that the buttons need: the row from the right, the `More…`
/// button at the left.
pub fn width_for(button_ws: &[f64], more_w: Option<f64>) -> f64 {
    let row: f64 = button_ws.iter().map(|w| w.max(BUTTON_MIN_W)).sum::<f64>()
        + BUTTON_GAP * button_ws.len().saturating_sub(1) as f64;
    let more = more_w.map(|m| m + BUTTON_GAP).unwrap_or(0.0);
    (PAD + more + row + PAD).clamp(WIDTH, WIDTH_MAX)
}

/// The layout of a panel of width `w` from the heights of its texts
/// (`more_h` for the queue line) and the widths of its buttons.
pub fn layout(
    w: f64,
    title_h: f64,
    detail_h: f64,
    more_h: Option<f64>,
    button_ws: &[f64],
    more_w: Option<f64>,
) -> Layout {
    use super::Rect;
    let text_x = PAD + ICON + ICON_GAP;
    let text_w = text_width(w);
    let text_h = title_h + LINE_GAP + detail_h + more_h.map(|m| LINE_GAP + m).unwrap_or(0.0);
    let h = (PAD + text_h.max(ICON) + PAD + BUTTON_H + PAD).ceil();
    let top = h - PAD;
    let icon = Rect::new(PAD, top - ICON, ICON, ICON);
    let mut y = top - title_h;
    let title = Rect::new(text_x, y, text_w, title_h);
    y -= LINE_GAP + detail_h;
    let detail = Rect::new(text_x, y, text_w, detail_h);
    let more = more_h.map(|m| {
        y -= LINE_GAP + m;
        Rect::new(text_x, y, text_w, m)
    });
    let mut buttons = vec![Rect::new(0.0, 0.0, 0.0, 0.0); button_ws.len()];
    let mut x = w - PAD;
    for (i, bw) in button_ws.iter().enumerate().rev() {
        let bw = bw.max(BUTTON_MIN_W);
        x -= bw;
        buttons[i] = Rect::new(x, PAD, bw, BUTTON_H);
        x -= BUTTON_GAP;
    }
    let more_button = more_w.map(|m| Rect::new(PAD, PAD, m, BUTTON_H));
    Layout {
        w,
        h,
        icon,
        title,
        detail,
        more,
        buttons,
        more_button,
    }
}

// ---- Contrast -------------------------------------------------------------

/// WCAG AA for normal text.
pub const AA: f64 = 4.5;

fn linear(c: f64) -> f64 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn luminance((r, g, b): (f64, f64, f64)) -> f64 {
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}

/// The WCAG contrast of a text color `fg` (sRGB with alpha, blended over
/// the background) on the background `bg`.
pub fn contrast(fg: (f64, f64, f64, f64), bg: (f64, f64, f64)) -> f64 {
    let (r, g, b, a) = fg;
    let blend = |f: f64, k: f64| f * a + k * (1.0 - a);
    let top = luminance((blend(r, bg.0), blend(g, bg.1), blend(b, bg.2)));
    let low = luminance(bg);
    let (hi, lo) = if top > low { (top, low) } else { (low, top) };
    (hi + 0.05) / (lo + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::broker::Connect;

    fn card(id: &str, pid: u32, created: u64, notice: bool) -> Pending {
        Pending {
            id: id.into(),
            created,
            capability: "connect".into(),
            connect: Some(Connect {
                app: "com.example.notes".into(),
                host: format!("{id}.example"),
                port: 443,
                pid,
                profile: "sample-notes".into(),
                label: "Sample Notes".into(),
                more: 3,
                notice,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn one_panel_per_session_and_a_panel_keeps_its_question() {
        let cards = vec![
            card("b", 10, 5, false),
            card("a", 10, 1, false),
            card("c", 20, 9, false),
            Pending {
                id: "agent".into(),
                ..Default::default()
            },
        ];
        let none = BTreeSet::new();
        let ids: Vec<&str> = pick(&cards, &none).iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["a", "c"], "the oldest per session, no other card");
        let kept: BTreeSet<String> = ["b".to_string()].into();
        let ids: Vec<&str> = pick(&cards, &kept).iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["b", "c"], "a panel keeps its question");
        let mut with_notice = cards.clone();
        with_notice.push(card("n", 10, 50, true));
        let ids: Vec<&str> = pick(&with_notice, &none)
            .iter()
            .map(|p| p.id.as_str())
            .collect();
        assert_eq!(ids, ["n", "c"], "the notice first");
    }

    #[test]
    fn a_panel_closes_only_when_its_card_is_gone() {
        let cards = [card("a", 10, 1, false)];
        let picked: Vec<&Pending> = cards.iter().collect();
        let key = (10, "sample-notes".to_string());
        let other = (20, "sample-editor".to_string());
        let panels = vec![
            ("a".to_string(), key.clone()),
            ("gone".to_string(), other.clone()),
            ("unread".to_string(), other.clone()),
            ("second".to_string(), key.clone()),
        ];
        let exists = |id: &str| id != "gone";
        assert_eq!(
            to_close(&panels, &picked, exists),
            ["gone", "second"],
            "an unread card keeps its panel; a second panel of a session goes"
        );
    }

    #[test]
    fn the_panel_says_app_host_port_and_the_queue() {
        let c = content(&card("api", 10, 1, false)).unwrap();
        assert_eq!(c.title, "Sample Notes wants to connect to api.example");
        assert_eq!(
            c.detail,
            "port 443 · first connection to this host · api.example and all its subdomains"
        );
        assert_eq!(c.more, "and 3 more waiting");
        let row: Vec<(&str, Role)> = c
            .buttons
            .iter()
            .map(|b| (b.title.as_str(), b.role))
            .collect();
        assert_eq!(
            row,
            [
                ("Deny", Role::Cancel),
                ("Allow once", Role::Plain),
                ("Allow api.example", Role::Default)
            ]
        );
        assert_eq!(c.buttons[2].answer, ConnectAnswer::AllowSite);
        // Command-A is the key of the permanent rule of this host only.
        assert_eq!(command_key(ConnectAnswer::AllowAlways), Some("a"));
        for a in [
            ConnectAnswer::AllowOnce,
            ConnectAnswer::AllowSession,
            ConnectAnswer::Deny,
            ConnectAnswer::DenyAlways,
        ] {
            assert_eq!(command_key(a), None);
        }
        assert_eq!(c.buttons[0].label, "Deny for api.example");
        let menu: Vec<ConnectAnswer> = c.menu.iter().map(|b| b.answer).collect();
        assert_eq!(
            menu,
            [
                ConnectAnswer::AllowSession,
                ConnectAnswer::DenyAlways,
                ConnectAnswer::AllowAlways,
                ConnectAnswer::AllowEverything
            ]
        );
        // The notice: two buttons, deny all on the left.
        let n = content(&card("n", 10, 1, true)).unwrap();
        assert!(n.title.contains("many new connections"), "{}", n.title);
        assert!(n.detail.contains("paddock app status sample-notes"));
        assert_eq!(n.buttons.len(), 2);
        assert_eq!(
            (n.buttons[0].answer, n.buttons[1].answer),
            (ConnectAnswer::Deny, ConnectAnswer::AllowOnce)
        );
        assert!(n.menu.is_empty());
        assert!(content(&Pending::default()).is_none());
    }

    fn inside(r: crate::mark::Rect, l: &Layout) -> bool {
        r.x >= 0.0 && r.y >= 0.0 && r.x + r.w <= l.w && r.y + r.h <= l.h
    }

    fn apart(a: crate::mark::Rect, b: crate::mark::Rect) -> bool {
        a.x + a.w <= b.x || b.x + b.w <= a.x || a.y + a.h <= b.y || b.y + b.h <= a.y
    }

    #[test]
    fn the_layout_keeps_every_part_inside_and_apart() {
        // A two-line title, a detail, the queue line, three buttons and More….
        let ws = [60.0, 88.0, 100.0];
        let w = width_for(&ws, Some(64.0));
        assert_eq!(w, 372.0, "the row and More… fit with the padding");
        assert_eq!(width_for(&[60.0], None), WIDTH, "never under 360 pt");
        let l = layout(w, 34.0, 14.0, Some(14.0), &ws, Some(64.0));
        let mut parts = vec![
            l.icon,
            l.title,
            l.detail,
            l.more.unwrap(),
            l.more_button.unwrap(),
        ];
        parts.extend(l.buttons.iter().copied());
        for p in &parts {
            assert!(inside(*p, &l), "{p:?} is outside {l:?}");
        }
        for (i, a) in parts.iter().enumerate() {
            for b in &parts[i + 1..] {
                if (a.x - l.icon.x).abs() < f64::EPSILON && (b.x - l.title.x).abs() < f64::EPSILON {
                    continue;
                }
                assert!(apart(*a, *b), "{a:?} overlaps {b:?}");
            }
        }
        // The default button is the rightmost, at the padding.
        assert_eq!(l.buttons[2].x + l.buttons[2].w, w - PAD);
        assert!(l.buttons[0].x < l.buttons[1].x && l.buttons[1].x < l.buttons[2].x);
        assert!(l
            .buttons
            .iter()
            .all(|b| b.h == BUTTON_H && b.w >= BUTTON_MIN_W));
        // The title on top, the queue line under the detail.
        assert!(l.title.y > l.detail.y && l.detail.y > l.more.unwrap().y);
        assert_eq!(l.title.y + l.title.h, l.h - PAD);
        assert_eq!(text_width(WIDTH), 286.0);
        // Long buttons widen the panel up to 380 pt, no more.
        assert_eq!(width_for(&[120.0, 120.0, 120.0], Some(80.0)), WIDTH_MAX);
        // A short text still leaves room for the 32 pt icon.
        let s = layout(WIDTH, 15.0, 13.0, None, &ws, None);
        assert_eq!(s.h, (PAD + ICON + PAD + BUTTON_H + PAD).ceil());
        assert!(s.more.is_none() && s.more_button.is_none());
    }

    #[test]
    fn contrast_follows_wcag_and_half_black_fails_aa() {
        let white = (1.0, 1.0, 1.0);
        assert!((contrast((0.0, 0.0, 0.0, 1.0), white) - 21.0).abs() < 0.01);
        assert!((contrast((1.0, 1.0, 1.0, 1.0), white) - 1.0).abs() < 0.01);
        // secondaryLabelColor of the Light appearance is black at about
        // half alpha: under AA on white, which is why the detail of the
        // panel uses labelColor.
        let half = contrast((0.0, 0.0, 0.0, 0.5), white);
        assert!(half < AA && half > 3.5, "{half}");
    }
}
