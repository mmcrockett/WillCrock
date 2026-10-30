use crate::{cli, config, db};
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::time::UNIX_EPOCH;

const IMAGE_EXTS: [&str; 6] = ["jpg", "jpeg", "png", "gif", "heic", "webp"];

#[derive(Debug, Default, PartialEq)]
pub struct ScanReport {
    pub added: usize,
    pub changed: usize,
    pub unchanged: usize,
    pub missing: usize,
}

pub struct Album {
    pub name: String,
    pub count: i64,
    pub taken_on: Option<String>,
}

pub struct Summary {
    pub albums: Vec<Album>,
    pub total: i64,
    pub missing: i64,
}

pub fn run(args: &[String]) {
    match args.first().map(String::as_str) {
        Some("scan") => scan_cmd(),
        _ => cli::fail("usage: willcrock photos scan"),
    }
}

fn scan_cmd() {
    let cfg = config::load().unwrap_or_else(|e| cli::fail(e));
    let Some(photos) = cfg.photos.as_ref() else {
        cli::fail("no [photos] section in config");
    };
    let mut conn = db::open(&cfg.db_path).unwrap_or_else(|e| cli::fail(e));
    let r = scan(&mut conn, Path::new(&photos.dir), cli::now()).unwrap_or_else(|e| cli::fail(e));
    println!(
        "added {} changed {} unchanged {} missing {}",
        r.added, r.changed, r.unchanged, r.missing
    );
}

pub fn summary(conn: &Connection) -> rusqlite::Result<Summary> {
    let mut stmt = conn.prepare(
        "SELECT album, COUNT(*), MIN(taken_on) FROM photos WHERE missing_at IS NULL \
         GROUP BY album ORDER BY album DESC",
    )?;
    let albums = stmt
        .query_map([], |r| {
            Ok(Album {
                name: r.get(0)?,
                count: r.get(1)?,
                taken_on: r.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let missing = conn.query_row(
        "SELECT COUNT(*) FROM photos WHERE missing_at IS NOT NULL",
        [],
        |r| r.get(0),
    )?;
    Ok(Summary {
        total: albums.iter().map(|a| a.count).sum(),
        albums,
        missing,
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
                    "UPDATE photos SET size = ?1, mtime = ?2, sha256 = ?3, missing_at = NULL \
                     WHERE id = ?4",
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

    fn temp_dir(tag: &str) -> PathBuf {
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
        assert_eq!(s.albums.len(), 3);
        fs::remove_dir_all(root).unwrap();
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
