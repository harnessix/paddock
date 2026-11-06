use crate::macvm::text as t;

fn has(s: &str, parts: &[&str]) {
    for p in parts {
        assert!(s.contains(p), "{s:?} lacks {p:?}");
    }
}

#[test]
fn ask_title() {
    assert_eq!(
        t::ask_title("ChatGPT", "api.example.com"),
        "ChatGPT wants to connect to api.example.com"
    );
}

#[test]
fn ask_detail() {
    assert_eq!(
        t::ask_detail(443),
        "port 443 · first connection to this host"
    );
}

#[test]
fn ask_more() {
    assert_eq!(t::ask_more(0), "");
    assert_eq!(t::ask_more(3), "and 3 more waiting");
}

#[test]
fn ask_button_label() {
    assert_eq!(
        t::ask_button_label("Deny", "a.example"),
        "Deny for a.example"
    );
}

#[test]
fn ask_more_button() {
    assert_eq!(t::ask_more_button(), "More…");
}

#[test]
fn asks_notice_summary() {
    has(
        &t::asks_notice_summary("ChatGPT", 21),
        &["ChatGPT", "21", "paused"],
    );
}

#[test]
fn asks_notice_detail() {
    has(
        &t::asks_notice_detail("ChatGPT", "chatgpt-desktop", 21),
        &[
            "many new connections",
            "paddock app status chatgpt-desktop",
            "Deny all",
            "Ask one by one",
            "21",
        ],
    );
}

#[test]
fn asks_notice_button() {
    assert_eq!(t::asks_notice_button(true), "Deny all");
    assert_eq!(t::asks_notice_button(false), "Ask one by one");
}

#[test]
fn asks_paused() {
    has(
        &t::asks_paused("ChatGPT", "chatgpt-desktop", 21),
        &[
            "more than 20",
            "21 open",
            "paused",
            "paddock app status chatgpt-desktop",
        ],
    );
}

#[test]
fn asks_resumed() {
    has(&t::asks_resumed(true, 4), &["denied the 4", "once"]);
    has(
        &t::asks_resumed(false, 4),
        &["one question at a time", "4 open"],
    );
}

#[test]
fn status_questions_row() {
    assert_eq!(t::status_questions_row(0, false, 0), "none open");
    has(
        &t::status_questions_row(2, false, 0),
        &["2 open", "one on the screen"],
    );
    has(
        &t::status_questions_row(21, true, 51),
        &[
            "21 open",
            "paused",
            "paddock app rules",
            "51 connections of the guest system",
        ],
    );
}

#[test]
fn status_question_line() {
    assert_eq!(
        t::status_question_line("com.openai.chat", "a.example", 443, 2, true),
        "  com.openai.chat -> a.example:443 (on the screen), 2 connections wait"
    );
    assert_eq!(
        t::status_question_line("x", "b.example", 80, 0, false),
        "  x -> b.example:80"
    );
}

#[test]
fn status_system_line() {
    has(
        &t::status_system_line("weather-edge.apple.com", 443, "com.apple.weather", 7),
        &[
            "weather-edge.apple.com:443",
            "x7",
            "guest system",
            "com.apple.weather",
        ],
    );
}

#[test]
fn ask_unknown_process() {
    has(
        &t::ask_unknown_process("a.example", 443),
        &[
            "a.example:443",
            "could not name",
            "without a question",
            "paddock app log",
        ],
    );
}
