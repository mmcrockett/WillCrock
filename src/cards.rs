use crate::{http::MAX_PHOTO_SIZE, images};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};

pub struct Card {
    pub id: i64,
    pub name: String,
    pub path: String,
    pub mime: String,
    pub sha256: String,
}

pub struct CardImage {
    pub path: String,
    pub mime: String,
    pub sha256: String,
}

#[derive(Debug)]
pub enum UploadError {
    NotFound,
    BackExists,
    Invalid(&'static str),
    Failed(String),
}

impl From<rusqlite::Error> for UploadError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Failed(error.to_string())
    }
}

impl From<std::io::Error> for UploadError {
    fn from(error: std::io::Error) -> Self {
        Self::Failed(error.to_string())
    }
}

pub fn storage_root(dir: &str, photos_dir: Option<&str>) -> Result<PathBuf, String> {
    let root = Path::new(dir)
        .canonicalize()
        .map_err(|e| format!("card directory: {e}"))?;
    if !root.is_dir() {
        return Err("card storage must be a directory".into());
    }
    if let Some(photos_dir) = photos_dir {
        let photos = Path::new(photos_dir)
            .canonicalize()
            .or_else(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    resolve_missing_path(Path::new(photos_dir))
                } else {
                    Err(error)
                }
            })
            .map_err(|e| format!("photo directory: {e}"))?;
        if root.starts_with(&photos) || photos.starts_with(&root) {
            return Err("card storage must be separate from the photo library".into());
        }
    }
    Ok(root)
}

fn resolve_missing_path(path: &Path) -> std::io::Result<PathBuf> {
    let mut remaining = std::path::absolute(path)?;
    let mut resolved = PathBuf::new();
    let mut links = 0;
    loop {
        let mut components = remaining.components();
        let Some(component) = components.next() else {
            return Ok(resolved);
        };
        let tail = components.as_path().to_path_buf();
        match component {
            Component::Normal(name) => {
                resolved.push(name);
                match std::fs::symlink_metadata(&resolved) {
                    Ok(metadata) if metadata.file_type().is_symlink() => {
                        links += 1;
                        if links > 40 {
                            return Err(std::io::Error::other("too many photo directory symlinks"));
                        }
                        let target = std::fs::read_link(&resolved)?;
                        resolved.pop();
                        remaining = target.join(tail);
                        continue;
                    }
                    Ok(metadata) if !metadata.is_dir() => {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::NotADirectory,
                            "photo directory path contains a non-directory",
                        ));
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
            }
            Component::ParentDir => {
                resolved.pop();
            }
            Component::CurDir => {}
            Component::RootDir | Component::Prefix(_) => resolved.push(component.as_os_str()),
        }
        remaining = tail;
    }
}

pub fn parse_name(name: &str) -> Result<String, &'static str> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Card name is required.");
    }
    Ok(name.to_string())
}

fn from_row(row: &rusqlite::Row) -> rusqlite::Result<Card> {
    Ok(Card {
        id: row.get(0)?,
        name: row.get(1)?,
        path: row.get(2)?,
        mime: row.get(3)?,
        sha256: row.get(4)?,
    })
}

pub fn list(conn: &Connection) -> rusqlite::Result<Vec<Card>> {
    conn.prepare("SELECT id, name, path, mime, sha256 FROM cards ORDER BY name COLLATE NOCASE, id")?
        .query_map([], from_row)?
        .collect()
}

pub fn get(conn: &Connection, id: i64) -> rusqlite::Result<Option<Card>> {
    conn.query_row(
        "SELECT id, name, path, mime, sha256 FROM cards WHERE id = ?1",
        [id],
        from_row,
    )
    .optional()
}

pub fn rename(conn: &Connection, id: i64, name: &str) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE cards SET name = ?1 WHERE id = ?2",
        params![name, id],
    )
}

pub fn get_back(conn: &Connection, id: i64) -> rusqlite::Result<Option<CardImage>> {
    conn.query_row(
        "SELECT path, mime, sha256 FROM card_backs WHERE card_id = ?1",
        [id],
        |row| {
            Ok(CardImage {
                path: row.get(0)?,
                mime: row.get(1)?,
                sha256: row.get(2)?,
            })
        },
    )
    .optional()
}

