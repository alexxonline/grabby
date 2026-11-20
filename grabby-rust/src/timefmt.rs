//! ISO-8601 helpers matching Python's `datetime.isoformat()` / `fromisoformat()` usage.

use chrono::{DateTime, FixedOffset, Local, NaiveDate, NaiveDateTime, TimeZone, Utc};

fn frac(micros: u32) -> String {
    if micros == 0 { String::new() } else { format!(".{micros:06}") }
}

/// `datetime.now(timezone.utc).isoformat()`.
pub fn now_utc() -> String {
    utc_iso(Utc::now())
}

pub fn utc_iso(d: DateTime<Utc>) -> String {
    format!("{}{}+00:00", d.format("%Y-%m-%dT%H:%M:%S"), frac(d.timestamp_subsec_micros()))
}

/// `datetime.astimezone().isoformat()` for a local time.
pub fn local_aware(d: DateTime<Local>) -> String {
    format!("{}{}{}", d.format("%Y-%m-%dT%H:%M:%S"), frac(d.timestamp_subsec_micros()), d.format("%:z"))
}

/// `datetime.now().isoformat()` (naive local time).
pub fn now_local_naive() -> String {
    let d = Local::now().naive_local();
    format!("{}{}", d.format("%Y-%m-%dT%H:%M:%S"), frac(d.and_utc().timestamp_subsec_micros()))
}

/// A parsed `fromisoformat` value: aware or naive.
pub enum Parsed {
    Aware(DateTime<FixedOffset>),
    Naive(NaiveDateTime),
}

/// `datetime.fromisoformat(value.replace('Z', '+00:00'))`.
pub fn parse_iso(value: &str) -> Option<Parsed> {
    let v = value.trim().replace('Z', "+00:00");
    for fmt in ["%Y-%m-%dT%H:%M:%S%.f%:z", "%Y-%m-%d %H:%M:%S%.f%:z", "%Y-%m-%dT%H:%M%:z", "%Y-%m-%dT%H:%M:%S%.f%z"] {
        if let Ok(d) = DateTime::parse_from_str(&v, fmt) {
            return Some(Parsed::Aware(d));
        }
    }
    for fmt in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M", "%Y-%m-%d %H:%M"] {
        if let Ok(d) = NaiveDateTime::parse_from_str(&v, fmt) {
            return Some(Parsed::Naive(d));
        }
    }
    NaiveDate::parse_from_str(&v, "%Y-%m-%d").ok().and_then(|d| d.and_hms_opt(0, 0, 0)).map(Parsed::Naive)
}

fn naive_local(n: NaiveDateTime) -> DateTime<Utc> {
    Local.from_local_datetime(&n).earliest().map(|d| d.with_timezone(&Utc)).unwrap_or_else(|| n.and_utc())
}

/// sync.utc(): normalise to UTC; naive values are local when `local`, else UTC.
pub fn to_utc(value: &str, local: bool) -> Option<String> {
    let d = match parse_iso(value)? {
        Parsed::Aware(d) => d.with_timezone(&Utc),
        Parsed::Naive(n) if local => naive_local(n),
        Parsed::Naive(n) => n.and_utc(),
    };
    Some(utc_iso(d))
}

/// `fromisoformat(..).timestamp()` (naive = local time), for sorting.
pub fn timestamp(value: &str) -> f64 {
    let d = match parse_iso(value) {
        Some(Parsed::Aware(d)) => d.with_timezone(&Utc),
        Some(Parsed::Naive(n)) => naive_local(n),
        None => return 0.0,
    };
    d.timestamp() as f64 + f64::from(d.timestamp_subsec_micros()) / 1e6
}

/// A short human date for the UI, like the web app's `toLocaleDateString` with time.
pub fn display(value: &str) -> String {
    let d = match parse_iso(value) {
        Some(Parsed::Aware(d)) => d.with_timezone(&Local),
        Some(Parsed::Naive(n)) => naive_local(n).with_timezone(&Local),
        None => return value.to_owned(),
    };
    d.format("%b %-d, %Y, %H:%M").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_python_formats() {
        assert_eq!(to_utc("2026-10-05T14:00:00Z", false).unwrap(), "2026-10-05T14:00:00+00:00");
        assert_eq!(to_utc("2026-10-05T16:00:00.250000+02:00", false).unwrap(), "2026-10-05T14:00:00.250000+00:00");
        assert_eq!(to_utc("2026-10-05 14:00:00", false).unwrap(), "2026-10-05T14:00:00+00:00");
        assert!(to_utc("not a date", false).is_none());
        assert!(timestamp("2026-10-05T14:00:00+00:00") > 0.0);
    }
}
