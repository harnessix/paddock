//! POSIX basic regular expressions for grep, evaluated inside the broker.

use anyhow::{bail, Result};
use std::ffi::CString;
use std::mem::MaybeUninit;

pub(super) struct Pattern(libc::regex_t);

impl Pattern {
    pub fn compile(text: &str) -> Result<Self> {
        let text = CString::new(text).map_err(|_| anyhow::anyhow!("grep pattern contains NUL"))?;
        let mut regex = MaybeUninit::<libc::regex_t>::uninit();
        // regcomp initializes the expression on success. REG_NOSUB means no
        // match array is needed for the line-by-line search.
        // SAFETY: `regex` points to writable storage, and `text` is NUL-terminated.
        let code = unsafe { libc::regcomp(regex.as_mut_ptr(), text.as_ptr(), libc::REG_NOSUB) };
        if code != 0 {
            bail!("invalid grep regular expression (error {code})");
        }
        // SAFETY: a successful regcomp initialized the expression.
        Ok(Self(unsafe { regex.assume_init() }))
    }

    pub fn matches(&self, line: &str) -> bool {
        let Ok(line) = CString::new(line) else {
            return false;
        };
        // SAFETY: the expression is initialized and the line is NUL-terminated.
        unsafe { libc::regexec(&self.0, line.as_ptr(), 0, std::ptr::null_mut(), 0) == 0 }
    }
}

impl Drop for Pattern {
    fn drop(&mut self) {
        // SAFETY: this expression was initialized by regcomp and is freed once.
        unsafe { libc::regfree(&mut self.0) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_basic_regular_expressions() {
        let pattern = Pattern::compile("canary-[0-9][0-9]").unwrap();
        assert!(pattern.matches("found canary-42"));
        assert!(!pattern.matches("canary-xx"));
        assert!(Pattern::compile("[").is_err());
    }
}
