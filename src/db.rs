use rusqlite::Connection;
use std::time::Duration;

const MIGRATIONS: &[&str] = &[
    "
CREATE TABLE users (
  id INTEGER PRIMARY KEY,
  email TEXT NOT NULL UNIQUE,
  name TEXT,
  is_admin INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  disabled_at INTEGER
);
CREATE TABLE login_codes (
  id INTEGER PRIMARY KEY,
  user_id INTEGER NOT NULL REFERENCES users(id),
  code_hash TEXT NOT NULL,
  expires_at INTEGER NOT NULL,
  attempts INTEGER NOT NULL DEFAULT 0,
  used_at INTEGER
);
CREATE TABLE sessions (
  id INTEGER PRIMARY KEY,
  user_id INTEGER NOT NULL REFERENCES users(id),
  token_hash TEXT NOT NULL UNIQUE,
  created_at INTEGER NOT NULL,
  last_seen_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL
);
",
    "
CREATE TABLE pickle_runs (
  id INTEGER PRIMARY KEY,
  started_at TEXT NOT NULL,
  finished_at TEXT,
  command TEXT NOT NULL,
  user TEXT,
  status TEXT NOT NULL DEFAULT 'running',
  summary TEXT,
  detail TEXT
);
CREATE INDEX pickle_runs_started_at ON pickle_runs(started_at);
CREATE TABLE pickle_requests (
  id INTEGER PRIMARY KEY,
  run_id INTEGER NOT NULL REFERENCES pickle_runs(id) ON DELETE CASCADE,
  created_at TEXT NOT NULL,
  context TEXT,
  method TEXT NOT NULL,
  url TEXT NOT NULL,
  attempt INTEGER NOT NULL DEFAULT 1,
  status INTEGER,
  duration_ms INTEGER,
  request_body TEXT,
  response_body TEXT,
  error TEXT
);
CREATE INDEX pickle_requests_run_id ON pickle_requests(run_id);
CREATE TABLE pickle_bookings (
  id TEXT PRIMARY KEY,
  start_time TEXT NOT NULL,
  end_time TEXT NOT NULL,
  court TEXT,
  checked_in INTEGER NOT NULL DEFAULT 0,
  refreshed_at TEXT NOT NULL
);
CREATE TABLE pickle_token (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  token TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
",
    "
CREATE TABLE user_access (
  user_id INTEGER NOT NULL REFERENCES users(id),
  feature TEXT NOT NULL,
  PRIMARY KEY (user_id, feature)
);
INSERT INTO user_access (user_id, feature) SELECT id, 'sms' FROM users;
",
    "
CREATE TABLE trips (
  id INTEGER PRIMARY KEY,
  location TEXT NOT NULL,
  start_date TEXT NOT NULL,
  end_date TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE TABLE trip_items (
  id INTEGER PRIMARY KEY,
  trip_id INTEGER NOT NULL REFERENCES trips(id) ON DELETE CASCADE,
  position INTEGER NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN ('link', 'map', 'note')),
  label TEXT,
  value TEXT NOT NULL
);
CREATE INDEX trip_items_trip_id ON trip_items(trip_id, position);
",
];

pub fn open(path: &str) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.pragma_update(None, "foreign_keys", true)?;
    Ok(conn)
}

pub fn migrate(conn: &mut Connection) -> rusqlite::Result<usize> {
    migrate_to(conn, MIGRATIONS.len())
}

pub fn migrate_to(conn: &mut Connection, version: usize) -> rusqlite::Result<usize> {
    let current: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let mut applied = 0;
    for (i, sql) in MIGRATIONS
        .iter()
        .enumerate()
        .take(version)
        .skip(current as usize)
    {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", (i + 1) as i64)?;
        tx.commit()?;
        applied += 1;
    }
    Ok(applied)
}

#[cfg(test)]
pub fn open_memory() -> Connection {
    let mut conn = Connection::open_in_memory().unwrap();
    conn.pragma_update(None, "foreign_keys", true).unwrap();
    migrate(&mut conn).unwrap();
    conn
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrate_is_idempotent() {
        let mut conn = Connection::open_in_memory().unwrap();
        assert_eq!(migrate(&mut conn).unwrap(), MIGRATIONS.len());
        assert_eq!(migrate(&mut conn).unwrap(), 0);
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v as usize, MIGRATIONS.len());
    }
}
