use super::{ops, time};
use crate::trips;
use jiff::{civil::Date, Span};
use rusqlite::{params, Connection};

pub const DAYS: [&str; 7] = [
    "monday",
    "tuesday",
    "wednesday",
    "thursday",
    "friday",
    "saturday",
    "sunday",
];
pub const MINUTES: [i64; 3] = [60, 90, 120];
pub const COURT_CHOICES: [&str; 7] = ["any", "all", "5", "5C", "5D", "6A", "6B"];

pub struct Slot {
    pub id: i64,
    pub day: Option<String>,
    pub date: Option<String>,
    pub start: String,
    pub minutes: i64,
    pub court: String,
}

#[derive(Debug, PartialEq)]
pub struct SlotInput {
    pub day: Option<String>,
    pub date: Option<Date>,
    pub start: String,
    pub minutes: i64,
    pub court: String,
}

pub struct Form<'a> {
    pub kind: &'a str,
    pub day: &'a str,
    pub date: &'a str,
    pub start: &'a str,
    pub minutes: &'a str,
    pub court: &'a str,
}

pub fn parse(f: &Form, now: i64) -> Result<SlotInput, String> {
    let (hr, min) = time::parse_hhmm(f.start.trim()).ok_or("Start time is invalid.")?;
    let minutes = f
        .minutes
        .trim()
        .parse::<i64>()
        .ok()
        .filter(|m| MINUTES.contains(m))
        .ok_or("Length must be 60, 90 or 120 minutes.")?;
    let court = f.court.trim();
    ops::court_attempts(court)?;

    let (day, date) = match f.kind {
        "weekly" => {
            let day = f.day.trim().to_lowercase();
            if !DAYS.contains(&day.as_str()) {
                return Err("Day is invalid.".into());
            }
            (Some(day), None)
        }
        "once" => {
            let date: Date = f.date.trim().parse().map_err(|_| "Date is invalid.")?;
            let earliest = trips::today(now)
                .checked_add(Span::new().days(8))
                .map_err(|e| e.to_string())?;
            if date < earliest {
                return Err(format!(
                    "Bookings open 7 days ahead at 6am, so the date must be {earliest} or later."
                ));
            }
            (None, Some(date))
        }
        _ => return Err("Choose weekly or one time.".into()),
    };
    Ok(SlotInput {
        day,
        date,
        start: format!("{hr:02}:{min:02}"),
        minutes,
        court: court.to_string(),
    })
}

const COLUMNS: &str = "id, day, date, start, minutes, court";

fn from_row(r: &rusqlite::Row) -> rusqlite::Result<Slot> {
    Ok(Slot {
        id: r.get(0)?,
        day: r.get(1)?,
        date: r.get(2)?,
        start: r.get(3)?,
        minutes: r.get(4)?,
        court: r.get(5)?,
    })
}

pub fn list(conn: &Connection) -> rusqlite::Result<Vec<Slot>> {
    conn.prepare(&format!(
        "SELECT {COLUMNS} FROM pickle_slots ORDER BY date IS NOT NULL, \
         CASE day WHEN 'monday' THEN 1 WHEN 'tuesday' THEN 2 WHEN 'wednesday' THEN 3 \
         WHEN 'thursday' THEN 4 WHEN 'friday' THEN 5 WHEN 'saturday' THEN 6 ELSE 7 END, \
         date, start"
    ))?
    .query_map([], from_row)?
    .collect()
}

pub fn due(conn: &Connection, weekday: &str, date: &str) -> rusqlite::Result<Vec<Slot>> {
    conn.prepare(&format!(
        "SELECT {COLUMNS} FROM pickle_slots WHERE day = ?1 OR date = ?2 ORDER BY start, id"
    ))?
    .query_map([weekday, date], from_row)?
    .collect()
}

// False when the same day/date and start time is already scheduled.
pub fn add(conn: &Connection, s: &SlotInput, now: i64) -> rusqlite::Result<bool> {
    let added = conn.execute(
        "INSERT OR IGNORE INTO pickle_slots (day, date, start, minutes, court, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            s.day,
            s.date.map(|d| d.to_string()),
            s.start,
            s.minutes,
            s.court,
            now
        ],
    )?;
    Ok(added > 0)
}

pub fn delete(conn: &Connection, id: i64) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM pickle_slots WHERE id = ?1", [id])
}

