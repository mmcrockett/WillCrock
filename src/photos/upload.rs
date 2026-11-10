use super::{add_date_tags, Visibility};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::{Cursor, Write};
use std::path::Path;

#[derive(Debug)]
pub enum UploadError {
    Invalid(&'static str),
    NotFound,
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

fn meta_contains_idat(data: &[u8]) -> bool {
    let mut at = 4;
    let mut found = false;
    while at < data.len() {
        let Some(header) = data.get(at..at.saturating_add(8)) else {
            return false;
        };
        let size = u32::from_be_bytes(header[..4].try_into().expect("four bytes")) as usize;
        if size < 8 || size > data.len() - at {
            return false;
        }
        if &header[4..8] == b"idat" && size > 8 {
            found = true;
        }
        at += size;
    }
    found
}

fn is_heic(bytes: &[u8]) -> bool {
    let mut at: usize = 0;
    let mut ftyp = false;
    let mut meta = false;
    let mut image_data = false;
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
            b"meta" => {
                meta = true;
                image_data |= meta_contains_idat(data);
            }
            b"mdat" if !data.is_empty() => image_data = true,
            _ => {}
        }
        at += size;
    }
    at == bytes.len() && ftyp && meta && image_data
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
    visibility: Visibility,
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
    let tx = super::photo_transaction(conn, &root).map_err(UploadError::Failed)?;
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
        if !super::visible(&tx, id, visibility)? {
            return Err(UploadError::NotFound);
        }
        tx.commit()?;
        return Ok(Uploaded {
            id,
            date,
            date_source: "existing",
            duplicate: true,
        });
    }

    let album = super::month_album(&date)
        .ok_or_else(|| UploadError::Failed(format!("invalid photo date: {date}")))?;
    let dir = root.join(&album);
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
    let rel = format!("{album}/{name}");
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
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![rel, album, bytes.len() as i64, mtime, hash, date, now],
            )?;
            let id = tx.last_insert_rowid();
            add_date_tags(&tx, id, &date)?;
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

