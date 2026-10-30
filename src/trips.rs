use crate::http::urlencode;
use crate::pickle::time;
use jiff::{civil::Date, tz::TimeZone, Timestamp};
use rusqlite::{params, Connection, Row};

const LINK_SCHEMES: [&str; 6] = ["http", "https", "tel", "mailto", "sms", "geo"];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Link,
    Map,
    Note,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Link, Kind::Map, Kind::Note];

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Link => "link",
            Kind::Map => "map",
            Kind::Note => "note",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Link => "Link",
            Kind::Map => "Map",
            Kind::Note => "Note",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

pub struct Trip {
    pub id: i64,
    pub location: String,
    pub start: Date,
    pub end: Date,
}

pub struct Item {
    pub id: i64,
    pub kind: Kind,
    pub label: Option<String>,
    pub value: String,
}

impl Item {
    pub fn href(&self) -> Option<String> {
        match self.kind {
            Kind::Link => Some(self.value.clone()),
            Kind::Map => Some(format!(
                "https://maps.google.com/?q={}",
                urlencode(&self.value)
            )),
            Kind::Note => None,
        }
    }

    pub fn display_label(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.value)
    }
}

pub struct TripInput {
    pub location: String,
    pub start: Date,
    pub end: Date,
}

pub struct ItemInput {
    pub kind: Kind,
    pub label: Option<String>,
    pub value: String,
}

pub fn parse_trip(location: &str, start: &str, end: &str) -> Result<TripInput, String> {
    let location = location.trim();
    if location.is_empty() {
        return Err("Location is required.".into());
    }
    let start: Date = start.trim().parse().map_err(|_| "Start date is invalid.")?;
    let end: Date = end.trim().parse().map_err(|_| "End date is invalid.")?;
    if end < start {
        return Err("End date is before the start date.".into());
    }
    Ok(TripInput {
        location: location.to_string(),
        start,
        end,
    })
}

pub fn parse_item(kind: &str, label: &str, value: &str) -> Result<ItemInput, String> {
    let kind = Kind::parse(kind).ok_or("Unknown item type.")?;
    let value = value.trim();
    if value.is_empty() {
        return Err("Value is required.".into());
    }
    if kind == Kind::Link && !has_link_scheme(value) {
        return Err("Links must start with http:, https:, tel:, mailto:, sms: or geo:.".into());
    }
    let label = label.trim();
    Ok(ItemInput {
        kind,
        label: (!label.is_empty()).then(|| label.to_string()),
        value: value.to_string(),
    })
}

fn has_link_scheme(value: &str) -> bool {
    value
        .split_once(':')
        .is_some_and(|(scheme, _)| LINK_SCHEMES.contains(&scheme.to_ascii_lowercase().as_str()))
}

pub fn format_range(a: Date, b: Date) -> String {
    if a.year() != b.year() {
        format!(
            "{} - {}",
            a.strftime("%b %-d, %Y"),
            b.strftime("%b %-d, %Y")
        )
    } else if a.month() != b.month() {
        format!("{} - {}", a.strftime("%b %-d"), b.strftime("%b %-d, %Y"))
    } else {
        format!("{}-{}", a.strftime("%b %-d"), b.strftime("%-d, %Y"))
    }
}

pub fn title(t: &Trip) -> String {
    format!("{} - {}", t.location, format_range(t.start, t.end))
}

pub fn today(now: i64) -> Date {
    let ts = Timestamp::from_second(now).unwrap_or(Timestamp::UNIX_EPOCH);
    time::central(ts)
        .map(|z| z.date())
        .unwrap_or_else(|_| ts.to_zoned(TimeZone::UTC).date())
}

fn date_col(r: &Row, i: usize) -> rusqlite::Result<Date> {
    let s: String = r.get(i)?;
    s.parse().map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(i, rusqlite::types::Type::Text, Box::new(e))
    })
}

