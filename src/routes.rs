use crate::auth::{self, Feature, User, SESSION_TTL};
use crate::config::Config;
use crate::http::{session_cookie, urlencode, Req, Resp};
use crate::{log, mail, pages, photos, pickle, sms, trips};
use rusqlite::{Connection, OptionalExtension};

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
    if let Some(status) = req.body_error {
        return Ok(if req.path == "/photos/upload" {
            Resp::json(
                status,
                &serde_json::json!({
                    "error": if status == 413 { "Photo exceeds 25 MB." } else { "Could not read the upload." }
                }),
            )
        } else {
            Resp::html(status, pages::error())
        });
    }
    match (req.method.as_str(), segs) {
        ("GET", []) => Ok(Resp::html(200, pages::home(user, features(cfg)))),
        ("POST", ["logout"]) => {
            auth::logout(conn, token)?;
            Ok(Resp::redirect("/login").header("Set-Cookie", session_cookie("", 0)))
        }
        (_, ["admin", ..]) if !user.is_admin => Ok(Resp::html(404, pages::not_found())),
        ("GET", ["admin", "users"]) => admin_page(conn, cfg, user),
        ("GET", ["sms"]) if user.can(Feature::Sms) => Ok(sms_page(cfg, user)),
        ("GET", ["pickle"]) if user.can(Feature::Pickle) => {
            pickle_page(conn, cfg, user, now, None, 200)
        }
        ("GET", ["photos"]) if user.can(Feature::Photos) => {
            let summary = if cfg.photos.is_some() {
                Some(photos::summary(conn)?)
            } else {
                None
            };
            Ok(Resp::html(
                200,
                pages::photos(user, features(cfg), summary.as_ref()),
            ))
        }
        ("POST", ["photos", "upload"]) if user.can(Feature::Photos) && cfg.photos.is_some() => {
            if req.requested_with != "1" || req.content_type != "application/octet-stream" {
                return Ok(Resp::json(
                    400,
                    &serde_json::json!({"error": "Invalid upload request."}),
                ));
            }
            let root = std::path::Path::new(&cfg.photos.as_ref().expect("checked above").dir);
            let filename = req
                .query
                .get("filename")
                .map(String::as_str)
                .unwrap_or("photo");
            Ok(match photos::upload(conn, root, filename, &req.body, now) {
                Ok(uploaded) => Resp::json(200, &uploaded),
                Err(error) => photo_upload_error(error),
            })
        }
        ("POST", ["photos", "i", id, "date"])
            if user.can(Feature::Photos) && cfg.photos.is_some() =>
        {
            if req.requested_with != "1" {
                return Ok(Resp::json(
                    400,
                    &serde_json::json!({"error": "Invalid request."}),
                ));
            }
            let Ok(id) = id.parse::<i64>() else {
                return Ok(Resp::json(
                    404,
                    &serde_json::json!({"error": "Photo not found."}),
                ));
            };
            let month = req.form.get("month").map(String::as_str).unwrap_or("");
            Ok(match photos::change_date(conn, id, month) {
                Ok(true) => Resp::json(200, &serde_json::json!({"date": month})),
                Ok(false) => Resp::json(404, &serde_json::json!({"error": "Photo not found."})),
                Err(error) => photo_upload_error(error),
            })
        }
        ("GET", ["photos", "t", tags @ ..])
            if user.can(Feature::Photos) && cfg.photos.is_some() && !tags.is_empty() =>
        {
            let mut names: Vec<String> = Vec::new();
            for t in tags {
                match photos::normalize_tag(t) {
                    Some(n) if !names.contains(&n) => names.push(n),
                    Some(_) => {}
                    None => return Ok(Resp::html(404, pages::not_found())),
                }
            }
            let page = match req.query.get("page") {
                None => 1,
                Some(raw) => match raw.parse::<i64>() {
                    Ok(page) if page > 0 => page,
                    _ => return Ok(Resp::html(400, pages::not_found())),
                },
            };
            let view = photos::tag_view(conn, &names, page)?;
            Ok(Resp::html(
                200,
                pages::photo_tag_view(user, features(cfg), &view),
            ))
        }
        ("GET", ["photos", "i", id, kind @ ("thumb" | "preview" | "original")])
            if user.can(Feature::Photos) && cfg.photos.is_some() =>
        {
            photo_image(conn, cfg, id, kind)
        }
        ("POST", ["photos", "i", id, "tags"])
            if user.can(Feature::Photos) && cfg.photos.is_some() =>
        {
            let Ok(id) = id.parse::<i64>() else {
                return Ok(Resp::html(404, pages::not_found()));
            };
            let raw = req.form.get("tags").map(String::as_str).unwrap_or("");
            let names = match photos::parse_tags(raw) {
                Ok(names) => names,
                Err(message) => {
                    return Ok(Resp::html(
                        400,
                        pages::photo_tags_error(user, features(cfg), &message),
                    ))
                }
            };
            if photos::set_tags(conn, id, &names)? {
                let return_to = req
                    .form
                    .get("return_to")
                    .filter(|path| {
                        path.starts_with("/photos/t/")
                            && path
                                .bytes()
                                .all(|b| b.is_ascii_alphanumeric() || b"/%?=&.-_~".contains(&b))
                    })
                    .map(String::as_str)
                    .unwrap_or("/photos");
                Ok(Resp::redirect(return_to))
            } else {
                Ok(Resp::html(404, pages::not_found()))
            }
        }
        ("GET", [page @ "recipes"]) if Feature::parse(page).is_some_and(|f| user.can(f)) => {
            let feature = Feature::parse(page).expect("matched above");
            Ok(Resp::html(
                200,
                pages::placeholder(user, features(cfg), feature),
            ))
        }
        (_, ["trips", rest @ ..]) if user.can(Feature::Trips) => {
            trips_route(conn, cfg, req, user, rest, now)
        }
        ("POST", ["pickle", "slots"]) if user.can(Feature::Pickle) => {
            pickle_slot_add(conn, cfg, req, user, now)
        }
        ("POST", ["pickle", "slots", id, "delete"]) if user.can(Feature::Pickle) => {
            match id.parse::<i64>() {
                Ok(id) if pickle::slots::delete(conn, id)? > 0 => Ok(Resp::redirect("/pickle")),
                _ => Ok(Resp::html(404, pages::not_found())),
            }
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

fn photo_upload_error(error: photos::UploadError) -> Resp {
    match error {
        photos::UploadError::Invalid(message) => {
            Resp::json(400, &serde_json::json!({"error": message}))
        }
        photos::UploadError::Failed(message) => {
            log::error(&format!("photo upload: {message}"));
            Resp::json(
                500,
                &serde_json::json!({"error": "Could not save the photo. Please try again."}),
            )
        }
    }
}

fn photo_image(conn: &Connection, cfg: &Config, id: &str, kind: &str) -> rusqlite::Result<Resp> {
    let not_found = || Resp::html(404, pages::not_found());
    let Ok(id) = id.parse::<i64>() else {
        return Ok(not_found());
    };
    let photo = conn
        .query_row(
            "SELECT path, sha256 FROM photos WHERE id = ?1 AND missing_at IS NULL",
            [id],
            |r| {
                Ok((
                    photos::Photo {
                        id,
                        path: r.get(0)?,
                        tags: Vec::new(),
                    },
                    r.get::<_, String>(1)?,
                ))
            },
        )
        .optional()?;
    let (Some((photo, hash)), Some(config)) = (photo, cfg.photos.as_ref()) else {
        return Ok(not_found());
    };
    let image_kind = match kind {
        "thumb" => photos::ImageKind::Thumbnail,
        "preview" => photos::ImageKind::Preview,
        _ => photos::ImageKind::Original,
    };
    let result = photos::image_file(std::path::Path::new(&config.dir), &photo, &hash, image_kind)
        .and_then(|file| {
            file.map(|file| {
                let content_type = if kind == "original" {
                    "application/octet-stream"
                } else {
                    "image/jpeg"
                };
                Resp::file(file, content_type).map(|resp| {
                    if kind == "original" {
                        let filename = std::path::Path::new(&photo.path)
                            .file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("photo");
                        resp.header(
                            "Content-Disposition",
                            format!("attachment; filename*=UTF-8''{}", urlencode(filename)),
                        )
                    } else {
                        resp
                    }
                })
            })
            .transpose()
            .map_err(|e| format!("photo {id} response: {e}"))
        });
    Ok(match result {
        Ok(Some(resp)) => resp,
        Ok(None) => not_found(),
        Err(e) => {
            log::error(&e);
            Resp::html(500, pages::error())
        }
    })
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

fn trips_route(
    conn: &Connection,
    cfg: &Config,
    req: &Req,
    user: &User,
    segs: &[&str],
    now: i64,
) -> rusqlite::Result<Resp> {
    let feats = features(cfg);
    let not_found = || Ok(Resp::html(404, pages::not_found()));
    let form = |key: &str| req.form.get(key).map(String::as_str).unwrap_or("");
    let trip_input = || trips::parse_trip(form("location"), form("start"), form("end"));
    let item_input = || trips::parse_item(form("kind"), form("label"), form("value"));

    let (method, id, rest) = match (req.method.as_str(), segs) {
        ("GET", []) => {
            let (upcoming, past) = trips::list(conn, trips::today(now))?;
            return Ok(Resp::html(
                200,
                pages::trips_index(user, feats, &upcoming, &past),
            ));
        }
        ("GET", ["new"]) => return Ok(Resp::html(200, pages::trip_new(user, feats, None))),
        ("POST", []) => {
            return match trip_input() {
                Ok(input) => {
                    let id = trips::create(conn, &input, now)?;
                    Ok(Resp::redirect(&format!("/trips/{id}/edit")))
                }
                Err(e) => Ok(Resp::html(400, pages::trip_new(user, feats, Some(&e)))),
            };
        }
        (method, [id, rest @ ..]) => (method, id, rest),
        _ => return not_found(),
    };
    let Ok(id) = id.parse::<i64>() else {
        return not_found();
    };
    let Some(trip) = trips::get(conn, id)? else {
        return not_found();
    };
    let edit_url = format!("/trips/{id}/edit");
    let edit_page = |status: u16, error: Option<&str>| -> rusqlite::Result<Resp> {
        let items = trips::items(conn, id)?;
        Ok(Resp::html(
            status,
            pages::trip_edit(user, feats, &trip, &items, error),
        ))
    };
    let done = |result: Result<(), String>| match result {
        Ok(()) => Ok(Resp::redirect(&edit_url)),
        Err(e) => edit_page(400, Some(&e)),
    };

    match (method, rest) {
        ("GET", []) => {
            let items = trips::items(conn, id)?;
            Ok(Resp::html(
                200,
                pages::trip_view(user, feats, &trip, &items),
            ))
        }
        ("GET", ["edit"]) => edit_page(200, None),
        ("POST", []) => match trip_input() {
            Ok(input) => {
                trips::update(conn, id, &input)?;
                Ok(Resp::redirect(&edit_url))
            }
            Err(e) => edit_page(400, Some(&e)),
        },
        ("POST", ["delete"]) => {
            trips::delete(conn, id)?;
            Ok(Resp::redirect("/trips"))
        }
        ("POST", ["items"]) => match item_input() {
            Ok(input) => {
                trips::add_item(conn, id, &input)?;
                done(Ok(()))
            }
            Err(e) => done(Err(e)),
        },
        ("POST", ["items", item]) => {
            let Ok(item) = item.parse::<i64>() else {
                return not_found();
            };
            match item_input() {
                Ok(input) if trips::update_item(conn, id, item, &input)? == 0 => not_found(),
                Ok(_) => done(Ok(())),
                Err(e) => done(Err(e)),
            }
        }
        ("POST", ["items", item, action @ ("delete" | "up" | "down")]) => {
            let Ok(item) = item.parse::<i64>() else {
                return not_found();
            };
            let found = match *action {
                "delete" => trips::delete_item(conn, id, item)? > 0,
                up_or_down => trips::move_item(conn, id, item, up_or_down == "up")?,
            };
            if found {
                done(Ok(()))
            } else {
                not_found()
            }
        }
        _ => not_found(),
    }
}

fn pickle_page(
    conn: &Connection,
    cfg: &Config,
    user: &User,
    now: i64,
    error: Option<&str>,
    status: u16,
) -> rusqlite::Result<Resp> {
    if cfg.pickle.is_none() {
        return Ok(Resp::html(404, pages::not_found()));
    }
    let data = pickle::dashboard::load(conn, now)?;
    Ok(Resp::html(
        status,
        pages::pickle(user, features(cfg), &data, now, error),
    ))
}

fn pickle_slot_add(
    conn: &Connection,
    cfg: &Config,
    req: &Req,
    user: &User,
    now: i64,
) -> rusqlite::Result<Resp> {
    let field = |key: &str| req.form.get(key).map(String::as_str).unwrap_or("");
    let form = pickle::slots::Form {
        repeat: req.form.contains_key("repeat"),
        date: field("date"),
        start: field("start"),
        minutes: field("minutes"),
        court: field("court"),
    };
    let error = match pickle::slots::parse(&form, now) {
        Ok(input) if pickle::slots::add(conn, &input, now)? => {
            return Ok(Resp::redirect("/pickle"))
        }
        Ok(_) => "That day and start time is already scheduled.".to_string(),
        Err(e) => e,
    };
    pickle_page(conn, cfg, user, now, Some(&error), 400)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn cfg() -> Config {
        toml::from_str(
            "db_path = \"x\"\n[smtp]\nhost=\"h\"\nusername=\"u\"\npassword=\"p\"\nfrom=\"f\"\n",
        )
        .unwrap()
    }

    fn user(access: Vec<Feature>) -> User {
        User {
            id: 1,
            email: "sam@example.com".into(),
            name: None,
            is_admin: false,
            disabled: false,
            access,
        }
    }

    fn call(conn: &Connection, u: &User, method: &str, path: &str, form: &str) -> (u16, String) {
        let req = Req::test(method, path, form);
        let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let resp = authed(conn, &cfg(), &req, &segs, u, "tok", 1_780_369_200).unwrap();
        let mut out = Vec::new();
        resp.write_to(&mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        let status = text["Status: ".len()..].split("\r\n").next().unwrap();
        (status.parse().unwrap(), text)
    }

    #[test]
    fn photo_gallery_and_image_routes_enforce_access_and_serve_binary_files() {
        let root = std::env::temp_dir().join(format!(
            "willcrock-gallery-routes-{}",
            rand::random::<u64>()
        ));
        std::fs::create_dir_all(root.join("2026/20261108")).unwrap();
        let path = root.join("2026/20261108/photo.png");
        image::RgbImage::new(40, 20).save(&path).unwrap();
        let original = std::fs::read(&path).unwrap();
        let mut conn = db::open_memory();
        photos::scan(&mut conn, &root, 100).unwrap();
        let id: i64 = conn
            .query_row("SELECT id FROM photos", [], |r| r.get(0))
            .unwrap();
        let mut config = cfg();
        config.photos = Some(crate::config::Photos {
            dir: root.to_str().unwrap().into(),
        });
        let granted = user(vec![Feature::Photos]);
        let denied = user(vec![]);
        let call = |u: &User, req: Req| {
            let segs: Vec<_> = req.path.split('/').filter(|s| !s.is_empty()).collect();
            let resp = authed(&conn, &config, &req, &segs, u, "tok", 100).unwrap();
            let mut out = Vec::new();
            resp.write_to(&mut out).unwrap();
            out
        };
        for kind in ["thumb", "preview", "original"] {
            let url = format!("/photos/i/{id}/{kind}");
            let response = call(&denied, Req::test("GET", &url, ""));
            assert!(response.starts_with(b"Status: 404\r\n"));
            let response = call(&granted, Req::test("GET", &url, ""));
            assert!(response.starts_with(b"Status: 200\r\n"));
            let split = response.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
            let body = &response[split + 4..];
            if kind == "original" {
                assert_eq!(body, original);
                assert!(String::from_utf8_lossy(&response[..split])
                    .contains("attachment; filename*=UTF-8''photo.png"));
            } else {
                assert!(image::load_from_memory(body).is_ok());
            }
            let mut out = Vec::new();
            handle(&conn, &config, &Req::test("GET", &url, ""), 100)
                .write_to(&mut out)
                .unwrap();
            assert!(String::from_utf8(out).unwrap().contains("Location: /login"));
        }
        let gallery = call(&granted, Req::test("GET", "/photos/t/2026", ""));
        let gallery = String::from_utf8(gallery).unwrap();
        assert!(gallery.contains("class=\"photo-grid\""));
        assert!(gallery.contains(&format!("/photos/i/{id}/thumb")));
        assert!(call(
            &denied,
            Req::test("POST", &format!("/photos/i/{id}/tags"), "tags=alice")
        )
        .starts_with(b"Status: 404\r\n"));
        let invalid = call(
            &granted,
            Req::test(
                "POST",
                &format!("/photos/i/{id}/tags"),
                "tags=bad%2Ftag&return_to=%2Fphotos%2Ft%2F2026",
            ),
        );
        assert!(invalid.starts_with(b"Status: 400\r\n"));
        assert!(String::from_utf8(invalid)
            .unwrap()
            .contains("cannot contain slashes"));
        let saved = call(
            &granted,
            Req::test(
                "POST",
                &format!("/photos/i/{id}/tags"),
                "tags=Alice%2C+Bob&return_to=%2Fphotos%2Ft%2F2026",
            ),
        );
        assert!(saved.starts_with(b"Status: 303\r\n"));
        assert!(String::from_utf8(saved)
            .unwrap()
            .contains("Location: /photos/t/2026"));
        let people =
            String::from_utf8(call(&granted, Req::test("GET", "/photos/t/alice", ""))).unwrap();
        assert!(people.contains(&format!("/photos/i/{id}/thumb")));
        assert!(people.contains("data-tags=\"alice, bob\""));
        assert!(people.contains("name=\"known_tag\" value=\"alice\""));
        let year =
            String::from_utf8(call(&granted, Req::test("GET", "/photos/t/2026", ""))).unwrap();
        assert!(year.contains("href=\"/photos/t/2026/alice\""));
        let cleared = call(
            &granted,
            Req::test("POST", &format!("/photos/i/{id}/tags"), "tags="),
        );
        assert!(cleared.starts_with(b"Status: 303\r\n"));
        assert!(
            String::from_utf8(call(&granted, Req::test("GET", "/photos/t/2026", "")))
                .unwrap()
                .contains("class=\"photo-grid\"")
        );
        assert!(
            String::from_utf8(call(&granted, Req::test("GET", "/photos/t/alice", "")))
                .unwrap()
                .contains("No photos.")
        );
        let landing = String::from_utf8(call(&granted, Req::test("GET", "/photos", ""))).unwrap();
        assert!(landing.contains("href=\"/photos/t/2026\""));
        assert!(!landing.contains("class=\"photo-grid\""));
        let mut bad_page = Req::test("GET", "/photos/t/2026", "");
        bad_page.query.insert("page".into(), "0".into());
        assert!(call(&granted, bad_page).starts_with(b"Status: 400\r\n"));
        conn.execute("UPDATE photos SET missing_at = 1", [])
            .unwrap();
        assert!(call(
            &granted,
            Req::test("GET", &format!("/photos/i/{id}/thumb"), "")
        )
        .starts_with(b"Status: 404\r\n"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn photo_upload_and_date_require_access_and_same_origin_header() {
        let root =
            std::env::temp_dir().join(format!("photo-upload-routes-{}", rand::random::<u64>()));
        std::fs::create_dir(&root).unwrap();
        let conn = db::open_memory();
        let mut config = cfg();
        config.photos = Some(crate::config::Photos {
            dir: root.to_str().unwrap().into(),
        });
        let call = |u: &User, req: &Req| {
            let segs: Vec<_> = req.path.split('/').filter(|s| !s.is_empty()).collect();
            let resp = authed(&conn, &config, req, &segs, u, "tok", 1_793_831_400).unwrap();
            let mut out = Vec::new();
            resp.write_to(&mut out).unwrap();
            String::from_utf8(out).unwrap()
        };
        let granted = user(vec![Feature::Photos]);
        let denied = user(vec![]);
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(2, 2)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        let mut req = Req::test("POST", "/photos/upload", "");
        req.body = bytes.into_inner();
        req.content_type = "application/octet-stream".into();
        req.requested_with = "1".into();
        assert!(call(&denied, &req).starts_with("Status: 404"));
        req.requested_with.clear();
        assert!(call(&granted, &req).starts_with("Status: 400"));
        req.requested_with = "1".into();
        req.body_error = Some(413);
        assert!(call(&granted, &req).contains("Photo exceeds 25 MB"));
        req.body_error = None;
        let response = call(&granted, &req);
        assert!(response.starts_with("Status: 200"));
        assert!(response.contains("\"duplicate\":false"));
        assert!(call(&granted, &req).contains("\"duplicate\":true"));
        let mut date = Req::test("POST", "/photos/i/1/date", "month=2022-03");
        assert!(call(&granted, &date).starts_with("Status: 400"));
        date.requested_with = "1".into();
        assert!(call(&denied, &date).starts_with("Status: 404"));
        assert!(call(&granted, &date).contains("\"date\":\"2022-03\""));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn trips_flow_end_to_end() {
        let conn = db::open_memory();
        let me = user(vec![Feature::Trips]);
        let (s, html) = call(&conn, &me, "GET", "/trips", "");
        assert_eq!(s, 200);
        assert!(html.contains("No trips yet."));

        let (s, html) = call(&conn, &me, "GET", "/trips/new", "");
        assert_eq!(s, 200);
        assert!(html.contains("action=\"/trips\""));

        let new_trip = "location=Galveston&start=2026-06-21&end=2026-06-26";
        let (s, html) = call(&conn, &me, "POST", "/trips", new_trip);
        assert_eq!(s, 303);
        assert!(html.contains("Location: /trips/1/edit"));
        let blank = "location=&start=2026-06-21&end=2026-06-26";
        let (s, html) = call(&conn, &me, "POST", "/trips", blank);
        assert_eq!(s, 400);
        assert!(html.contains("Location is required."));

        for form in [
            "kind=link&label=Hotel&value=https%3A%2F%2Fh.example",
            "kind=map&label=&value=1+Main+St",
            "kind=note&label=Code&value=1234",
        ] {
            assert_eq!(call(&conn, &me, "POST", "/trips/1/items", form).0, 303);
        }
        let bad = "kind=link&label=&value=javascript%3Aalert(1)";
        let (s, html) = call(&conn, &me, "POST", "/trips/1/items", bad);
        assert_eq!(s, 400);
        assert!(html.contains("Links must be"));

        let (_, html) = call(&conn, &me, "GET", "/trips/1", "");
        assert!(html.contains("Galveston - Jun 21-26, 2026"));
        assert!(html.contains("https://maps.google.com/?q=1%20Main%20St"));

        assert_eq!(call(&conn, &me, "POST", "/trips/1/items/3/up", "").0, 303);
        assert_eq!(
            call(&conn, &me, "POST", "/trips/1/items/1/delete", "").0,
            303
        );
        let (_, html) = call(&conn, &me, "GET", "/trips/1/edit", "");
        assert!(!html.contains("Hotel"));
        assert!(html.find("value=\"1234\"").unwrap() < html.find("value=\"1 Main St\"").unwrap());

        assert_eq!(
            call(&conn, &me, "POST", "/trips/1/items/99/delete", "").0,
            404
        );
        assert_eq!(call(&conn, &me, "POST", "/trips/1/delete", "").0, 303);
        assert_eq!(call(&conn, &me, "GET", "/trips/1", "").0, 404);
    }

    #[test]
    fn trips_need_the_grant() {
        let conn = db::open_memory();
        let granted = user(vec![Feature::Trips]);
        let new_trip = "location=A&start=2026-06-21&end=2026-06-22";
        call(&conn, &granted, "POST", "/trips", new_trip);
        let other = user(vec![Feature::Sms]);
        for (method, path) in [
            ("GET", "/trips"),
            ("GET", "/trips/1"),
            ("GET", "/trips/1/edit"),
            ("POST", "/trips"),
            ("POST", "/trips/1/delete"),
        ] {
            let (s, _) = call(&conn, &other, method, path, new_trip);
            assert_eq!(s, 404, "{method} {path}");
        }
        assert_eq!(call(&conn, &granted, "GET", "/trips/1", "").0, 200);
    }
}
