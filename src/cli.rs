use crate::{auth, config, db, mail};
use std::process::exit;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn fail(msg: impl std::fmt::Display) -> ! {
    eprintln!("{msg}");
    exit(1)
}

const USAGE: &str = "usage: willcrock [migrate | user add <email> [--admin] [--name <name>] | user list | user disable|enable|admin|unadmin <email> | user grant|revoke <email> <sms|pickle> | mail-test <address> | pickle sync|reserve-today|reserve|cancel|login]";

pub fn migrate() {
    let cfg = config::load().unwrap_or_else(|e| fail(e));
    let mut conn = db::open(&cfg.db_path).unwrap_or_else(|e| fail(e));
    let applied = db::migrate(&mut conn).unwrap_or_else(|e| fail(e));
    println!("migrations applied: {applied}");
}

pub fn mail_test(args: &[String]) {
    let Some(to) = args.first() else { fail(USAGE) };
    let cfg = config::load().unwrap_or_else(|e| fail(e));
    match mail::send_code(&cfg.smtp, to, "TEST1234") {
        Ok(()) => println!("sent to {to}"),
        Err(e) => fail(format!("send failed: {e}")),
    }
}

pub fn user(args: &[String]) {
    let cfg = config::load().unwrap_or_else(|e| fail(e));
    let conn = db::open(&cfg.db_path).unwrap_or_else(|e| fail(e));
    let result = match args.first().map(String::as_str) {
        Some("add") => add(&conn, &args[1..]),
        Some("list") => list(&conn),
        Some("disable") => set_disabled(&conn, &args[1..], true),
        Some("enable") => set_disabled(&conn, &args[1..], false),
        Some("admin") => set_admin(&conn, &args[1..], true),
        Some("unadmin") => set_admin(&conn, &args[1..], false),
        Some("grant") => set_access(&conn, &args[1..], true),
        Some("revoke") => set_access(&conn, &args[1..], false),
        _ => fail(USAGE),
    };
    result.unwrap_or_else(|e| fail(e));
}

fn add(conn: &rusqlite::Connection, args: &[String]) -> rusqlite::Result<()> {
    let Some(email) = args.first() else {
        fail(USAGE)
    };
    let is_admin = args.iter().any(|a| a == "--admin");
    let name = args
        .iter()
        .position(|a| a == "--name")
        .and_then(|i| args.get(i + 1))
        .map(String::as_str);
    if auth::add_user(conn, email, name, is_admin, now())? {
        println!("added {}", auth::normalize_email(email));
    } else {
        fail("user already exists");
    }
    Ok(())
}

fn list(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    for u in auth::list_users(conn)? {
        println!(
            "{}\t{}\t{}\t{}{}",
            u.id,
            u.email,
            if u.is_admin { "admin" } else { "member" },
            u.access
                .iter()
                .map(|f| f.as_str())
                .collect::<Vec<_>>()
                .join(","),
            if u.disabled { "\tdisabled" } else { "" }
        );
    }
    Ok(())
}

fn user_id(conn: &rusqlite::Connection, args: &[String]) -> rusqlite::Result<i64> {
    let Some(email) = args.first() else {
        fail(USAGE)
    };
    match auth::user_id_by_email(conn, email)? {
        Some(id) => Ok(id),
        None => fail("no such user"),
    }
}

fn set_disabled(
    conn: &rusqlite::Connection,
    args: &[String],
    disabled: bool,
) -> rusqlite::Result<()> {
    auth::set_disabled(conn, user_id(conn, args)?, disabled, now())
}

fn set_admin(conn: &rusqlite::Connection, args: &[String], is_admin: bool) -> rusqlite::Result<()> {
    auth::set_admin(conn, user_id(conn, args)?, is_admin)
}

fn set_access(conn: &rusqlite::Connection, args: &[String], on: bool) -> rusqlite::Result<()> {
    let Some(feature) = args.get(1).and_then(|f| auth::Feature::parse(f)) else {
        fail(USAGE)
    };
    auth::set_access(conn, user_id(conn, args)?, feature, on)
}

pub fn usage_exit() -> ! {
    fail(USAGE)
}