fn trip_from_row(r: &Row) -> rusqlite::Result<Trip> {
    Ok(Trip {
        id: r.get(0)?,
        location: r.get(1)?,
        start: date_col(r, 2)?,
        end: date_col(r, 3)?,
    })
}

fn kind_col(r: &Row, i: usize) -> rusqlite::Result<Kind> {
    let s: String = r.get(i)?;
    Kind::parse(&s)
        .ok_or_else(|| rusqlite::Error::InvalidColumnType(i, s, rusqlite::types::Type::Text))
}

/// Upcoming (end >= today, soonest first) and past (most recent first).
pub fn list(conn: &Connection, today: Date) -> rusqlite::Result<(Vec<Trip>, Vec<Trip>)> {
    let all: Vec<Trip> = conn
        .prepare("SELECT id, location, start_date, end_date FROM trips ORDER BY start_date, id")?
        .query_map([], trip_from_row)?
        .collect::<rusqlite::Result<_>>()?;
    let (upcoming, mut past): (Vec<Trip>, Vec<Trip>) =
        all.into_iter().partition(|t| t.end >= today);
    past.reverse();
    Ok((upcoming, past))
}

pub fn get(conn: &Connection, id: i64) -> rusqlite::Result<Option<Trip>> {
    conn.prepare("SELECT id, location, start_date, end_date FROM trips WHERE id = ?1")?
        .query_map([id], trip_from_row)?
        .next()
        .transpose()
}

pub fn items(conn: &Connection, trip_id: i64) -> rusqlite::Result<Vec<Item>> {
    conn.prepare(
        "SELECT id, kind, label, value FROM trip_items WHERE trip_id = ?1 ORDER BY position",
    )?
    .query_map([trip_id], |r| {
        Ok(Item {
            id: r.get(0)?,
            kind: kind_col(r, 1)?,
            label: r.get(2)?,
            value: r.get(3)?,
        })
    })?
    .collect()
}

pub fn create(conn: &Connection, t: &TripInput, now: i64) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO trips (location, start_date, end_date, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![t.location, t.start.to_string(), t.end.to_string(), now],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn update(conn: &Connection, id: i64, t: &TripInput) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE trips SET location = ?1, start_date = ?2, end_date = ?3 WHERE id = ?4",
        params![t.location, t.start.to_string(), t.end.to_string(), id],
    )
}

pub fn delete(conn: &Connection, id: i64) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM trips WHERE id = ?1", [id])
}

pub fn add_item(conn: &Connection, trip_id: i64, i: &ItemInput) -> rusqlite::Result<usize> {
    conn.execute(
        "INSERT INTO trip_items (trip_id, position, kind, label, value) VALUES \
         (?1, COALESCE((SELECT MAX(position) FROM trip_items WHERE trip_id = ?1), 0) + 1, ?2, ?3, ?4)",
        params![trip_id, i.kind.as_str(), i.label, i.value],
    )
}

pub fn update_item(
    conn: &Connection,
    trip_id: i64,
    item_id: i64,
    i: &ItemInput,
) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE trip_items SET kind = ?1, label = ?2, value = ?3 WHERE id = ?4 AND trip_id = ?5",
        params![i.kind.as_str(), i.label, i.value, item_id, trip_id],
    )
}

pub fn delete_item(conn: &Connection, trip_id: i64, item_id: i64) -> rusqlite::Result<usize> {
    conn.execute(
        "DELETE FROM trip_items WHERE id = ?1 AND trip_id = ?2",
        params![item_id, trip_id],
    )
}

