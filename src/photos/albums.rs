use super::Photo;
use rand::RngCore;
use rusqlite::{params, Connection, OptionalExtension};

pub struct Album {
    pub id: i64,
    pub token: String,
    pub title: String,
    pub expires_at: i64,
}

pub struct Input {
    title: String,
    days: i64,
    photo_ids: Vec<i64>,
}

pub fn parse(title: &str, days: Option<&str>, ids: &str) -> Result<Input, String> {
    let days = days.unwrap_or("30").parse::<i64>().ok();
    let Some(days) = days.filter(|d| (1..=180).contains(d)) else {
        return Err("Expiration must be a whole number from 1 to 180 days.".into());
    };
    let title = title.trim();
    if title.chars().count() > 100 {
        return Err("Album names must be at most 100 characters.".into());
    }
    let mut photo_ids = Vec::new();
    for raw in ids.split(',').filter(|s| !s.is_empty()) {
        let id = raw.parse::<i64>().ok().filter(|id| *id > 0);
        let Some(id) = id else {
            return Err("Invalid photo selection.".into());
        };
        if !photo_ids.contains(&id) {
            photo_ids.push(id);
        }
    }
    if photo_ids.is_empty() {
        return Err("Select at least one photo to share.".into());
    }
    Ok(Input {
        title: if title.is_empty() {
            "Shared album"
        } else {
            title
        }
        .into(),
        days,
        photo_ids,
    })
}

pub fn create(
    conn: &Connection,
    user_id: i64,
    input: &Input,
    now: i64,
) -> rusqlite::Result<Option<Album>> {
    let tx = conn.unchecked_transaction()?;
    for id in &input.photo_ids {
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM photos WHERE id = ?1 AND missing_at IS NULL)",
            [id],
            |r| r.get(0),
        )?;
        if !exists {
            return Ok(None);
        }
    }
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let expires_at = now + input.days * 86400;
    tx.execute(
        "INSERT INTO shared_albums (user_id, token, title, created_at, expires_at) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![user_id, token, input.title, now, expires_at],
    )?;
    let id = tx.last_insert_rowid();
    for (position, photo_id) in input.photo_ids.iter().enumerate() {
        tx.execute(
            "INSERT INTO shared_album_photos (album_id, photo_id, position) VALUES (?1, ?2, ?3)",
            params![id, photo_id, position as i64],
        )?;
    }
    tx.commit()?;
    Ok(Some(Album {
        id,
        token,
        title: input.title.clone(),
        expires_at,
    }))
}

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Album> {
    Ok(Album {
        id: r.get(0)?,
        token: r.get(1)?,
        title: r.get(2)?,
        expires_at: r.get(3)?,
    })
}

pub fn list(conn: &Connection, user_id: i64, now: i64) -> rusqlite::Result<Vec<Album>> {
    conn.prepare(
        "SELECT id, token, title, expires_at FROM shared_albums \
         WHERE user_id = ?1 AND expires_at > ?2 ORDER BY created_at DESC, id DESC",
    )?
    .query_map(params![user_id, now], row)?
    .collect()
}

pub fn get(conn: &Connection, token: &str, now: i64) -> rusqlite::Result<Option<Album>> {
    conn.query_row(
        "SELECT id, token, title, expires_at FROM shared_albums WHERE token = ?1 AND expires_at > ?2",
        params![token, now],
        row,
    )
    .optional()
}

pub fn contains(conn: &Connection, album_id: i64, photo_id: i64) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM shared_album_photos WHERE album_id = ?1 AND photo_id = ?2)",
        params![album_id, photo_id],
        |r| r.get(0),
    )
}

pub fn photos(conn: &Connection, album_id: i64) -> rusqlite::Result<Vec<Photo>> {
    conn.prepare(
        "SELECT p.id, p.path FROM shared_album_photos ap JOIN photos p ON p.id = ap.photo_id \
         WHERE ap.album_id = ?1 AND p.missing_at IS NULL ORDER BY ap.position",
    )?
    .query_map([album_id], |r| {
        Ok(Photo {
            id: r.get(0)?,
            path: r.get(1)?,
            tags: Vec::new(),
        })
    })?
    .collect()
}

pub fn delete(conn: &Connection, id: i64, user_id: i64) -> rusqlite::Result<usize> {
    conn.execute(
        "DELETE FROM shared_albums WHERE id = ?1 AND user_id = ?2",
        params![id, user_id],
    )
}

pub fn prune(conn: &Connection, now: i64) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM shared_albums WHERE expires_at <= ?1", [now])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{auth, db};

    #[test]
    fn validates_expiration_and_selection() {
        for days in ["0", "181", "-1", "1.5", "", "x"] {
            assert!(parse("", Some(days), "1").is_err());
        }
        for days in ["1", "180"] {
            assert!(parse("", Some(days), "1").is_ok());
        }
        assert!(parse("", None, "").is_err());
        assert!(parse("", None, "nope").is_err());
    }

    #[test]
    fn albums_expire_cascade_and_preserve_originals() {
        let conn = db::open_memory();
        auth::add_user(&conn, "a@example.com", None, false, 0).unwrap();
        conn.execute(
            "INSERT INTO photos (path, album, size, mtime, sha256, created_at) \
             VALUES ('one.jpg', '', 1, 0, 'hash', 0), ('two.jpg', '', 1, 0, 'hash', 0)",
            [],
        )
        .unwrap();
        let input = parse(" Test ", None, "2,1,2").unwrap();
        let album = create(&conn, 1, &input, 100).unwrap().unwrap();
        assert_eq!(album.expires_at, 100 + 30 * 86400);
        assert_eq!(
            photos(&conn, album.id)
                .unwrap()
                .iter()
                .map(|p| p.id)
                .collect::<Vec<_>>(),
            vec![2, 1]
        );
        assert!(contains(&conn, album.id, 1).unwrap());
        assert!(!contains(&conn, album.id, 3).unwrap());
        assert!(get(&conn, &album.token, album.expires_at - 1)
            .unwrap()
            .is_some());
        assert!(get(&conn, &album.token, album.expires_at)
            .unwrap()
            .is_none());
        assert_eq!(delete(&conn, album.id, 2).unwrap(), 0);
        assert!(create(&conn, 1, &parse("", None, "1,99").unwrap(), 100)
            .unwrap()
            .is_none());
        assert_eq!(list(&conn, 1, 100).unwrap().len(), 1);
        assert_eq!(prune(&conn, album.expires_at).unwrap(), 1);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM shared_album_photos", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM photos", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
    }
}
