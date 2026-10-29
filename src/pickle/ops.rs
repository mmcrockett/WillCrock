use super::api::{db_err, Client, Req, Res, Response, API};
use super::history::{self, BookingRow};
use super::time;
use jiff::Timestamp;
use serde_json::{json, Value};
use std::time::Duration;

const COURTS: [(&str, i64); 5] = [
    ("5", 1176),
    ("5C", 1178),
    ("5D", 1179),
    ("6A", 1180),
    ("6B", 1181),
];
const ANY_COURTS: [i64; 4] = [1178, 1179, 1180, 1181];
const LOCATION_ID: &str = "134";
const CHECKIN_WINDOW_BEFORE_MIN: f64 = 60.0;
const CHECKIN_WINDOW_AFTER_MIN: f64 = 150.0;
const BOOKING_OPEN_HOUR_CENTRAL: i8 = 6;
const BOOKING_OPEN_MAX_WAIT_SEC: f64 = 600.0;
const MAX_NOT_ELIGIBLE_RETRIES: u32 = 18;
const FAST_RETRY: Duration = Duration::from_secs(5);
const SLOW_RETRY: Duration = Duration::from_secs(10);
const FAST_RETRY_SECS_AFTER_OPEN: i64 = 30;

#[derive(Clone)]
pub struct BookingInfo {
    pub id: String,
    pub court: Option<String>,
    pub has_checkin: bool,
    pub start_time: String,
    pub end_time: String,
}

pub enum CancelOutcome {
    NotAllowed,
    Done(Response),
}

fn opt_string(v: Option<&Value>) -> Option<String> {
    match v {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(other) => Some(other.to_string()),
    }
}

fn non_empty(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
        Some(Value::String(s)) => !s.is_empty(),
        Some(_) => true,
    }
}

pub fn court_attempts(court: &str) -> Res<(String, Vec<Vec<i64>>)> {
    let label = format!("court {court}");
    match court.to_uppercase().as_str() {
        "ANY" => Ok((
            "any court".to_string(),
            ANY_COURTS.iter().map(|c| vec![*c]).collect(),
        )),
        "ALL" => Ok((label, vec![ANY_COURTS.to_vec()])),
        name => COURTS
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, id)| (label, vec![vec![*id]]))
            .ok_or_else(|| format!("unknown court {court:?}")),
    }
}

