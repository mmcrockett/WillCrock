use super::add_tag;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::{Cursor, Write};
use std::path::Path;

#[derive(Debug)]
pub enum UploadError {
    Invalid(&'static str),
    Failed(String),
}

impl From<rusqlite::Error> for UploadError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Failed(error.to_string())
    }
}

#[derive(Serialize)]
pub struct Uploaded {
    pub id: i64,
    pub date: Option<String>,
    pub date_source: &'static str,
    pub duplicate: bool,
}

fn photo_format(bytes: &[u8]) -> Result<&'static str, UploadError> {
    let format = image::guess_format(bytes).ok();
    match format {
        Some(image::ImageFormat::Jpeg) => Ok("jpg"),
        Some(image::ImageFormat::Png) => Ok("png"),
        Some(image::ImageFormat::Gif) => Ok("gif"),
        Some(image::ImageFormat::WebP) => Ok("webp"),
        _ if is_heic(bytes) => Ok("heic"),
        _ => Err(UploadError::Invalid(
            "Choose a JPEG, PNG, GIF, WebP, or HEIC photo.",
        )),
    }
}

fn is_heic(bytes: &[u8]) -> bool {
    let mut at: usize = 0;
    let mut ftyp = false;
    let mut meta = false;
    let mut mdat = false;
    while let Some(header) = bytes.get(at..at.saturating_add(8)) {
        let size = u32::from_be_bytes(header[..4].try_into().expect("four bytes")) as usize;
        if size < 8 || size > bytes.len() - at {
            return false;
        }
        let data = &bytes[at + 8..at + size];
        match &header[4..8] {
            b"ftyp" if at == 0 && data.len() >= 8 => {
                ftyp =
                    data[..4] == *b"heic"
                        || data[..4] == *b"heix"
                        || data[..4] == *b"hevc"
                        || data[..4] == *b"hevx"
                        || (data[..4] == *b"mif1"
                            && data[8..].as_chunks::<4>().0.iter().any(|brand| {
                                matches!(brand, b"heic" | b"heix" | b"hevc" | b"hevx")
                            }));
            }
            b"meta" => meta = true,
            b"mdat" if !data.is_empty() => mdat = true,
            _ => {}
        }
        at += size;
    }
    at == bytes.len() && ftyp && meta && mdat
}

fn validate_image(bytes: &[u8], ext: &str) -> Result<(), UploadError> {
    if ext == "heic" {
        return Ok(());
    }
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| UploadError::Invalid("Could not read the photo."))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(20_000);
    limits.max_image_height = Some(20_000);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    reader
        .decode()
        .map_err(|_| UploadError::Invalid("The photo is damaged or too large to decode."))?;
    Ok(())
}

fn metadata_date(bytes: &[u8]) -> Option<String> {
    let exif = exif::Reader::new()
        .read_from_container(&mut Cursor::new(bytes))
        .ok()?;
    [
        exif::Tag::DateTimeOriginal,
        exif::Tag::DateTimeDigitized,
        exif::Tag::DateTime,
    ]
    .into_iter()
    .find_map(|tag| {
        let field = exif.get_field(tag, exif::In::PRIMARY)?;
        let exif::Value::Ascii(values) = &field.value else {
            return None;
        };
        let value = std::str::from_utf8(values.first()?).ok()?;
        let date = value.get(..10)?.replace(':', "-");
        let date = date.parse::<jiff::civil::Date>().ok()?;
        (date.year() > 0).then(|| date.to_string())
    })
}

fn month_valid(month: &str) -> bool {
    month.len() == 7
        && month.as_bytes()[4] == b'-'
        && month
            .bytes()
            .enumerate()
            .all(|(i, b)| i == 4 || b.is_ascii_digit())
        && format!("{month}-01")
            .parse::<jiff::civil::Date>()
            .is_ok_and(|date| date.year() > 0)
}

fn io_error(context: &str, error: std::io::Error) -> UploadError {
    UploadError::Failed(format!("{context}: {error}"))
}

