use super::Photo;
use image::{codecs::jpeg::JpegEncoder, ImageDecoder, ImageReader};
use std::fs::{self, File};
use std::path::{Component, Path};

pub enum ImageKind {
    Thumbnail,
    Preview,
    Original,
}

pub fn image_file(
    root: &Path,
    photo: &Photo,
    sha256: &str,
    kind: ImageKind,
) -> Result<Option<File>, String> {
    let result = || -> Result<Option<File>, Box<dyn std::error::Error>> {
        if !Path::new(&photo.path)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
        {
            return Ok(None);
        }
        let root = root.canonicalize()?;
        let source = match root.join(&photo.path).canonicalize() {
            Ok(path) => path,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        if !source.starts_with(&root) || !source.is_file() {
            return Ok(None);
        }
        let size = match kind {
            ImageKind::Original => return Ok(Some(File::open(source)?)),
            ImageKind::Thumbnail => 400,
            ImageKind::Preview => 1600,
        };
        if !photo.can_preview() {
            return Ok(None);
        }
        if sha256.len() != 64 || !sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("invalid photo hash".into());
        }
        // Hidden so scans and library syncs never mistake generated images for originals.
        let cache_dir = root.join(".gallery-v1");
        fs::create_dir_all(&cache_dir)?;
        let cache = cache_dir.join(format!("{sha256}-{size}.jpg"));
        match File::open(&cache) {
            Ok(file) => return Ok(Some(file)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        let mut decoder = ImageReader::open(source)?
            .with_guessed_format()?
            .into_decoder()?;
        let orientation = decoder.orientation()?;
        let mut image = image::DynamicImage::from_decoder(decoder)?;
        image.apply_orientation(orientation);
        let image = image.thumbnail(size, size).to_rgb8();
        let mut bytes = Vec::new();
        JpegEncoder::new_with_quality(&mut bytes, 85).encode_image(&image)?;
        // Publish only complete JPEGs; concurrent requests may generate the same image.
        let temp = cache_dir.join(format!("{sha256}-{size}-{}.tmp", rand::random::<u64>()));
        let publish = || -> std::io::Result<()> {
            fs::write(&temp, &bytes)?;
            fs::rename(&temp, &cache)
        };
        if let Err(e) = publish() {
            if let Err(cleanup) = fs::remove_file(&temp) {
                if cleanup.kind() != std::io::ErrorKind::NotFound {
                    crate::log::error(&format!("remove thumbnail temp file: {cleanup}"));
                }
            }
            return Err(e.into());
        }
        Ok(Some(File::open(cache)?))
    };
    result().map_err(|e| format!("photo {}: {e}", photo.id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn generates_cached_previews_and_keeps_original_bytes() {
        let root = super::super::tests::temp_dir("previews");
        let source = root.join("portrait.png");
        image::RgbaImage::from_pixel(1000, 2000, image::Rgba([30, 60, 90, 128]))
            .save(&source)
            .unwrap();
        let photo = Photo {
            id: 1,
            path: "portrait.png".into(),
        };
        let hash = super::super::hash(&source).unwrap();
        let read = |kind| {
            let mut bytes = Vec::new();
            image_file(&root, &photo, &hash, kind)
                .unwrap()
                .unwrap()
                .read_to_end(&mut bytes)
                .unwrap();
            bytes
        };
        let thumbnail = read(ImageKind::Thumbnail);
        let decoded = image::load_from_memory(&thumbnail).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (200, 400));
        let preview = image::load_from_memory(&read(ImageKind::Preview)).unwrap();
        assert_eq!((preview.width(), preview.height()), (800, 1600));
        assert_eq!(read(ImageKind::Original), fs::read(&source).unwrap());
        // A cache hit must not decode the source again.
        fs::write(&source, b"not an image").unwrap();
        assert_eq!(read(ImageKind::Thumbnail), thumbnail);
        assert!(image_file(&root, &photo, &"a".repeat(64), ImageKind::Thumbnail).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refuses_paths_outside_library_and_handles_missing_and_heic() {
        let root = super::super::tests::temp_dir("private");
        let hash = "a".repeat(64);
        for path in ["../secret.jpg", "/etc/passwd", "missing.jpg"] {
            let photo = Photo {
                id: 1,
                path: path.into(),
            };
            assert!(image_file(&root, &photo, &hash, ImageKind::Original)
                .unwrap()
                .is_none());
        }
        std::os::unix::fs::symlink("/etc/passwd", root.join("escape.jpg")).unwrap();
        let photo = Photo {
            id: 1,
            path: "escape.jpg".into(),
        };
        assert!(image_file(&root, &photo, &hash, ImageKind::Original)
            .unwrap()
            .is_none());
        fs::write(root.join("phone.HEIC"), b"heic").unwrap();
        let photo = Photo {
            id: 2,
            path: "phone.HEIC".into(),
        };
        assert!(image_file(&root, &photo, &hash, ImageKind::Thumbnail)
            .unwrap()
            .is_none());
        assert!(image_file(&root, &photo, &hash, ImageKind::Original)
            .unwrap()
            .is_some());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn previews_apply_exif_orientation() {
        let root = super::super::tests::temp_dir("orientation");
        let mut jpeg = Vec::new();
        JpegEncoder::new(&mut jpeg)
            .encode_image(&image::RgbImage::new(120, 80))
            .unwrap();
        let exif = b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0\0\0\0\0";
        let mut oriented = vec![0xff, 0xd8, 0xff, 0xe1];
        oriented.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
        oriented.extend_from_slice(exif);
        oriented.extend_from_slice(&jpeg[2..]);
        fs::write(root.join("rotated.jpg"), oriented).unwrap();
        let photo = Photo {
            id: 1,
            path: "rotated.jpg".into(),
        };
        let mut bytes = Vec::new();
        image_file(&root, &photo, &"a".repeat(64), ImageKind::Thumbnail)
            .unwrap()
            .unwrap()
            .read_to_end(&mut bytes)
            .unwrap();
        let preview = image::load_from_memory(&bytes).unwrap();
        assert!(preview.height() > preview.width());
        fs::remove_dir_all(root).unwrap();
    }
}
