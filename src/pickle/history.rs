use rusqlite::{params, Connection, OptionalExtension};

pub const RETENTION_DAYS: i64 = 30;

pub struct RequestLog<'a> {
    pub method: &'a str,
    pub url: &'a str,
    pub context: &'a str,
    pub attempt: i64,
    pub status: Option<i64>,
    pub duration_ms: i64,
    pub request_body: Option<&'a str>,
    pub response_body: Option<&'a str>,
    pub error: Option<&'a str>,
}

pub struct BookingRow {
    pub id: String,
    pub start_time: String,
    pub end_time: String,
    pub court: Option<String>,
    pub checked_in: bool,
}

pub fn prune(conn: &Connection, cutoff: &str) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM pickle_runs WHERE started_at < ?1", [cutoff])?;
    Ok(())
}

pub fn start_run(conn: &Connection, command: &str, user: &str, now: &str) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO pickle_runs (started_at, command, user) VALUES (?1, ?2, ?3)",
        params![now, command, user],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn finish_run(
    conn: &Connection,
    run_id: i64,
    status: &str,
    summary: Option<&str>,
    detail: Option<&str>,
    now: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE pickle_runs SET finished_at = ?1, status = ?2, summary = ?3, detail = ?4 WHERE id = ?5",
        params![now, status, summary, detail, run_id],
    )?;
    Ok(())
}

pub fn record_request(
    conn: &Connection,
    run_id: i64,
    r: &RequestLog,
    now: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO pickle_requests (run_id, created_at, context, method, url, attempt, status, \
         duration_ms, request_body, response_body, error) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            run_id,
            now,
            r.context,
            r.method,
            r.url,
            r.attempt,
            r.status,
            r.duration_ms,
            r.request_body,
            r.response_body,
            r.error
        ],
    )?;
    Ok(())
}

pub fn replace_bookings(conn: &Connection, rows: &[BookingRow], now: &str) -> rusqlite::Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM pickle_bookings", [])?;
    for row in rows {
        tx.execute(
            "INSERT OR REPLACE INTO pickle_bookings (id, start_time, end_time, court, checked_in, refreshed_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![row.id, row.start_time, row.end_time, row.court, row.checked_in, now],
        )?;
    }
    tx.commit()
}

pub fn delete_booking(conn: &Connection, id: &str) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM pickle_bookings WHERE id = ?1", [id])?;
    Ok(())
}

pub fn load_token(conn: &Connection) -> rusqlite::Result<Option<String>> {
    conn.query_row("SELECT token FROM pickle_token WHERE id = 1", [], |r| {
        r.get(0)
    })
    .optional()
}

pub fn save_token(conn: &Connection, token: &str, now: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO pickle_token (id, token, updated_at) VALUES (1, ?1, ?2) \
         ON CONFLICT(id) DO UPDATE SET token = excluded.token, updated_at = excluded.updated_at",
        params![token, now],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_memory;

    fn log(context: &str) -> RequestLog<'_> {
        RequestLog {
            method: "GET",
            url: "/x",
            context,
            attempt: 1,
            status: Some(200),
            duration_ms: 5,
            request_body: None,
            response_body: Some("{}"),
            error: None,
        }
    }

    #[test]
    fn run_lifecycle_is_recorded() {
        let conn = open_memory();
        let id = start_run(&conn, "sync", "Mike", "2026-10-18T13:30:00Z").unwrap();
        record_request(&conn, id, &log("bookings"), "2026-10-18T13:30:01Z").unwrap();
        finish_run(&conn, id, "ok", Some("done"), None, "2026-10-18T13:30:02Z").unwrap();
        let (status, summary, requests): (String, String, i64) = conn
            .query_row(
                "SELECT status, summary, (SELECT COUNT(*) FROM pickle_requests) FROM pickle_runs",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            (status.as_str(), summary.as_str(), requests),
            ("ok", "done", 1)
        );
    }

    #[test]
    fn prune_drops_old_runs_and_their_requests() {
        let conn = open_memory();
        let old = start_run(&conn, "sync", "M", "2026-09-01T00:00:00Z").unwrap();
        record_request(&conn, old, &log("a"), "2026-09-01T00:00:01Z").unwrap();
        start_run(&conn, "sync", "M", "2026-10-18T00:00:00Z").unwrap();
        prune(&conn, "2026-09-18T00:00:00Z").unwrap();
        let runs: i64 = conn
            .query_row("SELECT COUNT(*) FROM pickle_runs", [], |r| r.get(0))
            .unwrap();
        let requests: i64 = conn
            .query_row("SELECT COUNT(*) FROM pickle_requests", [], |r| r.get(0))
            .unwrap();
        assert_eq!((runs, requests), (1, 0));
    }

    #[test]
    fn replace_bookings_swaps_the_set() {
        let conn = open_memory();
        let row = |id: &str| BookingRow {
            id: id.to_string(),
            start_time: "s".into(),
            end_time: "e".into(),
            court: None,
            checked_in: id == "2",
        };
        replace_bookings(&conn, &[row("1")], "t1").unwrap();
        replace_bookings(&conn, &[row("2"), row("3")], "t2").unwrap();
        let ids: Vec<(String, bool)> = conn
            .prepare("SELECT id, checked_in FROM pickle_bookings ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(ids, vec![("2".to_string(), true), ("3".to_string(), false)]);
    }

    #[test]
    fn token_upserts() {
        let conn = open_memory();
        assert_eq!(load_token(&conn).unwrap(), None);
        save_token(&conn, "a", "t1").unwrap();
        save_token(&conn, "b", "t2").unwrap();
        assert_eq!(load_token(&conn).unwrap().as_deref(), Some("b"));
    }
}
