//! Local admission gates for a wake.
use anyhow::{bail, Result};

fn minute(s: &str) -> Result<u16> {
    let (h, m) = s
        .split_once(':')
        .ok_or_else(|| anyhow::anyhow!("expected HH:MM"))?;
    if h.len() != 2 || m.len() != 2 {
        bail!("expected HH:MM");
    }
    let h: u16 = h.parse()?;
    let m: u16 = m.parse()?;
    if h >= 24 || m >= 60 {
        bail!("invalid time");
    }
    Ok(h * 60 + m)
}

pub fn quiet_range(s: &str) -> Result<(u16, u16)> {
    let (start, end) = s
        .split_once('-')
        .ok_or_else(|| anyhow::anyhow!("expected HH:MM-HH:MM"))?;
    let range = (minute(start)?, minute(end)?);
    if range.0 == range.1 {
        bail!("quiet hours must have different start and end");
    }
    Ok(range)
}

pub fn is_quiet(range: (u16, u16), now: u16) -> bool {
    if range.0 < range.1 {
        now >= range.0 && now < range.1
    } else {
        now >= range.0 || now < range.1
    }
}

pub fn local_minute(now: u64) -> Result<u16> {
    let t: libc::time_t = now.try_into()?;
    let mut result = std::mem::MaybeUninit::<libc::tm>::uninit();
    if unsafe { libc::localtime_r(&t, result.as_mut_ptr()) }.is_null() {
        bail!("cannot read local time");
    }
    let time = unsafe { result.assume_init() };
    Ok((time.tm_hour * 60 + time.tm_min) as u16)
}

pub fn host_load() -> Result<f64> {
    let mut load = 0.0;
    if unsafe { libc::getloadavg(&mut load, 1) } != 1 {
        bail!("cannot read host load");
    }
    Ok(load)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quiet_hours_use_a_fake_clock_across_midnight() {
        let q = quiet_range("22:00-07:00").unwrap();
        assert!(is_quiet(q, 23 * 60));
        assert!(is_quiet(q, 6 * 60));
        assert!(!is_quiet(q, 7 * 60));
        assert!(!is_quiet(q, 12 * 60));
        assert!(quiet_range("24:00-07:00").is_err());
        assert!(quiet_range("07:00-07:00").is_err());
    }
}