/// Swap with the neighbor above or below. Returns false when the item isn't in this trip;
/// moving past either end is a no-op that still returns true.
pub fn move_item(
    conn: &Connection,
    trip_id: i64,
    item_id: i64,
    up: bool,
) -> rusqlite::Result<bool> {
    let pos: Option<i64> = conn
        .query_row(
            "SELECT position FROM trip_items WHERE id = ?1 AND trip_id = ?2",
            params![item_id, trip_id],
            |r| r.get(0),
        )
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            e => Err(e),
        })?;
    let Some(pos) = pos else {
        return Ok(false);
    };
    let neighbor_sql = if up {
        "SELECT id, position FROM trip_items WHERE trip_id = ?1 AND position < ?2 \
         ORDER BY position DESC LIMIT 1"
    } else {
        "SELECT id, position FROM trip_items WHERE trip_id = ?1 AND position > ?2 \
         ORDER BY position LIMIT 1"
    };
    let neighbor: Option<(i64, i64)> = conn
        .prepare(neighbor_sql)?
        .query_map(params![trip_id, pos], |r| Ok((r.get(0)?, r.get(1)?)))?
        .next()
        .transpose()?;
    if let Some((other_id, other_pos)) = neighbor {
        conn.execute(
            "UPDATE trip_items SET position = CASE id WHEN ?1 THEN ?4 WHEN ?2 THEN ?3 END \
             WHERE trip_id = ?5 AND id IN (?1, ?2)",
            params![item_id, other_id, pos, other_pos, trip_id],
        )?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn d(s: &str) -> Date {
        s.parse().unwrap()
    }

    fn trip(conn: &Connection, loc: &str, start: &str, end: &str) -> i64 {
        create(conn, &parse_trip(loc, start, end).unwrap(), 0).unwrap()
    }

    fn item(conn: &Connection, trip_id: i64, kind: &str, value: &str) {
        add_item(conn, trip_id, &parse_item(kind, "", value).unwrap()).unwrap();
    }

    fn values(conn: &Connection, trip_id: i64) -> Vec<String> {
        items(conn, trip_id)
            .unwrap()
            .into_iter()
            .map(|i| i.value)
            .collect()
    }

    #[test]
    fn range_formats_match_the_php_pages() {
        assert_eq!(
            format_range(d("2026-06-21"), d("2026-06-26")),
            "Jun 21-26, 2026"
        );
        assert_eq!(
            format_range(d("2026-06-28"), d("2026-07-03")),
            "Jun 28 - Jul 3, 2026"
        );
        assert_eq!(
            format_range(d("2026-12-30"), d("2027-01-02")),
            "Dec 30, 2026 - Jan 2, 2027"
        );
    }

    #[test]
    fn title_joins_location_and_range() {
        let t = Trip {
            id: 1,
            location: "Galveston".into(),
            start: d("2026-06-21"),
            end: d("2026-06-26"),
        };
        assert_eq!(title(&t), "Galveston - Jun 21-26, 2026");
    }

    #[test]
    fn list_splits_upcoming_and_past_around_today() {
        let conn = db::open_memory();
        trip(&conn, "Old", "2026-01-01", "2026-01-05");
        trip(&conn, "Older", "2025-01-01", "2025-01-05");
        trip(&conn, "Ends today", "2026-05-30", "2026-06-01");
        trip(&conn, "Later", "2026-09-01", "2026-09-05");
        trip(&conn, "Soon", "2026-07-01", "2026-07-05");
        let (up, past) = list(&conn, d("2026-06-01")).unwrap();
        let names = |v: &[Trip]| v.iter().map(|t| t.location.clone()).collect::<Vec<_>>();
        assert_eq!(names(&up), ["Ends today", "Soon", "Later"]);
        assert_eq!(names(&past), ["Old", "Older"]);
    }

    #[test]
    fn today_uses_central_time() {
        // 2026-06-02T03:00:00Z is still June 1 in Chicago.
        assert_eq!(today(1_780_369_200), d("2026-06-01"));
    }

    #[test]
    fn deleting_a_trip_removes_its_items() {
        let conn = db::open_memory();
        let id = trip(&conn, "A", "2026-06-01", "2026-06-02");
        item(&conn, id, "note", "x");
        assert_eq!(delete(&conn, id).unwrap(), 1);
        assert!(get(&conn, id).unwrap().is_none());
        assert!(items(&conn, id).unwrap().is_empty());
    }

    #[test]
    fn items_append_and_move_within_bounds() {
        let conn = db::open_memory();
        let id = trip(&conn, "A", "2026-06-01", "2026-06-02");
        for v in ["a", "b", "c"] {
            item(&conn, id, "note", v);
        }
        assert_eq!(values(&conn, id), ["a", "b", "c"]);
        let ids: Vec<i64> = items(&conn, id).unwrap().iter().map(|i| i.id).collect();
        assert!(move_item(&conn, id, ids[2], true).unwrap());
        assert_eq!(values(&conn, id), ["a", "c", "b"]);
        assert!(move_item(&conn, id, ids[0], true).unwrap());
        assert!(move_item(&conn, id, ids[1], false).unwrap());
        assert_eq!(values(&conn, id), ["a", "c", "b"]);
        assert!(move_item(&conn, id, ids[0], false).unwrap());
        assert_eq!(values(&conn, id), ["c", "a", "b"]);
    }

    #[test]
    fn item_ops_are_scoped_to_their_trip() {
        let conn = db::open_memory();
        let a = trip(&conn, "A", "2026-06-01", "2026-06-02");
        let b = trip(&conn, "B", "2026-06-01", "2026-06-02");
        item(&conn, a, "note", "mine");
        let id = items(&conn, a).unwrap()[0].id;
        let edit = parse_item("note", "", "hijacked").unwrap();
        assert_eq!(update_item(&conn, b, id, &edit).unwrap(), 0);
        assert!(!move_item(&conn, b, id, true).unwrap());
        assert_eq!(delete_item(&conn, b, id).unwrap(), 0);
        assert_eq!(values(&conn, a), ["mine"]);
        assert_eq!(update_item(&conn, a, id, &edit).unwrap(), 1);
        assert_eq!(delete_item(&conn, a, id).unwrap(), 1);
    }

    #[test]
    fn update_changes_the_trip() {
        let conn = db::open_memory();
        let id = trip(&conn, "A", "2026-06-01", "2026-06-02");
        update(
            &conn,
            id,
            &parse_trip("B", "2026-07-01", "2026-07-09").unwrap(),
        )
        .unwrap();
        let t = get(&conn, id).unwrap().unwrap();
        assert_eq!(
            (t.location.as_str(), t.start, t.end),
            ("B", d("2026-07-01"), d("2026-07-09"))
        );
    }

    #[test]
    fn validation_rejects_bad_input() {
        assert!(parse_trip(" ", "2026-06-01", "2026-06-02").is_err());
        assert!(parse_trip("A", "nope", "2026-06-02").is_err());
        assert!(parse_trip("A", "2026-06-03", "2026-06-02").is_err());
        assert!(parse_trip("A", "2026-06-02", "2026-06-02").is_ok());
        assert!(parse_item("link", "", "javascript:alert(1)").is_err());
        assert!(parse_item("link", "", "JavaScript:alert(1)").is_err());
        assert!(parse_item("link", "", "example.com").is_err());
        assert!(parse_item("link", "", "tel:+15551234567").is_ok());
        assert!(parse_item("map", "", "  ").is_err());
        assert!(parse_item("bogus", "", "x").is_err());
        assert!(parse_item("note", "", "javascript:ok as text").is_ok());
    }

    #[test]
    fn hrefs_follow_the_kind() {
        let mk = |kind, value: &str| Item {
            id: 1,
            kind,
            label: None,
            value: value.into(),
        };
        assert_eq!(
            mk(Kind::Link, "https://a.b/c").href().unwrap(),
            "https://a.b/c"
        );
        assert_eq!(
            mk(Kind::Map, "1 Main St, Austin").href().unwrap(),
            "https://maps.google.com/?q=1%20Main%20St%2C%20Austin"
        );
        assert!(mk(Kind::Note, "hi").href().is_none());
        assert_eq!(mk(Kind::Map, "1 Main St").display_label(), "1 Main St");
    }
}
