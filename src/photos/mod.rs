use crate::{cli, config, db};
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::time::UNIX_EPOCH;

mod images;
pub use images::{image_file, ImageKind};

const IMAGE_EXTS: [&str; 6] = ["jpg", "jpeg", "png", "gif", "heic", "webp"];

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
    (!t.is_empty() && !t.contains('/')).then_some(t)
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

const LIVE_WITH_ALL_TAGS: &str = "SELECT p.id FROM photos p WHERE p.missing_at IS NULL AND \
     (SELECT COUNT(*) FROM photo_tags pt JOIN tags t ON t.id = pt.tag_id \
      WHERE pt.photo_id = p.id AND t.name IN ({in})) = {n}";

fn live_with_all_tags(tags: &[String]) -> String {
    let marks = vec!["?"; tags.len()].join(",");
    LIVE_WITH_ALL_TAGS
        .replace("{in}", &marks)
        .replace("{n}", &tags.len().to_string())
}

pub fn tag_view(conn: &Connection, tags: &[String], page: i64) -> rusqlite::Result<TagView> {
    let ids = live_with_all_tags(tags);
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
    Ok(TagView {
        tags: tags.to_vec(),
        subtags,
        photos,
        total,
        page,
    })
}

pub fn parse_tags(raw: &str) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    for item in raw.split(',').filter(|item| !item.trim().is_empty()) {
        let Some(name) = normalize_tag(item) else {
            return Err("Tags cannot contain slashes.".into());
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

pub fn set_tags(conn: &Connection, photo_id: i64, names: &[String]) -> rusqlite::Result<bool> {
    let tx = conn.unchecked_transaction()?;
    let Some(taken_on) = tx
        .query_row(
            "SELECT taken_on FROM photos WHERE id = ?1 AND missing_at IS NULL",
            [photo_id],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?
    else {
        return Ok(false);
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
    Ok(true)
}

pub fn run(args: &[String]) {
    match args.first().map(String::as_str) {
        Some("scan") => scan_cmd(),
        Some("unsynced") => unsynced_cmd(),
        Some("mark-synced") => mark_synced_cmd(),
        _ => cli::fail("usage: willcrock photos scan|unsynced|mark-synced"),
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

fn unsynced_cmd() {
    let (_, conn) = open_db();
    for path in unsynced(&conn).unwrap_or_else(|e| cli::fail(e)) {
        println!("{path}");
    }
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

pub fn summary(conn: &Connection) -> rusqlite::Result<Summary> {
    let mut stmt = conn.prepare(
        "SELECT t.name, COUNT(*) FROM photo_tags pt \
         JOIN tags t ON t.id = pt.tag_id JOIN photos p ON p.id = pt.photo_id \
         WHERE p.missing_at IS NULL GROUP BY t.id ORDER BY t.name",
    )?;
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
        "SELECT COUNT(*) FROM photos WHERE missing_at IS NULL",
        [],
        |r| r.get(0),
    )?;
    let untagged = conn.query_row(
        "SELECT COUNT(*) FROM photos p WHERE p.missing_at IS NULL \
         AND NOT EXISTS (SELECT 1 FROM photo_tags WHERE photo_id = p.id)",
        [],
        |r| r.get(0),
    )?;
    let missing = conn.query_row(
        "SELECT COUNT(*) FROM photos WHERE missing_at IS NOT NULL",
        [],
        |r| r.get(0),
    )?;
    let unsynced = conn.query_row(
        "SELECT COUNT(*) FROM photos WHERE missing_at IS NULL AND synced_at IS NULL",
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
    let mut files = Vec::new();
    walk(root, root, &mut files)?;
    let db_err = |e: rusqlite::Error| e.to_string();
    let tx = conn.transaction().map_err(db_err)?;
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
                    add_tag(&tx, id, &taken_on[..4]).map_err(db_err)?;
                    add_tag(&tx, id, &taken_on[..7]).map_err(db_err)?;
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
        let s = summary(&conn).unwrap();
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
        let year = tag_view(&conn, &["2018".to_string()], 1).unwrap();
        assert_eq!(year.total, 2);
        let months: Vec<_> = year.subtags.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(months, vec!["2018-03", "2018-09"]);
        let month = tag_view(&conn, &["2018".to_string(), "2018-03".to_string()], 1).unwrap();
        assert_eq!(month.photos[0].path, "2018/20180315_RedRiverTrip/IMG_1.JPG");
        assert!(month.subtags.is_empty());
        let none = tag_view(&conn, &["nope".to_string()], 1).unwrap();
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
        let first = tag_view(&conn, &tags, 1).unwrap();
        let last = tag_view(&conn, &tags, i64::MAX).unwrap();
        assert_eq!(first.total, 64);
        assert_eq!(first.photos.len(), 60);
        assert_eq!(last.page, 2);
        assert_eq!(last.photos.len(), 4);
        assert_eq!(first.photos[59].path, "059.jpg");
        assert_eq!(last.photos[0].path, "060.jpg");
        assert_eq!(tag_view(&conn, &["nope".into()], 100).unwrap().page, 1);
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
        let s = summary(&conn).unwrap();
        assert_eq!(s.other_tags.len(), 1);
        assert_eq!(s.other_tags[0].name, "mike");
        assert_eq!(s.untagged, 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn normalizes_tag_names() {
        assert_eq!(normalize_tag(" Mike ").as_deref(), Some("mike"));
        assert_eq!(normalize_tag("a/b"), None);
        assert_eq!(normalize_tag("  "), None);
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
        assert_eq!(summary(&conn).unwrap().missing, 1);
        let again = scan(&mut conn, &root, 300).unwrap();
        assert_eq!(again.missing, 0);
        write(&root, "mike.jpg", "a");
        let back = scan(&mut conn, &root, 400).unwrap();
        assert_eq!(back.changed + back.unchanged, 3);
        assert_eq!(summary(&conn).unwrap().missing, 0);
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
        assert_eq!(summary(&conn).unwrap().unsynced, 3);
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
        assert_eq!(summary(&conn).unwrap().unsynced, 2);
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
}
