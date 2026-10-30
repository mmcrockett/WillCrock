mod api;
pub mod dashboard;
mod history;
mod ops;
mod time;

use crate::{cli, config, db};
pub use api::UreqTransport;
use api::{Client, Res, Transport};
use jiff::Timestamp;
use ops::CancelOutcome;
use rusqlite::Connection;
use std::fs::{File, TryLockError};
use std::panic::{catch_unwind, AssertUnwindSafe};

#[derive(Debug, PartialEq)]
pub enum Cmd {
    Sync,
    ReserveToday {
        dry_run: bool,
    },
    Reserve {
        start: String,
        minutes: Option<i64>,
        court: Option<String>,
        dry_run: bool,
    },
    Cancel {
        id: String,
        date: Option<String>,
    },
    Login,
}

const USAGE: &str = "usage: willcrock pickle sync | reserve-today [--dry-run] | \
    reserve <HH:MM> [--minutes N] [--court C] [--dry-run] | cancel <id> [yyyymmdd] | login";

fn parse(args: &[String]) -> Result<Cmd, String> {
    let usage = || USAGE.to_string();
    let flag = |name: &str| args.iter().any(|a| a == name);
    let value = |name: &str| -> Result<Option<String>, String> {
        match args.iter().position(|a| a == name) {
            None => Ok(None),
            Some(i) => args.get(i + 1).cloned().map(Some).ok_or_else(usage),
        }
    };
    let positional: Vec<&String> = {
        let mut skip = false;
        args.iter()
            .skip(1)
            .filter(|a| {
                if skip {
                    skip = false;
                    return false;
                }
                if *a == "--minutes" || *a == "--court" {
                    skip = true;
                    return false;
                }
                !a.starts_with("--")
            })
            .collect()
    };

    match args.first().map(String::as_str) {
        Some("sync") => Ok(Cmd::Sync),
        Some("login") => Ok(Cmd::Login),
        Some("reserve-today") => Ok(Cmd::ReserveToday {
            dry_run: flag("--dry-run"),
        }),
        Some("reserve") => Ok(Cmd::Reserve {
            start: positional.first().ok_or_else(usage)?.to_string(),
            minutes: value("--minutes")?
                .map(|m| m.parse::<i64>().map_err(|_| usage()))
                .transpose()?,
            court: value("--court")?,
            dry_run: flag("--dry-run"),
        }),
        Some("cancel") => Ok(Cmd::Cancel {
            id: positional.first().ok_or_else(usage)?.to_string(),
            date: positional.get(1).map(|d| d.to_string()),
        }),
        _ => Err(usage()),
    }
}

fn exec(c: &mut Client, cmd: &Cmd) -> Res<()> {
    match cmd {
        Cmd::Sync => c.sync(),
        Cmd::ReserveToday { dry_run } => {
            let bodies: Vec<String> = c
                .reserve_today(*dry_run)?
                .into_iter()
                .map(|r| r.body)
                .collect();
            println!("responses: {bodies:?}");
            Ok(())
        }
        Cmd::Reserve {
            start,
            minutes,
            court,
            dry_run,
        } => {
            let resp = c.reserve(start, *minutes, court.as_deref(), *dry_run)?;
            println!("response: {}", resp.body);
            Ok(())
        }
        Cmd::Cancel { id, date } => {
            match c.cancel(id, date.as_deref())? {
                CancelOutcome::NotAllowed => println!("cancel response: not allowed"),
                CancelOutcome::Done(r) => println!("cancel response: {}", r.body),
            }
            c.sync()
        }
        Cmd::Login => {
            c.refresh_token()?;
            println!("login: token refreshed and saved");
            Ok(())
        }
    }
}

fn panic_message(p: Box<dyn std::any::Any + Send>) -> String {
    p.downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panic".to_string())
}

fn execute(
    conn: &Connection,
    cfg: &config::Pickle,
    transport: &dyn Transport,
    cmd: &Cmd,
    name: &str,
) -> Res<()> {
    in_run(conn, cfg, transport, name, |c| exec(c, cmd))
}

pub fn cancel_booking(
    conn: &Connection,
    cfg: &config::Pickle,
    transport: &dyn Transport,
    id: &str,
) -> Res<()> {
    in_run(conn, cfg, transport, "cancel", |c| {
        match c.cancel(id, None)? {
            CancelOutcome::Done(r) if r.ok() => {}
            CancelOutcome::Done(r) => return Err(r.message()),
            CancelOutcome::NotAllowed => return Err("not allowed".to_string()),
        }
        history::delete_booking(conn, id).map_err(api::db_err)?;
        if let Err(e) = c.sync() {
            c.detail(format!("sync after cancel failed: {e}"));
        }
        Ok(())
    })
}

fn in_run(
    conn: &Connection,
    cfg: &config::Pickle,
    transport: &dyn Transport,
    name: &str,
    work: impl FnOnce(&mut Client) -> Res<()>,
) -> Res<()> {
    let now = Timestamp::now();
    let cutoff = time::plus_millis(now, -history::RETENTION_DAYS * 24 * 3600 * 1000);
    history::prune(conn, &time::iso(cutoff)).map_err(api::db_err)?;
    let run_id =
        history::start_run(conn, name, &cfg.full_name, &time::iso(now)).map_err(api::db_err)?;

    let mut client = Client::new(
        transport,
        conn,
        cfg,
        run_id,
        Box::new(Timestamp::now),
        Box::new(std::thread::sleep),
    )?;
    let error = match catch_unwind(AssertUnwindSafe(|| work(&mut client))) {
        Ok(Ok(())) => None,
        Ok(Err(e)) => Some(e),
        Err(p) => Some(panic_message(p)),
    };
    client.finish(error.as_deref())?;
    error.map_or(Ok(()), Err)
}

