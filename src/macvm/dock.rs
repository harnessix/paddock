//! The Dock face and the menus of an app session, macOS only: the app
//! delegate with the Dock menu, the app menu, the `Window` menu, the quit
//! through Cmd-Q, the Dock and the app menu, the observer of the active
//! Space, and the menu bar replica: the `NSMenu`s built from the tree of
//! `menu.rs` after the paddock app menu, rebuilt per the changes of its
//! diff, with `menuPress:` as the action of every item.
//!
//! The process gets its name and icon from the bundle that holds its
//! executable: `packaging/macos/shim/Info.plist` for the development build,
//! the per-app bundle for `paddock app run`. Nothing here sets a name or an
//! icon.

use super::menu::{Change, MenuItem};
use super::text;
use super::windows::{HostEvent, HostSink};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationDelegate, NSApplicationDidBecomeActiveNotification,
    NSApplicationDidChangeScreenParametersNotification, NSApplicationDidResignActiveNotification,
    NSApplicationTerminateReply, NSControlStateValueOff, NSControlStateValueOn,
    NSEventModifierFlags, NSMenu, NSMenuItem, NSWorkspace,
    NSWorkspaceActiveSpaceDidChangeNotification, NSWorkspaceDidActivateApplicationNotification,
};
use objc2_foundation::{
    NSDistributedNotificationCenter, NSNotification, NSNotificationCenter, NSString,
};
use std::cell::RefCell;

/// An entry of the Dock menu or the app menu that the user chose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockAction {
    About,
    Hide,
    FirewallRules,
    /// `Open <App> again`, shown after the app exited.
    OpenAgain,
    RestartAgent,
    /// `Restart guest`, shown after 30 s without health.
    RestartGuest,
    /// A window entry: raise it.
    Window(u32),
    /// `Resume <App>` of a suspended session, and a click on its icon.
    Resume,
}

/// What the Dock menu shows, kept by the session.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DockState {
    pub app: String,
    /// The app exited and a launch request is known: `Open <App> again`.
    pub app_exited: bool,
    /// No `health` for 30 s: `Restart guest`.
    pub silent: bool,
    /// The windows, front first.
    pub windows: Vec<(u32, String)>,
    /// The session is suspended: the menu offers `Resume`, a click on the
    /// icon resumes it.
    pub suspended: bool,
}

/// The tags of the fixed entries.
const TAG_ABOUT: isize = 1;
const TAG_HIDE: isize = 2;
const TAG_FIREWALL: isize = 3;
const TAG_OPEN_AGAIN: isize = 4;
const TAG_RESTART_AGENT: isize = 5;
const TAG_RESTART_GUEST: isize = 6;
const TAG_QUIT: isize = 7;
const TAG_RESUME: isize = 8;

pub struct DockIvars {
    sink: HostSink,
    state: RefCell<DockState>,
    /// The Accessibility path of each item of the replica, by the tag of
    /// its `NSMenuItem`. A stale item after a rebuild keeps its old path,
    /// which the fallback state then drops.
    paths: RefCell<Vec<Vec<usize>>>,
    /// The `Window` menu of the first pass, put back when the replica
    /// goes.
    window_menu: RefCell<Option<Retained<NSMenu>>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PaddockAppDelegate"]
    #[ivars = DockIvars]
    pub struct AppDelegate;

    unsafe impl NSObjectProtocol for AppDelegate {}

    unsafe impl NSApplicationDelegate for AppDelegate {
        #[unsafe(method_id(applicationDockMenu:))]
        fn dock_menu(&self, _app: &NSApplication) -> Option<Retained<NSMenu>> {
            Some(self.build_dock_menu())
        }

        /// `Quit` of the Dock and `terminate:`: the session runs its quit
        /// path and exits itself, so AppKit never terminates.
        #[unsafe(method(applicationShouldTerminate:))]
        fn should_terminate(&self, _app: &NSApplication) -> NSApplicationTerminateReply {
            (self.ivars().sink)(HostEvent::Quit);
            NSApplicationTerminateReply::TerminateCancel
        }

