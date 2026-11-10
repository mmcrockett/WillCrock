use crate::{cli, config, db};
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::time::UNIX_EPOCH;

pub mod albums;
mod images;
mod import_tags;
mod upload;
pub use images::{image_file, ImageKind};
pub use upload::{change_date, upload, UploadError};

const IMAGE_EXTS: [&str; 6] = ["jpg", "jpeg", "png", "gif", "heic", "webp"];
pub const ADMIN_ONLY_TAG: &str = "admin-only";

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    Admin,
    Restricted,
}

impl Visibility {
    pub fn for_user(user: &crate::auth::User) -> Self {
        if user.is_admin {
            Self::Admin
        } else {
            Self::Restricted
        }
    }

    // Callers alias the photos table as p.
    pub fn sql(self) -> String {
        match self {
            Self::Admin => "1".into(),
            Self::Restricted => format!(
                "NOT EXISTS (SELECT 1 FROM photo_tags private_pt \
                 JOIN tags private_t ON private_t.id = private_pt.tag_id \
                 WHERE private_pt.photo_id = p.id AND private_t.name = '{ADMIN_ONLY_TAG}')"
            ),
        }
    }
}

pub fn visible(conn: &Connection, id: i64, visibility: Visibility) -> rusqlite::Result<bool> {
    conn.query_row(
        &format!(
            "SELECT EXISTS(SELECT 1 FROM photos p \
             WHERE p.id = ?1 AND p.missing_at IS NULL AND {})",
            visibility.sql()
        ),
        [id],
        |r| r.get(0),
    )
}

#[derive(Debug, Default, PartialEq)]
pub struct ScanReport {
    pub added: usize,
    pub changed: usize,
    pub unchanged: usize,
    pub missing: usize,
}

pub struct TagCount {
    pub name: String,
    pub count: i64,
}

pub struct Summary {
    pub years: Vec<TagCount>,
    pub other_tags: Vec<TagCount>,
    pub total: i64,
    pub untagged: i64,
    pub missing: i64,
    pub unsynced: i64,
}

pub struct TagView {
    pub tags: Vec<String>,
    pub subtags: Vec<TagCount>,
    pub known_tags: Vec<String>,
    pub photos: Vec<Photo>,
    pub total: i64,
    pub page: i64,
}

#[derive(Debug, PartialEq)]
pub struct Photo {
    pub id: i64,
    pub path: String,
    pub tags: Vec<String>,
}

impl Photo {
    pub fn can_preview(&self) -> bool {
        !Path::new(&self.path)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("heic"))
    }
}

pub const TAG_VIEW_LIMIT: i64 = 60;

pub fn normalize_tag(raw: &str) -> Option<String> {
    let t = raw.trim().to_lowercase();
    (!t.is_empty() && !t.contains('/') && t != "." && t != "..").then_some(t)
}

pub fn is_year_tag(name: &str) -> bool {
    name.len() == 4 && name.bytes().all(|b| b.is_ascii_digit())
}

fn is_month_tag(name: &str) -> bool {
    name.len() == 7
        && name.as_bytes()[4] == b'-'
        && name[..4].bytes().all(|b| b.is_ascii_digit())
        && name[5..].bytes().all(|b| b.is_ascii_digit())
}

fn add_tag(tx: &rusqlite::Transaction, photo_id: i64, name: &str) -> rusqlite::Result<()> {
    tx.execute("INSERT OR IGNORE INTO tags (name) VALUES (?1)", [name])?;
    tx.execute(
        "INSERT OR IGNORE INTO photo_tags (photo_id, tag_id) \
         SELECT ?1, id FROM tags WHERE name = ?2",
        params![photo_id, name],
    )?;
    Ok(())
}

fn add_date_tags(tx: &rusqlite::Transaction, photo_id: i64, date: &str) -> rusqlite::Result<()> {
    add_tag(tx, photo_id, &date[..4])?;
    add_tag(tx, photo_id, &date[..7])
}

fn month_album(date: &str) -> Option<String> {
    let month = date.get(..7)?;
    let (year, number) = month.split_once('-')?;
    if year.len() != 4
        || number.len() != 2
        || !year.bytes().all(|b| b.is_ascii_digit())
        || !number.bytes().all(|b| b.is_ascii_digit())
        || !format!("{month}-01")
            .parse::<jiff::civil::Date>()
            .is_ok_and(|date| date.year() > 0)
    {
        return None;
    }
    Some(format!("{year}/{year}{number}00"))
}

fn photo_file(root: &Path, rel: &str) -> Result<std::path::PathBuf, String> {
    if !Path::new(rel)
        .components()
        .all(|component| matches!(component, std::path::Component::Normal(_)))
    {
        return Err(format!("invalid photo path: {rel}"));
    }
    let path = root.join(rel);
    let parent = path
        .parent()
        .ok_or_else(|| format!("invalid photo path: {rel}"))?
        .canonicalize()
        .map_err(|e| format!("photo directory for {rel}: {e}"))?;
    if !parent.starts_with(root) {
        return Err(format!("photo path is outside library: {rel}"));
    }
    let metadata = std::fs::symlink_metadata(&path).map_err(|e| format!("photo {rel}: {e}"))?;
    if !metadata.file_type().is_file() {
        return Err(format!("photo is not a regular file: {rel}"));
    }
    Ok(path)
}

fn photo_destination(root: &Path, new_path: &str) -> Result<std::path::PathBuf, String> {
    if !Path::new(new_path)
        .components()
        .all(|component| matches!(component, std::path::Component::Normal(_)))
    {
        return Err(format!("invalid photo path: {new_path}"));
    }
    let destination = root.join(new_path);
    let parent = destination
        .parent()
        .ok_or_else(|| format!("invalid photo path: {new_path}"))?;
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("create photo directory for {new_path}: {e}"))?;
    let parent = parent
        .canonicalize()
        .map_err(|e| format!("photo directory for {new_path}: {e}"))?;
    if !parent.starts_with(root) {
        return Err(format!("photo path is outside library: {new_path}"));
    }
    Ok(parent.join(
        Path::new(new_path)
            .file_name()
            .ok_or_else(|| format!("invalid photo path: {new_path}"))?,
    ))
}

