//! A fake [`Screen`] for the tests of computer use: the guest server
//! here, the host client in `macvm`. Its "PNG" is a marker and the
//! pixels, so a test sees which picture went out.

use super::computer::{AxWindow, Scene, Screen, Server, Shot};
use super::windows::RawWindow;
use crate::macvm::computer::{Code, Differ, Element, Failure, Geometry};
use crate::macvm::computer_input::Low;
use crate::macvm::proto::{Display, Frame, Level, Window};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The start of every fake PNG.
pub const PNG: &[u8] = b"\x89PNG-fake:";

#[derive(Debug, Clone)]
pub struct FakeState {
    pub display: Display,
    pub scale: f64,
    pub screen_recording: bool,
    pub accessibility: bool,
    pub locked: bool,
    pub front: Option<u32>,
    pub front_app: Option<String>,
    pub on_screen: Vec<RawWindow>,
    /// What the screen shows: a test changes it to make a new frame.
    pub pixels: Vec<u8>,
    pub captures: Vec<(Geometry, Option<Vec<u32>>, bool)>,
    pub encoded: u32,
    pub posted: Vec<Low>,
    pub activated: Vec<u32>,
    /// An activation brings this pid to the front.
    pub activate_to: Option<u32>,
    pub elements: Vec<Element>,
    pub ax_reads: Vec<(Vec<u32>, u32, u32)>,
    pub pressed: Vec<(u32, Vec<usize>, Option<String>)>,
    pub capture_error: Option<Failure>,
    pub post_error: Option<Failure>,
    /// The post with this number (from 1) fails once with an input error.
    pub fail_post: Option<usize>,
    /// The count of posts tried.
    pub tries: usize,
    /// After this many posted events another app comes to the front
    /// (the pid), as a key that opens a panel of the system does.
    pub front_after: Option<(usize, u32)>,
    /// After this many posted events this menu window opens, as a key
    /// that the app maps to a menu does.
    pub menu_after: Option<(usize, RawWindow)>,
    /// The keyboard layout: the character of a key code with cmd, and
    /// with shift when the bool is true. None: the layout cannot say.
    pub layout: Option<Vec<(u16, bool, char)>>,
    /// The count of reads of the window server list.
    pub looks: usize,
    /// At this read of the window server list (from 1) this menu window
    /// opens, as a menu that opens late after a press does.
    pub menu_at_look: Option<(usize, RawWindow)>,
    /// The sum of the sleeps of the server.
    pub slept: Duration,
}

impl Default for FakeState {
    fn default() -> Self {
        FakeState {
            display: Display { w: 1440, h: 900 },
            scale: 2.0,
            screen_recording: true,
            accessibility: true,
            locked: false,
            front: Some(500),
            front_app: Some("com.example.notes".into()),
            on_screen: Vec::new(),
            pixels: vec![1, 2, 3],
            captures: Vec::new(),
            encoded: 0,
            posted: Vec::new(),
            activated: Vec::new(),
            activate_to: None,
            elements: Vec::new(),
            ax_reads: Vec::new(),
            pressed: Vec::new(),
            capture_error: None,
            post_error: None,
            fail_post: None,
            tries: 0,
            front_after: None,
            menu_after: None,
            layout: None,
            looks: 0,
            menu_at_look: None,
            slept: Duration::ZERO,
        }
    }
}

pub struct FakeScreen(pub Arc<Mutex<FakeState>>);

impl FakeScreen {
    pub fn new() -> (Self, Arc<Mutex<FakeState>>) {
        let st = Arc::new(Mutex::new(FakeState::default()));
        (FakeScreen(st.clone()), st)
    }

    fn st(&self) -> std::sync::MutexGuard<'_, FakeState> {
        self.0.lock().unwrap()
    }
}

impl Screen for FakeScreen {
    fn display(&mut self) -> (Display, f64) {
        let s = self.st();
        (s.display, s.scale)
    }

    fn permissions(&mut self) -> (bool, bool) {
        let s = self.st();
        (s.screen_recording, s.accessibility)
    }

    fn on_screen(&mut self) -> Vec<RawWindow> {
        let mut s = self.st();
        s.looks += 1;
        if let Some((n, w)) = s.menu_at_look.clone() {
            if s.looks >= n {
                s.menu_at_look = None;
                s.on_screen.insert(0, w);
            }
        }
        s.on_screen.clone()
    }

    fn locked(&mut self) -> bool {
        self.st().locked
    }

    fn front_pid(&mut self) -> Option<u32> {
        self.st().front
    }

    fn front_app(&mut self) -> Option<String> {
        self.st().front_app.clone()
    }

