//! The firewall panels of the marker: at most one panel per session, near
//! the badge of the window of the session that asks, for the question
//! that `panels::pick` chooses. The drawing is in `ask_panel.rs`.
//!
//! The store is the truth: the marker reads it every second, so a panel
//! shows the same question after a restart of the marker, and an answer
//! goes through `broker::answer_connect`, the way `paddock approvals
//! answer` does from a terminal. A panel closes after the store confirms
//! its own card was selected or retired. The panels are listed in
//! `mark-panels.json` of the run folder, so `paddock windows --json`
//! shows them with `kind = approval`.

use super::ask_panel::{self, Drawn};
use super::*;
use crate::broker::{self, ConnectAnswer, Pending, CONNECT_CAPABILITY};
use crate::mark::panels::{self, Content, SessionKey};

/// The panel of one question.
pub(super) struct AskPanel {
    drawn: Drawn,
    content: Content,
    key: SessionKey,
    card: Pending,
    /// The last frame, in window server coordinates.
    rect: Option<Rect>,
    shown: bool,
    sandbox: String,
    harness: String,
}

impl AskPanel {
    fn number(&self) -> isize {
        self.drawn.panel.windowNumber()
    }
}

/// The `connect` questions of the store.
pub(super) fn load_questions(env: &HostEnv) -> Vec<Pending> {
    broker::pending(env)
        .into_iter()
        .filter(|p| p.capability == CONNECT_CAPABILITY)
        .collect()
}

/// The window of the session that asks: the front window of the pid
/// that the question names.
fn window_of(wins: &[Win], pid: u32) -> Option<Rect> {
    wins.iter()
        .find(|w| w.pid == pid && w.layer == 0 && w.bounds.w >= crate::mark::MIN_WINDOW)
        .map(|w| w.bounds)
}

fn draw(mtm: MainThreadMarker, q: &Pending, content: &Content, me: &AnyObject) -> Drawn {
    let icon = q.connect.as_ref().and_then(|c| c.icon.clone());
    ask_panel::draw(
        mtm,
        content,
        icon.as_deref(),
        me,
        sel!(answerConnect:),
        sel!(askMore:),
    )
}

/// Bring the panels up to date with the questions: one per session,
/// placed by the window of its session, gone when its card is.
pub(super) fn refresh_panels(
    st: &mut State,
    mtm: MainThreadMarker,
    wins: &[Win],
    screens: &[Rect],
    main_h: f64,
    me: &AnyObject,
) {
    let has: BTreeSet<String> = st.panels.keys().cloned().collect();
    let picked = panels::pick(&st.questions, &has);
    let open: Vec<(String, SessionKey)> = st
        .panels
        .iter()
        .map(|(id, p)| (id.clone(), p.key.clone()))
        .collect();
    let close = panels::to_close(&open, |id, _| {
        st.panels
            .get(id)
            .is_some_and(|p| broker::connect_card_retired(&st.env, &p.card).unwrap_or(false))
    });
    for id in &close {
        if let Some(p) = st.panels.remove(id) {
            p.drawn.panel.orderOut(None);
        }
    }
    for q in picked {
        let (Some(c), Some(key), Some(content)) =
            (&q.connect, panels::session_of(q), panels::content(q))
        else {
            continue;
        };
        if close.contains(&q.id) || st.panels.iter().any(|(id, p)| id != &q.id && p.key == key) {
            continue;
        }
        let win = window_of(wins, c.pid);
        let p = st.panels.entry(q.id.clone()).or_insert_with(|| AskPanel {
            drawn: draw(mtm, q, &content, me),
            content: content.clone(),
            key,
            card: q.clone(),
            rect: None,
            shown: false,
            sandbox: q.sandbox.clone(),
            harness: q.harness.clone(),
        });
        if p.content != content {
            // New text (the count of the queue): the same panel, drawn
            // again in place, so it never leaves the screen.
            let icon = c.icon.clone();
            ask_panel::redraw(
                &mut p.drawn,
                mtm,
                &content,
                icon.as_deref(),
                me,
                sel!(answerConnect:),
                sel!(askMore:),
            );
            p.content = content;
            p.rect = None;
        }
        let rect = crate::mark::panel_rect(win, p.drawn.size, screens);
        if p.rect != Some(rect) {
            p.rect = Some(rect);
            p.drawn
                .panel
                .setFrame_display(ns_rect(rect.flip(main_h)), true);
        }
        if !p.shown {
            // In front, without the keyboard: the user's typing stays
            // where it is until a click into the panel.
            p.drawn.panel.orderFrontRegardless();
            p.shown = true;
        }
    }
    let own = std::process::id();
    let rows: Vec<crate::mark::PanelRow> = st
        .panels
        .iter()
        .map(|(id, p)| crate::mark::PanelRow {
            question: id.clone(),
            window: p.number() as u32,
            pid: own,
            sandbox: p.sandbox.clone(),
            harness: p.harness.clone(),
        })
        .collect();
    let sig = format!("{rows:?}");
    if sig != st.panels_sig {
        st.panels_sig = sig;
        crate::mark::write_panels(&st.env, &rows);
    }
}

