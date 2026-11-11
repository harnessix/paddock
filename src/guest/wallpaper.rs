//! The desktop picture of the guest: plain white. A forwarded window shows
//! the guest display behind it for a moment while the guest catches up
//! with a resize; white is calmer there than a photo, and a guest
//! framebuffer has no alpha, so it cannot be transparent. The agent sets
//! it at each start through `NSWorkspace` in a JavaScript for Automation
//! child: no Apple event, so no Automation permission in the guest.

/// A 1 x 1 white PNG. macOS scales it to fill each screen.
pub const WHITE_PNG: [u8; 69] = [
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
    0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xff, 0xff, 0x3f,
    0x00, 0x05, 0xfe, 0x02, 0xfe, 0x0d, 0xef, 0x46, 0xb8, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e,
    0x44, 0xae, 0x42, 0x60, 0x82,
];

/// The file name of the picture in the agent's support folder.
pub const FILE: &str = "wallpaper-white.png";

/// The script that sets `path` on every screen, as filling.
pub fn script(path: &str) -> String {
    let p = path.replace('\\', "\\\\").replace('"', "\\\"");
    JS.replace("PATH", &p)
}

#[rustfmt::skip]
const JS: &str = "ObjC.import('AppKit'); var ws = $.NSWorkspace.sharedWorkspace; var url = $.NSURL.fileURLWithPath(\"PATH\"); var screens = $.NSScreen.screens; var ok = 0; for (var i = 0; i < screens.count; i++) { if (ws.setDesktopImageURLForScreenOptionsError(url, screens.objectAtIndex(i), $({}), null)) ok++; } ok + ' of ' + screens.count"; // not user text

/// Set the white picture. Errors are logged; the session goes on.
#[cfg(target_os = "macos")]
pub fn set_white(support: &std::path::Path) {
    let path = support.join(FILE);
    if std::fs::read(&path).ok().as_deref() != Some(&WHITE_PNG[..]) {
        if let Err(e) = std::fs::write(&path, WHITE_PNG) {
            log::warn!("wallpaper: cannot write {}: {e}", path.display());
            return;
        }
    }
    let out = std::process::Command::new("/usr/bin/osascript")
        .args(["-l", "JavaScript", "-e"])
        .arg(script(&path.to_string_lossy()))
        .stdin(std::process::Stdio::null())
        .output();
    match out {
        Ok(o) if o.status.success() => log::info!(
            "wallpaper: white on {} screens",
            String::from_utf8_lossy(&o.stdout).trim()
        ),
        Ok(o) => log::warn!(
            "wallpaper: osascript failed: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        ),
        Err(e) => log::warn!("wallpaper: cannot run osascript: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_picture_is_a_white_png_and_the_script_quotes_the_path() {
        assert_eq!(&WHITE_PNG[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(
            &WHITE_PNG[WHITE_PNG.len() - 8..WHITE_PNG.len() - 4],
            b"IEND"
        );
        let s = script("/a \"b\"/w.png");
        assert!(s.contains("fileURLWithPath(\"/a \\\"b\\\"/w.png\")"), "{s}");
        assert!(s.contains("setDesktopImageURLForScreenOptionsError"));
    }
}
