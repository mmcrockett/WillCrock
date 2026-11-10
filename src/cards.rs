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

pub fn upload(
    conn: &Connection,
    root: &Path,
    name: &str,
    bytes: &[u8],
    now: i64,
) -> Result<i64, UploadError> {
    let name = parse_name(name).map_err(UploadError::Invalid)?;
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
        let saved = (|| {
            let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
            tx.execute(
                "INSERT INTO cards (name, path, mime, sha256, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![name, filename, mime, hash, now],
            )?;
            let id = tx.last_insert_rowid();
            tx.commit()?;
            Ok::<_, UploadError>(id)
        })();
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
