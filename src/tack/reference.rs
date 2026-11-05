//! The reference: the string that a process sees instead of a secret value.
//!
//! A reference is `tack://NAME`, where NAME is the name of the secret and
//! of the environment variable. It is built from the name only, so it never
//! holds the value or anything derived from it (no hash, no length).
//!
//! A string that starts with `tack://` but has another form is refused as
//! unknown, not taken as a value: a newer paddock can add forms, and an
//! older one must not mistake them for a plain value.

use std::fmt;

pub const SCHEME: &str = "tack://";

/// The longest secret name.
pub const MAX_NAME: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Reference {
    name: String,
}

/// Why a string is not a reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefError {
    /// It does not start with `tack://`: a plain value, or anything else.
    NotAReference,
    /// It starts with `tack://`, but the rest is not a name.
    Unknown(String),
}

impl fmt::Display for RefError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RefError::NotAReference => write!(f, "it does not start with {SCHEME}"),
            RefError::Unknown(why) => write!(
                f,
                "it starts with {SCHEME} but is not a reference that this paddock knows ({why}); a newer paddock may have written it"
            ),
        }
    }
}

impl std::error::Error for RefError {}

/// Whether `name` can name a secret: an environment variable name of
/// letters, digits and `_`, not starting with a digit.
pub fn valid_name(name: &str) -> bool {
    let mut b = name.bytes();
    matches!(b.next(), Some(c) if c.is_ascii_alphabetic() || c == b'_')
        && b.all(|c| c.is_ascii_alphanumeric() || c == b'_')
        && name.len() <= MAX_NAME
}

impl Reference {
    /// The reference for one secret. None when `name` is not a valid name.
    pub fn new(name: &str) -> Option<Self> {
        valid_name(name).then(|| Self {
            name: name.to_string(),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Read a reference. Surrounding white space is not part of it.
    pub fn parse(s: &str) -> Result<Self, RefError> {
        let Some(rest) = s.strip_prefix(SCHEME) else {
            return Err(RefError::NotAReference);
        };
        if rest.is_empty() {
            return Err(RefError::Unknown("no name".into()));
        }
        if rest.len() > MAX_NAME {
            return Err(RefError::Unknown(format!(
                "the name is longer than {MAX_NAME} characters"
            )));
        }
        Self::new(rest).ok_or_else(|| {
            RefError::Unknown(
                "a name has only letters, digits and '_', and does not start with a digit".into(),
            )
        })
    }

    /// Whether `s` is meant as a reference, valid or not.
    pub fn looks_like(s: &str) -> bool {
        s.starts_with(SCHEME)
    }
}

impl fmt::Display for Reference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{SCHEME}{}", self.name)
    }
}

impl std::str::FromStr for Reference {
    type Err = RefError;
    fn from_str(s: &str) -> Result<Self, RefError> {
        Self::parse(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prints_and_parses_back() {
        for name in ["GH_TOKEN", "_X", "a1", &"N".repeat(MAX_NAME)] {
            let r = Reference::new(name).unwrap();
            let text = r.to_string();
            assert_eq!(text, format!("tack://{name}"));
            assert_eq!(Reference::parse(&text).unwrap(), r);
            assert_eq!(text.parse::<Reference>().unwrap().name(), name);
        }
    }

    #[test]
    fn a_reference_holds_the_name_and_nothing_of_the_value() {
        // The printer takes no value: two secrets with the same name and
        // different values have the same reference, and the text is only
        // the scheme and the name.
        let r = Reference::new("GH_TOKEN").unwrap();
        assert_eq!(r.to_string().len(), SCHEME.len() + "GH_TOKEN".len());
        let value = "fake-value-0123456789";
        assert!(!r.to_string().contains(value));
        assert!(!format!("{r:?}").contains(value));
    }

    #[test]
    fn names_that_are_not_variable_names_are_refused() {
        for bad in [
            "",
            "1A",
            "A-B",
            "A B",
            "A=B",
            "Ä",
            "A/B",
            &"N".repeat(MAX_NAME + 1),
        ] {
            assert!(Reference::new(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn plain_values_are_not_references() {
        for s in [
            "ghp_fake",
            "",
            "tack:/X",
            "TACK://X",
            " tack://X",
            "https://x",
        ] {
            assert_eq!(Reference::parse(s), Err(RefError::NotAReference), "{s:?}");
        }
    }

    #[test]
    fn unknown_forms_are_named_as_such_and_never_taken_as_values() {
        for s in [
            "tack://",
            "tack://A/B",
            "tack://A?x=1",
            "tack://1A",
            "tack://A ",
            "tack://v2:A",
        ] {
            let e = Reference::parse(s).unwrap_err();
            assert!(matches!(e, RefError::Unknown(_)), "{s:?}: {e:?}");
            assert!(e.to_string().contains("newer paddock"), "{e}");
            assert!(Reference::looks_like(s));
        }
        let long = format!("tack://{}", "N".repeat(MAX_NAME + 1));
        assert!(Reference::parse(&long)
            .unwrap_err()
            .to_string()
            .contains("longer"));
    }
}