pub fn delete(conn: &Connection, root: &Path, id: i64) -> Result<bool, String> {
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    let Some(card) = get(&tx, id).map_err(|e| e.to_string())? else {
        return Ok(false);
    };
    let back = get_back(&tx, id).map_err(|e| e.to_string())?;
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let mut paths = Vec::new();
    for filename in
        std::iter::once(card.path.as_str()).chain(back.as_ref().map(|b| b.path.as_str()))
    {
        let mut components = Path::new(filename).components();
        if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
            return Err("invalid card image path".into());
        }
        let path = root.join(filename);
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() || metadata.file_type().is_symlink() => {
                paths.push(path)
            }
            Ok(_) => return Err("card image path is not a file".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    if tx
        .execute("DELETE FROM cards WHERE id = ?1", [id])
        .map_err(|e| e.to_string())?
        != 1
    {
        return Ok(false);
    }
    let mut staged_files = Vec::new();
    let result = (|| {
        for path in paths {
            let staged = root.join(format!(".card-delete-{:032x}.tmp", rand::random::<u128>()));
            match std::fs::symlink_metadata(&staged) {
                Ok(_) => return Err("temporary card image path already exists".to_string()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.to_string()),
            }
            std::fs::rename(&path, &staged).map_err(|e| e.to_string())?;
            staged_files.push((path, staged));
        }
        tx.commit().map_err(|e| e.to_string())
    })();
    if let Err(mut error) = result {
        for (path, staged) in staged_files.iter().rev() {
            if let Err(restore_error) = std::fs::rename(staged, path) {
                error.push_str(&format!(
                    "; restoring card image {} failed: {restore_error}",
                    path.display()
                ));
            }
        }
        return Err(error);
    }
    for (_, staged) in staged_files {
        if let Err(error) = std::fs::remove_file(&staged) {
            crate::log::error(&format!(
                "remove staged card image {}: {error}",
                staged.display()
            ));
        }
    }
    Ok(true)
}

pub fn upload(
    conn: &Connection,
    root: &Path,
    name: &str,
    bytes: &[u8],
    now: i64,
) -> Result<i64, UploadError> {
    let name = parse_name(name).map_err(UploadError::Invalid)?;
    save_image(root, bytes, |image| {
        let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO cards (name, path, mime, sha256, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![name, image.path, image.mime, image.sha256, now],
        )?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        Ok(id)
    })
}

pub fn upload_back(
    conn: &Connection,
    root: &Path,
    id: i64,
    bytes: &[u8],
    now: i64,
) -> Result<i64, UploadError> {
    save_image(root, bytes, |image| {
        let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
        if get(&tx, id)?.is_none() {
            return Err(UploadError::NotFound);
        }
        if get_back(&tx, id)?.is_some() {
            return Err(UploadError::BackExists);
        }
        tx.execute(
            "INSERT INTO card_backs (card_id, path, mime, sha256, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, image.path, image.mime, image.sha256, now],
        )?;
        tx.commit()?;
        Ok(id)
    })
}

fn save_image(
    root: &Path,
    bytes: &[u8],
    save: impl FnOnce(&CardImage) -> Result<i64, UploadError>,
) -> Result<i64, UploadError> {
    if bytes.len() > MAX_PHOTO_SIZE {
        return Err(UploadError::Invalid("Card image exceeds 25 MB."));
    }
    let (ext, mime) = match image::guess_format(bytes).ok() {
        Some(image::ImageFormat::Jpeg) => ("jpg", "image/jpeg"),
        Some(image::ImageFormat::Png) => ("png", "image/png"),
        Some(image::ImageFormat::Gif) => ("gif", "image/gif"),
        Some(image::ImageFormat::WebP) => ("webp", "image/webp"),
        _ => {
            return Err(UploadError::Invalid(
                "Choose a JPEG, PNG, GIF, or WebP image. HEIC and PDF files are not supported.",
            ));
        }
    };
    images::validate(bytes)
        .map_err(|_| UploadError::Invalid("The card image is damaged or too large to decode."))?;
    let hash = format!("{:x}", Sha256::digest(bytes));
    let filename = format!("{:032x}.{ext}", rand::random::<u128>());
    let path = root.join(&filename);
    let temporary = root.join(format!(".{filename}.tmp"));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)?;
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::hard_link(&temporary, &path)?;
        let saved = save(&CardImage {
            path: filename,
            mime: mime.into(),
            sha256: hash,
        });
        if saved.is_err() {
            remove_upload_file(&path);
        }
        saved
    })();
    remove_upload_file(&temporary);
    result
}

