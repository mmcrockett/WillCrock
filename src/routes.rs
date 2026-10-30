use crate::auth::{self, Feature, User, SESSION_TTL};
use crate::config::Config;
use crate::http::{session_cookie, urlencode, Req, Resp};
use crate::{log, mail, pages, pickle, sms};
use rusqlite::Connection;

fn features(cfg: &Config) -> pages::Features {
    pages::Features {
        sms: cfg.quackr.is_some(),
        pickle: cfg.pickle.is_some(),
    }
}

pub fn handle(conn: &Connection, cfg: &Config, req: &Req, now: i64) -> Resp {
    match dispatch(conn, cfg, req, now) {
        Ok(resp) => resp,
        Err(e) => {
            log::error(&format!("request failed: {e}"));
            Resp::html(500, pages::error())
        }
    }
}

fn dispatch(conn: &Connection, cfg: &Config, req: &Req, now: i64) -> rusqlite::Result<Resp> {
    let segs: Vec<&str> = req.path.split('/').filter(|s| !s.is_empty()).collect();
    let method = req.method.as_str();

    match (method, segs.as_slice()) {
        ("GET", ["login"]) => return Ok(Resp::html(200, pages::login(None))),
        ("POST", ["login"]) => return post_login(conn, cfg, req, now),
        ("GET", ["login", "code"]) => {
            let email = req.query.get("email").map(String::as_str).unwrap_or("");
            return Ok(Resp::html(200, pages::code(email, false)));
        }
        ("POST", ["login", "code"]) => return post_code(conn, req, now),
        _ => {}
    }

    let session = match req.cookie("session") {
        Some(token) => auth::session_user(conn, token, now)?.map(|(u, slid)| (u, slid, token)),
        None => None,
    };
    let Some((user, slid, token)) = session else {
        return Ok(Resp::redirect("/login"));
    };

    let resp = authed(conn, cfg, req, &segs, &user, token, now)?;
    Ok(if slid {
        resp.header("Set-Cookie", session_cookie(token, SESSION_TTL))
    } else {
        resp
    })
}

fn authed(
    conn: &Connection,
    cfg: &Config,
    req: &Req,
    segs: &[&str],
    user: &User,
    token: &str,
    now: i64,
) -> rusqlite::Result<Resp> {
    match (req.method.as_str(), segs) {
        ("GET", []) => Ok(Resp::html(200, pages::home(user, features(cfg)))),
        ("POST", ["logout"]) => {
            auth::logout(conn, token)?;
            Ok(Resp::redirect("/login").header("Set-Cookie", session_cookie("", 0)))
        }
        (_, ["admin", ..]) if !user.is_admin => Ok(Resp::html(404, pages::not_found())),
        ("GET", ["admin", "users"]) => admin_page(conn, cfg, user),
        ("GET", ["sms"]) if user.can(Feature::Sms) => Ok(sms_page(cfg, user)),
        ("GET", ["pickle"]) if user.can(Feature::Pickle) => pickle_page(conn, cfg, user, now),
        ("GET", [page @ ("recipes" | "photos" | "trips")])
            if Feature::parse(page).is_some_and(|f| user.can(f)) =>
        {
            let feature = Feature::parse(page).expect("matched above");
            Ok(Resp::html(
                200,
                pages::placeholder(user, features(cfg), feature),
            ))
        }
        ("POST", ["pickle", "bookings", id, "cancel"]) if user.can(Feature::Pickle) => {
            pickle_cancel(conn, cfg, user, id)
        }
        ("POST", ["admin", "users"]) => {
            if let Some(email) = req.form.get("email").filter(|e| !e.trim().is_empty()) {
                let name = req
                    .form
                    .get("name")
                    .map(|n| n.trim())
                    .filter(|n| !n.is_empty());
                auth::add_user(conn, email, name, false, now)?;
            }
            Ok(Resp::redirect("/admin/users"))
        }
        ("POST", ["admin", "users", id, action @ ("grant" | "revoke"), feature]) => {
            let (Ok(id), Some(feature)) = (id.parse::<i64>(), Feature::parse(feature)) else {
                return Ok(Resp::html(404, pages::not_found()));
            };
            auth::set_access(conn, id, feature, *action == "grant")?;
            Ok(Resp::redirect("/admin/users"))
        }
        ("POST", ["admin", "users", id, action]) => {
            let Ok(id) = id.parse::<i64>() else {
                return Ok(Resp::html(404, pages::not_found()));
            };
            if id != user.id {
                match *action {
                    "disable" => auth::set_disabled(conn, id, true, now)?,
                    "enable" => auth::set_disabled(conn, id, false, now)?,
                    _ => return Ok(Resp::html(404, pages::not_found())),
                }
            }
            Ok(Resp::redirect("/admin/users"))
        }
        _ => Ok(Resp::html(404, pages::not_found())),
    }
}

