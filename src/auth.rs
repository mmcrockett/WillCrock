use rand::Rng;
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};

const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const CODE_LEN: usize = 8;
const CODE_TTL: i64 = 600;
const MAX_ATTEMPTS: i64 = 5;
const RESEND_COOLDOWN: i64 = 60;
pub const SESSION_TTL: i64 = 30 * 24 * 3600;
const SLIDE_AFTER: i64 = 3600;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Feature {
    Sms,
    Pickle,
}

impl Feature {
    pub const ALL: [Feature; 2] = [Feature::Sms, Feature::Pickle];

    pub fn as_str(self) -> &'static str {
        match self {
            Feature::Sms => "sms",
            Feature::Pickle => "pickle",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Feature::Sms => "SMS",
            Feature::Pickle => "Pickle",
        }
    }

    pub fn parse(s: &str) -> Option<Feature> {
        Feature::ALL.into_iter().find(|f| f.as_str() == s)
    }
}

pub struct User {
    pub id: i64,
    pub email: String,
    pub name: Option<String>,
    pub is_admin: bool,
    pub disabled: bool,
    pub access: Vec<Feature>,
}

impl User {
    pub fn can(&self, feature: Feature) -> bool {
        self.is_admin || self.access.contains(&feature)
    }
}

fn parse_access(list: Option<String>) -> Vec<Feature> {
    list.iter()
        .flat_map(|l| l.split(','))
        .filter_map(Feature::parse)
        .collect()
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hash(s: &str) -> String {
    to_hex(&Sha256::digest(s.as_bytes()))
}

pub fn normalize_email(email: &str) -> String {
    email.trim().to_lowercase()
}

pub fn normalize_code(input: &str) -> String {
    input
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| match c.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            c => c,
        })
        .collect()
}

pub fn display_code(code: &str) -> String {
    format!("{}-{}", &code[..4], &code[4..])
}

fn generate_code() -> String {
    let mut rng = rand::thread_rng();
    (0..CODE_LEN)
        .map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char)
        .collect()
}

fn generate_token() -> String {
    to_hex(&rand::thread_rng().gen::<[u8; 32]>())
}

fn user_from_row(r: &rusqlite::Row) -> rusqlite::Result<User> {
    Ok(User {
        id: r.get(0)?,
        email: r.get(1)?,
        name: r.get(2)?,
        is_admin: r.get(3)?,
        disabled: r.get::<_, Option<i64>>(4)?.is_some(),
        access: parse_access(r.get(5)?),
    })
}

const USER_COLS: &str = "id, email, name, is_admin, disabled_at,
    (SELECT group_concat(feature) FROM user_access WHERE user_id = users.id)";

fn find_active_user(conn: &Connection, email: &str) -> rusqlite::Result<Option<User>> {
    conn.query_row(
        &format!("SELECT {USER_COLS} FROM users WHERE email = ?1 AND disabled_at IS NULL"),
        [normalize_email(email)],
        user_from_row,
    )
    .optional()
}

/// Returns the (stored email, code) to send, or None when nothing should be sent.
pub fn request_code(
    conn: &Connection,
    email: &str,
    now: i64,
) -> rusqlite::Result<Option<(String, String)>> {
    let Some(user) = find_active_user(conn, email)? else {
        return Ok(None);
    };
    let recent: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM login_codes
           WHERE user_id = ?1 AND used_at IS NULL AND expires_at > ?2)",
        params![user.id, now + CODE_TTL - RESEND_COOLDOWN],
        |r| r.get(0),
    )?;
    if recent {
        return Ok(None);
    }
    conn.execute(
        "UPDATE login_codes SET used_at = ?2 WHERE user_id = ?1 AND used_at IS NULL",
        params![user.id, now],
    )?;
    let code = generate_code();
    conn.execute(
        "INSERT INTO login_codes (user_id, code_hash, expires_at) VALUES (?1, ?2, ?3)",
        params![user.id, hash(&code), now + CODE_TTL],
    )?;
    Ok(Some((user.email, code)))
}