fn remove_upload_file(path: &Path) {
    if let Err(e) = std::fs::remove_file(path) {
        crate::log::error(&format!("remove card upload file {}: {e}", path.display()));
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::db;
    use std::{fs, io::Cursor};

    pub fn temp_dir() -> PathBuf {
        let root = std::env::temp_dir().join(format!("willcrock-cards-{}", rand::random::<u64>()));
        fs::create_dir(&root).unwrap();
        root
    }

    pub fn image(format: image::ImageFormat) -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(80, 40)
            .write_to(&mut bytes, format)
            .unwrap();
        bytes.into_inner()
    }

    #[test]
    fn back_is_optional_persistent_and_limited_to_one() {
        let root = temp_dir();
        let db_path = root.join("test.sqlite");
        let mut conn = db::open(db_path.to_str().unwrap()).unwrap();
        db::migrate(&mut conn).unwrap();
        let front = image(image::ImageFormat::Png);
        let back = image(image::ImageFormat::Jpeg);
        let id = upload(&conn, &root, "Library", &front, 100).unwrap();
        assert!(get_back(&conn, id).unwrap().is_none());
        assert_eq!(upload_back(&conn, &root, id, &back, 101).unwrap(), id);
        let count = fs::read_dir(&root).unwrap().count();
        assert!(matches!(
            upload_back(&conn, &root, id, &front, 102),
            Err(UploadError::BackExists)
        ));
        assert!(matches!(
            upload_back(&conn, &root, 999, &front, 102),
            Err(UploadError::NotFound)
        ));
        assert_eq!(fs::read_dir(&root).unwrap().count(), count);
        drop(conn);
        let conn = db::open(db_path.to_str().unwrap()).unwrap();
        let card = get(&conn, id).unwrap().unwrap();
        let saved_back = get_back(&conn, id).unwrap().unwrap();
        assert_eq!(list(&conn).unwrap().len(), 1);
        assert_eq!(fs::read(root.join(&card.path)).unwrap(), front);
        assert_eq!(saved_back.mime, "image/jpeg");
        assert_eq!(fs::read(root.join(&saved_back.path)).unwrap(), back);
        assert!(delete(&conn, &root, id).unwrap());
        assert!(get_back(&conn, id).unwrap().is_none());
        assert!(!root.join(card.path).exists());
        assert!(!root.join(saved_back.path).exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn back_upload_rejects_bad_images_and_cleans_up_database_failures() {
        let root = temp_dir();
        let conn = db::open_memory();
        let png = image(image::ImageFormat::Png);
        let id = upload(&conn, &root, "Card", &png, 100).unwrap();
        for bytes in [
            b"%PDF-1.0".as_slice(),
            &[0xff, 0xd8, 0xff],
            &vec![0; MAX_PHOTO_SIZE + 1],
        ] {
            assert!(matches!(
                upload_back(&conn, &root, id, bytes, 100),
                Err(UploadError::Invalid(_))
            ));
        }
        conn.execute_batch("CREATE TRIGGER fail_back BEFORE INSERT ON card_backs BEGIN SELECT RAISE(FAIL, 'test failure'); END;").unwrap();
        assert!(matches!(
            upload_back(&conn, &root, id, &png, 100),
            Err(UploadError::Failed(_))
        ));
        assert!(get_back(&conn, id).unwrap().is_none());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_back_uploads_cannot_replace_each_other() {
        let root = temp_dir();
        let db_path = root.join("test.sqlite");
        let mut conn = db::open(db_path.to_str().unwrap()).unwrap();
        db::migrate(&mut conn).unwrap();
        let png = image(image::ImageFormat::Png);
        let id = upload(&conn, &root, "Card", &png, 100).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let threads: Vec<_> = (0..2)
            .map(|_| {
                let root = root.clone();
                let db_path = db_path.clone();
                let png = png.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let conn = db::open(db_path.to_str().unwrap()).unwrap();
                    barrier.wait();
                    upload_back(&conn, &root, id, &png, 100)
                })
            })
            .collect();
        let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|r| matches!(r, Err(UploadError::BackExists)))
                .count(),
            1
        );
        assert_eq!(fs::read_dir(&root).unwrap().count(), 3);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn uploads_are_shared_persistent_and_independent_of_photos() {
        let root = temp_dir();
        let db_path = root.join("test.sqlite");
        let mut conn = db::open(db_path.to_str().unwrap()).unwrap();
        db::migrate(&mut conn).unwrap();
        let png = image(image::ImageFormat::Png);
        let id = upload(&conn, &root, "  Library  ", &png, 100).unwrap();
        let duplicate = upload(&conn, &root, "Library", &png, 101).unwrap();
        let alpha = upload(&conn, &root, "aaa", &png, 102).unwrap();
        assert_ne!(id, duplicate);
        assert_eq!(
            list(&conn)
                .unwrap()
                .iter()
                .map(|c| c.id)
                .collect::<Vec<_>>(),
            [alpha, id, duplicate]
        );
        assert_eq!(rename(&conn, alpha, "library").unwrap(), 1);
        assert_eq!(rename(&conn, 999, "Missing").unwrap(), 0);
        assert!(get(&conn, 999).unwrap().is_none());
        drop(conn);
        let conn = db::open(db_path.to_str().unwrap()).unwrap();
        let card = get(&conn, id).unwrap().unwrap();
        assert_eq!(card.name, "Library");
        assert_eq!(card.mime, "image/png");
        assert_eq!(fs::read(root.join(&card.path)).unwrap(), png);
        assert!(!card.path.contains("Library"));
        assert_eq!(list(&conn).unwrap().len(), 3);
        for table in ["photos", "photo_tags", "tags"] {
            let count: i64 = conn
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 0);
        }
        assert!(crate::photos::unsynced(&conn).unwrap().is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn validates_formats_names_and_decoding_before_writing() {
        let root = temp_dir();
        let conn = db::open_memory();
        assert_eq!(parse_name(" \t "), Err("Card name is required."));
        for format in [
            image::ImageFormat::Jpeg,
            image::ImageFormat::Png,
            image::ImageFormat::Gif,
            image::ImageFormat::WebP,
        ] {
            let bytes = image(format);
            upload(&conn, &root, "Card", &bytes, 100).unwrap();
        }
        let count = fs::read_dir(&root).unwrap().count();
        let mut oversized = image(image::ImageFormat::Png);
        oversized.resize(MAX_PHOTO_SIZE + 1, 0);
        for bytes in [
            b"%PDF-1.0".as_slice(),
            b"\0\0\0\x14ftypheic\0\0\0\0mif1",
            &[0xff, 0xd8, 0xff],
            b"not an image",
            &oversized,
        ] {
            assert!(matches!(
                upload(&conn, &root, "Card", bytes, 100),
                Err(UploadError::Invalid(_))
            ));
        }
        assert!(matches!(
            upload(&conn, &root, " ", &image(image::ImageFormat::Png), 100),
            Err(UploadError::Invalid(_))
        ));
        assert_eq!(fs::read_dir(&root).unwrap().count(), count);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rolls_back_files_on_database_failure() {
        let root = temp_dir();
        let conn = db::open_memory();
        conn.execute_batch("CREATE TRIGGER fail_card BEFORE INSERT ON cards BEGIN SELECT RAISE(FAIL, 'test failure'); END;").unwrap();
        assert!(matches!(
            upload(&conn, &root, "Card", &image(image::ImageFormat::Png), 100),
            Err(UploadError::Failed(_))
        ));
        assert!(list(&conn).unwrap().is_empty());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn delete_removes_card_record_and_image() {
        let root = temp_dir();
        let conn = db::open_memory();
        let id = upload(&conn, &root, "Card", &image(image::ImageFormat::Png), 100).unwrap();
        let path = root.join(get(&conn, id).unwrap().unwrap().path);

        assert!(delete(&conn, &root, id).unwrap());
        assert!(get(&conn, id).unwrap().is_none());
        assert!(!path.exists());
        assert!(!delete(&conn, &root, id).unwrap());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn delete_restores_image_when_database_commit_fails() {
        let root = temp_dir();
        let db_path = root.join("test.sqlite");
        let writer = db::open(db_path.to_str().unwrap()).unwrap();
        let mut writer = writer;
        db::migrate(&mut writer).unwrap();
        let id = upload(&writer, &root, "Card", &image(image::ImageFormat::Png), 100).unwrap();
        let card = get(&writer, id).unwrap().unwrap();
        let path = root.join(&card.path);
        let back_bytes = image(image::ImageFormat::Jpeg);
        upload_back(&writer, &root, id, &back_bytes, 101).unwrap();
        let back_path = root.join(get_back(&writer, id).unwrap().unwrap().path);
        let reader = db::open(db_path.to_str().unwrap()).unwrap();
        let reader_tx =
            rusqlite::Transaction::new_unchecked(&reader, TransactionBehavior::Deferred).unwrap();
        reader_tx
            .query_row("SELECT id FROM cards WHERE id = ?1", [id], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap();
        writer.busy_timeout(std::time::Duration::ZERO).unwrap();

        assert!(delete(&writer, &root, id).is_err());
        assert_eq!(get(&writer, id).unwrap().unwrap().path, card.path);
        assert!(path.is_file());
        assert_eq!(fs::read(&back_path).unwrap(), back_bytes);
        assert!(get_back(&writer, id).unwrap().is_some());
        assert_eq!(
            fs::read_dir(&root)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".card-delete-"))
                .count(),
            0
        );

        drop(reader_tx);
        drop(reader);
        drop(writer);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn delete_validates_back_before_moving_front() {
        let root = temp_dir();
        let conn = db::open_memory();
        let png = image(image::ImageFormat::Png);
        let id = upload(&conn, &root, "Card", &png, 100).unwrap();
        upload_back(&conn, &root, id, &png, 101).unwrap();
        let front = root.join(get(&conn, id).unwrap().unwrap().path);
        conn.execute(
            "UPDATE card_backs SET path = '../outside.png' WHERE card_id = ?1",
            [id],
        )
        .unwrap();
        assert!(delete(&conn, &root, id).is_err());
        assert_eq!(fs::read(front).unwrap(), png);
        assert!(get(&conn, id).unwrap().is_some());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn delete_rejects_paths_outside_card_storage() {
        let root = temp_dir();
        let outside = temp_dir();
        let conn = db::open_memory();
        let id = upload(&conn, &root, "Card", &image(image::ImageFormat::Png), 100).unwrap();
        let secret = outside.join("outside.png");
        fs::write(&secret, b"private").unwrap();
        conn.execute(
            "UPDATE cards SET path = ?1 WHERE id = ?2",
            params![secret.to_string_lossy(), id],
        )
        .unwrap();

        assert!(delete(&conn, &root, id).is_err());
        assert!(get(&conn, id).unwrap().is_some());
        assert_eq!(fs::read(secret).unwrap(), b"private");

        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }

    #[test]
    fn requires_separate_storage_even_through_symlinks() {
        let root = temp_dir();
        let photos = root.join("photos");
        let cards = root.join("cards");
        fs::create_dir(&photos).unwrap();
        fs::create_dir(&cards).unwrap();
        let nested = photos.join("cards");
        fs::create_dir(&nested).unwrap();
        let photos_dir = Some(photos.to_str().unwrap());
        assert!(storage_root(cards.to_str().unwrap(), photos_dir).is_ok());
        assert!(storage_root(cards.to_str().unwrap(), None).is_ok());
        assert!(storage_root(photos.to_str().unwrap(), photos_dir).is_err());
        assert!(storage_root(nested.to_str().unwrap(), photos_dir).is_err());
        assert!(storage_root(root.to_str().unwrap(), photos_dir).is_err());
        let link = root.join("link");
        std::os::unix::fs::symlink(&nested, &link).unwrap();
        assert!(storage_root(link.to_str().unwrap(), photos_dir).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_photos_storage_still_checks_resolved_overlap() {
        let root = temp_dir();
        let cards = root.join("cards");
        fs::create_dir(&cards).unwrap();
        let missing = root.join("missing/photos");
        assert!(storage_root(cards.to_str().unwrap(), Some(missing.to_str().unwrap())).is_ok());
        assert!(!missing.exists());
        let nested = cards.join("missing/photos");
        assert!(storage_root(cards.to_str().unwrap(), Some(nested.to_str().unwrap())).is_err());
        let alias = root.join("alias");
        std::os::unix::fs::symlink("cards", &alias).unwrap();
        let nested_alias = alias.join("missing/photos");
        assert!(storage_root(
            cards.to_str().unwrap(),
            Some(nested_alias.to_str().unwrap())
        )
        .is_err());
        let dangling = root.join("dangling");
        std::os::unix::fs::symlink("cards/not-restored", &dangling).unwrap();
        assert!(storage_root(cards.to_str().unwrap(), Some(dangling.to_str().unwrap())).is_err());
        let missing_alias = root.join("missing/../alias/photos");
        assert!(storage_root(
            cards.to_str().unwrap(),
            Some(missing_alias.to_str().unwrap())
        )
        .is_err());
        let separate_link = root.join("separate-link");
        std::os::unix::fs::symlink("missing/photos", &separate_link).unwrap();
        assert!(storage_root(
            cards.to_str().unwrap(),
            Some(separate_link.to_str().unwrap())
        )
        .is_ok());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn photos_resolution_does_not_ignore_non_missing_errors() {
        let root = temp_dir();
        let cards = root.join("cards");
        fs::create_dir(&cards).unwrap();
        let file = root.join("not-a-directory");
        fs::write(&file, b"file").unwrap();
        let invalid = file.join("photos");
        assert!(storage_root(cards.to_str().unwrap(), Some(invalid.to_str().unwrap())).is_err());
        let cycle = root.join("cycle");
        std::os::unix::fs::symlink("missing/../cycle", &cycle).unwrap();
        assert!(storage_root(cards.to_str().unwrap(), Some(cycle.to_str().unwrap())).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
