use std::path::{Path, PathBuf};

use base64::Engine as _;
use hone_proto::{Content, ContentPart};

/// Build a multimodal Content from one or more image paths plus a text prompt.
///
/// Behavior:
///   * If `image_paths` is empty, returns `Content::Text(prompt.into())`.
///   * Otherwise, returns `Content::Parts(vec![Image(p1), Image(p2), ..., Text(prompt)])`
///     in that order (images before text).
///   * Each image is read from disk, base64-encoded, and the media_type is
///     guessed from the file extension via `guess_media_type`.
///   * Errors: missing file -> Err(ImageError::Io). Unsupported extension ->
///     Err(ImageError::Unsupported).
pub fn build_content(
    image_paths: &[PathBuf],
    prompt: &str,
) -> Result<Content, ImageError> {
    if image_paths.is_empty() {
        return Ok(Content::Text(prompt.to_string()));
    }

    // Validate ALL extensions before any I/O.
    let media_types: Vec<&'static str> = image_paths
        .iter()
        .map(|p| guess_media_type(p))
        .collect::<Result<_, _>>()?;

    let mut parts: Vec<ContentPart> = Vec::with_capacity(image_paths.len() + 1);
    for (path, media_type) in image_paths.iter().zip(media_types) {
        let bytes =
            std::fs::read(path).map_err(|source| ImageError::Io { path: path.to_path_buf(), source })?;
        let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
        parts.push(ContentPart::Image {
            media_type: media_type.to_string(),
            data,
        });
    }
    parts.push(ContentPart::Text { text: prompt.to_string() });

    Ok(Content::Parts(parts))
}