/// The question of the panel of a window number.
fn question_of(st: &State, number: isize) -> Option<String> {
    st.panels
        .iter()
        .find(|(_, p)| p.number() == number)
        .map(|(id, _)| id.clone())
}

fn answer(st: &mut State, id: &str, tag: isize) {
    let Some(answer) = usize::try_from(tag)
        .ok()
        .and_then(|i| ConnectAnswer::ALL.get(i).copied())
    else {
        return;
    };
    if let Err(e) = broker::answer_connect(&st.env, id, answer) {
        log::warn!("the firewall panel could not answer {id}: {e:#}");
        // The write may have failed or its result may be unknown. Keep the
        // panel until the store confirms a selection or retirement.
        return;
    }
    if let Some(p) = st.panels.remove(id) {
        p.drawn.panel.orderOut(None);
    }
    st.questions.retain(|q| q.id != id);
}

/// A click on an answer button: the answer goes into the store, and the
/// panel goes at once.
pub(super) fn answer_from_button(st: &mut State, sender: &AnyObject) {
    // SAFETY: the sender of `answerConnect:` is one of the buttons, a view.
    let view: &NSView = unsafe { &*(sender as *const AnyObject).cast::<NSView>() };
    let tag: isize = unsafe { msg_send![sender, tag] };
    let Some(window) = view.window() else {
        return;
    };
    let Some(id) = question_of(st, window.windowNumber()) else {
        return;
    };
    answer(st, &id, tag);
}

/// The menu of the `More…` button: the answers past the first three.
/// `menu_for` remembers the question for [`answer_from_menu`]. The caller
/// pops the menu up after the state is let go.
pub(super) fn more_menu(
    st: &mut State,
    mtm: MainThreadMarker,
    sender: &AnyObject,
    me: &AnyObject,
) -> Option<Retained<NSMenu>> {
    // SAFETY: the sender of `askMore:` is the `More…` button, a view.
    let view: &NSView = unsafe { &*(sender as *const AnyObject).cast::<NSView>() };
    let id = question_of(st, view.window()?.windowNumber())?;
    let p = st.panels.get(&id)?;
    let menu = NSMenu::new(mtm);
    menu.setAutoenablesItems(false);
    for b in &p.content.menu {
        // VoiceOver reads the title of a menu item.
        menu.addItem(&item(
            mtm,
            &b.title,
            Some(sel!(answerConnectMenu:)),
            ask_panel::tag_of(b.answer),
            me,
        ));
    }
    st.menu_for = Some(id);
    Some(menu)
}

/// An item of the `More…` menu.
pub(super) fn answer_from_menu(st: &mut State, sender: &AnyObject) {
    let tag: isize = unsafe { msg_send![sender, tag] };
    if let Some(id) = st.menu_for.take() {
        answer(st, &id, tag);
    }
}