fn queue_photo_move(
    tx: &rusqlite::Transaction<'_>,
    root: &Path,
    id: i64,
    old_path: &str,
    date: &str,
) -> Result<(), String> {
    let album = month_album(date).ok_or_else(|| format!("invalid photo date: {date}"))?;
    let name = Path::new(old_path)
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("invalid photo path: {old_path}"))?;
    let new_path = format!("{album}/{name}");
    let source = photo_file(root, old_path)?;
    let destination = photo_destination(root, &new_path)?;
    match std::fs::symlink_metadata(&destination) {
        Ok(_) => return Err(format!("photo destination already exists: {new_path}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("check photo destination {new_path}: {e}")),
    }
    let indexed: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM photos WHERE path = ?1)",
            [&new_path],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if indexed {
        return Err(format!("photo destination is already indexed: {new_path}"));
    }
    tx.execute(
        "INSERT INTO pending_photo_moves (photo_id, old_path, new_path, taken_on, sha256) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, old_path, new_path, date, hash(&source)?],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn photo_transaction<'a>(
    conn: &'a Connection,
    root: &Path,
) -> Result<rusqlite::Transaction<'a>, String> {
    loop {
        let tx =
            rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)
                .map_err(|e| e.to_string())?;
        let pending: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM pending_photo_moves)",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if !pending {
            return Ok(tx);
        }
        drop(tx);
        recover_photo_moves(conn, root)?;
    }
}