fn admin_page(conn: &Connection, cfg: &Config, me: &User) -> rusqlite::Result<Resp> {
    let users = auth::list_users(conn)?;
    Ok(Resp::html(
        200,
        pages::admin_users(&users, me, features(cfg)),
    ))
}

fn sms_page(cfg: &Config, user: &User) -> Resp {
    let Some(quackr) = cfg.quackr.as_ref() else {
        return Resp::html(404, pages::not_found());
    };
    let messages = sms::fetch(quackr)
        .map_err(|e| log::error(&format!("quackr fetch failed: {e}")))
        .ok();
    Resp::html(
        200,
        pages::sms(user, features(cfg), quackr, messages.as_deref()),
    )
}

fn pickle_page(conn: &Connection, cfg: &Config, user: &User, now: i64) -> rusqlite::Result<Resp> {
    if cfg.pickle.is_none() {
        return Ok(Resp::html(404, pages::not_found()));
    }
    let data = pickle::dashboard::load(conn, now)?;
    Ok(Resp::html(
        200,
        pages::pickle(user, features(cfg), &data, now),
    ))
}

fn pickle_cancel(conn: &Connection, cfg: &Config, user: &User, id: &str) -> rusqlite::Result<Resp> {
    let Some(pickle_cfg) = cfg.pickle.as_ref() else {
        return Ok(Resp::html(404, pages::not_found()));
    };
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        return Ok(Resp::html(404, pages::not_found()));
    }
    let failed = |message: &str| {
        Resp::html(
            200,
            pages::pickle_cancel_failed(user, features(cfg), message),
        )
    };
    let _lock = match pickle::acquire_lock() {
        Ok(Some(lock)) => lock,
        Ok(None) => {
            return Ok(failed(
                "Another pickle job is running. Try again in a minute.",
            ))
        }
        Err(e) => {
            log::error(&format!("pickle lock failed: {e}"));
            return Ok(failed("Could not start the cancel. Try again in a moment."));
        }
    };
    Ok(
        match pickle::cancel_booking(conn, pickle_cfg, &pickle::UreqTransport::new(), id) {
            Ok(()) => Resp::redirect("/pickle"),
            Err(e) => {
                log::error(&format!("pickle cancel {id} failed: {e}"));
                failed(&e)
            }
        },
    )
}

fn post_login(conn: &Connection, cfg: &Config, req: &Req, now: i64) -> rusqlite::Result<Resp> {
    let email = req.form.get("email").map(String::as_str).unwrap_or("");
    if let Some((to, code)) = auth::request_code(conn, email, now)? {
        if let Err(e) = mail::send_code(&cfg.smtp, &to, &code) {
            log::error(&format!("send login code failed: {e}"));
        }
    }
    Ok(Resp::redirect(&format!(
        "/login/code?email={}",
        urlencode(&auth::normalize_email(email))
    )))
}

fn post_code(conn: &Connection, req: &Req, now: i64) -> rusqlite::Result<Resp> {
    let email = req.form.get("email").map(String::as_str).unwrap_or("");
    let code = req.form.get("code").map(String::as_str).unwrap_or("");
    Ok(match auth::verify_code(conn, email, code, now)? {
        Some(token) => {
            Resp::redirect("/").header("Set-Cookie", session_cookie(&token, SESSION_TTL))
        }
        None => Resp::html(400, pages::code(email, true)),
    })
}
