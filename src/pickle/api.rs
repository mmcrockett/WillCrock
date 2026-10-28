use super::history::{self, RequestLog};
use super::ops::BookingInfo;
use super::time;
use crate::config;
use jiff::Timestamp;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

const BASE_URL: &str = "https://balbuton.com";
pub const API: &str = "/api/v1";
const REQUEST_MAX_ATTEMPTS: u32 = 3;
const BACKOFF_BASE_SEC: u64 = 2;

pub type Res<T> = Result<T, String>;

pub fn db_err(e: rusqlite::Error) -> String {
    format!("db: {e}")
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Method {
    Get,
    Post,
}

impl Method {
    fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
        }
    }
}

#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub body: String,
}

impl Response {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn json(&self) -> Option<Value> {
        serde_json::from_str(&self.body).ok()
    }

    pub fn message(&self) -> String {
        self.json()
            .and_then(|v| v.get("message")?.as_str().map(str::to_string))
            .unwrap_or_else(|| format!("status {}", self.status))
    }
}

pub trait Transport {
    fn send(
        &self,
        method: Method,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<&str>,
    ) -> Res<Response>;
}

pub struct UreqTransport {
    agent: ureq::Agent,
}

impl UreqTransport {
    pub fn new() -> Self {
        UreqTransport {
            agent: ureq::AgentBuilder::new()
                .timeout(Duration::from_secs(30))
                .build(),
        }
    }
}

impl Transport for UreqTransport {
    fn send(
        &self,
        method: Method,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<&str>,
    ) -> Res<Response> {
        let url = format!("{BASE_URL}{path}");
        let mut req = match method {
            Method::Get => self.agent.get(&url),
            Method::Post => self.agent.post(&url),
        };
        for (k, v) in headers {
            req = req.set(k, v);
        }
        let result = match method {
            Method::Get => req.call(),
            Method::Post => req.send_string(body.unwrap_or("")),
        };
        let resp = match result {
            Ok(r) | Err(ureq::Error::Status(_, r)) => r,
            Err(e) => return Err(e.to_string()),
        };
        let status = resp.status();
        let body = resp.into_string().map_err(|e| e.to_string())?;
        Ok(Response { status, body })
    }
}

pub struct Req<'b> {
    pub method: Method,
    pub path: &'b str,
    pub context: &'b str,
    pub body: Option<String>,
    pub auth: bool,
    pub redact: bool,
}

impl<'b> Req<'b> {
    pub fn get(path: &'b str, context: &'b str) -> Self {
        Req {
            method: Method::Get,
            path,
            context,
            body: None,
            auth: true,
            redact: false,
        }
    }

    pub fn post(path: &'b str, context: &'b str, body: Value) -> Self {
        Req {
            method: Method::Post,
            path,
            context,
            body: Some(body.to_string()),
            auth: true,
            redact: false,
        }
    }
}

pub struct Client<'a> {
    pub(super) transport: &'a dyn Transport,
    pub(super) conn: &'a Connection,
    pub(super) cfg: &'a config::Pickle,
    pub(super) run_id: i64,
    pub(super) now: Box<dyn Fn() -> Timestamp + 'a>,
    pub(super) sleep: Box<dyn Fn(Duration) + 'a>,
    pub(super) token: Option<String>,
    pub(super) notes: Vec<String>,
    pub(super) details: Vec<String>,
    pub(super) failed: bool,
    pub(super) bookings_cache: Option<Vec<BookingInfo>>,
}

impl<'a> Client<'a> {
    pub fn new(
        transport: &'a dyn Transport,
        conn: &'a Connection,
        cfg: &'a config::Pickle,
        run_id: i64,
        now: Box<dyn Fn() -> Timestamp + 'a>,
        sleep: Box<dyn Fn(Duration) + 'a>,
    ) -> Res<Self> {
        let token = history::load_token(conn).map_err(db_err)?;
        Ok(Client {
            transport,
            conn,
            cfg,
            run_id,
            now,
            sleep,
            token,
            notes: Vec::new(),
            details: Vec::new(),
            failed: false,
            bookings_cache: None,
        })
    }

    pub fn now(&self) -> Timestamp {
        (self.now)()
    }

    pub(super) fn note(&mut self, text: String) {
        self.notes.push(text);
    }

    pub(super) fn fail_note(&mut self, text: String) {
        self.failed = true;
        self.notes.push(text);
    }

    pub(super) fn detail(&mut self, text: String) {
        self.details.push(text);
    }

    pub fn finish(&self, error: Option<&str>) -> Res<()> {
        let summary = self.notes.join("; ");
        let mut detail = self.details.clone();
        if let Some(e) = error {
            detail.push(e.chars().take(500).collect());
        }
        let detail = detail.join("\n");
        history::finish_run(
            self.conn,
            self.run_id,
            if error.is_some() || self.failed {
                "error"
            } else {
                "ok"
            },
            Some(&summary).filter(|s| !s.is_empty()).map(String::as_str),
            Some(&detail).filter(|d| !d.is_empty()).map(String::as_str),
            &time::iso(self.now()),
        )
        .map_err(db_err)
    }