        /// A closed last window keeps the session.
        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn after_last_window(&self, _app: &NSApplication) -> bool {
            false
        }

        /// A click on the Dock icon of a session whose app exited opens
        /// the app again, as the Dock entry does.
        #[unsafe(method(applicationShouldHandleReopen:hasVisibleWindows:))]
        fn reopen(&self, _app: &NSApplication, visible: bool) -> bool {
            let suspended = self.ivars().state.borrow().suspended;
            if suspended {
                (self.ivars().sink)(HostEvent::Dock(DockAction::Resume));
            } else if !visible && self.ivars().state.borrow().app_exited {
                (self.ivars().sink)(HostEvent::Dock(DockAction::OpenAgain));
            }
            // AppKit unminimizes nothing of its own: the session's windows
            // follow the guest.
            false
        }
    }

    impl AppDelegate {
        #[unsafe(method(dockAction:))]
        fn dock_action(&self, sender: &NSMenuItem) {
            let ev = match sender.tag() {
                TAG_ABOUT => HostEvent::Dock(DockAction::About),
                TAG_HIDE => HostEvent::Dock(DockAction::Hide),
                TAG_FIREWALL => HostEvent::Dock(DockAction::FirewallRules),
                TAG_OPEN_AGAIN => HostEvent::Dock(DockAction::OpenAgain),
                TAG_RESTART_AGENT => HostEvent::Dock(DockAction::RestartAgent),
                TAG_RESTART_GUEST => HostEvent::Dock(DockAction::RestartGuest),
                TAG_QUIT => HostEvent::Quit,
                TAG_RESUME => HostEvent::Dock(DockAction::Resume),
                _ => return,
            };
            (self.ivars().sink)(ev);
        }

        #[unsafe(method(selectWindow:))]
        fn select_window(&self, sender: &NSMenuItem) {
            let id = sender.tag();
            if let Ok(id) = u32::try_from(id) {
                (self.ivars().sink)(HostEvent::Dock(DockAction::Window(id)));
            }
        }

        /// An item of the menu bar replica: its path to the session.
        #[unsafe(method(menuPress:))]
        fn menu_press(&self, sender: &NSMenuItem) {
            let path = usize::try_from(sender.tag())
                .ok()
                .and_then(|t| self.ivars().paths.borrow().get(t).cloned());
            if let Some(path) = path {
                (self.ivars().sink)(HostEvent::MenuPress(path));
            }
        }

        #[unsafe(method(spaceChanged:))]
        fn space_changed(&self, _note: &NSNotification) {
            (self.ivars().sink)(HostEvent::SpaceChanged);
        }

        #[unsafe(method(screensChanged:))]
        fn screens_changed(&self, _note: &NSNotification) {
            (self.ivars().sink)(HostEvent::ScreensChanged);
        }

        #[unsafe(method(appFocus:))]
        fn app_focus(&self, _note: &NSNotification) {
            (self.ivars().sink)(HostEvent::AppFocus);
        }

        #[unsafe(method(inputSourceChanged:))]
        fn input_source_changed(&self, _note: &NSNotification) {
            (self.ivars().sink)(HostEvent::InputSourceChanged);
        }
    }
);