pub fn upload(
    conn: &Connection,
    root: &Path,
    filename: &str,
    bytes: &[u8],
    now: i64,
) -> Result<Uploaded, UploadError> {
    let ext = photo_format(bytes)?;
    validate_image(bytes, ext)?;
    let (date, date_source) = match metadata_date(bytes) {
        Some(date) => (date, "metadata"),
        None => {
            let timestamp = jiff::Timestamp::from_second(now)
                .map_err(|e| UploadError::Failed(e.to_string()))?;
            let local = timestamp
                .in_tz("America/Chicago")
                .map_err(|e| UploadError::Failed(e.to_string()))?;
            (local.strftime("%Y-%m").to_string(), "current month")
        }
    };
    let root = root
        .canonicalize()
        .map_err(|e| io_error("photo directory", e))?;
    let hash = format!("{:x}", Sha256::digest(bytes));
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let duplicate = {
        let mut stmt = tx.prepare(
            "SELECT id, taken_on, path FROM photos WHERE sha256 = ?1 AND missing_at IS NULL",
        )?;
        let rows = stmt.query_map([&hash], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        let mut found = None;
        for row in rows {
            let (id, date, path) = row?;
            let path = match root.join(path).canonicalize() {
                Ok(path) => path,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(io_error("check existing photo", e)),
            };
            if path.starts_with(&root)
                && path.is_file()
                && super::hash(&path).map_err(UploadError::Failed)? == hash
            {
                found = Some((id, date));
                break;
            }
        }
        found
    };
    if let Some((id, date)) = duplicate {
        tx.commit()?;
        return Ok(Uploaded {
            id,
            date,
            date_source: "existing",
            duplicate: true,
        });
    }

    let dir = root.join("uploads");
    std::fs::create_dir_all(&dir).map_err(|e| io_error("create upload directory", e))?;
    let dir = dir
        .canonicalize()
        .map_err(|e| io_error("upload directory", e))?;
    if !dir.starts_with(&root) {
        return Err(UploadError::Failed(
            "upload directory is outside photo library".into(),
        ));
    }
    let stem: String = Path::new(filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("photo")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .take(80)
        .collect();
    let stem = if stem.is_empty() { "photo" } else { &stem };
    let name = format!("{stem}-{:032x}.{ext}", rand::random::<u128>());
    let rel = format!("uploads/{name}");
    let path = dir.join(&name);
    let tmp = dir.join(format!(".{name}.tmp"));
    let mut created = false;
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .map_err(|e| io_error("create temporary photo", e))?;
        created = true;
        file.write_all(bytes)
            .map_err(|e| io_error("write photo", e))?;
        file.sync_all().map_err(|e| io_error("sync photo", e))?;
        let meta = file.metadata().map_err(|e| io_error("stat photo", e))?;
        let mtime = meta
            .modified()
            .map_err(|e| io_error("photo mtime", e))?
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| UploadError::Failed(format!("photo mtime: {e}")))?
            .as_secs() as i64;
        std::fs::hard_link(&tmp, &path).map_err(|e| io_error("publish photo", e))?;
        let database_result = (|| -> Result<i64, UploadError> {
            tx.execute(
                "INSERT INTO photos (path, album, size, mtime, sha256, taken_on, created_at) \
                 VALUES (?1, 'uploads', ?2, ?3, ?4, ?5, ?6)",
                params![rel, bytes.len() as i64, mtime, hash, date, now],
            )?;
            let id = tx.last_insert_rowid();
            add_tag(&tx, id, &date[..4])?;
            add_tag(&tx, id, &date[..7])?;
            tx.commit()?;
            Ok(id)
        })();
        if database_result.is_err() {
            if let Err(e) = std::fs::remove_file(&path) {
                crate::log::error(&format!("remove failed upload {}: {e}", path.display()));
            }
        }
        database_result
    })();
    if created {
        if let Err(e) = std::fs::remove_file(&tmp) {
            crate::log::error(&format!("remove temporary upload {}: {e}", tmp.display()));
        }
    }
    Ok(Uploaded {
        id: result?,
        date: Some(date),
        date_source,
        duplicate: false,
    })
}