pub fn prune_past(conn: &Connection, today: &str) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM pickle_slots WHERE date < ?1", [today])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_memory;

    // Mon 2026-10-12 06:00 Central
    const NOW: i64 = 1_791_802_800;

    fn form<'a>(kind: &'a str, day: &'a str, date: &'a str) -> Form<'a> {
        Form {
            kind,
            day,
            date,
            start: "7:30",
            minutes: "90",
            court: "5C",
        }
    }

    #[test]
    fn parses_weekly_and_once() {
        let w = parse(&form("weekly", "Saturday", ""), NOW).unwrap();
        assert_eq!(w.day.as_deref(), Some("saturday"));
        assert_eq!(
            (w.start.as_str(), w.minutes, w.court.as_str()),
            ("07:30", 90, "5C")
        );
        let o = parse(&form("once", "", "2026-10-20"), NOW).unwrap();
        assert_eq!(o.date.map(|d| d.to_string()).as_deref(), Some("2026-10-20"));
        assert!(o.day.is_none());
    }

    #[test]
    fn once_must_be_after_the_day_the_6am_run_can_still_reach() {
        assert!(parse(&form("once", "", "2026-10-19"), NOW).is_err());
        assert!(parse(&form("once", "", "2026-10-20"), NOW).is_ok());
    }

    #[test]
    fn rejects_bad_fields() {
        let bad = |f: Form| assert!(parse(&f, NOW).is_err());
        bad(Form {
            start: "730",
            ..form("weekly", "monday", "")
        });
        bad(Form {
            minutes: "45",
            ..form("weekly", "monday", "")
        });
        bad(Form {
            minutes: "x",
            ..form("weekly", "monday", "")
        });
        bad(Form {
            court: "9z",
            ..form("weekly", "monday", "")
        });
        bad(form("weekly", "funday", ""));
        bad(form("once", "", "nope"));
        bad(form("sometimes", "monday", ""));
    }

    fn input(day: Option<&str>, date: Option<&str>, start: &str) -> SlotInput {
        SlotInput {
            day: day.map(str::to_string),
            date: date.map(|d| d.parse().unwrap()),
            start: start.to_string(),
            minutes: 90,
            court: "any".into(),
        }
    }

    #[test]
    fn list_puts_weekly_first_in_week_order_then_one_offs_by_date() {
        let conn = open_memory();
        for s in [
            input(None, Some("2026-11-02"), "09:00"),
            input(Some("sunday"), None, "08:00"),
            input(Some("monday"), None, "10:00"),
            input(None, Some("2026-10-30"), "09:00"),
            input(Some("monday"), None, "07:00"),
        ] {
            assert!(add(&conn, &s, 0).unwrap());
        }
        let rows = list(&conn).unwrap();
        let order: Vec<String> = rows
            .iter()
            .map(|s| {
                format!(
                    "{}{}",
                    s.day.as_deref().or(s.date.as_deref()).unwrap(),
                    s.start
                )
            })
            .collect();
        assert_eq!(
            order,
            [
                "monday07:00",
                "monday10:00",
                "sunday08:00",
                "2026-10-3009:00",
                "2026-11-0209:00"
            ]
        );
    }

    #[test]
    fn duplicates_are_refused() {
        let conn = open_memory();
        assert!(add(&conn, &input(Some("monday"), None, "07:00"), 0).unwrap());
        assert!(!add(&conn, &input(Some("monday"), None, "07:00"), 0).unwrap());
        assert!(add(&conn, &input(None, Some("2026-11-02"), "07:00"), 0).unwrap());
        assert!(!add(&conn, &input(None, Some("2026-11-02"), "07:00"), 0).unwrap());
        assert_eq!(list(&conn).unwrap().len(), 2);
    }

    #[test]
    fn due_matches_the_weekday_and_the_target_date() {
        let conn = open_memory();
        add(&conn, &input(Some("monday"), None, "07:00"), 0).unwrap();
        add(&conn, &input(Some("tuesday"), None, "07:00"), 0).unwrap();
        add(&conn, &input(None, Some("2026-10-19"), "06:00"), 0).unwrap();
        add(&conn, &input(None, Some("2026-10-26"), "06:00"), 0).unwrap();
        let starts: Vec<String> = due(&conn, "monday", "2026-10-19")
            .unwrap()
            .into_iter()
            .map(|s| s.start)
            .collect();
        assert_eq!(starts, ["06:00", "07:00"]);
    }

    #[test]
    fn delete_and_prune_past() {
        let conn = open_memory();
        add(&conn, &input(Some("monday"), None, "07:00"), 0).unwrap();
        add(&conn, &input(None, Some("2026-10-10"), "07:00"), 0).unwrap();
        add(&conn, &input(None, Some("2026-10-12"), "07:00"), 0).unwrap();
        prune_past(&conn, "2026-10-12").unwrap();
        let rows = list(&conn).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(delete(&conn, rows[0].id).unwrap(), 1);
        assert_eq!(delete(&conn, rows[0].id).unwrap(), 0);
        assert_eq!(list(&conn).unwrap().len(), 1);
    }
}