/// On a correct code, returns a fresh session token.
pub fn verify_code(
    conn: &Connection,
    email: &str,
    input: &str,
    now: i64,
) -> rusqlite::Result<Option<String>> {
    let Some(user) = find_active_user(conn, email)? else {
        return Ok(None);
    };
    let row: Option<(i64, String, i64)> = conn
        .query_row(
            "SELECT id, code_hash, attempts FROM login_codes
             WHERE user_id = ?1 AND used_at IS NULL AND expires_at > ?2
             ORDER BY id DESC LIMIT 1",
            params![user.id, now],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((id, code_hash, attempts)) = row else {
        return Ok(None);
    };
    if attempts >= MAX_ATTEMPTS {
        return Ok(None);
    }
    conn.execute(
        "UPDATE login_codes SET attempts = attempts + 1 WHERE id = ?1",
        [id],
    )?;
    if hash(&normalize_code(input)) != code_hash {
        return Ok(None);
    }
    conn.execute(
        "UPDATE login_codes SET used_at = ?2 WHERE id = ?1",
        params![id, now],
    )?;
    create_session(conn, user.id, now).map(Some)
}

fn create_session(conn: &Connection, user_id: i64, now: i64) -> rusqlite::Result<String> {
    conn.execute("DELETE FROM sessions WHERE expires_at <= ?1", [now])?;
    conn.execute(
        "DELETE FROM login_codes WHERE expires_at <= ?1",
        [now - 86400],
    )?;
    let token = generate_token();
    conn.execute(
        "INSERT INTO sessions (user_id, token_hash, created_at, last_seen_at, expires_at)
         VALUES (?1, ?2, ?3, ?3, ?4)",
        params![user_id, hash(&token), now, now + SESSION_TTL],
    )?;
    Ok(token)
}

/// Returns the session's user and whether its expiry was extended (cookie needs re-sending).
pub fn session_user(
    conn: &Connection,
    token: &str,
    now: i64,
) -> rusqlite::Result<Option<(User, bool)>> {
    let row = conn
        .query_row(
            "SELECT s.id, s.last_seen_at, u.id, u.email, u.name, u.is_admin, u.disabled_at,
               (SELECT group_concat(feature) FROM user_access WHERE user_id = u.id)
             FROM sessions s JOIN users u ON u.id = s.user_id
             WHERE s.token_hash = ?1 AND s.expires_at > ?2 AND u.disabled_at IS NULL",
            params![hash(token), now],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    User {
                        id: r.get(2)?,
                        email: r.get(3)?,
                        name: r.get(4)?,
                        is_admin: r.get(5)?,
                        disabled: false,
                        access: parse_access(r.get(7)?),
                    },
                ))
            },
        )
        .optional()?;
    let Some((session_id, last_seen, user)) = row else {
        return Ok(None);
    };
    let slid = now - last_seen > SLIDE_AFTER;
    if slid {
        conn.execute(
            "UPDATE sessions SET last_seen_at = ?2, expires_at = ?3 WHERE id = ?1",
            params![session_id, now, now + SESSION_TTL],
        )?;
    }
    Ok(Some((user, slid)))
}

pub fn logout(conn: &Connection, token: &str) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM sessions WHERE token_hash = ?1", [hash(token)])?;
    Ok(())
}

/// Returns false when the email already exists.
pub fn add_user(
    conn: &Connection,
    email: &str,
    name: Option<&str>,
    is_admin: bool,
    now: i64,
) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "INSERT OR IGNORE INTO users (email, name, is_admin, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![normalize_email(email), name, is_admin, now],
    )?;
    Ok(n > 0)
}

pub fn list_users(conn: &Connection) -> rusqlite::Result<Vec<User>> {
    conn.prepare(&format!("SELECT {USER_COLS} FROM users ORDER BY email"))?
        .query_map([], user_from_row)?
        .collect()
}

pub fn set_disabled(conn: &Connection, id: i64, disabled: bool, now: i64) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE users SET disabled_at = ?2 WHERE id = ?1",
        params![id, disabled.then_some(now)],
    )?;
    if disabled {
        conn.execute("DELETE FROM sessions WHERE user_id = ?1", [id])?;
    }
    Ok(())
}

pub fn set_admin(conn: &Connection, id: i64, is_admin: bool) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE users SET is_admin = ?2 WHERE id = ?1",
        params![id, is_admin],
    )?;
    Ok(())
}