    pub fn login(&mut self) -> Res<String> {
        let body = json!({
            "username": self.cfg.username,
            "password": self.cfg.password,
            "front": true,
        });
        let path = format!("{API}/auth/login");
        let req = Req {
            auth: false,
            redact: true,
            ..Req::post(&path, "login", body)
        };
        let resp = self.call(&req, 1)?;
        if !resp.ok() {
            return Err(format!("login failed (status {})", resp.status));
        }
        resp.json()
            .and_then(|v| v.pointer("/payload/token")?.as_str().map(str::to_string))
            .ok_or_else(|| "login response missing payload.token".to_string())
    }

    pub fn refresh_token(&mut self) -> Res<()> {
        let token = self.login()?;
        history::save_token(self.conn, &token, &time::iso(self.now())).map_err(db_err)?;
        self.token = Some(token);
        Ok(())
    }

    pub(super) fn call(&mut self, req: &Req, attempt: i64) -> Res<Response> {
        if req.auth && self.token.is_none() {
            self.refresh_token()?;
        }
        let token = self.token.clone().unwrap_or_default();
        let mut headers = vec![
            ("Content-Type", "application/json"),
            ("x-location-id", "134"),
            ("x-facility-group-id", "144"),
        ];
        if req.auth {
            headers.push(("x-access-token", token.as_str()));
        }

        let started = Instant::now();
        let result = self
            .transport
            .send(req.method, req.path, &headers, req.body.as_deref());
        let duration_ms = started.elapsed().as_millis() as i64;

        let (status, response_body, error) = match &result {
            Ok(r) => (Some(r.status as i64), Some(r.body.as_str()), None),
            Err(e) => (None, None, Some(e.as_str())),
        };
        history::record_request(
            self.conn,
            self.run_id,
            &RequestLog {
                method: req.method.as_str(),
                url: req.path,
                context: req.context,
                attempt,
                status,
                duration_ms,
                request_body: if req.redact {
                    None
                } else {
                    req.body.as_deref()
                },
                response_body: if req.redact { None } else { response_body },
                error,
            },
            &time::iso(self.now()),
        )
        .map_err(db_err)?;
        result
    }

    pub(super) fn request(&mut self, req: &Req) -> Res<Response> {
        let mut attempt: u32 = 1;
        let mut refreshed = false;
        loop {
            let resp = self.call(req, attempt as i64)?;
            if resp.ok() && resp.json().is_some_and(|v| v.is_object()) {
                return Ok(resp);
            }
            if !refreshed && resp.status == 401 && req.auth {
                refreshed = true;
                self.refresh_token()?;
                continue;
            }
            if attempt >= REQUEST_MAX_ATTEMPTS {
                return Err(format!(
                    "request failed after {attempt} attempts: {} {} (status {}): {}",
                    req.method.as_str(),
                    req.path,
                    resp.status,
                    resp.body
                ));
            }
            (self.sleep)(Duration::from_secs(BACKOFF_BASE_SEC.pow(attempt)));
            attempt += 1;
        }
    }
}

#[cfg(test)]
pub mod testing {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    pub struct Call {
        pub method: Method,
        pub path: String,
        pub headers: Vec<(String, String)>,
        pub body: Option<String>,
    }

    #[derive(Default)]
    pub struct Fake {
        pub responses: RefCell<VecDeque<Res<Response>>>,
        pub calls: RefCell<Vec<Call>>,
    }

    impl Fake {
        pub fn push(&self, status: u16, body: &str) {
            self.responses.borrow_mut().push_back(Ok(Response {
                status,
                body: body.to_string(),
            }));
        }

        pub fn paths(&self) -> Vec<String> {
            self.calls.borrow().iter().map(|c| c.path.clone()).collect()
        }
    }

    impl Transport for Fake {
        fn send(
            &self,
            method: Method,
            path: &str,
            headers: &[(&str, &str)],
            body: Option<&str>,
        ) -> Res<Response> {
            self.calls.borrow_mut().push(Call {
                method,
                path: path.to_string(),
                headers: headers
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
                body: body.map(str::to_string),
            });
            self.responses
                .borrow_mut()
                .pop_front()
                .expect("unexpected request")
        }
    }

    pub fn cfg() -> config::Pickle {
        config::Pickle {
            full_name: "Michael Crockett".into(),
            member_id: "176064".into(),
            username: "willcrock".into(),
            password: "pw".into(),
            schedule: vec![],
        }
    }

    pub struct Clock {
        pub slept: Rc<RefCell<Vec<Duration>>>,
    }

