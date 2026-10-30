use super::time;
use rusqlite::Connection;
use std::collections::HashMap;

pub const HISTORY_DAYS: i64 = 7;

pub struct Booking {
    pub id: String,
    pub start_time: String,
    pub end_time: String,
    pub court: Option<String>,
    pub checked_in: bool,
    pub refreshed_at: String,
}

pub struct Request {
    pub created_at: String,
    pub context: Option<String>,
    pub method: String,
    pub url: String,
    pub attempt: i64,
    pub status: Option<i64>,
    pub duration_ms: Option<i64>,
    pub request_body: Option<String>,
    pub response_body: Option<String>,
    pub error: Option<String>,
}

pub struct Run {
    pub started_at: String,
    pub command: String,
    pub status: String,
    pub summary: Option<String>,
    pub detail: Option<String>,
    pub requests: Vec<Request>,
}

pub struct Dashboard {
    pub bookings: Vec<Booking>,
    pub runs: Vec<Run>,
}

pub fn load(conn: &Connection, now: i64) -> rusqlite::Result<Dashboard> {
    let cutoff = time::plus_millis(
        jiff::Timestamp::from_second(now).unwrap_or(jiff::Timestamp::UNIX_EPOCH),
        -HISTORY_DAYS * 24 * 3600 * 1000,
    );
    Ok(Dashboard {
        bookings: bookings(conn)?,
        runs: runs_since(conn, &time::iso(cutoff))?,
    })
}

pub fn bookings(conn: &Connection) -> rusqlite::Result<Vec<Booking>> {
    conn.prepare(
        "SELECT id, start_time, end_time, court, checked_in, refreshed_at \
         FROM pickle_bookings ORDER BY start_time",
    )?
    .query_map([], |r| {
        Ok(Booking {
            id: r.get(0)?,
            start_time: r.get(1)?,
            end_time: r.get(2)?,
            court: r.get(3)?,
            checked_in: r.get::<_, i64>(4)? != 0,
            refreshed_at: r.get(5)?,
        })
    })?
    .collect()
}

pub fn runs_since(conn: &Connection, cutoff: &str) -> rusqlite::Result<Vec<Run>> {
    let mut requests: HashMap<i64, Vec<Request>> = HashMap::new();
    let mut stmt = conn.prepare(
        "SELECT r.run_id, r.created_at, r.context, r.method, r.url, r.attempt, r.status, \
         r.duration_ms, r.request_body, r.response_body, r.error \
         FROM pickle_requests r JOIN pickle_runs ON pickle_runs.id = r.run_id \
         WHERE pickle_runs.started_at >= ?1 ORDER BY r.id",
    )?;
    let rows = stmt.query_map([cutoff], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            Request {
                created_at: r.get(1)?,
                context: r.get(2)?,
                method: r.get(3)?,
                url: r.get(4)?,
                attempt: r.get(5)?,
                status: r.get(6)?,
                duration_ms: r.get(7)?,
                request_body: r.get(8)?,
                response_body: r.get(9)?,
                error: r.get(10)?,
            },
        ))
    })?;
    for row in rows {
        let (run_id, request) = row?;
        requests.entry(run_id).or_default().push(request);
    }

    conn.prepare(
        "SELECT id, started_at, command, status, summary, detail FROM pickle_runs \
         WHERE started_at >= ?1 ORDER BY started_at DESC, id DESC",
    )?
    .query_map([cutoff], |r| {
        let id: i64 = r.get(0)?;
        Ok(Run {
            started_at: r.get(1)?,
            command: r.get(2)?,
            status: r.get(3)?,
            summary: r.get(4)?,
            detail: r.get(5)?,
            requests: requests.remove(&id).unwrap_or_default(),
        })
    })?
    .collect()
}

pub fn is_future(iso: &str, now: i64) -> bool {
    time::parse(iso)
        .map(|t| t.as_second() > now)
        .unwrap_or(false)
}

pub fn local(iso: &str, format: &str) -> String {
    time::parse(iso)
        .and_then(time::central)
        .map(|z| z.strftime(format).to_string())
        .unwrap_or_else(|_| iso.to_string())
}

pub fn pretty_body(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .filter(|v| v.is_object() || v.is_array())
        .and_then(|v| serde_json::to_string_pretty(&v).ok())
        .unwrap_or_else(|| body.to_string())
}