pub fn change_date(conn: &Connection, id: i64, month: &str) -> Result<bool, UploadError> {
    if !month_valid(month) {
        return Err(UploadError::Invalid("Choose a valid year and month."));
    }
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let old = tx
        .query_row(
            "SELECT taken_on FROM photos WHERE id = ?1 AND missing_at IS NULL",
            [id],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?;
    let Some(old) = old else {
        return Ok(false);
    };
    if let Some(old) = old {
        if old.get(..7) == Some(month) {
            return Ok(true);
        }
        tx.execute(
            "DELETE FROM photo_tags WHERE photo_id = ?1 AND tag_id IN \
             (SELECT id FROM tags WHERE name IN (?2, ?3))",
            params![id, old.get(..4).unwrap_or(""), old.get(..7).unwrap_or("")],
        )?;
    }
    tx.execute(
        "UPDATE photos SET taken_on = ?1 WHERE id = ?2",
        params![month, id],
    )?;
    add_tag(&tx, id, &month[..4])?;
    add_tag(&tx, id, month)?;
    tx.commit()?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db, photos};

    #[test]
    fn heic_container_validation() {
        let heic = b"\0\0\0\x14ftypheic\0\0\0\0mif1\0\0\0\x0cmeta\0\0\0\0\0\0\0\x09mdat\x01";
        assert!(is_heic(heic));
        assert!(!is_heic(&heic[..heic.len() - 1]));
        assert!(!is_heic(b"\0\0\0\x10ftypheic\0\0\0\0"));
    }

    fn jpeg(with_exif: bool) -> Vec<u8> {
        let mut jpeg = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(2, 2)
            .write_to(&mut jpeg, image::ImageFormat::Jpeg)
            .unwrap();
        let jpeg = jpeg.into_inner();
        if !with_exif {
            return jpeg;
        }
        let mut tiff = b"II\x2a\x00\x08\x00\x00\x00".to_vec();
        tiff.extend_from_slice(&[1, 0, 0x69, 0x87, 4, 0, 1, 0, 0, 0, 26, 0, 0, 0, 0, 0, 0, 0]);
        tiff.extend_from_slice(&[1, 0, 3, 0x90, 2, 0, 20, 0, 0, 0, 44, 0, 0, 0, 0, 0, 0, 0]);
        tiff.extend_from_slice(b"2018:03:15 12:34:56\0");
        let mut segment = b"Exif\0\0".to_vec();
        segment.extend(tiff);
        let mut result = jpeg[..2].to_vec();
        result.extend_from_slice(&[0xff, 0xe1]);
        result.extend_from_slice(&((segment.len() + 2) as u16).to_be_bytes());
        result.extend(segment);
        result.extend_from_slice(&jpeg[2..]);
        result
    }

    #[test]
    fn upload_dates_deduplicates_and_preserves_changes_on_scan() {
        let root = std::env::temp_dir().join(format!("photo-upload-{}", rand::random::<u64>()));
        std::fs::create_dir(&root).unwrap();
        let mut conn = db::open_memory();
        let bytes = jpeg(true);
        let first = upload(&conn, &root, "../family.jpg", &bytes, 1_793_831_400).unwrap();
        assert_eq!(first.date.as_deref(), Some("2018-03-15"));
        assert_eq!(first.date_source, "metadata");
        assert!(!first.duplicate);
        let duplicate = upload(&conn, &root, "another.jpg", &bytes, 1_793_831_400).unwrap();
        assert_eq!(duplicate.id, first.id);
        assert!(duplicate.duplicate);
        assert_eq!(photos::unsynced(&conn).unwrap().len(), 1);
        conn.execute("INSERT INTO tags (name) VALUES ('family')", [])
            .unwrap();
        conn.execute(
            "INSERT INTO photo_tags SELECT ?1, id FROM tags WHERE name = 'family'",
            [first.id],
        )
        .unwrap();
        assert!(change_date(&conn, first.id, "2020-05").unwrap());
        assert!(matches!(
            change_date(&conn, first.id, "2020-13"),
            Err(UploadError::Invalid(_))
        ));
        assert!(!change_date(&conn, 999, "2020-05").unwrap());
        photos::scan(&mut conn, &root, 100).unwrap();
        let date: String = conn
            .query_row(
                "SELECT taken_on FROM photos WHERE id = ?1",
                [first.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(date, "2020-05");
        assert_eq!(
            photos::tag_view(&conn, &["2020".into(), "2020-05".into()], 1)
                .unwrap()
                .total,
            1
        );
        assert_eq!(
            photos::tag_view(&conn, &["family".into()], 1)
                .unwrap()
                .total,
            1
        );
        assert_eq!(
            photos::tag_view(&conn, &["2018".into()], 1).unwrap().total,
            0
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_metadata_falls_back_to_central_month_and_bad_files_are_rejected() {
        let root = std::env::temp_dir().join(format!("photo-upload-{}", rand::random::<u64>()));
        std::fs::create_dir(&root).unwrap();
        let conn = db::open_memory();
        let now = "2026-11-01T04:30:00Z"
            .parse::<jiff::Timestamp>()
            .unwrap()
            .as_second();
        let uploaded = upload(&conn, &root, "x.jpg", &jpeg(false), now).unwrap();
        assert_eq!(uploaded.date.as_deref(), Some("2026-10"));
        assert_eq!(uploaded.date_source, "current month");
        assert!(matches!(
            upload(&conn, &root, "fake.jpg", b"not a photo", now),
            Err(UploadError::Invalid(_))
        ));
        assert!(matches!(
            upload(&conn, &root, "broken.jpg", &[0xff, 0xd8, 0xff], now),
            Err(UploadError::Invalid(_))
        ));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stale_hash_does_not_prevent_upload() {
        let root = std::env::temp_dir().join(format!("photo-upload-{}", rand::random::<u64>()));
        std::fs::create_dir(&root).unwrap();
        let conn = db::open_memory();
        let bytes = jpeg(false);
        let first = upload(&conn, &root, "photo.jpg", &bytes, 1_793_831_400).unwrap();
        let path: String = conn
            .query_row("SELECT path FROM photos WHERE id = ?1", [first.id], |r| {
                r.get(0)
            })
            .unwrap();
        std::fs::write(root.join(path), b"changed").unwrap();
        let second = upload(&conn, &root, "photo.jpg", &bytes, 1_793_831_400).unwrap();
        assert!(!second.duplicate);
        assert_ne!(first.id, second.id);
        std::fs::remove_dir_all(root).unwrap();
    }
}