    pub fn client<'a>(
        fake: &'a Fake,
        conn: &'a Connection,
        cfg: &'a config::Pickle,
        start: &str,
    ) -> (Client<'a>, Clock) {
        let ms = Rc::new(std::cell::Cell::new(
            start.parse::<Timestamp>().unwrap().as_millisecond(),
        ));
        let slept = Rc::new(RefCell::new(Vec::new()));
        let (now_ms, sleep_ms, sleep_log) = (ms.clone(), ms.clone(), slept.clone());
        let run_id = history::start_run(conn, "test", "t", "2026-10-18T00:00:00Z").unwrap();
        let client = Client::new(
            fake,
            conn,
            cfg,
            run_id,
            Box::new(move || Timestamp::from_millisecond(now_ms.get()).unwrap()),
            Box::new(move |d| {
                sleep_ms.set(sleep_ms.get() + d.as_millis() as i64);
                sleep_log.borrow_mut().push(d);
            }),
        )
        .unwrap();
        (client, Clock { slept })
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;
    use crate::db::open_memory;

    const T0: &str = "2026-10-18T13:00:00Z";
    const LOGIN_OK: &str = r#"{"payload":{"token":"fresh","password":"HASH"}}"#;

    #[test]
    fn missing_token_logs_in_first_and_redacts_the_login() {
        let conn = open_memory();
        let (fake, cfg) = (Fake::default(), cfg());
        fake.push(200, LOGIN_OK);
        fake.push(200, r#"{"ok":true}"#);
        let (mut c, _) = client(&fake, &conn, &cfg, T0);
        c.request(&Req::get("/api/v1/x", "x")).unwrap();

        assert_eq!(fake.paths(), ["/api/v1/auth/login", "/api/v1/x"]);
        let calls = fake.calls.borrow();
        assert!(calls[1]
            .headers
            .contains(&("x-access-token".to_string(), "fresh".to_string())));
        assert_eq!(
            history::load_token(&conn).unwrap().as_deref(),
            Some("fresh")
        );
        let leaked: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pickle_requests WHERE context = 'login' \
                 AND (request_body IS NOT NULL OR response_body IS NOT NULL)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(leaked, 0);
    }

    #[test]
    fn a_401_refreshes_the_token_once_then_retries() {
        let conn = open_memory();
        let (fake, cfg) = (Fake::default(), cfg());
        history::save_token(&conn, "stale", "t").unwrap();
        fake.push(401, r#"{"message":"expired"}"#);
        fake.push(200, LOGIN_OK);
        fake.push(200, r#"{"ok":true}"#);
        let (mut c, clock) = client(&fake, &conn, &cfg, T0);
        c.request(&Req::get("/api/v1/x", "x")).unwrap();

        assert_eq!(
            fake.paths(),
            ["/api/v1/x", "/api/v1/auth/login", "/api/v1/x"]
        );
        assert!(clock.slept.borrow().is_empty());
        assert_eq!(
            history::load_token(&conn).unwrap().as_deref(),
            Some("fresh")
        );
    }

    #[test]
    fn transient_failures_back_off_then_give_up_after_three_attempts() {
        let conn = open_memory();
        let (fake, cfg) = (Fake::default(), cfg());
        history::save_token(&conn, "tok", "t").unwrap();
        for _ in 0..3 {
            fake.push(504, "<html>gateway</html>");
        }
        let (mut c, clock) = client(&fake, &conn, &cfg, T0);
        let err = c.request(&Req::get("/api/v1/x", "x")).unwrap_err();

        assert!(err.contains("after 3 attempts"), "{err}");
        assert!(err.contains("status 504"), "{err}");
        assert_eq!(
            *clock.slept.borrow(),
            [Duration::from_secs(2), Duration::from_secs(4)]
        );
    }

    #[test]
    fn a_refresh_that_still_401s_is_not_retried_forever() {
        let conn = open_memory();
        let (fake, cfg) = (Fake::default(), cfg());
        history::save_token(&conn, "stale", "t").unwrap();
        fake.push(401, "{}");
        fake.push(200, LOGIN_OK);
        fake.push(401, "{}");
        fake.push(401, "{}");
        fake.push(401, "{}");
        let (mut c, _) = client(&fake, &conn, &cfg, T0);
        assert!(c.request(&Req::get("/api/v1/x", "x")).is_err());
        assert_eq!(fake.calls.borrow().len(), 5);
    }

    #[test]
    fn login_without_a_token_in_the_response_fails() {
        let conn = open_memory();
        let (fake, cfg) = (Fake::default(), cfg());
        fake.push(200, r#"{"payload":{}}"#);
        let (mut c, _) = client(&fake, &conn, &cfg, T0);
        assert_eq!(
            c.login().unwrap_err(),
            "login response missing payload.token"
        );
    }
}