fn court_name(ids: &[i64]) -> String {
    ids.iter()
        .map(|id| {
            COURTS
                .iter()
                .find(|(_, c)| c == id)
                .map(|(n, _)| n.to_string())
                .unwrap_or_else(|| id.to_string())
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn reserve_body(
    start: Timestamp,
    end: Timestamp,
    courts: &[i64],
    member_id: &str,
    full_name: &str,
) -> Value {
    json!({
        "start_time": time::iso(start),
        "end_time": time::iso(end),
        "instantBook": true,
        "courts": courts,
        "userId": member_id,
        "force": false,
        "sport_id": "1",
        "fullName": full_name,
        "assigned": [],
        "tags": [],
        "partners": [],
        "guests": [],
        "add_on_id": null,
    })
}

// The booking window not being open yet comes back as an eligibility error
// rather than a distinct status, so it is matched on the message.
fn not_eligible(resp: &Response) -> bool {
    resp.message().to_lowercase().contains("not eligible")
}

fn retry_delay(now: Timestamp, fast_until: Timestamp) -> Duration {
    if now < fast_until {
        FAST_RETRY
    } else {
        SLOW_RETRY
    }
}

pub fn pick_checkin(bookings: &[BookingInfo], now: Timestamp) -> Option<&BookingInfo> {
    bookings
        .iter()
        .filter(|b| !b.has_checkin)
        .filter_map(|b| {
            let start = time::parse(&b.start_time).ok()?;
            let minutes = time::seconds_between(now, start) / 60.0;
            (-CHECKIN_WINDOW_AFTER_MIN..=CHECKIN_WINDOW_BEFORE_MIN)
                .contains(&minutes)
                .then_some((b, minutes.abs()))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(b, _)| b)
}

impl Client<'_> {
    fn fetch_booking_list(&mut self, context: &str) -> Res<Vec<Value>> {
        let now = self.now();
        let body = json!({
            "startDate": time::rfc3339_offset(&time::central_at(now, 0, 0, 0)?),
            "endDate": time::rfc3339_offset(&time::central_at(now, 7, 23, 0)?),
            "type": "0",
            "is_coach": false,
        });
        let path = format!("{API}/members/booking/{}", self.cfg.member_id);
        let resp = self.request(&Req::post(&path, context, body))?;
        Ok(resp
            .json()
            .and_then(|v| v.pointer("/payload/bookings_history")?.as_array().cloned())
            .unwrap_or_default())
    }

    fn booking_detail(&mut self, id: &str) -> Res<(bool, String)> {
        let path = format!("{API}/appointment/get/{id}");
        let resp = self.request(&Req::get(&path, "booking"))?;
        let payload = resp
            .json()
            .and_then(|v| v.get("payload").cloned())
            .ok_or_else(|| format!("booking {id}: response missing payload"))?;
        Ok((
            non_empty(payload.get("checkins")),
            opt_string(payload.get("start_time")).unwrap_or_default(),
        ))
    }

    fn bookings(&mut self) -> Res<Vec<BookingInfo>> {
        if let Some(cached) = &self.bookings_cache {
            return Ok(cached.clone());
        }
        let mut out = Vec::new();
        for appt in self.fetch_booking_list("bookings")? {
            let id = opt_string(appt.get("id")).ok_or("booking without an id")?;
            let (has_checkin, _) = self.booking_detail(&id)?;
            out.push(BookingInfo {
                id,
                court: opt_string(appt.get("court_names")),
                has_checkin,
                start_time: opt_string(appt.get("start_time")).unwrap_or_default(),
                end_time: opt_string(appt.get("end_time")).unwrap_or_default(),
            });
        }
        self.bookings_cache = Some(out.clone());
        Ok(out)
    }

    pub fn check_in_next(&mut self) -> Res<()> {
        let now = self.now();
        let bookings = self.bookings()?;
        let Some(next) = pick_checkin(&bookings, now).cloned() else {
            println!(
                "checkin: no booking within the checkin window at {}",
                time::iso(now)
            );
            return Ok(());
        };
        println!(
            "checkin: checking in booking {} ({}) at {}",
            next.id,
            next.start_time,
            time::iso(now)
        );

        let location = self.request(&Req::post(
            &format!("{API}/checkin"),
            "checkin_location",
            json!({ "location_id": LOCATION_ID }),
        ))?;
        if location.ok() {
            let date = time::rfc3339_offset(&time::central(now)?);
            self.request(&Req::post(
                &format!("{API}/members_checkin/addcheckin/{}", next.id),
                "checkin_member",
                json!({ "date": date, "users": [self.cfg.member_id] }),
            ))?;
            self.note(format!(
                "checked in {}",
                time::slot_label(time::parse(&next.start_time)?, None)
            ));
        }
        self.bookings_cache = None;
        Ok(())
    }

    pub fn sync(&mut self) -> Res<()> {
        if let Err(e) = self.check_in_next() {
            self.fail_note(format!("check_in_next: {e}"));
        }
        let rows: Vec<BookingRow> = self
            .bookings()?
            .into_iter()
            .map(|b| BookingRow {
                id: b.id,
                start_time: b.start_time,
                end_time: b.end_time,
                court: b.court,
                checked_in: b.has_checkin,
            })
            .collect();
        history::replace_bookings(self.conn, &rows, &time::iso(self.now())).map_err(db_err)
    }

    pub fn reserve_today(&mut self, dry_run: bool) -> Res<Vec<Response>> {
        let weekday = time::central(self.now())?
            .strftime("%A")
            .to_string()
            .to_lowercase();
        let entries: Vec<_> = self
            .cfg
            .schedule
            .iter()
            .filter(|e| e.day.to_lowercase() == weekday)
            .map(|e| (e.start.clone(), e.duration, e.court.clone()))
            .collect();

        if !entries.is_empty() {
            if let Err(e) = self.fetch_booking_list("warmup") {
                self.detail(format!("warmup failed: {e}"));
                self.refresh_token()?;
            }
            self.wait_for_booking_open()?;
        }
        entries
            .into_iter()
            .map(|(start, minutes, court)| {
                self.reserve(&start, Some(minutes), Some(&court), dry_run)
            })
            .collect()
    }

    fn wait_for_booking_open(&mut self) -> Res<()> {
        let now = self.now();
        let open_at = time::central_at(now, 0, BOOKING_OPEN_HOUR_CENTRAL, 0)?;
        let remaining = time::seconds_between(now, open_at.timestamp());
        if remaining > 0.0 && remaining <= BOOKING_OPEN_MAX_WAIT_SEC {
            println!(
                "reserve: waiting {remaining:.2}s until bookings open at {}",
                time::rfc3339_offset(&open_at)
            );
            (self.sleep)(Duration::from_secs_f64(remaining));
        }
        Ok(())
    }

    pub fn reserve(
        &mut self,
        start: &str,
        minutes: Option<i64>,
        court: Option<&str>,
        dry_run: bool,
    ) -> Res<Response> {
        let minutes = minutes.unwrap_or(90);
        let (label, attempts) = court_attempts(court.unwrap_or("ANY"))?;
        let (hr, min) = start
            .split_once(':')
            .and_then(|(h, m)| Some((h.trim().parse::<i8>().ok()?, m.trim().parse::<i8>().ok()?)))
            .ok_or_else(|| format!("bad start {start:?}, expected HH:MM"))?;

        let start_ts = time::central_at(self.now(), 7, hr, min)?.timestamp();
        let end_ts = time::plus_millis(start_ts, minutes * 60_000);
        let slot = format!("{} {label}", time::slot_label(start_ts, Some(end_ts)));
        let prefix = if dry_run { "dry run " } else { "" };

        let open_at = time::central_at(self.now(), 0, BOOKING_OPEN_HOUR_CENTRAL, 0)?;
        let fast_until = time::plus_millis(open_at.timestamp(), FAST_RETRY_SECS_AFTER_OPEN * 1000);
        let mut retries: u32 = 0;
        let mut last = None;
        for courts in &attempts {
            let body = reserve_body(
                start_ts,
                end_ts,
                courts,
                &self.cfg.member_id,
                &self.cfg.full_name,
            );
            println!("rattempt: {}", time::iso(self.now()));
            println!("rrequest: {body}");

            let retries_before = retries;
            let book_path = format!("{API}/appointment/add_book");
            let resp = loop {
                let req = if dry_run {
                    Req::get("", "reserve")
                } else {
                    Req::post(&book_path, "reserve", body.clone())
                };
                let resp = self.call(&req, 1)?;
                if !not_eligible(&resp) || retries >= MAX_NOT_ELIGIBLE_RETRIES {
                    break resp;
                }
                retries += 1;
                (self.sleep)(retry_delay(self.now(), fast_until));
            };
            let retried = retries - retries_before;

            self.detail(format!(
                "{}: {}{}",
                court_name(courts),
                resp.message(),
                if retried > 0 {
                    format!(" (retried {retried}x)")
                } else {
                    String::new()
                }
            ));
            if resp.ok() {
                self.note(format!("{prefix}booked {slot}"));
                return Ok(resp);
            }
            last = Some(resp);
        }

        self.fail_note(format!("{prefix}failed to book {slot}"));
        last.ok_or_else(|| "no courts to try".to_string())
    }

    pub fn cancel(&mut self, id: &str, confirm_date: Option<&str>) -> Res<CancelOutcome> {
        if let Some(date) = confirm_date {
            let (_, start) = self.booking_detail(id)?;
            let actual = time::central(time::parse(&start)?)?
                .strftime("%Y%m%d")
                .to_string();
            if actual != date {
                self.fail_note(format!("cancel {id}: not allowed"));
                return Ok(CancelOutcome::NotAllowed);
            }
        }

        let resp = self.call(
            &Req::post(
                &format!("{API}/appointment/cancel/{id}"),
                "cancel",
                json!({ "cancel_for": "court", "apt_id": id }),
            ),
            1,
        )?;
        if resp.ok() {
            self.note(format!("cancelled {id}"));
        } else {
            self.fail_note(format!("cancel {id}: {}", resp.message()));
        }
        Ok(CancelOutcome::Done(resp))
    }
}

#[cfg(test)]
mod tests {
    use super::super::api::testing::*;
    use super::super::api::Method;
    use super::*;
    use crate::db::open_memory;

    const MONDAY_6AM: &str = "2026-10-12T11:00:00Z";

    fn ts(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    fn info(id: &str, start: &str, has_checkin: bool) -> BookingInfo {
        BookingInfo {
            id: id.to_string(),
            court: None,
            has_checkin,
            start_time: start.to_string(),
            end_time: start.to_string(),
        }
    }

    fn with_token(conn: &rusqlite::Connection) {
        history::save_token(conn, "tok", "t").unwrap();
    }

    #[test]
    fn checkin_window_picks_the_closest_unchecked_booking() {
        let now = ts("2026-10-18T13:00:00Z");
        let bookings = [
            info("late", "2026-10-18T15:40:00Z", false),
            info("done", "2026-10-18T13:10:00Z", true),
            info("far", "2026-10-18T13:50:00Z", false),
            info("near", "2026-10-18T13:20:00Z", false),
            info("past", "2026-10-18T12:00:00Z", false),
        ];
        assert_eq!(pick_checkin(&bookings, now).unwrap().id, "near");
        assert!(pick_checkin(&bookings[..1], now).is_none());
        assert!(pick_checkin(&[info("x", "2026-10-18T10:29:00Z", false)], now).is_none());
        assert!(pick_checkin(&[info("x", "2026-10-18T10:31:00Z", false)], now).is_some());
    }

    #[test]
    fn courts_resolve_to_attempts() {
        assert_eq!(court_attempts("any").unwrap().1.len(), 4);
        assert_eq!(court_attempts("ANY").unwrap().0, "any court");
        assert_eq!(court_attempts("all").unwrap().1, vec![ANY_COURTS.to_vec()]);
        assert_eq!(
            court_attempts("6b").unwrap(),
            ("court 6b".into(), vec![vec![1181]])
        );
        assert!(court_attempts("9z").is_err());
        assert_eq!(court_name(&[1178, 1181]), "5C, 6B");
    }

    #[test]
    fn any_court_falls_through_to_the_next_court() {
        let conn = open_memory();
        let (fake, cfg) = (Fake::default(), cfg());
        with_token(&conn);
        fake.push(409, r#"{"message":"Overlap Error"}"#);
        fake.push(200, r#"{"message":"ok"}"#);
        let (mut c, _) = client(&fake, &conn, &cfg, MONDAY_6AM);
        let resp = c.reserve("07:30", Some(90), None, false).unwrap();

        assert!(resp.ok());
        let calls = fake.calls.borrow();
        assert_eq!(calls.len(), 2);
        let second: Value = serde_json::from_str(calls[1].body.as_ref().unwrap()).unwrap();
        assert_eq!(second["courts"], json!([1179]));
        assert_eq!(second["start_time"], "2026-10-19T12:30:00Z");
        assert_eq!(second["end_time"], "2026-10-19T14:00:00Z");
        assert_eq!(second["userId"], "176064");
        drop(calls);
        assert_eq!(c.notes, ["booked Mon Oct 19 7:30-9:00am any court"]);
        assert_eq!(c.details, ["5C: Overlap Error", "5D: ok"]);
        assert!(!c.failed);
    }

    #[test]
    fn not_eligible_is_retried_until_the_window_opens() {
        let conn = open_memory();
        let (fake, cfg) = (Fake::default(), cfg());
        with_token(&conn);
        for _ in 0..3 {
            fake.push(400, r#"{"message":"User is Not Eligible"}"#);
        }
        fake.push(200, r#"{"message":"ok"}"#);
        let (mut c, clock) = client(&fake, &conn, &cfg, MONDAY_6AM);
        c.reserve("07:30", None, Some("5C"), false).unwrap();

        assert_eq!(*clock.slept.borrow(), [FAST_RETRY; 3]);
        assert_eq!(c.details, ["5C: ok (retried 3x)"]);
    }

    #[test]
    fn not_eligible_retries_every_5s_until_half_past_then_every_10s_up_to_18() {
        let conn = open_memory();
        let (fake, cfg) = (Fake::default(), cfg());
        with_token(&conn);
        for _ in 0..60 {
            fake.push(400, r#"{"message":"not eligible"}"#);
        }
        let (mut c, clock) = client(&fake, &conn, &cfg, MONDAY_6AM);
        let resp = c.reserve("07:30", None, Some("5C"), false).unwrap();

        assert_eq!(resp.status, 400);
        assert_eq!(fake.calls.borrow().len(), 19);
        let mut expected = vec![FAST_RETRY; 6];
        expected.extend([SLOW_RETRY; 12]);
        assert_eq!(*clock.slept.borrow(), expected);
        assert!(c.failed);
        assert_eq!(c.notes, ["failed to book Mon Oct 19 7:30-9:00am court 5C"]);
        assert_eq!(c.details, ["5C: not eligible (retried 18x)"]);
    }

    #[test]
    fn the_retry_budget_is_shared_across_courts() {
        let conn = open_memory();
        let (fake, cfg) = (Fake::default(), cfg());
        with_token(&conn);
        for _ in 0..100 {
            fake.push(400, r#"{"message":"not eligible"}"#);
        }
        let (mut c, _) = client(&fake, &conn, &cfg, MONDAY_6AM);
        c.reserve("07:30", None, None, false).unwrap();

        assert_eq!(fake.calls.borrow().len(), 22);
    }

    #[test]
    fn dry_run_never_posts() {
        let conn = open_memory();
        let (fake, cfg) = (Fake::default(), cfg());
        with_token(&conn);
        fake.push(200, "<html>ok</html>");
        let (mut c, _) = client(&fake, &conn, &cfg, MONDAY_6AM);
        c.reserve("07:30", None, Some("5C"), true).unwrap();

        let calls = fake.calls.borrow();
        assert_eq!((calls[0].method, calls[0].path.as_str()), (Method::Get, ""));
        assert_eq!(calls.len(), 1);
        drop(calls);
        assert_eq!(c.notes, ["dry run booked Mon Oct 19 7:30-9:00am court 5C"]);
    }

    #[test]
    fn bad_start_and_court_are_errors_before_any_request() {
        let conn = open_memory();
        let (fake, cfg) = (Fake::default(), cfg());
        let (mut c, _) = client(&fake, &conn, &cfg, MONDAY_6AM);
        assert!(c.reserve("730", None, None, false).is_err());
        assert!(c.reserve("07:30", None, Some("zz"), false).is_err());
        assert!(fake.calls.borrow().is_empty());
    }

    #[test]
    fn reserve_today_waits_for_six_then_books_todays_entries() {
        let conn = open_memory();
        let (fake, mut cfg) = (Fake::default(), cfg());
        with_token(&conn);
        cfg.schedule = vec![
            crate::config::ScheduleEntry {
                day: "Saturday".into(),
                start: "10:00".into(),
                duration: 60,
                court: "5C".into(),
            },
            crate::config::ScheduleEntry {
                day: "monday".into(),
                start: "10:00".into(),
                duration: 60,
                court: "5C".into(),
            },
        ];
        fake.push(200, r#"{"payload":{"bookings_history":[]}}"#);
        fake.push(200, r#"{"message":"ok"}"#);
        let (mut c, clock) = client(&fake, &conn, &cfg, "2026-10-17T10:55:00Z");
        let resps = c.reserve_today(false).unwrap();

        assert_eq!(resps.len(), 1);
        assert_eq!(*clock.slept.borrow(), [Duration::from_secs(300)]);
        assert_eq!(
            fake.paths(),
            [
                "/api/v1/members/booking/176064",
                "/api/v1/appointment/add_book"
            ]
        );
        assert_eq!(c.notes, ["booked Sat Oct 24 10:00-11:00am court 5C"]);
    }

    #[test]
    fn a_failing_warmup_refreshes_the_token_and_still_books() {
        let conn = open_memory();
        let (fake, mut cfg) = (Fake::default(), cfg());
        with_token(&conn);
        cfg.schedule = vec![crate::config::ScheduleEntry {
            day: "saturday".into(),
            start: "10:00".into(),
            duration: 60,
            court: "5C".into(),
        }];
        for _ in 0..3 {
            fake.push(500, "boom");
        }
        fake.push(200, r#"{"payload":{"token":"fresh"}}"#);
        fake.push(200, r#"{"message":"ok"}"#);
        let (mut c, clock) = client(&fake, &conn, &cfg, "2026-10-17T10:55:00Z");
        c.reserve_today(false).unwrap();

        let paths = fake.paths();
        assert_eq!(paths[3], "/api/v1/auth/login");
        assert_eq!(paths[4], "/api/v1/appointment/add_book");
        assert!(c.details[0].starts_with("warmup failed: request failed after 3 attempts"));
        assert_eq!(c.notes, ["booked Sat Oct 24 10:00-11:00am court 5C"]);
        assert!(!c.failed);
        assert_eq!(clock.slept.borrow().last(), Some(&Duration::from_secs(294)));
    }

    #[test]
    fn reserve_today_is_silent_when_nothing_is_scheduled() {
        let conn = open_memory();
        let (fake, cfg) = (Fake::default(), cfg());
        let (mut c, clock) = client(&fake, &conn, &cfg, MONDAY_6AM);
        assert!(c.reserve_today(false).unwrap().is_empty());
        assert!(fake.calls.borrow().is_empty());
        assert!(clock.slept.borrow().is_empty());
    }

    #[test]
    fn cancel_refuses_a_mismatched_date() {
        let conn = open_memory();
        let (fake, cfg) = (Fake::default(), cfg());
        with_token(&conn);
        fake.push(
            200,
            r#"{"payload":{"checkins":[],"start_time":"2026-10-19T03:00:00.000Z"}}"#,
        );
        let (mut c, _) = client(&fake, &conn, &cfg, MONDAY_6AM);
        let out = c.cancel("55", Some("20261019")).unwrap();

        assert!(matches!(out, CancelOutcome::NotAllowed));
        assert_eq!(fake.paths(), ["/api/v1/appointment/get/55"]);
        assert!(c.failed);
    }

    #[test]
    fn cancel_posts_when_the_central_date_matches() {
        let conn = open_memory();
        let (fake, cfg) = (Fake::default(), cfg());
        with_token(&conn);
        fake.push(
            200,
            r#"{"payload":{"checkins":[],"start_time":"2026-10-19T03:00:00.000Z"}}"#,
        );
        fake.push(200, r#"{"message":"cancelled"}"#);
        let (mut c, _) = client(&fake, &conn, &cfg, MONDAY_6AM);
        let out = c.cancel("55", Some("20261018")).unwrap();

        assert!(matches!(out, CancelOutcome::Done(r) if r.ok()));
        let calls = fake.calls.borrow();
        assert_eq!(calls[1].path, "/api/v1/appointment/cancel/55");
        assert_eq!(
            serde_json::from_str::<Value>(calls[1].body.as_ref().unwrap()).unwrap(),
            json!({"cancel_for": "court", "apt_id": "55"})
        );
        drop(calls);
        assert_eq!(c.notes, ["cancelled 55"]);
    }

    #[test]
    fn sync_checks_in_and_refreshes_the_bookings_table() {
        let conn = open_memory();
        let (fake, cfg) = (Fake::default(), cfg());
        with_token(&conn);
        fake.push(
            200,
            r#"{"payload":{"bookings_history":[{"id":5,"court_names":"6B",
                "start_time":"2026-10-18T13:30:00.000Z","end_time":"2026-10-18T15:00:00.000Z"}]}}"#,
        );
        fake.push(200, r#"{"payload":{"checkins":null,"start_time":"x"}}"#);
        fake.push(200, r#"{"ok":true}"#);
        fake.push(200, r#"{"ok":true}"#);
        fake.push(
            200,
            r#"{"payload":{"bookings_history":[{"id":5,"court_names":"6B",
                "start_time":"2026-10-18T13:30:00.000Z","end_time":"2026-10-18T15:00:00.000Z"}]}}"#,
        );
        fake.push(
            200,
            r#"{"payload":{"checkins":[{"id":1}],"start_time":"x"}}"#,
        );
        let (mut c, _) = client(&fake, &conn, &cfg, "2026-10-18T13:00:00Z");
        c.sync().unwrap();

        assert_eq!(
            fake.paths(),
            [
                "/api/v1/members/booking/176064",
                "/api/v1/appointment/get/5",
                "/api/v1/checkin",
                "/api/v1/members_checkin/addcheckin/5",
                "/api/v1/members/booking/176064",
                "/api/v1/appointment/get/5",
            ]
        );
        assert_eq!(c.notes, ["checked in Sun Oct 18 8:30am"]);
        let (id, court, checked_in): (String, String, bool) = conn
            .query_row(
                "SELECT id, court, checked_in FROM pickle_bookings",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!((id.as_str(), court.as_str(), checked_in), ("5", "6B", true));
    }

    #[test]
    fn a_failing_checkin_is_noted_but_the_sync_still_completes() {
        let conn = open_memory();
        let (fake, cfg) = (Fake::default(), cfg());
        with_token(&conn);
        for _ in 0..3 {
            fake.push(500, "boom");
        }
        for _ in 0..3 {
            fake.push(500, "boom");
        }
        let (mut c, _) = client(&fake, &conn, &cfg, "2026-10-18T13:00:00Z");
        assert!(c.sync().is_err());
        assert!(c.failed);
        assert!(c.notes[0].starts_with("check_in_next: request failed after 3 attempts"));
    }

    #[test]
    fn finish_records_status_summary_and_detail() {
        let conn = open_memory();
        let (fake, cfg) = (Fake::default(), cfg());
        let (mut c, _) = client(&fake, &conn, &cfg, MONDAY_6AM);
        c.note("booked x".into());
        c.detail("5C: ok".into());
        c.finish(Some("kaboom")).unwrap();
        let (status, summary, detail): (String, String, String) = conn
            .query_row("SELECT status, summary, detail FROM pickle_runs", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        assert_eq!(status, "error");
        assert_eq!(summary, "booked x");
        assert_eq!(detail, "5C: ok\nkaboom");
    }
}
