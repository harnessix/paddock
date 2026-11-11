use std::io::Write;

pub(crate) fn respond(client: &mut impl Write, code: u16, reason: &str) {
    let _ = write!(
        client,
        "HTTP/1.1 {code} {reason}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
}

pub(super) fn fail(client: &mut impl Write, e: &anyhow::Error) {
    if e.to_string().starts_with("refused") {
        respond(client, 403, "Forbidden");
    } else {
        respond(client, 502, "Bad Gateway");
    }
}
