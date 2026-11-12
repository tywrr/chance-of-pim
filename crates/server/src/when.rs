//! Dates and times: what tools accept, and how iCalendar values are shown.

use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;

pub fn zone(name: &str) -> Result<Tz, String> {
    name.parse::<Tz>().map_err(|_| {
        format!("unknown time zone {name:?} (use an IANA name such as America/Los_Angeles)")
    })
}

/// What a tool argument named: a whole day, or an instant.
pub enum When {
    Date(NaiveDate),
    Instant(DateTime<Utc>, NaiveDateTime),
}

/// Accepts `YYYY-MM-DD`, `YYYY-MM-DDTHH:MM[:SS]` (in `tz`), or RFC 3339 with an offset.
pub fn parse(input: &str, tz: Tz) -> Result<When, String> {
    let s = input.trim();
    if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Ok(When::Date(d));
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        let utc = dt.with_timezone(&Utc);
        return Ok(When::Instant(utc, utc.with_timezone(&tz).naive_local()));
    }
    for fmt in [
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
    ] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(s, fmt) {
            let local = tz
                .from_local_datetime(&naive)
                .earliest()
                .ok_or_else(|| format!("{s} does not exist in {tz} (clock change)"))?;
            return Ok(When::Instant(local.with_timezone(&Utc), naive));
        }
    }
    Err(format!(
        "could not read {input:?} as a date or time (use 2026-11-12 or 2026-11-12T14:30)"
    ))
}

pub fn stamp(utc: DateTime<Utc>) -> String {
    utc.format("%Y%m%dT%H%M%SZ").to_string()
}

/// The UTC stamp of a bound: a date means the start of that day in `tz`.
pub fn bound(input: &str, tz: Tz) -> Result<DateTime<Utc>, String> {
    match parse(input, tz)? {
        When::Instant(utc, _) => Ok(utc),
        When::Date(d) => tz
            .from_local_datetime(&d.and_hms_opt(0, 0, 0).expect("midnight"))
            .earliest()
            .map(|l| l.with_timezone(&Utc))
            .ok_or_else(|| format!("{input} has no midnight in {tz}")),
    }
}

/// An iCalendar DATE or DATE-TIME value, shown in `tz` when it is in UTC, else as written.
/// Returns the text and a sort key.
pub fn show(value: &str, tzid: Option<&str>, tz: Tz) -> (String, String) {
    let v = value.trim();
    if v.len() == 8
        && let Ok(d) = NaiveDate::parse_from_str(v, "%Y%m%d")
    {
        return (format!("{d} (all day)"), format!("{d}T00:00"));
    }
    if let Some(utc) = v.strip_suffix('Z')
        && let Ok(n) = NaiveDateTime::parse_from_str(utc, "%Y%m%dT%H%M%S")
    {
        let local = Utc.from_utc_datetime(&n).with_timezone(&tz);
        return (
            local.format("%Y-%m-%d %H:%M %Z").to_string(),
            local.format("%Y-%m-%dT%H:%M").to_string(),
        );
    }
    if let Ok(n) = NaiveDateTime::parse_from_str(v, "%Y%m%dT%H%M%S") {
        let key = n.format("%Y-%m-%dT%H:%M").to_string();
        return match tzid {
            Some(z) => (format!("{} {z}", n.format("%Y-%m-%d %H:%M")), key),
            None => (n.format("%Y-%m-%d %H:%M (floating)").to_string(), key),
        };
    }
    (v.to_owned(), v.to_owned())
}

pub fn now_stamp() -> String {
    stamp(Utc::now())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_shows() {
        let tz = zone("America/Los_Angeles").unwrap();
        assert_eq!(stamp(bound("2026-11-12", tz).unwrap()), "20261112T080000Z");
        assert_eq!(
            stamp(bound("2026-11-12T14:30", tz).unwrap()),
            "20261112T223000Z"
        );
        assert_eq!(
            stamp(bound("2026-11-12T14:30:00Z", tz).unwrap()),
            "20261112T143000Z"
        );
        assert_eq!(show("20261112T223000Z", None, tz).0, "2026-11-12 14:30 PST");
        assert_eq!(show("20261112", None, tz).0, "2026-11-12 (all day)");
        assert_eq!(
            show("20261112T090000", Some("Europe/Paris"), tz).0,
            "2026-11-12 09:00 Europe/Paris"
        );
    }
}
