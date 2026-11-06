//! The header line of the guest proxy: the first line of each `guest-proxy`
//! connection, before the CONNECT or request line. It names the target and
//! the guest process behind the connection. One JSON object, at most
//! [`proto::LINE_MAX`] bytes, and it starts with `{`, so the host proxy
//! tells it from a request line by the first byte (checklist item 15).
//!
//! The identity is reported by the agent in the guest, where a root
//! process can forge it. The host uses it for rules and the log, never as
//! the only reason to allow a host that the mode forbids.

use serde::{Deserialize, Serialize};
use std::fmt;

/// The version of the header line.
pub const HEADER_VERSION: u32 = 1;

/// The guest process behind a connection: what the agent found out.
/// Every field can be unknown.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    /// The process that opened the loopback connection.
    #[serde(default)]
    pub pid: Option<u32>,
    /// Its bundle identifier from the code signature, such as
    /// `com.openai.chat`.
    #[serde(default)]
    pub app: Option<String>,
    /// The team identifier of the signature.
    #[serde(default)]
    pub team: Option<String>,
    /// The executable path in the guest.
    #[serde(default)]
    pub path: Option<String>,
    /// The outermost app of the parent chain, for a helper process.
    #[serde(default)]
    pub parent_app: Option<String>,
}

impl Identity {
    /// The app for a rule or a log line: the bundle, else the file name
    /// of the path, else `unknown`.
    pub fn app_name(&self) -> String {
        if let Some(app) = &self.app {
            return app.clone();
        }
        self.path
            .as_deref()
            .and_then(|p| p.rsplit('/').next())
            .filter(|n| !n.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| "unknown".into())
    }
}

impl fmt::Display for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "app = {}", self.app_name())?;
        if let Some(t) = &self.team {
            write!(f, ", team = {t}")?;
        }
        if let Some(p) = self.pid {
            write!(f, ", pid = {p}")?;
        }
        if let Some(p) = &self.parent_app {
            write!(f, ", parent_app = {p}")?;
        }
        Ok(())
    }
}

/// The header line.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyHeader {
    pub v: u32,
    /// The session token. The host refuses a header with another one.
    pub token: String,
    pub host: String,
    pub port: u16,
    #[serde(flatten)]
    pub id: Identity,
}

impl ProxyHeader {
    pub fn new(token: &str, host: &str, port: u16, id: Identity) -> Self {
        Self {
            v: HEADER_VERSION,
            token: token.to_string(),
            host: host.to_string(),
            port,
            id,
        }
    }

    /// The line with its newline.
    pub fn line(&self) -> String {
        let mut s = serde_json::to_string(self).unwrap_or_default();
        s.push('\n');
        s
    }

    /// Parse one line (without the newline). A line longer than
    /// [`super::proto::LINE_MAX`], not JSON, or of another version is an
    /// error.
    pub fn parse(line: &str) -> Result<Self, String> {
        if line.len() > super::proto::LINE_MAX {
            return Err(super::text::header_too_long(super::proto::LINE_MAX));
        }
        let h: Self =
            serde_json::from_str(line).map_err(|e| super::text::not_a_header(&e.to_string()))?;
        if h.v != HEADER_VERSION {
            return Err(super::text::header_version_mismatch(h.v, HEADER_VERSION));
        }
        if h.host.is_empty() {
            return Err("the header names no host".into());
        }
        Ok(h)
    }
}

/// No token in the debug text.
impl fmt::Debug for ProxyHeader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProxyHeader")
            .field("v", &self.v)
            .field("token", &"<token>")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("id", &self.id)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "plantedtoken0123plantedtoken0123";

    fn id() -> Identity {
        Identity {
            pid: Some(612),
            app: Some("com.openai.chat".into()),
            team: Some("2DC432GLL2".into()),
            path: Some("/Volumes/paddock/ChatGPT.app/Contents/MacOS/ChatGPT".into()),
            parent_app: None,
        }
    }

    #[test]
    fn header_round_trips() {
        let h = ProxyHeader::new(TOKEN, "chatgpt.com", 443, id());
        let line = h.line();
        assert!(line.starts_with("{\"v\":1,\"token\":\""), "{line}");
        assert!(line.ends_with('\n'));
        assert!(line.contains("\"parent_app\":null"), "{line}");
        assert!(line.contains("\"app\":\"com.openai.chat\""), "{line}");
        let back = ProxyHeader::parse(line.trim_end()).unwrap();
        assert_eq!(back, h);
        // A complete example line parses.
        let example = r#"{"v":1,"token":"x","host":"chatgpt.com","port":443,"pid":612,"app":"com.openai.chat","team":"2DC432GLL2","path":"/Volumes/paddock/apps/ChatGPT.app/Contents/MacOS/ChatGPT","parent_app":null}"#;
        let e = ProxyHeader::parse(example).unwrap();
        assert_eq!(e.id.pid, Some(612));
        assert_eq!(e.id.parent_app, None);
    }

    #[test]
    fn unknown_identity_is_fine() {
        let line = r#"{"v":1,"token":"x","host":"a.example","port":80}"#;
        let h = ProxyHeader::parse(line).unwrap();
        assert_eq!(h.id, Identity::default());
        assert_eq!(h.id.app_name(), "unknown");
        let i = Identity {
            path: Some("/usr/bin/curl".into()),
            ..Default::default()
        };
        assert_eq!(i.app_name(), "curl");
        assert_eq!(i.to_string(), "app = curl");
        assert_eq!(
            id().to_string(),
            "app = com.openai.chat, team = 2DC432GLL2, pid = 612"
        );
    }

    #[test]
    fn bad_lines_are_errors() {
        assert!(ProxyHeader::parse("CONNECT a:443 HTTP/1.1").is_err());
        assert!(ProxyHeader::parse("{}").is_err());
        assert!(ProxyHeader::parse(r#"{"v":2,"token":"x","host":"a","port":1}"#).is_err());
        assert!(ProxyHeader::parse(r#"{"v":1,"token":"x","host":"","port":1}"#).is_err());
        let long = format!(
            "{{\"v\":1,\"token\":\"x\",\"host\":\"a\",\"port\":1,\"path\":\"{}\"}}",
            "p".repeat(super::super::proto::LINE_MAX)
        );
        assert!(ProxyHeader::parse(&long).unwrap_err().contains("longer"));
    }

    #[test]
    fn debug_strips_the_token() {
        let h = ProxyHeader::new(TOKEN, "chatgpt.com", 443, id());
        let d = format!("{h:?}");
        assert!(!d.contains(TOKEN), "{d}");
        assert!(d.contains("chatgpt.com"));
    }
}