fn recover_photo_moves(conn: &Connection, root: &Path) -> Result<(), String> {
    let root = root
        .canonicalize()
        .map_err(|e| format!("photo directory: {e}"))?;
    loop {
        let tx =
            rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)
                .map_err(|e| e.to_string())?;
        let pending = tx
            .query_row(
                "SELECT m.photo_id, m.old_path, m.new_path, m.taken_on, m.sha256, \
                 p.path, p.taken_on FROM pending_photo_moves m \
                 JOIN photos p ON p.id = m.photo_id ORDER BY m.photo_id LIMIT 1",
                [],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, String>(5)?,
                        r.get::<_, Option<String>>(6)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let Some((id, old_path, new_path, date, sha256, indexed_path, old_date)) = pending else {
            return Ok(());
        };
        let destination = photo_destination(&root, &new_path)?;
        match std::fs::symlink_metadata(&destination) {
            Ok(_) => {
                let path = photo_file(&root, &new_path)?;
                if hash(&path)? != sha256 {
                    return Err(format!("moved photo contents differ: {new_path}"));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let source = photo_file(&root, &old_path)?;
                if hash(&source)? != sha256 {
                    return Err(format!("photo contents changed during move: {old_path}"));
                }
                std::fs::hard_link(&source, &destination)
                    .map_err(|e| format!("publish moved photo {new_path}: {e}"))?;
            }
            Err(e) => return Err(format!("check moved photo {new_path}: {e}")),
        }
        if indexed_path == old_path {
            let metadata = std::fs::metadata(&destination)
                .map_err(|e| format!("stat moved photo {new_path}: {e}"))?;
            let mtime = metadata
                .modified()
                .map_err(|e| format!("photo mtime: {e}"))?
                .duration_since(UNIX_EPOCH)
                .map_err(|e| format!("photo mtime: {e}"))?
                .as_secs() as i64;
            if old_date.as_deref() != Some(&date) {
                if let Some(old) = &old_date {
                    tx.execute(
                        "DELETE FROM photo_tags WHERE photo_id = ?1 AND tag_id IN \
                         (SELECT id FROM tags WHERE name IN (?2, ?3))",
                        params![id, old.get(..4).unwrap_or(""), old.get(..7).unwrap_or("")],
                    )
                    .map_err(|e| e.to_string())?;
                }
                add_date_tags(&tx, id, &date).map_err(|e| e.to_string())?;
            }
            tx.execute(
                "UPDATE photos SET path = ?1, album = ?2, taken_on = ?3, sha256 = ?4, \
                 size = ?5, mtime = ?6, synced_at = NULL, missing_at = NULL WHERE id = ?7",
                params![
                    new_path,
                    album_of(&new_path),
                    date,
                    sha256,
                    metadata.len() as i64,
                    mtime,
                    id
                ],
            )
            .map_err(|e| e.to_string())?;
            record_photo_move(&tx, id, &old_path, &new_path, &sha256).map_err(|e| e.to_string())?;
            // Commit the new identity before unlinking; the intent survives until cleanup.
            tx.commit().map_err(|e| e.to_string())?;
            continue;
        }
        if indexed_path != new_path {
            return Err(format!("photo path changed during move: {indexed_path}"));
        }
        match std::fs::symlink_metadata(root.join(&old_path)) {
            Ok(_) => {
                let source = photo_file(&root, &old_path)?;
                if hash(&source)? != sha256 {
                    return Err(format!(
                        "old photo contents changed during move: {old_path}"
                    ));
                }
                std::fs::remove_file(&source)
                    .map_err(|e| format!("remove old photo {old_path}: {e}"))?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("check old photo {old_path}: {e}")),
        }
        tx.execute("DELETE FROM pending_photo_moves WHERE photo_id = ?1", [id])
            .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
    }
}

fn record_photo_move(
    tx: &rusqlite::Transaction<'_>,
    photo_id: i64,
    old_path: &str,
    new_path: &str,
    sha256: &str,
) -> rusqlite::Result<()> {
    tx.execute(
        "UPDATE photo_moves SET new_path = ?1, sha256 = ?2 \
         WHERE photo_id = ?3 AND new_path = ?4",
        params![new_path, sha256, photo_id, old_path],
    )?;
    tx.execute(
        "INSERT OR REPLACE INTO photo_moves (photo_id, old_path, new_path, sha256) \
         VALUES (?1, ?2, ?3, ?4)",
        params![photo_id, old_path, new_path, sha256],
    )?;
    tx.execute(
        "DELETE FROM photo_moves WHERE photo_id = ?1 AND old_path = new_path",
        [photo_id],
    )?;
    Ok(())
}

const LIVE_WITH_ALL_TAGS: &str = "SELECT p.id FROM photos p WHERE p.missing_at IS NULL AND \
     (SELECT COUNT(*) FROM photo_tags pt JOIN tags t ON t.id = pt.tag_id \
      WHERE pt.photo_id = p.id AND t.name IN ({in})) = {n}";

fn live_with_all_tags(tags: &[String], visibility: Visibility) -> String {
    let marks = vec!["?"; tags.len()].join(",");
    let ids = LIVE_WITH_ALL_TAGS
        .replace("{in}", &marks)
        .replace("{n}", &tags.len().to_string());
    format!("{ids} AND {}", visibility.sql())
}

pub fn tag_view(
    conn: &Connection,
    tags: &[String],
    page: i64,
    visibility: Visibility,
) -> rusqlite::Result<TagView> {
    let ids = live_with_all_tags(tags, visibility);
    let args = || rusqlite::params_from_iter(tags.iter());
    let marks = vec!["?"; tags.len()].join(",");
    let mut stmt = conn.prepare(&format!(
        "SELECT t.name, COUNT(*) FROM photo_tags pt JOIN tags t ON t.id = pt.tag_id \
         WHERE pt.photo_id IN ({ids}) AND t.name NOT IN ({marks}) \
         GROUP BY t.id ORDER BY t.name"
    ))?;
    let subtags = stmt
        .query_map(
            rusqlite::params_from_iter(tags.iter().chain(tags.iter())),
            |r| {
                Ok(TagCount {
                    name: r.get(0)?,
                    count: r.get(1)?,
                })
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let total: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM ({ids})"), args(), |r| {
        r.get(0)
    })?;
    let page = page.clamp(1, ((total - 1) / TAG_VIEW_LIMIT + 1).max(1));
    let offset = (page - 1) * TAG_VIEW_LIMIT;
    let mut stmt = conn.prepare(&format!(
        "SELECT id, path FROM photos WHERE id IN ({ids}) ORDER BY path \
         LIMIT {TAG_VIEW_LIMIT} OFFSET {offset}"
    ))?;
    let photos = stmt
        .query_map(args(), |r| {
            Ok(Photo {
                id: r.get(0)?,
                path: r.get(1)?,
                tags: Vec::new(),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut tag_stmt = conn.prepare(
        "SELECT t.name FROM photo_tags pt JOIN tags t ON t.id = pt.tag_id \
         WHERE pt.photo_id = ?1 ORDER BY t.name",
    )?;
    let mut photos = photos;
    for photo in &mut photos {
        photo.tags = tag_stmt
            .query_map([photo.id], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .filter(|tag| !is_year_tag(tag) && !is_month_tag(tag))
            .collect();
    }
    let mut stmt = conn.prepare(&format!(
        "SELECT DISTINCT t.name FROM tags t \
         JOIN photo_tags pt ON pt.tag_id = t.id \
         JOIN photos p ON p.id = pt.photo_id \
         WHERE p.missing_at IS NULL AND {} ORDER BY t.name",
        visibility.sql()
    ))?;
    let known_tags = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .filter(|tag| !is_year_tag(tag) && !is_month_tag(tag))
        .collect();
    Ok(TagView {
        tags: tags.to_vec(),
        subtags,
        known_tags,
        photos,
        total,
        page,
    })
}

pub fn parse_tags(raw: &str) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    for item in raw.split(',').filter(|item| !item.trim().is_empty()) {
        let Some(name) = normalize_tag(item) else {
            return Err("Tags cannot contain slashes or be '.' or '..'.".into());
        };
        if name.chars().count() > 50 || is_year_tag(&name) || is_month_tag(&name) {
            return Err(
                "Tags must be at most 50 characters and cannot be year or month labels.".into(),
            );
        }
        if !names.contains(&name) {
            names.push(name);
        }
    }
    Ok(names)
}

#[derive(Debug, PartialEq)]
pub enum TagUpdate {
    Saved,
    NotFound,
    ReservedTag,
}

pub fn set_tags(
    conn: &Connection,
    photo_id: i64,
    names: &[String],
    visibility: Visibility,
) -> rusqlite::Result<TagUpdate> {
    if visibility == Visibility::Restricted && names.iter().any(|name| name == ADMIN_ONLY_TAG) {
        return Ok(TagUpdate::ReservedTag);
    }
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
    let Some(taken_on) = tx
        .query_row(
            &format!(
                "SELECT taken_on FROM photos p \
                 WHERE p.id = ?1 AND p.missing_at IS NULL AND {}",
                visibility.sql()
            ),
            [photo_id],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?
    else {
        return Ok(TagUpdate::NotFound);
    };
    let (year, month) = taken_on
        .as_deref()
        .map(|date| {
            (
                date.get(..4).unwrap_or("").to_string(),
                date.get(..7).unwrap_or("").to_string(),
            )
        })
        .unwrap_or_default();
    tx.execute(
        "DELETE FROM photo_tags WHERE photo_id = ?1 AND tag_id NOT IN \
         (SELECT id FROM tags WHERE name IN (?2, ?3))",
        params![photo_id, year, month],
    )?;
    for name in names {
        add_tag(&tx, photo_id, name)?;
    }
    tx.commit()?;
    Ok(TagUpdate::Saved)
}

pub fn run(args: &[String]) {
    match args.first().map(String::as_str) {
        Some("scan") => scan_cmd(),
        Some("migrate-uploads") => migrate_uploads_cmd(),
        Some("unsynced") => unsynced_cmd(),
        Some("mark-synced") => mark_synced_cmd(),
        Some("moved") => moved_cmd(),
        Some("ack-moves") => ack_moves_cmd(),
        Some("import-tags") => import_tags::run(&args[1..]),
        Some("prune-albums") => {
            let (_, conn) = open_db();
            let n = albums::prune(&conn, cli::now()).unwrap_or_else(|e| cli::fail(e));
            println!("removed {n} expired albums");
        }
        _ => cli::fail(
            "usage: willcrock photos scan|migrate-uploads|unsynced|mark-synced|moved|ack-moves|prune-albums|import-tags [--dry-run|--apply]",
        ),
    }
}

fn open_db() -> (config::Config, Connection) {
    let cfg = config::load().unwrap_or_else(|e| cli::fail(e));
    let conn = db::open(&cfg.db_path).unwrap_or_else(|e| cli::fail(e));
    (cfg, conn)
}

fn scan_cmd() {
    let (cfg, mut conn) = open_db();
    let Some(photos) = cfg.photos.as_ref() else {
        cli::fail("no [photos] section in config");
    };
    let r = scan(&mut conn, Path::new(&photos.dir), cli::now()).unwrap_or_else(|e| cli::fail(e));
    println!(
        "added {} changed {} unchanged {} missing {}",
        r.added, r.changed, r.unchanged, r.missing
    );
}

fn migrate_uploads_cmd() {
    let (cfg, mut conn) = open_db();
    let Some(photos) = cfg.photos.as_ref() else {
        cli::fail("no [photos] section in config");
    };
    let n = migrate_uploads(&mut conn, Path::new(&photos.dir)).unwrap_or_else(|e| cli::fail(e));
    println!("migrated {n} uploads");
}

fn unsynced_cmd() {
    let (_, conn) = open_db();
    for path in unsynced(&conn).unwrap_or_else(|e| cli::fail(e)) {
        println!("{path}");
    }
}

fn moved_cmd() {
    let (_, conn) = open_db();
    let mut stmt = conn
        .prepare("SELECT old_path, new_path, sha256 FROM photo_moves ORDER BY old_path")
        .unwrap_or_else(|e| cli::fail(e));
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .unwrap_or_else(|e| cli::fail(e));
    for row in rows {
        let (old_path, new_path, sha256) = row.unwrap_or_else(|e| cli::fail(e));
        println!("{old_path}\t{new_path}\t{sha256}");
    }
}

fn ack_moves_cmd() {
    let (_, conn) = open_db();
    let n =
        acknowledge_photo_moves(&conn, std::io::stdin().lock()).unwrap_or_else(|e| cli::fail(e));
    println!("acknowledged {n} moved paths");
}

fn mark_synced_cmd() {
    let (_, mut conn) = open_db();
    let n = mark_synced(&mut conn, std::io::stdin().lock(), cli::now())
        .unwrap_or_else(|e| cli::fail(e));
    println!("marked {n}");
}

pub fn unsynced(conn: &Connection) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT path FROM photos WHERE missing_at IS NULL AND synced_at IS NULL ORDER BY path",
    )?;
    let rows = stmt.query_map([], |r| r.get(0))?;
    rows.collect()
}

/// Reads `path<TAB>size<TAB>mtime` lines describing the local copy and marks matching rows synced.
pub fn mark_synced(
    conn: &mut Connection,
    input: impl std::io::BufRead,
    now: i64,
) -> Result<usize, String> {
    let db_err = |e: rusqlite::Error| e.to_string();
    let tx = conn.transaction().map_err(db_err)?;
    let mut marked = 0;
    for line in input.lines() {
        let line = line.map_err(|e| format!("read manifest: {e}"))?;
        if line.is_empty() {
            continue;
        }
        let mut f = line.split('\t');
        let (Some(path), Some(size), Some(mtime), None) = (f.next(), f.next(), f.next(), f.next())
        else {
            return Err(format!("bad manifest line: {line}"));
        };
        let size: i64 = size.parse().map_err(|_| format!("bad size: {line}"))?;
        let mtime: i64 = mtime.parse().map_err(|_| format!("bad mtime: {line}"))?;
        marked += tx
            .execute(
                "UPDATE photos SET synced_at = ?1 \
                 WHERE path = ?2 AND size = ?3 AND mtime = ?4 AND missing_at IS NULL",
                params![now, path, size, mtime],
            )
            .map_err(db_err)?;
    }
    tx.commit().map_err(db_err)?;
    Ok(marked)
}

pub fn acknowledge_photo_moves(
    conn: &Connection,
    input: impl std::io::BufRead,
) -> Result<usize, String> {
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    let mut acknowledged = 0;
    for line in input.lines() {
        let path = line.map_err(|e| format!("read move manifest: {e}"))?;
        if path.is_empty() || path.contains('\t') {
            return Err("invalid move manifest path".into());
        }
        acknowledged += tx
            .execute("DELETE FROM photo_moves WHERE old_path = ?1", [path])
            .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(acknowledged)
}

pub fn summary(conn: &Connection, visibility: Visibility) -> rusqlite::Result<Summary> {
    let visible = visibility.sql();
    let mut stmt = conn.prepare(&format!(
        "SELECT t.name, COUNT(*) FROM photo_tags pt \
         JOIN tags t ON t.id = pt.tag_id JOIN photos p ON p.id = pt.photo_id \
         WHERE p.missing_at IS NULL AND {visible} GROUP BY t.id ORDER BY t.name"
    ))?;
    let counts = stmt
        .query_map([], |r| {
            Ok(TagCount {
                name: r.get(0)?,
                count: r.get(1)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let (mut years, other_tags): (Vec<_>, Vec<_>) = counts
        .into_iter()
        .filter(|t| !is_month_tag(&t.name))
        .partition(|t| is_year_tag(&t.name));
    years.reverse();
    let total = conn.query_row(
        &format!("SELECT COUNT(*) FROM photos p WHERE missing_at IS NULL AND {visible}"),
        [],
        |r| r.get(0),
    )?;
    let untagged = conn.query_row(
        &format!(
            "SELECT COUNT(*) FROM photos p WHERE p.missing_at IS NULL AND {visible} \
             AND NOT EXISTS (SELECT 1 FROM photo_tags WHERE photo_id = p.id)"
        ),
        [],
        |r| r.get(0),
    )?;
    let missing = conn.query_row(
        &format!("SELECT COUNT(*) FROM photos p WHERE missing_at IS NOT NULL AND {visible}"),
        [],
        |r| r.get(0),
    )?;
    let unsynced = conn.query_row(
        &format!(
            "SELECT COUNT(*) FROM photos p \
             WHERE missing_at IS NULL AND synced_at IS NULL AND {visible}"
        ),
        [],
        |r| r.get(0),
    )?;
    Ok(Summary {
        years,
        other_tags,
        total,
        untagged,
        missing,
        unsynced,
    })
}

pub fn scan(conn: &mut Connection, root: &Path, now: i64) -> Result<ScanReport, String> {
    let db_err = |e: rusqlite::Error| e.to_string();
    // Keep uploads from entering the database after the filesystem snapshot.
    let tx = photo_transaction(conn, root)?;
    let mut files = Vec::new();
    walk(root, root, &mut files)?;
    let mut report = ScanReport::default();
    let mut seen = HashSet::new();
    for f in &files {
        seen.insert(f.rel.clone());
        let existing: Option<(i64, i64, i64, Option<i64>)> = tx
            .query_row(
                "SELECT id, size, mtime, missing_at FROM photos WHERE path = ?1",
                [&f.rel],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()
            .map_err(db_err)?;
        match existing {
            Some((id, size, mtime, missing_at)) if size == f.size && mtime == f.mtime => {
                if missing_at.is_some() {
                    tx.execute("UPDATE photos SET missing_at = NULL WHERE id = ?1", [id])
                        .map_err(db_err)?;
                }
                report.unchanged += 1;
            }
            Some((id, ..)) => {
                tx.execute(
                    "UPDATE photos SET size = ?1, mtime = ?2, sha256 = ?3, missing_at = NULL, \
                     synced_at = NULL WHERE id = ?4",
                    params![f.size, f.mtime, hash(&f.abs)?, id],
                )
                .map_err(db_err)?;
                report.changed += 1;
            }
            None => {
                tx.execute(
                    "INSERT INTO photos (path, album, size, mtime, sha256, taken_on, created_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        f.rel,
                        album_of(&f.rel),
                        f.size,
                        f.mtime,
                        hash(&f.abs)?,
                        taken_on_from_album(album_of(&f.rel)),
                        now
                    ],
                )
                .map_err(db_err)?;
                let id = tx.last_insert_rowid();
                if let Some(taken_on) = taken_on_from_album(album_of(&f.rel)) {
                    add_date_tags(&tx, id, &taken_on).map_err(db_err)?;
                }
                report.added += 1;
            }
        }
    }
    let live: Vec<String> = {
        let mut stmt = tx
            .prepare("SELECT path FROM photos WHERE missing_at IS NULL")
            .map_err(db_err)?;
        let rows = stmt.query_map([], |r| r.get(0)).map_err(db_err)?;
        rows.collect::<rusqlite::Result<_>>().map_err(db_err)?
    };
    for path in live.iter().filter(|p| !seen.contains(*p)) {
        tx.execute(
            "UPDATE photos SET missing_at = ?1 WHERE path = ?2",
            params![now, path],
        )
        .map_err(db_err)?;
        report.missing += 1;
    }
    tx.commit().map_err(db_err)?;
    Ok(report)
}

pub fn migrate_uploads(conn: &mut Connection, root: &Path) -> Result<usize, String> {
    let root = root
        .canonicalize()
        .map_err(|e| format!("photo directory: {e}"))?;
    let mut migrated = 0;
    loop {
        let tx = photo_transaction(conn, &root)?;
        let photo = tx
            .query_row(
                "SELECT id, path, taken_on FROM photos \
                 WHERE substr(path, 1, 8) = 'uploads/' AND missing_at IS NULL ORDER BY id LIMIT 1",
                [],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let Some((id, old_path, date)) = photo else {
            return Ok(migrated);
        };
        let date = date.ok_or_else(|| format!("upload {old_path} has no date"))?;
        queue_photo_move(&tx, &root, id, &old_path, &date)?;
        tx.commit().map_err(|e| e.to_string())?;
        recover_photo_moves(conn, &root)?;
        migrated += 1;
    }
}

struct Found {
    rel: String,
    abs: std::path::PathBuf,
    size: i64,
    mtime: i64,
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<Found>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("read {}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("read {}: {e}", dir.display()))?;
        let path = entry.path();
        let Some(name) = entry.file_name().to_str().map(String::from) else {
            eprintln!("skipping non-UTF-8 name in {}", dir.display());
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        let meta = entry
            .metadata()
            .map_err(|e| format!("stat {}: {e}", path.display()))?;
        if meta.is_dir() {
            walk(root, &path, out)?;
        } else if meta.is_file() && is_image(&name) {
            let Ok(rel) = path.strip_prefix(root) else {
                continue;
            };
            let Some(rel) = rel.to_str() else {
                eprintln!("skipping non-UTF-8 path {}", path.display());
                continue;
            };
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            out.push(Found {
                rel: rel.replace('\\', "/"),
                abs: path,
                size: meta.len() as i64,
                mtime,
            });
        }
    }
    Ok(())
}

fn is_image(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| IMAGE_EXTS.contains(&e.to_ascii_lowercase().as_str()))
}

fn album_of(rel: &str) -> &str {
    rel.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("")
}

fn hash(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| format!("read {}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

pub fn taken_on_from_album(album: &str) -> Option<String> {
    let dir = album.rsplit('/').next().unwrap_or("");
    let digits: String = dir.chars().take_while(char::is_ascii_digit).collect();
    if digits.len() != 6 && digits.len() != 8 {
        return None;
    }
    let year = &digits[0..4];
    let month: u32 = digits[4..6].parse().ok()?;
    if !(1..=12).contains(&month) {
        return None;
    }
    let day: u32 = if digits.len() == 8 {
        digits[6..8].parse().ok()?
    } else {
        0
    };
    match day {
        1..=31 => Some(format!("{year}-{month:02}-{day:02}")),
        _ => Some(format!("{year}-{month:02}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    pub(super) fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("willcrock-photos-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(root: &Path, rel: &str, body: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    fn library(tag: &str) -> PathBuf {
        let root = temp_dir(tag);
        write(&root, "mike.jpg", "a");
        write(&root, "2018/20180315_RedRiverTrip/IMG_1.JPG", "bb");
        write(&root, "2018/20180900/IMG_2.resize.jpg", "ccc");
        write(&root, "2018/.DS_Store", "x");
        write(&root, "2001/20010501e_UTHockey/.s3cmd.abc.tmp", "x");
        write(&root, "2001/20010501e_UTHockey/notes.tmp", "x");
        write(&root, "2001/20010501e_UTHockey/readme.txt", "x");
        root
    }

    #[test]
    fn first_scan_adds_images_and_second_is_unchanged() {
        let root = library("idem");
        let mut conn = db::open_memory();
        let first = scan(&mut conn, &root, 100).unwrap();
        assert_eq!(
            first,
            ScanReport {
                added: 3,
                ..Default::default()
            }
        );
        let second = scan(&mut conn, &root, 200).unwrap();
        assert_eq!(
            second,
            ScanReport {
                unchanged: 3,
                ..Default::default()
            }
        );
        let s = summary(&conn, Visibility::Admin).unwrap();
        assert_eq!(s.total, 3);
        assert_eq!(s.untagged, 1);
        let years: Vec<_> = s.years.iter().map(|t| (t.name.as_str(), t.count)).collect();
        assert_eq!(years, vec![("2018", 2)]);
        assert!(s.other_tags.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn tag_view_drills_into_months_and_intersects() {
        let root = library("tagview");
        let mut conn = db::open_memory();
        scan(&mut conn, &root, 100).unwrap();
        let year = tag_view(&conn, &["2018".to_string()], 1, Visibility::Admin).unwrap();
        assert_eq!(year.total, 2);
        let months: Vec<_> = year.subtags.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(months, vec!["2018-03", "2018-09"]);
        let month = tag_view(
            &conn,
            &["2018".to_string(), "2018-03".to_string()],
            1,
            Visibility::Admin,
        )
        .unwrap();
        assert_eq!(month.photos[0].path, "2018/20180315_RedRiverTrip/IMG_1.JPG");
        assert!(month.subtags.is_empty());
        let none = tag_view(&conn, &["nope".to_string()], 1, Visibility::Admin).unwrap();
        assert_eq!(none.total, 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn gallery_pages_cover_all_matching_photos_without_overlap() {
        let mut conn = db::open_memory();
        let tx = conn.transaction().unwrap();
        for i in 0..65 {
            tx.execute(
                "INSERT INTO photos (path, album, size, mtime, sha256, created_at) \
                 VALUES (?1, '', 1, 0, 'hash', 0)",
                [format!("{i:03}.jpg")],
            )
            .unwrap();
            add_tag(&tx, tx.last_insert_rowid(), "family").unwrap();
        }
        tx.execute(
            "UPDATE photos SET missing_at = 1 WHERE path = '064.jpg'",
            [],
        )
        .unwrap();
        tx.commit().unwrap();
        let tags = ["family".to_string()];
        let first = tag_view(&conn, &tags, 1, Visibility::Admin).unwrap();
        let last = tag_view(&conn, &tags, i64::MAX, Visibility::Admin).unwrap();
        assert_eq!(first.total, 64);
        assert_eq!(first.photos.len(), 60);
        assert_eq!(last.page, 2);
        assert_eq!(last.photos.len(), 4);
        assert_eq!(first.photos[59].path, "059.jpg");
        assert_eq!(last.photos[0].path, "060.jpg");
        assert_eq!(
            tag_view(&conn, &["nope".into()], 100, Visibility::Admin)
                .unwrap()
                .page,
            1
        );
    }

    #[test]
    fn admin_only_filters_metadata_and_pagination_before_counting() {
        let mut conn = db::open_memory();
        let tx = conn.transaction().unwrap();
        for id in 1..=70 {
            tx.execute(
                "INSERT INTO photos (id, path, album, size, mtime, sha256, created_at) \
                 VALUES (?1, ?2, '', 1, 0, 'hash', 0)",
                params![id, format!("{id:03}.jpg")],
            )
            .unwrap();
            add_tag(&tx, id, "family").unwrap();
            if id <= 5 || id == 70 {
                for tag in [ADMIN_ONLY_TAG, "secret", "1999", "1999-01"] {
                    add_tag(&tx, id, tag).unwrap();
                }
            }
        }
        tx.execute("UPDATE photos SET missing_at = 1 WHERE id = 70", [])
            .unwrap();
        tx.execute(
            "INSERT INTO photos (path, album, size, mtime, sha256, created_at) \
             VALUES ('untagged.jpg', '', 1, 0, 'hash', 0)",
            [],
        )
        .unwrap();
        tx.commit().unwrap();

        let s = summary(&conn, Visibility::Restricted).unwrap();
        assert_eq!((s.total, s.untagged, s.missing, s.unsynced), (65, 1, 0, 65));
        assert!(s.years.is_empty());
        assert_eq!(s.other_tags.len(), 1);
        assert_eq!(
            (s.other_tags[0].name.as_str(), s.other_tags[0].count),
            ("family", 64)
        );
        let admin = summary(&conn, Visibility::Admin).unwrap();
        assert_eq!((admin.total, admin.missing, admin.unsynced), (70, 1, 70));
        assert_eq!(
            (admin.years[0].name.as_str(), admin.years[0].count),
            ("1999", 5)
        );

        let tags = ["family".into()];
        let first = tag_view(&conn, &tags, 1, Visibility::Restricted).unwrap();
        let last = tag_view(&conn, &tags, i64::MAX, Visibility::Restricted).unwrap();
        assert_eq!((first.total, first.photos.len()), (64, 60));
        assert_eq!((last.page, last.photos.len()), (2, 4));
        assert_eq!(first.photos[0].path, "006.jpg");
        assert_eq!(last.photos[0].path, "066.jpg");
        assert!(first.subtags.is_empty());
        assert_eq!(first.known_tags, ["family"]);
        let hidden = tag_view(&conn, &["secret".into()], 1, Visibility::Restricted).unwrap();
        assert_eq!(hidden.total, 0);
        assert!(hidden.photos.is_empty());
        let admin = tag_view(&conn, &tags, 1, Visibility::Admin).unwrap();
        assert_eq!(admin.total, 69);
        assert!(admin.known_tags.contains(&"secret".into()));
        assert!(admin
            .subtags
            .iter()
            .any(|tag| tag.name == ADMIN_ONLY_TAG && tag.count == 5));
    }

    #[test]
    fn only_admin_tag_updates_can_hide_or_unhide_photos() {
        let conn = db::open_memory();
        conn.execute(
            "INSERT INTO photos (path, album, size, mtime, sha256, taken_on, created_at) \
             VALUES ('one.jpg', '', 1, 0, 'hash', '2026-11', 0)",
            [],
        )
        .unwrap();
        let tags = parse_tags("family, ADMIN-ONLY ").unwrap();
        assert_eq!(
            set_tags(&conn, 1, &tags, Visibility::Restricted).unwrap(),
            TagUpdate::ReservedTag
        );
        assert!(visible(&conn, 1, Visibility::Restricted).unwrap());
        assert_eq!(
            set_tags(&conn, 1, &tags, Visibility::Admin).unwrap(),
            TagUpdate::Saved
        );
        assert_eq!(
            set_tags(&conn, 1, &[], Visibility::Restricted).unwrap(),
            TagUpdate::NotFound
        );
        assert!(!visible(&conn, 1, Visibility::Restricted).unwrap());
        assert!(visible(&conn, 1, Visibility::Admin).unwrap());
        assert_eq!(
            set_tags(&conn, 1, &["family".into()], Visibility::Admin).unwrap(),
            TagUpdate::Saved
        );
        assert!(visible(&conn, 1, Visibility::Restricted).unwrap());
    }

    #[test]
    fn manual_tags_show_as_other_and_migration_backfills() {
        let root = library("manual");
        let mut conn = db::open_memory();
        scan(&mut conn, &root, 100).unwrap();
        let id: i64 = conn
            .query_row("SELECT id FROM photos WHERE path = 'mike.jpg'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let tx = conn.transaction().unwrap();
        add_tag(&tx, id, "mike").unwrap();
        tx.commit().unwrap();
        let s = summary(&conn, Visibility::Admin).unwrap();
        assert_eq!(s.other_tags.len(), 1);
        assert_eq!(s.other_tags[0].name, "mike");
        assert_eq!(s.untagged, 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn normalizes_tag_names() {
        assert_eq!(normalize_tag(" Mike ").as_deref(), Some("mike"));
        assert_eq!(normalize_tag("a/b"), None);
        assert_eq!(normalize_tag("."), None);
        assert_eq!(normalize_tag(".."), None);
        assert_eq!(normalize_tag("  "), None);
        assert!(parse_tags(".").is_err());
        assert!(parse_tags("..").is_err());
    }

    #[test]
    fn rewritten_file_counts_as_changed() {
        let root = library("changed");
        let mut conn = db::open_memory();
        scan(&mut conn, &root, 100).unwrap();
        write(&root, "mike.jpg", "different length");
        let r = scan(&mut conn, &root, 200).unwrap();
        assert_eq!((r.changed, r.unchanged), (1, 2));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn deleted_file_is_marked_missing_and_restored() {
        let root = library("missing");
        let mut conn = db::open_memory();
        scan(&mut conn, &root, 100).unwrap();
        fs::remove_file(root.join("mike.jpg")).unwrap();
        let r = scan(&mut conn, &root, 200).unwrap();
        assert_eq!((r.missing, r.unchanged), (1, 2));
        assert_eq!(summary(&conn, Visibility::Admin).unwrap().missing, 1);
        let again = scan(&mut conn, &root, 300).unwrap();
        assert_eq!(again.missing, 0);
        write(&root, "mike.jpg", "a");
        let back = scan(&mut conn, &root, 400).unwrap();
        assert_eq!(back.changed + back.unchanged, 3);
        assert_eq!(summary(&conn, Visibility::Admin).unwrap().missing, 0);
        fs::remove_dir_all(root).unwrap();
    }

    fn mtime_of(root: &Path, rel: &str) -> i64 {
        fs::metadata(root.join(rel))
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
    }

    #[test]
    fn mark_synced_marks_only_matching_rows() {
        let root = library("mark");
        let mut conn = db::open_memory();
        scan(&mut conn, &root, 100).unwrap();
        assert_eq!(unsynced(&conn).unwrap().len(), 3);
        assert_eq!(summary(&conn, Visibility::Admin).unwrap().unsynced, 3);
        let m = mtime_of(&root, "mike.jpg");
        let manifest =
            format!("mike.jpg\t1\t{m}\n2018/20180315_RedRiverTrip/IMG_1.JPG\t99\t{m}\n\n");
        let n = mark_synced(&mut conn, manifest.as_bytes(), 500).unwrap();
        assert_eq!(n, 1);
        assert_eq!(
            unsynced(&conn).unwrap(),
            vec![
                "2018/20180315_RedRiverTrip/IMG_1.JPG",
                "2018/20180900/IMG_2.resize.jpg"
            ]
        );
        assert_eq!(summary(&conn, Visibility::Admin).unwrap().unsynced, 2);
        assert!(mark_synced(&mut conn, "garbage\n".as_bytes(), 500).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rewritten_file_is_unsynced_again() {
        let root = library("resync");
        let mut conn = db::open_memory();
        scan(&mut conn, &root, 100).unwrap();
        let m = mtime_of(&root, "mike.jpg");
        mark_synced(&mut conn, format!("mike.jpg\t1\t{m}\n").as_bytes(), 150).unwrap();
        assert!(!unsynced(&conn).unwrap().contains(&"mike.jpg".to_string()));
        write(&root, "mike.jpg", "different length");
        scan(&mut conn, &root, 200).unwrap();
        assert!(unsynced(&conn).unwrap().contains(&"mike.jpg".to_string()));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parses_taken_on_from_album_names() {
        let t = taken_on_from_album;
        assert_eq!(
            t("2018/20180315_RedRiverTrip").as_deref(),
            Some("2018-03-15")
        );
        assert_eq!(t("2001/20010501e_UTHockey").as_deref(), Some("2001-05-01"));
        assert_eq!(t("2018/20180900").as_deref(), Some("2018-09"));
        assert_eq!(t("2022/202202").as_deref(), Some("2022-02"));
        assert_eq!(t("2022/AmiraEcp"), None);
        assert_eq!(t("ScannedPhotos"), None);
        assert_eq!(t("2018/20181300"), None);
        assert_eq!(t(""), None);
    }

    #[test]
    fn interrupted_moves_recover_identity_dates_tags_and_shared_albums() {
        for legacy in [true, false] {
            for stage in ["intent", "published", "cleanup"] {
                let root = temp_dir(&format!("move-recovery-{legacy}-{stage}"));
                let database = root.join(".recovery.sqlite");
                let mut conn = db::open(database.to_str().unwrap()).unwrap();
                db::migrate(&mut conn).unwrap();
                let old_path = if legacy {
                    "uploads/photo.jpg"
                } else {
                    "2018/20180300/photo.jpg"
                };
                let date = if legacy { "2018-03-15" } else { "2020-05" };
                let new_path = if legacy {
                    "2018/20180300/photo.jpg"
                } else {
                    "2020/20200500/photo.jpg"
                };
                write(&root, old_path, "original photo");
                scan(&mut conn, &root, 100).unwrap();
                let id: i64 = conn
                    .query_row("SELECT id FROM photos WHERE path = ?1", [old_path], |r| {
                        r.get(0)
                    })
                    .unwrap();
                conn.execute(
                    "UPDATE photos SET taken_on = '2018-03-15' WHERE id = ?1",
                    [id],
                )
                .unwrap();
                let tx = conn.unchecked_transaction().unwrap();
                add_date_tags(&tx, id, "2018-03-15").unwrap();
                add_tag(&tx, id, "family").unwrap();
                tx.commit().unwrap();
                crate::auth::add_user(&conn, "test@example.com", None, false, 0).unwrap();
                let album = albums::create(
                    &conn,
                    1,
                    &albums::parse("Family", None, &id.to_string()).unwrap(),
                    100,
                )
                .unwrap()
                .unwrap();
                let tx = photo_transaction(&conn, &root).unwrap();
                queue_photo_move(&tx, &root.canonicalize().unwrap(), id, old_path, date).unwrap();
                tx.commit().unwrap();
                assert!(root.join(old_path).is_file());
                assert!(!root.join(new_path).exists());

                if stage != "intent" {
                    let trigger = if stage == "published" {
                        "CREATE TRIGGER interrupt_move BEFORE UPDATE OF path ON photos \
                         BEGIN SELECT RAISE(ABORT, 'interrupted move'); END;"
                    } else {
                        "CREATE TRIGGER interrupt_move BEFORE DELETE ON pending_photo_moves \
                         BEGIN SELECT RAISE(ABORT, 'interrupted cleanup'); END;"
                    };
                    conn.execute_batch(trigger).unwrap();
                    assert!(recover_photo_moves(&conn, &root).is_err());
                    assert!(root.join(new_path).is_file());
                    assert_eq!(root.join(old_path).exists(), stage == "published");
                    let indexed_path: String = conn
                        .query_row("SELECT path FROM photos WHERE id = ?1", [id], |r| r.get(0))
                        .unwrap();
                    assert_eq!(
                        indexed_path,
                        if stage == "published" {
                            old_path
                        } else {
                            new_path
                        }
                    );
                    conn.execute_batch("DROP TRIGGER interrupt_move").unwrap();
                }
                drop(conn);

                let mut conn = db::open(database.to_str().unwrap()).unwrap();
                if legacy {
                    migrate_uploads(&mut conn, &root).unwrap();
                } else {
                    assert!(change_date(&conn, &root, id, date, Visibility::Admin).unwrap());
                }
                let report = scan(&mut conn, &root, 200).unwrap();
                assert_eq!(report.added + report.missing, 0);
                assert_eq!(report.unchanged, 1);
                assert_eq!(fs::read(root.join(new_path)).unwrap(), b"original photo");
                assert!(!root.join(old_path).exists());
                let indexed: (i64, String, String, String) = conn
                    .query_row("SELECT id, path, album, taken_on FROM photos", [], |r| {
                        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                    })
                    .unwrap();
                assert_eq!(
                    indexed,
                    (id, new_path.into(), album_of(new_path).into(), date.into())
                );
                assert_eq!(albums::photos(&conn, album.id).unwrap()[0].id, id);
                assert_eq!(
                    tag_view(&conn, &["family".into()], 1, Visibility::Admin)
                        .unwrap()
                        .total,
                    1
                );
                if !legacy {
                    assert_eq!(
                        tag_view(&conn, &["2018".into()], 1, Visibility::Admin)
                            .unwrap()
                            .total,
                        0
                    );
                }
                assert_eq!(
                    tag_view(
                        &conn,
                        &[date[..4].into(), date[..7].into()],
                        1,
                        Visibility::Admin
                    )
                    .unwrap()
                    .total,
                    1
                );
                let pending: i64 = conn
                    .query_row("SELECT COUNT(*) FROM pending_photo_moves", [], |r| r.get(0))
                    .unwrap();
                assert_eq!(pending, 0);
                drop(conn);
                fs::remove_dir_all(root).unwrap();
            }
        }
    }

    #[test]
    fn scan_recovers_published_move_and_refuses_conflicting_contents() {
        let root = temp_dir("scan-move-recovery");
        let mut conn = db::open_memory();
        let old_path = "uploads/photo.jpg";
        let new_path = "2018/20180300/photo.jpg";
        write(&root, old_path, "original photo");
        scan(&mut conn, &root, 100).unwrap();
        conn.execute("UPDATE photos SET taken_on = '2018-03'", [])
            .unwrap();
        let tx = photo_transaction(&conn, &root).unwrap();
        queue_photo_move(&tx, &root.canonicalize().unwrap(), 1, old_path, "2018-03").unwrap();
        tx.commit().unwrap();
        write(&root, new_path, "different photo");
        assert!(scan(&mut conn, &root, 200)
            .unwrap_err()
            .contains("contents differ"));
        assert_eq!(fs::read(root.join(old_path)).unwrap(), b"original photo");
        assert_eq!(fs::read(root.join(new_path)).unwrap(), b"different photo");
        fs::remove_file(root.join(new_path)).unwrap();
        fs::hard_link(root.join(old_path), root.join(new_path)).unwrap();
        let report = scan(&mut conn, &root, 300).unwrap();
        assert_eq!(report.added + report.missing, 0);
        assert_eq!(report.unchanged, 1);
        assert!(!root.join(old_path).exists());
        let id: i64 = conn
            .query_row("SELECT id FROM photos WHERE path = ?1", [new_path], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(id, 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn move_ledger_tracks_original_paths_across_repeated_moves() {
        let conn = db::open_memory();
        conn.execute(
            "INSERT INTO photos (path, album, size, mtime, sha256, created_at) \
             VALUES ('uploads/photo.jpg', 'uploads', 1, 0, 'hash', 0)",
            [],
        )
        .unwrap();
        let photo_id = conn.last_insert_rowid();
        let tx = conn.unchecked_transaction().unwrap();
        record_photo_move(
            &tx,
            photo_id,
            "uploads/photo.jpg",
            "2026/20260100/photo.jpg",
            "hash",
        )
        .unwrap();
        record_photo_move(
            &tx,
            photo_id,
            "2026/20260100/photo.jpg",
            "2026/20260200/photo.jpg",
            "hash",
        )
        .unwrap();
        tx.commit().unwrap();
        let mut stmt = conn
            .prepare("SELECT old_path, new_path, sha256 FROM photo_moves ORDER BY old_path")
            .unwrap();
        let moves: Vec<(String, String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            moves,
            vec![
                (
                    "2026/20260100/photo.jpg".into(),
                    "2026/20260200/photo.jpg".into(),
                    "hash".into()
                ),
                (
                    "uploads/photo.jpg".into(),
                    "2026/20260200/photo.jpg".into(),
                    "hash".into()
                )
            ]
        );
        assert_eq!(
            acknowledge_photo_moves(
                &conn,
                "uploads/photo.jpg\n2026/20260100/photo.jpg\n".as_bytes()
            )
            .unwrap(),
            2
        );
        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM photo_moves", [], |r| r.get(0))
            .unwrap();
        assert_eq!(remaining, 0);
    }
}
