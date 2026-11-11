//! Conservative registrable-site calculation for panel rules.
use std::net::IpAddr;

const TWO_LEVEL: &[&str] = &[
    "co.uk",
    "org.uk",
    "ac.uk",
    "gov.uk",
    "com.au",
    "net.au",
    "org.au",
    "co.jp",
    "ne.jp",
    "or.jp",
    "com.cn",
    "net.cn",
    "org.cn",
    "gov.cn",
    "edu.cn",
    "com.hk",
    "com.tw",
    "co.kr",
    "co.nz",
    "com.br",
    "com.mx",
    "co.in",
    "co.za",
    "com.sg",
    "github.io",
    "vercel.app",
    "netlify.app",
    "pages.dev",
    "workers.dev",
    "herokuapp.com",
    "appspot.com",
    "cloudfront.net",
    "amazonaws.com",
    "azurewebsites.net",
    "web.app",
    "firebaseapp.com",
];

pub fn registrable_site(host: &str) -> Option<String> {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if host.trim_matches(&['[', ']'][..]).parse::<IpAddr>().is_ok() || !super::valid_name(&host) {
        return None;
    }
    let labels: Vec<_> = host.split('.').collect();
    if labels.len() < 2 {
        return None;
    }
    let suffix = labels[labels.len() - 2..].join(".");
    let n = if TWO_LEVEL.contains(&suffix.as_str()) {
        3
    } else {
        2
    };
    (labels.len() >= n).then(|| labels[labels.len() - n..].join("."))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sites_and_public_suffixes() {
        for (host, expected) in [
            ("accounts.google.com", Some("google.com")),
            ("google.com", Some("google.com")),
            ("a.b.co.uk", Some("b.co.uk")),
            ("x.a.github.io", Some("a.github.io")),
            ("github.io", None),
            ("co.uk", None),
            ("com", None),
            ("127.0.0.1", None),
            ("[::1]", None),
            ("A.Example.COM.", Some("example.com")),
            ("bad..com", None),
        ] {
            assert_eq!(registrable_site(host).as_deref(), expected, "{host}");
        }
    }
}

#[cfg(test)]
mod answer_tests {
    use super::super::*;
    #[test]
    fn answers_make_site_and_temporary_rules() {
        let now = crate::state::now();
        let conn = Conn::new("app", "accounts.google.com", 443);
        let site = rule_of_answer(ConnectAnswer::AllowSite, &conn, BY_USER, now).unwrap();
        assert_eq!(site.host, "google.com");
        assert_eq!(site.port, Port::Any);
        let ip = Conn::new("app", "127.0.0.1", 443);
        let fallback = rule_of_answer(ConnectAnswer::AllowSite, &ip, BY_USER, now).unwrap();
        assert_eq!(fallback.host, "127.0.0.1");
        assert_eq!(fallback.port, Port::Num(443));
        let mut temporary =
            rule_of_answer(ConnectAnswer::AllowEverything, &conn, BY_USER, now).unwrap();
        assert_eq!(temporary.expires, Some(now + 600));
        assert!(matches!(
            decide(Mode::Ask, None, &[temporary.clone()], &conn),
            Decision::Allow(_)
        ));
        temporary.expires = Some(now);
        assert_eq!(decide(Mode::Ask, None, &[temporary], &conn), Decision::Ask);
    }
}