pub fn set_access(conn: &Connection, id: i64, feature: Feature, on: bool) -> rusqlite::Result<()> {
    if on {
        conn.execute(
            "INSERT OR IGNORE INTO user_access (user_id, feature) VALUES (?1, ?2)",
            params![id, feature.as_str()],
        )?;
    } else {
        conn.execute(
            "DELETE FROM user_access WHERE user_id = ?1 AND feature = ?2",
            params![id, feature.as_str()],
        )?;
    }
    Ok(())
}

pub fn user_id_by_email(conn: &Connection, email: &str) -> rusqlite::Result<Option<i64>> {
    conn.query_row(
        "SELECT id FROM users WHERE email = ?1",
        [normalize_email(email)],
        |r| r.get(0),
    )
    .optional()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_memory;

    const T0: i64 = 1_000_000;

    fn setup() -> Connection {
        let conn = open_memory();
        add_user(&conn, "Mike@Example.com", Some("Mike"), true, T0).unwrap();
        conn
    }

    #[test]
    fn code_alphabet_and_length() {
        for _ in 0..200 {
            let c = generate_code();
            assert_eq!(c.len(), CODE_LEN);
            assert!(c.bytes().all(|b| ALPHABET.contains(&b)));
        }
    }

    #[test]
    fn normalizes_ambiguous_input() {
        assert_eq!(normalize_code(" ab0o-i1lz "), "AB00111Z");
        assert_eq!(display_code("ABCD1234"), "ABCD-1234");
    }

    #[test]
    fn login_round_trip() {
        let conn = setup();
        let (email, code) = request_code(&conn, " MIKE@example.com ", T0)
            .unwrap()
            .unwrap();
        assert_eq!(email, "mike@example.com");
        let token = verify_code(&conn, &email, &display_code(&code).to_lowercase(), T0 + 5)
            .unwrap()
            .unwrap();
        let (user, slid) = session_user(&conn, &token, T0 + 10).unwrap().unwrap();
        assert!(user.is_admin);
        assert!(!slid);
    }

    #[test]
    fn unknown_and_disabled_emails_get_no_code() {
        let conn = setup();
        assert!(request_code(&conn, "nobody@example.com", T0)
            .unwrap()
            .is_none());
        let id = user_id_by_email(&conn, "mike@example.com")
            .unwrap()
            .unwrap();
        set_disabled(&conn, id, true, T0).unwrap();
        assert!(request_code(&conn, "mike@example.com", T0)
            .unwrap()
            .is_none());
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM login_codes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn code_is_single_use() {
        let conn = setup();
        let (email, code) = request_code(&conn, "mike@example.com", T0)
            .unwrap()
            .unwrap();
        assert!(verify_code(&conn, &email, &code, T0 + 1).unwrap().is_some());
        assert!(verify_code(&conn, &email, &code, T0 + 2).unwrap().is_none());
    }

    #[test]
    fn code_expires() {
        let conn = setup();
        let (email, code) = request_code(&conn, "mike@example.com", T0)
            .unwrap()
            .unwrap();
        assert!(verify_code(&conn, &email, &code, T0 + CODE_TTL)
            .unwrap()
            .is_none());
    }

    #[test]
    fn attempts_are_capped() {
        let conn = setup();
        let (email, code) = request_code(&conn, "mike@example.com", T0)
            .unwrap()
            .unwrap();
        for _ in 0..MAX_ATTEMPTS {
            assert!(verify_code(&conn, &email, "00000000", T0 + 1)
                .unwrap()
                .is_none());
        }
        assert!(verify_code(&conn, &email, &code, T0 + 2).unwrap().is_none());
    }

    #[test]
    fn resend_cooldown_then_new_code_replaces_old() {
        let conn = setup();
        let (email, first) = request_code(&conn, "mike@example.com", T0)
            .unwrap()
            .unwrap();
        assert!(request_code(&conn, &email, T0 + 30).unwrap().is_none());
        let (_, second) = request_code(&conn, &email, T0 + RESEND_COOLDOWN + 1)
            .unwrap()
            .unwrap();
        assert!(verify_code(&conn, &email, &first, T0 + 70)
            .unwrap()
            .is_none());
        assert!(verify_code(&conn, &email, &second, T0 + 71)
            .unwrap()
            .is_some());
    }

    #[test]
    fn session_slides_and_expires() {
        let conn = setup();
        let (email, code) = request_code(&conn, "mike@example.com", T0)
            .unwrap()
            .unwrap();
        let token = verify_code(&conn, &email, &code, T0).unwrap().unwrap();

        let later = T0 + SLIDE_AFTER + 1;
        let (_, slid) = session_user(&conn, &token, later).unwrap().unwrap();
        assert!(slid);

        let almost = later + SESSION_TTL - 1;
        assert!(session_user(&conn, &token, almost).unwrap().is_some());
        let (_, _) = session_user(&conn, &token, almost).unwrap().unwrap();
        assert!(session_user(&conn, &token, almost + SESSION_TTL * 2)
            .unwrap()
            .is_none());
    }

    #[test]
    fn disabling_kills_sessions() {
        let conn = setup();
        let (email, code) = request_code(&conn, "mike@example.com", T0)
            .unwrap()
            .unwrap();
        let token = verify_code(&conn, &email, &code, T0).unwrap().unwrap();
        let id = user_id_by_email(&conn, &email).unwrap().unwrap();
        set_disabled(&conn, id, true, T0 + 1).unwrap();
        assert!(session_user(&conn, &token, T0 + 2).unwrap().is_none());
    }

    #[test]
    fn logout_removes_session() {
        let conn = setup();
        let (email, code) = request_code(&conn, "mike@example.com", T0)
            .unwrap()
            .unwrap();
        let token = verify_code(&conn, &email, &code, T0).unwrap().unwrap();
        logout(&conn, &token).unwrap();
        assert!(session_user(&conn, &token, T0 + 1).unwrap().is_none());
    }

    #[test]
    fn access_round_trips_and_admin_can_do_everything() {
        let conn = setup();
        add_user(&conn, "sam@example.com", None, false, T0).unwrap();
        let id = user_id_by_email(&conn, "sam@example.com").unwrap().unwrap();
        let sam = |conn: &Connection| {
            list_users(conn)
                .unwrap()
                .into_iter()
                .find(|u| u.id == id)
                .unwrap()
        };
        assert!(!sam(&conn).can(Feature::Pickle));

        set_access(&conn, id, Feature::Pickle, true).unwrap();
        set_access(&conn, id, Feature::Pickle, true).unwrap();
        assert_eq!(sam(&conn).access, vec![Feature::Pickle]);
        assert!(sam(&conn).can(Feature::Pickle));
        assert!(!sam(&conn).can(Feature::Sms));

        set_access(&conn, id, Feature::Pickle, false).unwrap();
        assert!(sam(&conn).access.is_empty());

        let mike = list_users(&conn)
            .unwrap()
            .into_iter()
            .find(|u| u.is_admin)
            .unwrap();
        assert!(Feature::ALL.into_iter().all(|f| mike.can(f)));
    }

    #[test]
    fn session_user_carries_access() {
        let conn = setup();
        let (email, code) = request_code(&conn, "mike@example.com", T0)
            .unwrap()
            .unwrap();
        let token = verify_code(&conn, &email, &code, T0).unwrap().unwrap();
        let id = user_id_by_email(&conn, &email).unwrap().unwrap();
        set_access(&conn, id, Feature::Sms, true).unwrap();
        let (user, _) = session_user(&conn, &token, T0 + 1).unwrap().unwrap();
        assert_eq!(user.access, vec![Feature::Sms]);
    }

    #[test]
    fn migration_grants_sms_to_existing_users() {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::migrate_to(&mut conn, 2).unwrap();
        conn.execute(
            "INSERT INTO users (email, created_at) VALUES ('old@example.com', 1)",
            [],
        )
        .unwrap();
        crate::db::migrate(&mut conn).unwrap();
        let users = list_users(&conn).unwrap();
        assert_eq!(users[0].access, vec![Feature::Sms]);
    }

    #[test]
    fn parse_feature_rejects_unknown() {
        assert_eq!(Feature::parse("pickle"), Some(Feature::Pickle));
        assert_eq!(Feature::parse("nope"), None);
    }

    #[test]
    fn add_user_rejects_duplicates() {
        let conn = setup();
        assert!(!add_user(&conn, "MIKE@example.com", None, false, T0).unwrap());
    }
}
