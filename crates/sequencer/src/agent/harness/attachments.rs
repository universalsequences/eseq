//! Images the user drops on the agent pane, for their next message. Each is
//! copied into a staging folder (a screenshot's temporary file goes away),
//! shrunk on the way to what the model looks at, and read back as an image
//! block when the message is sent.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use image::ImageFormat;
use serde_json::Value;

/// The formats the pane takes: what the API reads and the pane's `image`
/// widget can draw.
const SENDABLE: [ImageFormat; 2] = [ImageFormat::Png, ImageFormat::Jpeg];
/// The API refuses an image over 5 MB of base64, about 3.75 MB of file.
const MAX_BYTES: usize = 3_750_000;
/// The long side an image is shrunk to: the model sees no more than this,
/// and the thumbnail's texture stays small.
const MAX_SIDE: u32 = 2048;

fn sendable_format(bytes: &[u8]) -> Option<ImageFormat> {
    image::guess_format(bytes).ok().filter(|format| SENDABLE.contains(format))
}

/// Whether `path` names an image the pane takes, by its extension: what a
/// drop routes on before reading anything.
pub fn is_image_path(path: &Path) -> bool {
    ImageFormat::from_path(path).is_ok_and(|format| SENDABLE.contains(&format))
}

fn encode(image: &image::DynamicImage, format: ImageFormat) -> Result<Vec<u8>, String> {
    let mut out = std::io::Cursor::new(Vec::new());
    image.write_to(&mut out, format).map(|()| out.into_inner()).map_err(|error| error.to_string())
}

/// The image as it will be sent, with its format: as it is when small
/// enough, else shrunk to [`MAX_SIDE`] in its own format (a JPEG when a PNG
/// is still too big).
fn sendable(bytes: Vec<u8>) -> Result<(Vec<u8>, ImageFormat), String> {
    let format = sendable_format(&bytes).ok_or("not a PNG or JPEG image")?;
    let decoded = image::load_from_memory_with_format(&bytes, format).map_err(|error| error.to_string())?;
    if bytes.len() <= MAX_BYTES && decoded.width().max(decoded.height()) <= MAX_SIDE {
        return Ok((bytes, format));
    }
    let shrunk = decoded.resize(MAX_SIDE, MAX_SIDE, image::imageops::FilterType::Triangle);
    if format == ImageFormat::Png {
        let png = encode(&shrunk, ImageFormat::Png)?;
        if png.len() <= MAX_BYTES {
            return Ok((png, ImageFormat::Png));
        }
    }
    let jpeg = encode(&image::DynamicImage::ImageRgb8(shrunk.to_rgb8()), ImageFormat::Jpeg)?;
    if jpeg.len() <= MAX_BYTES { Ok((jpeg, ImageFormat::Jpeg)) } else { Err("too big to send, even shrunk".to_string()) }
}

/// Copy the image at `source` into `dir`, ready to send, and return the copy.
pub fn stage(source: &Path, dir: &Path) -> Result<PathBuf, String> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let name = source.file_name().map_or_else(|| "image".into(), |name| name.to_string_lossy());
    let fail = |why: String| format!("{name}: {why}");
    let bytes = std::fs::read(source).map_err(|error| fail(error.to_string()))?;
    let (bytes, format) = sendable(bytes).map_err(fail)?;
    std::fs::create_dir_all(dir).map_err(|error| fail(error.to_string()))?;
    let millis = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_millis());
    let extension = format.extensions_str()[0];
    let path = dir.join(format!("{millis}-{}.{extension}", NEXT.fetch_add(1, Ordering::Relaxed)));
    std::fs::write(&path, bytes).map_err(|error| fail(error.to_string()))?;
    Ok(path)
}

/// A staged image as a Messages API image block.
pub fn image_block(path: &Path) -> Result<Value, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let format = sendable_format(&bytes).ok_or_else(|| format!("{} is not a PNG or JPEG image", path.display()))?;
    Ok(super::mcp_client::image_block(format.to_mime_type(), &bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbImage::from_fn(width, height, |x, y| image::Rgb([(x * 7) as u8, (y * 13) as u8, ((x ^ y) * 31) as u8]));
        encode(&image::DynamicImage::ImageRgb8(image), ImageFormat::Png).unwrap()
    }

    #[test]
    fn a_small_image_is_staged_as_it_is_and_sent_as_a_base64_block() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("Screenshot 1.PNG");
        let bytes = png(4, 3);
        std::fs::write(&source, &bytes).unwrap();
        assert!(is_image_path(&source));
        let staged = stage(&source, &dir.path().join("attachments")).unwrap();
        assert_eq!(std::fs::read(&staged).unwrap(), bytes);
        assert_eq!(staged.extension().unwrap(), "png");
        let block = image_block(&staged).unwrap();
        assert_eq!(block["source"]["media_type"], "image/png");
        assert!(!block["source"]["data"].as_str().unwrap().is_empty());
    }

    #[test]
    fn an_image_wider_than_the_model_sees_is_shrunk() {
        let (bytes, format) = sendable(png(MAX_SIDE + 2, 4)).unwrap();
        assert_eq!(format, ImageFormat::Png);
        assert_eq!(image::load_from_memory(&bytes).unwrap().width(), MAX_SIDE);
    }

    #[test]
    fn what_is_not_a_png_or_jpeg_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("fake.png");
        std::fs::write(&source, b"not an image").unwrap();
        let error = stage(&source, dir.path()).unwrap_err();
        assert!(error.starts_with("fake.png: not a PNG"), "{error}");
        assert!(!is_image_path(Path::new("kick.wav")));
        assert!(!is_image_path(Path::new("loop.gif")));
    }
}