// Replaces cron's setlock -n: a run that finds the lock held skips quietly.
pub fn acquire_lock() -> Result<Option<File>, String> {
    let path = std::env::temp_dir().join("willcrock-pickle.lock");
    let file = File::create(&path).map_err(|e| format!("create {}: {e}", path.display()))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(e)) => Err(format!("lock {}: {e}", path.display())),
    }
}

pub fn run(args: &[String]) -> ! {
    let cmd = parse(args).unwrap_or_else(|e| cli::fail(e));
    let cfg = config::load().unwrap_or_else(|e| cli::fail(e));
    let Some(pickle_cfg) = cfg.pickle.as_ref() else {
        cli::fail("config: missing [pickle] section")
    };
    let _lock = match acquire_lock() {
        Ok(Some(lock)) => lock,
        Ok(None) => {
            println!("pickle: another run holds the lock, skipping");
            std::process::exit(0)
        }
        Err(e) => cli::fail(e),
    };
    let conn = db::open(&cfg.db_path).unwrap_or_else(|e| cli::fail(e));
    let name = args.first().map(String::as_str).unwrap_or_default();
    match execute(&conn, pickle_cfg, &UreqTransport::new(), &cmd, name) {
        Ok(()) => std::process::exit(0),
        Err(e) => cli::fail(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn parses_every_command() {
        assert_eq!(parse(&args("sync")).unwrap(), Cmd::Sync);
        assert_eq!(parse(&args("login")).unwrap(), Cmd::Login);
        assert_eq!(
            parse(&args("reserve-today --dry-run")).unwrap(),
            Cmd::ReserveToday { dry_run: true }
        );
        assert_eq!(
            parse(&args("reserve 07:30 --minutes 60 --court 5C --dry-run")).unwrap(),
            Cmd::Reserve {
                start: "07:30".into(),
                minutes: Some(60),
                court: Some("5C".into()),
                dry_run: true
            }
        );
        assert_eq!(
            parse(&args("reserve --court 5C 07:30")).unwrap(),
            Cmd::Reserve {
                start: "07:30".into(),
                minutes: None,
                court: Some("5C".into()),
                dry_run: false
            }
        );
        assert_eq!(
            parse(&args("cancel 55 20261018")).unwrap(),
            Cmd::Cancel {
                id: "55".into(),
                date: Some("20261018".into())
            }
        );
        assert_eq!(
            parse(&args("cancel 55")).unwrap(),
            Cmd::Cancel {
                id: "55".into(),
                date: None
            }
        );
    }

    #[test]
    fn rejects_bad_invocations() {
        for bad in [
            "",
            "nope",
            "reserve",
            "cancel",
            "reserve 07:30 --minutes",
            "reserve 07:30 --minutes x",
        ] {
            assert!(parse(&args(bad)).is_err(), "{bad}");
        }
    }

    fn seed_booking(conn: &Connection, id: &str) {
        conn.execute(
            "INSERT INTO pickle_bookings VALUES (?1, '2026-11-05T13:30:00.000Z', '2026-11-05T15:00:00.000Z', 'Pickleball 5D', 0, 'r')",
            [id],
        )
        .unwrap();
    }

    fn bookings_left(conn: &Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM pickle_bookings", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn cancel_booking_deletes_then_resyncs() {
        use api::testing::{cfg, Fake};
        let conn = crate::db::open_memory();
        history::save_token(&conn, "tok", "t").unwrap();
        seed_booking(&conn, "55");
        let fake = Fake::default();
        fake.push(200, r#"{"message":"Success"}"#);
        fake.push(200, r#"{"payload":[]}"#);
        cancel_booking(&conn, &cfg(), &fake, "55").unwrap();

        assert_eq!(
            fake.paths(),
            [
                "/api/v1/appointment/cancel/55",
                "/api/v1/appointment/booked?sport_id=1&location_id=134"
            ]
        );
        assert_eq!(bookings_left(&conn), 0);
        let (command, status): (String, String) = conn
            .query_row("SELECT command, status FROM pickle_runs", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!((command.as_str(), status.as_str()), ("cancel", "ok"));
    }

    #[test]
    fn cancel_booking_reports_a_refusal_and_keeps_the_booking() {
        use api::testing::{cfg, Fake};
        let conn = crate::db::open_memory();
        history::save_token(&conn, "tok", "t").unwrap();
        seed_booking(&conn, "55");
        let fake = Fake::default();
        fake.push(400, r#"{"message":"Cancellation window has passed"}"#);
        let err = cancel_booking(&conn, &cfg(), &fake, "55").unwrap_err();

        assert_eq!(err, "Cancellation window has passed");
        assert_eq!(bookings_left(&conn), 1);
        let status: String = conn
            .query_row("SELECT status FROM pickle_runs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(status, "error");
    }

    #[test]
    fn cancel_booking_survives_a_failing_resync() {
        use api::testing::{cfg, Fake};
        let conn = crate::db::open_memory();
        history::save_token(&conn, "tok", "t").unwrap();
        seed_booking(&conn, "55");
        let fake = Fake::default();
        fake.push(200, r#"{"message":"Success"}"#);
        for _ in 0..2 {
            fake.responses
                .borrow_mut()
                .push_back(Err("connection refused".to_string()));
        }
        cancel_booking(&conn, &cfg(), &fake, "55").unwrap();
        assert_eq!(bookings_left(&conn), 0);
    }
}
