use image::{codecs::jpeg::JpegEncoder, ImageDecoder, ImageReader};
use std::fs::{self, File};
use std::io::Cursor;
use std::path::{Component, Path};

pub enum ImageKind {
    Thumbnail,
    Preview,
    Original,
}

fn limits() -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(20_000);
    limits.max_image_height = Some(20_000);
    limits.max_alloc = Some(256 * 1024 * 1024);
    limits
}

pub fn validate(bytes: &[u8]) -> image::ImageResult<()> {
    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    reader.limits(limits());
    reader.decode().map(|_| ())
}

pub fn image_file(
    root: &Path,
    path: &str,
    sha256: &str,
    kind: ImageKind,
) -> Result<Option<File>, String> {
    let result = || -> Result<Option<File>, Box<dyn std::error::Error>> {
        if path.is_empty()
            || !Path::new(path)
                .components()
                .all(|c| matches!(c, Component::Normal(_)))
        {
            return Ok(None);
        }
        let root = root.canonicalize()?;
        let source = match root.join(path).canonicalize() {
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
        if sha256.len() != 64 || !sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("invalid image hash".into());
        }
        let cache_dir = root.join(".gallery-v1");
        fs::create_dir_all(&cache_dir)?;
        let cache_dir = cache_dir.canonicalize()?;
        if !cache_dir.starts_with(&root) {
            return Err("image cache is outside storage directory".into());
        }
        let cache = cache_dir.join(format!("{sha256}-{size}.jpg"));
        match cache.canonicalize() {
            Ok(path) if path.starts_with(&cache_dir) => return Ok(Some(File::open(path)?)),
            Ok(_) => return Err("cached image is outside storage directory".into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        let mut reader = ImageReader::open(source)?.with_guessed_format()?;
        reader.limits(limits());
        let mut decoder = reader.into_decoder()?;
        let orientation = decoder.orientation()?;
        let mut image = image::DynamicImage::from_decoder(decoder)?;
        image.apply_orientation(orientation);
        let image = image.thumbnail(size, size).to_rgb8();
        let mut bytes = Vec::new();
        JpegEncoder::new_with_quality(&mut bytes, 85).encode_image(&image)?;
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
    result().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_limits_decoded_dimensions() {
        let mut bytes = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(20_001, 1)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        assert!(matches!(
            validate(bytes.get_ref()),
            Err(image::ImageError::Limits(_))
        ));
    }

    #[test]
    fn refuses_escaped_cache_directory_and_files() {
        let root = crate::cards::tests::temp_dir();
        let outside = crate::cards::tests::temp_dir();
        let hash = "a".repeat(64);
        fs::write(
            root.join("card.png"),
            crate::cards::tests::image(image::ImageFormat::Png),
        )
        .unwrap();
        let cache_dir = root.join(".gallery-v1");
        std::os::unix::fs::symlink(&outside, &cache_dir).unwrap();
        assert!(image_file(&root, "card.png", &hash, ImageKind::Thumbnail).is_err());
        fs::remove_file(&cache_dir).unwrap();
        fs::create_dir(&cache_dir).unwrap();
        let secret = outside.join("private.jpg");
        fs::write(&secret, b"private").unwrap();
        std::os::unix::fs::symlink(secret, cache_dir.join(format!("{hash}-400.jpg"))).unwrap();
        assert!(image_file(&root, "card.png", &hash, ImageKind::Thumbnail).is_err());
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }
}
