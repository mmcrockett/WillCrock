mod auth;
mod cli;
mod config;
mod db;
mod http;
mod log;
mod mail;
mod pages;
mod photos;
mod pickle;
mod routes;
mod sms;
mod trips;

use std::sync::Arc;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => serve(),
        Some("migrate") => cli::migrate(),
        Some("user") => cli::user(&args[1..]),
        Some("mail-test") => cli::mail_test(&args[1..]),
        Some("pickle") => pickle::run(&args[1..]),
        Some("photos") => photos::run(&args[1..]),
        Some(_) => cli::usage_exit(),
    }
}

fn serve() {
    let cfg = match config::load() {
        Ok(c) => Arc::new(c),
        Err(e) => {
            log::error(&format!("config: {e}"));
            std::process::exit(1);
        }
    };
    fastcgi::run(move |mut req| {
        let parsed = http::Req::from_fastcgi(&mut req);
        let resp = match db::open(&cfg.db_path) {
            Ok(conn) => routes::handle(&conn, &cfg, &parsed, cli::now()),
            Err(e) => {
                log::error(&format!("db open failed: {e}"));
                http::Resp::html(500, pages::error())
            }
        };
        let _ = resp.write_to(&mut req.stdout());
    });
}
