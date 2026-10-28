use crate::{auth, config, db, mail};
use std::process::exit;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn fail(msg: impl std::fmt::Display) -> ! {
    eprintln!("{msg}");
    exit(1)
}

const USAGE: &str = "usage: willcrock [migrate | user add <email> [--admin] [--name <name>] | user list | user disable <email> | mail-test <address>]";

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
        Some("disable") => disable(&conn, &args[1..]),
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
            "{}\t{}\t{}{}",
            u.id,
            u.email,
            if u.is_admin { "admin" } else { "member" },
            if u.disabled { "\tdisabled" } else { "" }
        );
    }
    Ok(())
}

fn disable(conn: &rusqlite::Connection, args: &[String]) -> rusqlite::Result<()> {
    let Some(email) = args.first() else {
        fail(USAGE)
    };
    match auth::user_id_by_email(conn, email)? {
        Some(id) => auth::set_disabled(conn, id, true, now()),
        None => fail("no such user"),
    }
}

pub fn usage_exit() -> ! {
    fail(USAGE)
}
