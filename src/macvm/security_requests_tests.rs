//! One of every request of the protocol, for the password check of the
//! security checklist (`security_tests.rs`, item 3).

use super::{PASSWORD, TOKEN};
use crate::macvm::proto::{self, Frame, Request, VERSION};
use std::collections::BTreeMap;

/// One of every request. A new variant fails to compile here until it is
/// added, so the password check above sees every request.
pub(super) fn every_request() -> Vec<Request> {
    let f = Frame::new(0.0, 0.0, 1.0, 1.0);
    let all = vec![
        Request::Token {
            bench: false,
            now_ms: None,
            token: TOKEN.into(),
            v: VERSION,
        },
        Request::ClockSet {
            unix_ms: 1,
            password: super::PASSWORD.into(),
        },
        Request::WindowMove { id: 1, frame: f },
        Request::WindowRaise { id: 1 },
        Request::WindowMinimize { id: 1 },
        Request::WindowClose { id: 1 },
        Request::WindowFocus { id: 1 },
        Request::AppLaunch {
            path: "/a".into(),
            args: vec![],
            env: BTreeMap::new(),
        },
        Request::AppQuit {
            app: "a".into(),
            force: false,
        },
        Request::AppCopy {
            from: "/a".into(),
            to: "/b".into(),
            version: "1".into(),
            unquarantine: false,
        },
        Request::ClipboardSet {
            count: 1,
            items: vec![],
            transfer: None,
        },
        Request::ClipboardWatch { on: true },
        Request::Drop {
            window: 1,
            point: proto::Point { x: 0.0, y: 0.0 },
            paths: vec![],
        },
        Request::DisplaySet {
            w: 1,
            h: 1,
            scale: 1.0,
        },
        Request::DataPrepare {
            password: PASSWORD.into(),
            data_id: "d".into(),
        },
        Request::Configure {
            password: PASSWORD.into(),
            steps: vec![],
        },
        Request::Ping { t: 1 },
        Request::AgentUpdate {
            path: "/a".into(),
            sha256: "0".into(),
            version: "1".into(),
        },
        Request::AgentRestart,
        Request::InputSet {
            sources: vec![],
            current: "x".into(),
            reduce_motion: false,
            increase_contrast: false,
        },
        Request::LogFetch,
        Request::DiskLow,
        Request::Menu { app: "a".into() },
        Request::MenuPress { path: vec![] },
        Request::AxWatch { on: true },
        Request::AxTree { window: 1 },
        Request::AxPress {
            window: 1,
            path: vec![],
            role: "r".into(),
        },
        Request::AxSet {
            window: 1,
            path: vec![],
            role: "r".into(),
            value: "v".into(),
        },
        Request::BenchAnimate { window: 1, secs: 1 },
        Request::BenchText { window: 1 },
        Request::BenchHang,
        Request::BenchSpawnChild { argv: vec![] },
        Request::BenchPasteboard {
            action: "get".into(),
            text: String::new(),
        },
        Request::BenchEnv,
        Request::BenchDockMenu,
        Request::Shutdown {
            password: PASSWORD.into(),
        },
        Request::KeychainUnlock {
            password: PASSWORD.into(),
        },
        Request::SharesOpen,
        Request::BenchLock,
        Request::BenchKeychain,
        Request::LoginForward {
            port: 1455,
            ticket: "00112233445566778899aabbccddeeff".into(),
        },
    ];
    for r in &all {
        // The exhaustive match: a new variant must be added to the list above.
        match r {
            Request::Token { .. }
            | Request::ClockSet { .. }
            | Request::WindowMove { .. }
            | Request::WindowRaise { .. }
            | Request::WindowMinimize { .. }
            | Request::WindowClose { .. }
            | Request::WindowFocus { .. }
            | Request::AppLaunch { .. }
            | Request::AppQuit { .. }
            | Request::AppCopy { .. }
            | Request::ClipboardSet { .. }
            | Request::ClipboardWatch { .. }
            | Request::Drop { .. }
            | Request::DisplaySet { .. }
            | Request::DataPrepare { .. }
            | Request::Configure { .. }
            | Request::Ping { .. }
            | Request::AgentUpdate { .. }
            | Request::AgentRestart
            | Request::InputSet { .. }
            | Request::LogFetch
            | Request::DiskLow
            | Request::Menu { .. }
            | Request::MenuPress { .. }
            | Request::AxWatch { .. }
            | Request::AxTree { .. }
            | Request::AxPress { .. }
            | Request::AxSet { .. }
            | Request::BenchAnimate { .. }
            | Request::BenchText { .. }
            | Request::BenchHang
            | Request::BenchSpawnChild { .. }
            | Request::BenchPasteboard { .. }
            | Request::BenchEnv
            | Request::BenchDockMenu
            | Request::Shutdown { .. }
            | Request::KeychainUnlock { .. }
            | Request::SharesOpen
            | Request::BenchLock
            | Request::BenchKeychain
            | Request::LoginForward { .. } => {}
        }
    }
    all
}
