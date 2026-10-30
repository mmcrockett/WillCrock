use jiff::{civil::Date, Span, Timestamp, Zoned};

pub const TZ: &str = "America/Chicago";

pub fn iso(ts: Timestamp) -> String {
    Timestamp::from_second(ts.as_second())
        .map(|t| t.to_string())
        .unwrap_or_default()
}

pub fn iso_millis(ts: Timestamp) -> String {
    ts.strftime("%Y-%m-%dT%H:%M:%S.%3fZ").to_string()
}

pub fn plus_millis(ts: Timestamp, ms: i64) -> Timestamp {
    Timestamp::from_millisecond(ts.as_millisecond() + ms).unwrap_or(ts)
}

pub fn seconds_between(from: Timestamp, to: Timestamp) -> f64 {
    (to.as_millisecond() - from.as_millisecond()) as f64 / 1000.0
}

pub fn central(ts: Timestamp) -> Result<Zoned, String> {
    ts.in_tz(TZ).map_err(|e| e.to_string())
}

pub fn central_at(now: Timestamp, days_offset: i64, hr: i8, min: i8) -> Result<Zoned, String> {
    let date: Date = central(now)?
        .date()
        .checked_add(Span::new().days(days_offset))
        .map_err(|e| e.to_string())?;
    date.at(hr, min, 0, 0).in_tz(TZ).map_err(|e| e.to_string())
}

pub fn rfc3339_offset(z: &Zoned) -> String {
    z.strftime("%Y-%m-%dT%H:%M:%S%:z").to_string()
}

pub fn parse(iso: &str) -> Result<Timestamp, String> {
    iso.parse::<Timestamp>()
        .map_err(|e| format!("bad time {iso:?}: {e}"))
}

pub fn slot_label(start: Timestamp, end: Option<Timestamp>) -> String {
    let Ok(from) = central(start) else {
        return iso(start);
    };
    match end.and_then(|e| central(e).ok()) {
        None => from.strftime("%a %b %-d %-l:%M%P").to_string(),
        Some(to) => format!(
            "{}-{}",
            from.strftime("%a %b %-d %-l:%M"),
            to.strftime("%-l:%M%P")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    #[test]
    fn central_at_uses_the_central_calendar_day() {
        let z = central_at(ts("2026-10-12T03:30:00Z"), 7, 7, 30).unwrap();
        assert_eq!(rfc3339_offset(&z), "2026-10-18T07:30:00-05:00");
        assert_eq!(iso(z.timestamp()), "2026-10-18T12:30:00Z");
    }

    #[test]
    fn central_at_crosses_the_dst_change() {
        let z = central_at(ts("2026-10-31T12:00:00Z"), 7, 7, 30).unwrap();
        assert_eq!(rfc3339_offset(&z), "2026-11-07T07:30:00-06:00");
    }

    #[test]
    fn slot_labels() {
        let start = ts("2026-10-18T13:30:00Z");
        let end = ts("2026-10-18T15:00:00Z");
        assert_eq!(slot_label(start, None), "Sun Oct 18 8:30am");
        assert_eq!(slot_label(start, Some(end)), "Sun Oct 18 8:30-10:00am");
    }

    #[test]
    fn iso_drops_fractional_seconds() {
        assert_eq!(iso(ts("2026-10-18T13:30:00.250Z")), "2026-10-18T13:30:00Z");
    }

    #[test]
    fn parse_accepts_millis_and_offsets() {
        assert!(parse("2026-10-18T13:30:00.000Z").is_ok());
        assert!(parse("2026-10-18T08:30:00-05:00").is_ok());
        assert!(parse("nope").is_err());
    }
}