impl AppDelegate {
    /// Make the delegate, put it on the application, build the main menu
    /// and observe the active Space.
    pub fn install(mtm: MainThreadMarker, sink: HostSink, app: &str) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(DockIvars {
            sink,
            state: RefCell::new(DockState {
                app: app.to_string(),
                ..DockState::default()
            }),
            paths: RefCell::new(Vec::new()),
            window_menu: RefCell::new(None),
        });
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        let ns_app = NSApplication::sharedApplication(mtm);
        ns_app.setDelegate(Some(ProtocolObject::from_ref(&*this)));
        this.build_main_menu(mtm, &ns_app);
        let center = NSWorkspace::sharedWorkspace().notificationCenter();
        // SAFETY: the delegate answers `spaceChanged:` and lives as long
        // as the session; the name is a constant of AppKit.
        unsafe {
            center.addObserver_selector_name_object(
                this.as_ref(),
                sel!(spaceChanged:),
                Some(NSWorkspaceActiveSpaceDidChangeNotification),
                None,
            );
        }
        // A screen plugged in or out, or a new arrangement.
        // SAFETY: the delegate answers `screensChanged:` and lives as long
        // as the session; the name is a constant of AppKit.
        unsafe {
            NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
                this.as_ref(),
                sel!(screensChanged:),
                Some(NSApplicationDidChangeScreenParametersNotification),
                None,
            );
        }
        // Another app became active, or the shim became or resigned the
        // active app: the input check reads the focus again (IME1).
        // SAFETY: the delegate answers `appFocus:` and lives as long as
        // the session; the names are constants of AppKit.
        unsafe {
            center.addObserver_selector_name_object(
                this.as_ref(),
                sel!(appFocus:),
                Some(NSWorkspaceDidActivateApplicationNotification),
                None,
            );
            for name in [
                NSApplicationDidBecomeActiveNotification,
                NSApplicationDidResignActiveNotification,
            ] {
                NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
                    this.as_ref(),
                    sel!(appFocus:),
                    Some(name),
                    None,
                );
            }
        }
        // The change of the host's input source is a distributed
        // notification of the Text Input Sources.
        let distributed = NSDistributedNotificationCenter::defaultCenter();
        // SAFETY: the delegate answers `inputSourceChanged:` and lives as
        // long as the session; the name comes from the framework.
        unsafe {
            distributed.addObserver_selector_name_object(
                this.as_ref(),
                sel!(inputSourceChanged:),
                Some(&super::tis::changed_notification()),
                None,
            );
        }
        this
    }

    /// Change what the Dock menu shows.
    pub fn update(&self, f: impl FnOnce(&mut DockState)) {
        f(&mut self.ivars().state.borrow_mut());
    }

    pub fn state(&self) -> DockState {
        self.ivars().state.borrow().clone()
    }

    /// One entry with the delegate as its target.
    fn item(&self, menu: &NSMenu, title: &str, sel: objc2::runtime::Sel, key: &str, tag: isize) {
        // SAFETY: the selectors are methods of this class.
        let item = unsafe {
            menu.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str(title),
                Some(sel),
                &NSString::from_str(key),
            )
        };
        item.setTag(tag);
        let target: &AnyObject = self.as_ref();
        unsafe { item.setTarget(Some(target)) };
        if tag == TAG_FIREWALL {
            // The firewall rules panel is not built yet.
            item.setEnabled(false);
        }
    }

    /// The app menu (`About`, `Firewall rules…`, `Hide`, `Quit`) and the
    /// `Window` menu, which AppKit fills with the session windows.
    fn build_main_menu(&self, mtm: MainThreadMarker, ns_app: &NSApplication) {
        let app = self.ivars().state.borrow().app.clone();
        let main = NSMenu::new(mtm);
        main.setAutoenablesItems(false);
        let app_item = NSMenuItem::new(mtm);
        main.addItem(&app_item);
        let app_menu = NSMenu::initWithTitle(mtm.alloc(), &NSString::from_str(&app));
        app_menu.setAutoenablesItems(false);
        self.item(
            &app_menu,
            &text::menu_about(&app),
            sel!(dockAction:),
            "",
            TAG_ABOUT,
        );
        app_menu.addItem(&NSMenuItem::separatorItem(mtm));
        self.item(
            &app_menu,
            &text::firewall_rules(),
            sel!(dockAction:),
            "",
            TAG_FIREWALL,
        );
        app_menu.addItem(&NSMenuItem::separatorItem(mtm));
        self.item(
            &app_menu,
            &text::menu_hide(&app),
            sel!(dockAction:),
            "h",
            TAG_HIDE,
        );
        self.item(&app_menu, &text::quit(), sel!(dockAction:), "q", TAG_QUIT);
        main.setSubmenu_forItem(Some(&app_menu), &app_item);
        let window_item = NSMenuItem::new(mtm);
        main.addItem(&window_item);
        let window_menu =
            NSMenu::initWithTitle(mtm.alloc(), &NSString::from_str(&text::menu_window()));
        main.setSubmenu_forItem(Some(&window_menu), &window_item);
        ns_app.setWindowsMenu(Some(&window_menu));
        *self.ivars().window_menu.borrow_mut() = Some(window_menu);
        ns_app.setMainMenu(Some(&main));
    }

    /// The changes of `menu::Fallback` on the main menu: the paddock app
    /// menu stays first, the app's menus follow it, or the `Window` menu
    /// of the first pass when `items` is empty.
    pub fn apply_menu(&self, items: &[MenuItem], changes: &[Change]) {
        let mtm = MainThreadMarker::from(self);
        let ns_app = NSApplication::sharedApplication(mtm);
        let Some(main) = ns_app.mainMenu() else {
            return;
        };
        for c in changes {
            match c {
                Change::Rebuild(position) if position.is_empty() => {
                    self.rebuild_bar(mtm, &ns_app, &main, items);
                }
                Change::Rebuild(position) => {
                    let menu = self.entry_at(&main, position).and_then(|e| e.submenu());
                    if let (Some(menu), Some(item)) = (menu, MenuItem::at(items, position)) {
                        self.fill(mtm, &menu, &item.children);
                    }
                }
                Change::State {
                    position,
                    enabled,
                    checked,
                } => {
                    if let Some(entry) = self.entry_at(&main, position) {
                        entry.setEnabled(*enabled);
                        entry.setState(state_of(*checked));
                    }
                }
            }
        }
    }

    /// The titles of the main menu after the app menu, for the control
    /// socket of the Mac checks.
    pub fn menu_titles(&self) -> Vec<String> {
        let mtm = MainThreadMarker::from(self);
        let Some(main) = NSApplication::sharedApplication(mtm).mainMenu() else {
            return Vec::new();
        };
        (1..main.numberOfItems())
            .filter_map(|i| main.itemAtIndex(i))
            .map(|e| e.title().to_string())
            .collect()
    }

    /// The `NSMenuItem` at a position of the tree: the first index counts
    /// from the entry after the paddock app menu.
    fn entry_at(&self, main: &NSMenu, position: &[usize]) -> Option<Retained<NSMenuItem>> {
        let (first, rest) = position.split_first()?;
        let mut entry = main.itemAtIndex(*first as isize + 1)?;
        for i in rest {
            entry = entry.submenu()?.itemAtIndex(*i as isize)?;
        }
        Some(entry)
    }

    /// Everything after the paddock app menu, built again from `items`,
    /// or the `Window` menu of the first pass for an empty tree.
    fn rebuild_bar(
        &self,
        mtm: MainThreadMarker,
        ns_app: &NSApplication,
        main: &NSMenu,
        items: &[MenuItem],
    ) {
        while main.numberOfItems() > 1 {
            main.removeItemAtIndex(main.numberOfItems() - 1);
        }
        self.ivars().paths.borrow_mut().clear();
        if items.is_empty() {
            if let Some(w) = self.ivars().window_menu.borrow().as_ref() {
                let entry = NSMenuItem::new(mtm);
                main.addItem(&entry);
                main.setSubmenu_forItem(Some(w), &entry);
                ns_app.setWindowsMenu(Some(w));
            }
            return;
        }
        // The guest's own Window menu lists the guest windows; AppKit
        // must not add the host windows to a menu the user cannot see.
        ns_app.setWindowsMenu(None);
        for top in items {
            let entry = NSMenuItem::new(mtm);
            entry.setTitle(&NSString::from_str(&top.title));
            entry.setEnabled(top.enabled);
            main.addItem(&entry);
            let menu = NSMenu::initWithTitle(mtm.alloc(), &NSString::from_str(&top.title));
            menu.setAutoenablesItems(false);
            self.fill(mtm, &menu, &top.children);
            main.setSubmenu_forItem(Some(&menu), &entry);
        }
    }

    /// The entries of one menu from the items, the submenus included.
    fn fill(&self, mtm: MainThreadMarker, menu: &NSMenu, items: &[MenuItem]) {
        menu.removeAllItems();
        for item in items {
            if item.is_separator() {
                menu.addItem(&NSMenuItem::separatorItem(mtm));
                continue;
            }
            let title = NSString::from_str(&item.title);
            let entry = if item.children.is_empty() {
                let (key, flags) = item
                    .shortcut
                    .as_ref()
                    .map(|s| (s.key_equivalent(), s.flags()))
                    .unwrap_or_default();
                // SAFETY: the selector is a method of this class, which
                // lives as long as the session.
                let entry = unsafe {
                    NSMenuItem::initWithTitle_action_keyEquivalent(
                        mtm.alloc(),
                        &title,
                        Some(sel!(menuPress:)),
                        &NSString::from_str(&key),
                    )
                };
                entry.setKeyEquivalentModifierMask(NSEventModifierFlags::from_bits_retain(
                    flags as usize,
                ));
                let target: &AnyObject = self.as_ref();
                unsafe { entry.setTarget(Some(target)) };
                let mut paths = self.ivars().paths.borrow_mut();
                entry.setTag(paths.len() as isize);
                paths.push(item.path.clone());
                entry
            } else {
                let entry = NSMenuItem::new(mtm);
                entry.setTitle(&title);
                let sub = NSMenu::initWithTitle(mtm.alloc(), &title);
                sub.setAutoenablesItems(false);
                self.fill(mtm, &sub, &item.children);
                entry.setSubmenu(Some(&sub));
                entry
            };
            entry.setEnabled(item.enabled);
            entry.setState(state_of(item.checked));
            menu.addItem(&entry);
        }
    }

    /// The Dock menu, built fresh at each click from the state: the windows,
    /// `Open <App> again`, `Restart guest agent`, `Restart guest`, `Firewall
    /// rules…`, `Quit`.
    fn build_dock_menu(&self) -> Retained<NSMenu> {
        let mtm = MainThreadMarker::from(self);
        let state = self.ivars().state.borrow().clone();
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);
        if state.suspended {
            let name = text::dock_resume(&state.app);
            self.item(&menu, &name, sel!(dockAction:), "", TAG_RESUME);
            menu.addItem(&NSMenuItem::separatorItem(mtm));
            self.item(&menu, &text::quit(), sel!(dockAction:), "", TAG_QUIT);
            return menu;
        }
        for (id, title) in &state.windows {
            let shown = if title.is_empty() {
                state.app.clone()
            } else {
                title.clone()
            };
            self.item(&menu, &shown, sel!(selectWindow:), "", *id as isize);
        }
        if !state.windows.is_empty() {
            menu.addItem(&NSMenuItem::separatorItem(mtm));
        }
        if state.app_exited {
            self.item(
                &menu,
                &text::open_again(&state.app),
                sel!(dockAction:),
                "",
                TAG_OPEN_AGAIN,
            );
        }
        self.item(
            &menu,
            &text::restart_agent(),
            sel!(dockAction:),
            "",
            TAG_RESTART_AGENT,
        );
        if state.silent {
            self.item(
                &menu,
                &text::restart_guest(),
                sel!(dockAction:),
                "",
                TAG_RESTART_GUEST,
            );
        }
        self.item(
            &menu,
            &text::firewall_rules(),
            sel!(dockAction:),
            "",
            TAG_FIREWALL,
        );
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        self.item(&menu, &text::quit(), sel!(dockAction:), "", TAG_QUIT);
        menu
    }
}

/// The `NSControlStateValue` of a checked item.
fn state_of(checked: bool) -> isize {
    if checked {
        NSControlStateValueOn
    } else {
        NSControlStateValueOff
    }
}
