//! Property lists of app bundles and code signatures: the few values that
//! the app mode on macOS reads.

/// A property list as XML text. A binary plist is converted with `plutil`
/// on macOS; elsewhere it gives None.
pub fn plist_xml(bytes: &[u8]) -> Option<String> {
    if bytes.starts_with(b"bplist") {
        return binary_plist_xml(bytes);
    }
    let s = String::from_utf8_lossy(bytes);
    // codesign puts a blob header before the XML.
    let start = s.find("<?xml").or_else(|| s.find("<plist"))?;
    Some(s[start..].to_string())
}

#[cfg(target_os = "macos")]
fn binary_plist_xml(bytes: &[u8]) -> Option<String> {
    use std::io::Write as _;
    let mut c = std::process::Command::new("/usr/bin/plutil")
        .args(["-convert", "xml1", "-o", "-", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    c.stdin.take()?.write_all(bytes).ok()?;
    let out = c.wait_with_output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(not(target_os = "macos"))]
fn binary_plist_xml(_bytes: &[u8]) -> Option<String> {
    None
}

/// The text right after `<key>KEY</key>`, with spaces trimmed.
fn after_key<'a>(xml: &'a str, key: &str) -> Option<&'a str> {
    let tag = format!("<key>{}</key>", xml_escape(key));
    let i = xml.find(&tag)?;
    Some(xml[i + tag.len()..].trim_start())
}

/// A boolean value of a top-level key.
pub fn plist_bool(xml: &str, key: &str) -> Option<bool> {
    let rest = after_key(xml, key)?;
    let compact: String = rest
        .chars()
        .take(64)
        .filter(|c| !c.is_whitespace())
        .collect();
    // codesign prints either <true/> or <true></true>, depending on the tool.
    if compact.starts_with("<true/>") || compact.starts_with("<true></true>") {
        Some(true)
    } else if compact.starts_with("<false/>") || compact.starts_with("<false></false>") {
        Some(false)
    } else {
        None
    }
}

/// A string value of a top-level key.
pub fn plist_string(xml: &str, key: &str) -> Option<String> {
    let rest = after_key(xml, key)?.strip_prefix("<string>")?;
    let end = rest.find("</string>")?;
    Some(xml_unescape(&rest[..end]))
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// The entitlement of Apple's App Sandbox.
pub const APP_SANDBOX_KEY: &str = "com.apple.security.app-sandbox";

/// Whether entitlements (the XML that `codesign -d --entitlements :-` prints)
/// turn on the App Sandbox.
pub fn app_sandboxed(entitlements: &str) -> bool {
    plist_xml(entitlements.as_bytes())
        .and_then(|x| plist_bool(&x, APP_SANDBOX_KEY))
        .unwrap_or(false)
}