/// Map a file extension to an IANA media type. Returns Err for unsupported.
/// Supported extensions (case-insensitive):
///   png -> image/png
///   jpg, jpeg -> image/jpeg
///   gif -> image/gif
///   webp -> image/webp
pub fn guess_media_type(path: &Path) -> Result<&'static str, ImageError> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());

    match ext.as_deref() {
        Some("png") => Ok("image/png"),
        Some("jpg") | Some("jpeg") => Ok("image/jpeg"),
        Some("gif") => Ok("image/gif"),
        Some("webp") => Ok("image/webp"),
        _ => Err(ImageError::Unsupported { path: path.to_path_buf() }),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ImageError {
    #[error("io reading {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("unsupported image extension for {path} (supported: png, jpg, jpeg, gif, webp)")]
    Unsupported { path: PathBuf },
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use std::io::Write as _;
    use tempfile::TempDir;

    // ---------------------------------------------------------------------------
    // guess_media_type
    // ---------------------------------------------------------------------------

    #[test]
    fn guess_png_returns_image_png() {
        let p = PathBuf::from("photo.png");
        assert_eq!(guess_media_type(&p).unwrap(), "image/png");
    }

    #[test]
    fn guess_jpg_and_jpeg_both_return_image_jpeg() {
        assert_eq!(
            guess_media_type(&PathBuf::from("a.jpg")).unwrap(),
            "image/jpeg"
        );
        assert_eq!(
            guess_media_type(&PathBuf::from("b.jpeg")).unwrap(),
            "image/jpeg"
        );
    }

    #[test]
    fn guess_gif_returns_image_gif() {
        assert_eq!(
            guess_media_type(&PathBuf::from("anim.gif")).unwrap(),
            "image/gif"
        );
    }

    #[test]
    fn guess_webp_returns_image_webp() {
        assert_eq!(
            guess_media_type(&PathBuf::from("img.webp")).unwrap(),
            "image/webp"
        );
    }

    #[test]
    fn guess_uppercase_extension_works() {
        // Foo.PNG should resolve to image/png
        let p = PathBuf::from("Foo.PNG");
        assert_eq!(guess_media_type(&p).unwrap(), "image/png");
    }

    #[test]
    fn guess_no_extension_returns_unsupported() {
        let p = PathBuf::from("Makefile");
        assert!(
            matches!(guess_media_type(&p), Err(ImageError::Unsupported { .. })),
            "expected Unsupported, got {:?}",
            guess_media_type(&p)
        );
    }

    #[test]
    fn guess_unknown_extension_returns_unsupported() {
        let p = PathBuf::from("cat.bmp");
        assert!(
            matches!(guess_media_type(&p), Err(ImageError::Unsupported { .. })),
            "expected Unsupported, got {:?}",
            guess_media_type(&p)
        );
    }

    // ---------------------------------------------------------------------------
    // build_content helpers
    // ---------------------------------------------------------------------------

    fn write_temp_file(dir: &TempDir, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.path().join(name);
        let mut f = std::fs::File::create(&path).expect("create temp file");
        f.write_all(bytes).expect("write temp file");
        path
    }

    // ---------------------------------------------------------------------------
    // build_content
    // ---------------------------------------------------------------------------

    #[test]
    fn build_content_with_no_images_returns_text() {
        let result = build_content(&[], "hello").unwrap();
        assert_eq!(result, Content::Text("hello".into()));
    }

    #[test]
    fn build_content_with_one_image_returns_parts_image_then_text() {
        let dir = TempDir::new().unwrap();
        let img_path = write_temp_file(&dir, "cat.png", b"\x89PNG\r\n");

        let result = build_content(&[img_path], "describe").unwrap();

        let Content::Parts(parts) = result else {
            panic!("expected Content::Parts, got {:?}", result);
        };
        assert_eq!(parts.len(), 2);
        assert!(
            matches!(&parts[0], ContentPart::Image { .. }),
            "first part must be Image"
        );
        assert_eq!(
            parts[1],
            ContentPart::Text {
                text: "describe".into()
            }
        );
    }

    #[test]
    fn build_content_with_multiple_images_preserves_order() {
        let dir = TempDir::new().unwrap();
        let a = write_temp_file(&dir, "a.png", b"png-bytes");
        let b = write_temp_file(&dir, "b.jpg", b"jpg-bytes");
        let c = write_temp_file(&dir, "c.gif", b"gif-bytes");

        let result = build_content(&[a, b, c], "go").unwrap();

        let Content::Parts(parts) = result else {
            panic!("expected Content::Parts");
        };
        assert_eq!(parts.len(), 4);

        assert!(
            matches!(&parts[0], ContentPart::Image { media_type, .. } if media_type == "image/png"),
            "part[0] must be image/png, got {:?}",
            parts[0]
        );
        assert!(
            matches!(&parts[1], ContentPart::Image { media_type, .. } if media_type == "image/jpeg"),
            "part[1] must be image/jpeg, got {:?}",
            parts[1]
        );
        assert!(
            matches!(&parts[2], ContentPart::Image { media_type, .. } if media_type == "image/gif"),
            "part[2] must be image/gif, got {:?}",
            parts[2]
        );
        assert_eq!(
            parts[3],
            ContentPart::Text { text: "go".into() }
        );
    }

    #[test]
    fn build_content_image_data_is_base64_of_file_bytes() {
        let dir = TempDir::new().unwrap();
        let known = b"abc";
        let path = write_temp_file(&dir, "test.png", known);

        let result = build_content(&[path], "prompt").unwrap();

        let Content::Parts(parts) = result else {
            panic!("expected Content::Parts");
        };
        let ContentPart::Image { data, .. } = &parts[0] else {
            panic!("expected Image part");
        };
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(data)
            .expect("valid base64");
        assert_eq!(decoded.as_slice(), known);
    }

    #[test]
    fn build_content_image_media_type_matches_extension() {
        let dir = TempDir::new().unwrap();
        let path = write_temp_file(&dir, "picture.webp", b"webp-data");

        let result = build_content(&[path], "look").unwrap();

        let Content::Parts(parts) = result else {
            panic!("expected Content::Parts");
        };
        assert!(
            matches!(&parts[0], ContentPart::Image { media_type, .. } if media_type == "image/webp"),
            "expected image/webp, got {:?}",
            parts[0]
        );
    }

    #[test]
    fn build_content_missing_file_returns_io_error() {
        let path = PathBuf::from("/tmp/this_file_does_not_exist_hone_test_12345.png");
        let result = build_content(&[path], "prompt");
        assert!(
            matches!(result, Err(ImageError::Io { .. })),
            "expected Io error, got {:?}",
            result
        );
    }

    #[test]
    fn build_content_unsupported_extension_errors_before_reading_file() {
        // File does NOT exist — extension check must fire before any I/O.
        let path = PathBuf::from("/tmp/nonexistent_hone_test_12345.bmp");
        let result = build_content(&[path], "prompt");
        assert!(
            matches!(result, Err(ImageError::Unsupported { .. })),
            "expected Unsupported error before I/O, got {:?}",
            result
        );
    }

    #[test]
    fn build_content_empty_prompt_with_image_emits_empty_text_part() {
        let dir = TempDir::new().unwrap();
        let path = write_temp_file(&dir, "img.png", b"data");

        let result = build_content(&[path], "").unwrap();

        let Content::Parts(parts) = result else {
            panic!("expected Content::Parts");
        };
        assert_eq!(parts.len(), 2);
        assert!(matches!(&parts[0], ContentPart::Image { .. }));
        assert_eq!(
            parts[1],
            ContentPart::Text { text: "".into() }
        );
    }
}