pub fn change_date(
    conn: &Connection,
    root: &Path,
    id: i64,
    month: &str,
    visibility: Visibility,
) -> Result<bool, UploadError> {
    if !month_valid(month) {
        return Err(UploadError::Invalid("Choose a valid year and month."));
    }
    let root = root
        .canonicalize()
        .map_err(|e| io_error("photo directory", e))?;
    let tx = super::photo_transaction(conn, &root).map_err(UploadError::Failed)?;
    let photo = tx
        .query_row(
            &format!(
                "SELECT taken_on, path FROM photos p \
                 WHERE p.id = ?1 AND p.missing_at IS NULL AND {}",
                visibility.sql()
            ),
            [id],
            |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, String>(1)?)),
        )
        .optional()?;
    let Some((old_date, old_path)) = photo else {
        return Ok(false);
    };
    if old_date.as_deref().and_then(|date| date.get(..7)) == Some(month) {
        return Ok(true);
    }

    if super::month_album(month).as_deref() == Some(super::album_of(&old_path)) {
        if let Some(old) = &old_date {
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
        add_date_tags(&tx, id, month)?;
        tx.commit()?;
        return Ok(true);
    }
    super::queue_photo_move(&tx, &root, id, &old_path, month).map_err(UploadError::Failed)?;
    tx.commit()?;
    super::recover_photo_moves(conn, &root).map_err(UploadError::Failed)?;
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
        let heic_with_idat =
            b"\0\0\0\x14ftypheic\0\0\0\0mif1\0\0\0\x15meta\0\0\0\0\0\0\0\x09idat\x01";
        assert!(is_heic(heic_with_idat));
        let heic_with_empty_idat =
            b"\0\0\0\x14ftypheic\0\0\0\0mif1\0\0\0\x14meta\0\0\0\0\0\0\0\x08idat";
        assert!(!is_heic(heic_with_empty_idat));
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
        let first = upload(
            &conn,
            &root,
            "../family.jpg",
            &bytes,
            1_793_831_400,
            Visibility::Restricted,
        )
        .unwrap();
        assert_eq!(first.date.as_deref(), Some("2018-03-15"));
        assert_eq!(first.date_source, "metadata");
        assert!(!first.duplicate);
        let path: String = conn
            .query_row("SELECT path FROM photos WHERE id = ?1", [first.id], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(path.starts_with("2018/20180300/"));
        assert!(root.join(&path).is_file());
        let duplicate = upload(
            &conn,
            &root,
            "another.jpg",
            &bytes,
            1_793_831_400,
            Visibility::Restricted,
        )
        .unwrap();
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
        assert!(change_date(&conn, &root, first.id, "2020-05", Visibility::Restricted).unwrap());
        let corrected: String = conn
            .query_row("SELECT path FROM photos WHERE id = ?1", [first.id], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(corrected.starts_with("2020/20200500/"));
        assert!(root.join(&corrected).is_file());
        assert!(!root.join(&path).exists());
        assert!(matches!(
            change_date(&conn, &root, first.id, "2020-13", Visibility::Restricted),
            Err(UploadError::Invalid(_))
        ));
        assert!(!change_date(&conn, &root, 999, "2020-05", Visibility::Restricted).unwrap());
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
            photos::tag_view(
                &conn,
                &["2020".into(), "2020-05".into()],
                1,
                Visibility::Restricted
            )
            .unwrap()
            .total,
            1
        );
        assert_eq!(
            photos::tag_view(&conn, &["family".into()], 1, Visibility::Restricted)
                .unwrap()
                .total,
            1
        );
        assert_eq!(
            photos::tag_view(&conn, &["2018".into()], 1, Visibility::Restricted)
                .unwrap()
                .total,
            0
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn new_uploads_use_month_buckets_and_legacy_uploads_migrate_once() {
        let root = std::env::temp_dir().join(format!("photo-upload-{}", rand::random::<u64>()));
        std::fs::create_dir(&root).unwrap();
        let mut conn = db::open_memory();
        let uploaded = upload(
            &conn,
            &root,
            "phone.jpg",
            &jpeg(false),
            1_793_831_400,
            Visibility::Restricted,
        )
        .unwrap();
        let path: String = conn
            .query_row(
                "SELECT path FROM photos WHERE id = ?1",
                [uploaded.id],
                |r| r.get(0),
            )
            .unwrap();
        let date = uploaded.date.as_deref().unwrap();
        let month_folder = format!("{}/{}00", &date[..4], date[..7].replace('-', ""));
        assert!(path.starts_with(&format!("{month_folder}/")));

        let legacy = root.join("uploads/legacy-123.jpg");
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        let bytes = jpeg(false);
        std::fs::write(&legacy, &bytes).unwrap();
        let metadata = std::fs::metadata(&legacy).unwrap();
        let mtime = metadata
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        conn.execute(
            "INSERT INTO photos (path, album, size, mtime, sha256, taken_on, created_at) \
             VALUES ('uploads/legacy-123.jpg', 'uploads', ?1, ?2, ?3, '2018-03-15', 0)",
            params![
                metadata.len() as i64,
                mtime,
                format!("{:x}", Sha256::digest(&bytes))
            ],
        )
        .unwrap();
        let legacy_id = conn.last_insert_rowid();

        assert_eq!(photos::migrate_uploads(&mut conn, &root).unwrap(), 1);
        assert_eq!(photos::migrate_uploads(&mut conn, &root).unwrap(), 0);
        let migrated: (String, String) = conn
            .query_row(
                "SELECT path, album FROM photos WHERE id = ?1",
                [legacy_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            migrated,
            (
                "2018/20180300/legacy-123.jpg".into(),
                "2018/20180300".into()
            )
        );
        assert!(!legacy.exists());
        assert!(root.join(&migrated.0).is_file());
        let report = photos::scan(&mut conn, &root, 100).unwrap();
        assert_eq!(report.unchanged, 2);
        assert_eq!(report.added + report.missing, 0);
        let move_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM photo_moves", [], |r| r.get(0))
            .unwrap();
        assert_eq!(move_count, 1);
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
        let uploaded = upload(
            &conn,
            &root,
            "x.jpg",
            &jpeg(false),
            now,
            Visibility::Restricted,
        )
        .unwrap();
        assert_eq!(uploaded.date.as_deref(), Some("2026-10"));
        assert_eq!(uploaded.date_source, "current month");
        assert!(matches!(
            upload(
                &conn,
                &root,
                "fake.jpg",
                b"not a photo",
                now,
                Visibility::Restricted
            ),
            Err(UploadError::Invalid(_))
        ));
        assert!(matches!(
            upload(
                &conn,
                &root,
                "broken.jpg",
                &[0xff, 0xd8, 0xff],
                now,
                Visibility::Restricted
            ),
            Err(UploadError::Invalid(_))
        ));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn admin_only_blocks_duplicate_metadata_and_date_changes_and_survives_scan() {
        let root =
            std::env::temp_dir().join(format!("photo-private-upload-{}", rand::random::<u64>()));
        std::fs::create_dir(&root).unwrap();
        let mut conn = db::open_memory();
        let bytes = jpeg(true);
        let first = upload(&conn, &root, "one.jpg", &bytes, 100, Visibility::Admin).unwrap();
        photos::set_tags(
            &conn,
            first.id,
            &["family".into(), photos::ADMIN_ONLY_TAG.into()],
            Visibility::Admin,
        )
        .unwrap();
        assert!(matches!(
            upload(&conn, &root, "two.jpg", &bytes, 100, Visibility::Restricted),
            Err(UploadError::NotFound)
        ));
        assert_eq!(photos::unsynced(&conn).unwrap().len(), 1);
        let original_path: String = conn
            .query_row("SELECT path FROM photos WHERE id = ?1", [first.id], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(!change_date(&conn, &root, first.id, "2020-05", Visibility::Restricted).unwrap());
        assert!(root.join(&original_path).is_file());
        assert!(!root.join("2020/20200500").exists());
        let duplicate = upload(&conn, &root, "two.jpg", &bytes, 100, Visibility::Admin).unwrap();
        assert!(duplicate.duplicate);
        assert_eq!(duplicate.id, first.id);
        assert_eq!(duplicate.date.as_deref(), Some("2018-03-15"));
        assert!(change_date(&conn, &root, first.id, "2020-05", Visibility::Admin).unwrap());
        photos::scan(&mut conn, &root, 200).unwrap();
        let admin = photos::tag_view(&conn, &[], 1, Visibility::Admin).unwrap();
        assert_eq!(admin.photos.len(), 1);
        assert_eq!(admin.photos[0].id, first.id);
        assert!(admin.photos[0].path.starts_with("2020/20200500/"));
        assert!(!root.join(&original_path).exists());
        assert_eq!(admin.photos[0].tags, [photos::ADMIN_ONLY_TAG, "family"]);
        assert_eq!(
            photos::tag_view(&conn, &["2020-05".into()], 1, Visibility::Admin)
                .unwrap()
                .total,
            1
        );
        assert_eq!(
            photos::summary(&conn, Visibility::Restricted)
                .unwrap()
                .total,
            0
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stale_hash_does_not_prevent_upload() {
        let root = std::env::temp_dir().join(format!("photo-upload-{}", rand::random::<u64>()));
        std::fs::create_dir(&root).unwrap();
        let conn = db::open_memory();
        let bytes = jpeg(false);
        let first = upload(
            &conn,
            &root,
            "photo.jpg",
            &bytes,
            1_793_831_400,
            Visibility::Restricted,
        )
        .unwrap();
        let path: String = conn
            .query_row("SELECT path FROM photos WHERE id = ?1", [first.id], |r| {
                r.get(0)
            })
            .unwrap();
        std::fs::write(root.join(path), b"changed").unwrap();
        let second = upload(
            &conn,
            &root,
            "photo.jpg",
            &bytes,
            1_793_831_400,
            Visibility::Restricted,
        )
        .unwrap();
        assert!(!second.duplicate);
        assert_ne!(first.id, second.id);
        std::fs::remove_dir_all(root).unwrap();
    }
}