    fn capture(
        &mut self,
        g: &Geometry,
        windows: Option<&[u32]>,
        cursor: bool,
    ) -> Result<Shot, Failure> {
        let mut s = self.st();
        if let Some(e) = &s.capture_error {
            return Err(e.clone());
        }
        if !s.screen_recording {
            return Err(Failure::new(Code::ScreenRecording, "no permission"));
        }
        s.captures.push((*g, windows.map(|w| w.to_vec()), cursor));
        Ok(Shot {
            width: g.width,
            height: g.height,
            digest: Differ::pixels(&s.pixels),
            image: Box::new(s.pixels.clone()),
        })
    }

    fn png(&mut self, shot: &Shot) -> Result<Vec<u8>, Failure> {
        self.st().encoded += 1;
        let px = shot
            .image
            .downcast_ref::<Vec<u8>>()
            .cloned()
            .unwrap_or_default();
        Ok([PNG, &px].concat())
    }

    fn post(&mut self, ev: &Low) -> Result<(), Failure> {
        let mut s = self.st();
        if let Some(e) = &s.post_error {
            return Err(e.clone());
        }
        s.tries += 1;
        if s.fail_post == Some(s.tries) {
            return Err(Failure::new(Code::Input, "post failed"));
        }
        s.posted.push(ev.clone());
        if let Some((n, pid)) = s.front_after {
            if s.posted.len() >= n {
                s.front = Some(pid);
            }
        }
        if let Some((n, w)) = s.menu_after.clone() {
            if s.posted.len() >= n {
                s.menu_after = None;
                s.on_screen.insert(0, w);
            }
        }
        Ok(())
    }

    fn activate(&mut self, pid: u32) -> Result<(), Failure> {
        let mut s = self.st();
        s.activated.push(pid);
        if let Some(p) = s.activate_to {
            s.front = Some(p);
        }
        Ok(())
    }

    fn ax(
        &mut self,
        windows: &[AxWindow],
        depth: u32,
        max_nodes: u32,
    ) -> Result<(Vec<Element>, bool), Failure> {
        let mut s = self.st();
        s.ax_reads
            .push((windows.iter().map(|w| w.id).collect(), depth, max_nodes));
        let all = s.elements.clone();
        let cut = all.len() > max_nodes as usize;
        Ok((all.into_iter().take(max_nodes as usize).collect(), cut))
    }

    fn ax_press(
        &mut self,
        window: AxWindow,
        path: &[usize],
        role: Option<&str>,
    ) -> Result<(String, String), Failure> {
        let mut s = self.st();
        s.pressed
            .push((window.id, path.to_vec(), role.map(str::to_string)));
        let id = crate::macvm::computer::element_id(window.id, path);
        let e = s
            .elements
            .iter()
            .find(|e| e.id == id)
            .cloned()
            .ok_or_else(|| Failure::new(Code::NoElement, "gone"))?;
        if role.is_some_and(|r| r != e.role) {
            return Err(Failure::new(Code::NoElement, "role"));
        }
        Ok((e.role, e.label))
    }

    fn key_char(&mut self, code: u16, shift: bool) -> Option<char> {
        let s = self.st();
        let layout = s.layout.as_ref()?;
        // A key that the layout lists without a character gives none.
        Some(
            layout
                .iter()
                .find(|(c, sh, _)| *c == code && *sh == shift)
                .map_or('\0', |(_, _, ch)| *ch),
        )
    }

    fn sleep(&mut self, d: Duration) {
        self.st().slept += d;
    }
}

/// A window of the app.
pub fn window(id: u32, pid: u32, frame: Frame) -> Window {
    Window {
        id,
        pid,
        app: "com.example.notes".into(),
        title: format!("Window {id}"), // not user text
        frame,
        level: Level::Normal,
        focused: id == 10,
        resizable: true,
        movable: true,
        ..Default::default()
    }
}

/// A window of the window server list.
pub fn raw(id: u32, pid: u32, owner: &str, bounds: Frame) -> RawWindow {
    RawWindow {
        id,
        pid,
        owner: owner.into(),
        bundle: Some(format!("com.example.{}", owner.to_lowercase())), // not user text
        layer: 0,
        bounds,
        alpha: 1.0,
    }
}

/// The scene of an app with pid 500 and two windows: 10 at 100,100
/// 800x600 and 11 at 300,200 400x300.
pub fn scene() -> Scene {
    Scene {
        app: Some("com.example.notes".into()),
        root: Some(500),
        pids: [500, 501].into_iter().collect(),
        windows: vec![
            window(10, 500, Frame::new(100.0, 100.0, 800.0, 600.0)),
            window(11, 500, Frame::new(300.0, 200.0, 400.0, 300.0)),
        ],
    }
}

/// A server on a fake screen with a scene that the test can change.
pub fn server(scene: Scene) -> (Server<FakeScreen>, Arc<Mutex<FakeState>>, Arc<Mutex<Scene>>) {
    let (screen, st) = FakeScreen::new();
    let sc = Arc::new(Mutex::new(scene));
    let sc2 = sc.clone();
    let server = Server::new(screen, Arc::new(move || sc2.lock().unwrap().clone()));
    (server, st, sc)
}