pub fn pretty_message(text: &str) -> String {
    text.lines()
        .map(|line| match line.find(['{', '[']) {
            Some(i) if pretty_body(&line[i..]) != line[i..] => {
                format!("{}\n{}", line[..i].trim_end(), pretty_body(&line[i..]))
            }
            _ => line.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_memory;

    #[test]
    fn bookings_are_ordered_by_start() {
        let conn = open_memory();
        conn.execute_batch(
            "INSERT INTO pickle_bookings VALUES ('2', '2026-10-20T15:00:00.000Z', '2026-10-20T16:30:00.000Z', 'Pickleball 6B', 1, 'r');
             INSERT INTO pickle_bookings VALUES ('1', '2026-10-18T13:30:00.000Z', '2026-10-18T15:00:00.000Z', NULL, 0, 'r');",
        )
        .unwrap();
        let rows = bookings(&conn).unwrap();
        assert_eq!(rows[0].id, "1");
        assert!(!rows[0].checked_in);
        assert!(rows[1].checked_in);
        assert_eq!(rows[1].court.as_deref(), Some("Pickleball 6B"));
    }

    #[test]
    fn runs_group_their_requests_newest_first_within_cutoff() {
        let conn = open_memory();
        conn.execute_batch(
            "INSERT INTO pickle_runs (id, started_at, command, status) VALUES (1, '2026-10-01T10:00:00Z', 'sync', 'ok');
             INSERT INTO pickle_runs (id, started_at, command, status) VALUES (2, '2026-10-20T10:00:00Z', 'sync', 'ok');
             INSERT INTO pickle_runs (id, started_at, command, status, summary) VALUES (3, '2026-10-21T10:00:00Z', 'reserve-today', 'error', 'boom');
             INSERT INTO pickle_requests (run_id, created_at, method, url) VALUES (3, '2026-10-21T10:00:01Z', 'POST', '/a');
             INSERT INTO pickle_requests (run_id, created_at, method, url) VALUES (3, '2026-10-21T10:00:02Z', 'GET', '/b');
             INSERT INTO pickle_requests (run_id, created_at, method, url) VALUES (1, '2026-10-01T10:00:01Z', 'GET', '/old');",
        )
        .unwrap();
        let runs = runs_since(&conn, "2026-10-14T00:00:00Z").unwrap();
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].command, "reserve-today");
        assert_eq!(runs[0].summary.as_deref(), Some("boom"));
        assert_eq!(runs[0].requests.len(), 2);
        assert_eq!(runs[0].requests[0].url, "/a");
        assert!(runs[1].requests.is_empty());
    }

    #[test]
    fn load_uses_a_seven_day_cutoff() {
        let conn = open_memory();
        conn.execute_batch(
            "INSERT INTO pickle_runs (id, started_at, command, status) VALUES (1, '2026-10-20T00:00:00Z', 'sync', 'ok');
             INSERT INTO pickle_runs (id, started_at, command, status) VALUES (2, '2026-10-29T00:00:00Z', 'sync', 'ok');",
        )
        .unwrap();
        let now = "2026-10-30T00:00:00Z"
            .parse::<jiff::Timestamp>()
            .unwrap()
            .as_second();
        let data = load(&conn, now).unwrap();
        assert_eq!(data.runs.len(), 1);
    }

    #[test]
    fn formats_central_time_from_either_iso_shape() {
        assert_eq!(local("2026-10-18T13:30:00Z", "%-I:%M%P"), "8:30am");
        assert_eq!(
            local("2026-10-18T13:30:00.000Z", "%A - %b %d"),
            "Sunday - Oct 18"
        );
        assert_eq!(local("not a time", "%Y"), "not a time");
    }

    #[test]
    fn future_check_compares_to_now() {
        assert!(is_future("2026-10-18T13:30:00Z", 0));
        assert!(!is_future("1970-01-01T00:00:10Z", 100));
        assert!(!is_future("garbage", 0));
    }

    #[test]
    fn pretty_body_formats_json_only() {
        assert_eq!(pretty_body("{\"a\":1}"), "{\n  \"a\": 1\n}");
        assert_eq!(pretty_body("<html>504</html>"), "<html>504</html>");
        assert_eq!(pretty_body("42"), "42");
    }

    #[test]
    fn pretty_message_formats_trailing_json_only() {
        assert_eq!(
            pretty_message("x (status 500): {\"a\":1}"),
            "x (status 500):\n{\n  \"a\": 1\n}"
        );
        assert_eq!(pretty_message("plain text"), "plain text");
        assert_eq!(pretty_message("bad {json"), "bad {json");
    }
}
