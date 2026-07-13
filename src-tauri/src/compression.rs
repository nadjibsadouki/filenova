use image::{
    codecs::{
        jpeg::JpegEncoder,
        png::{CompressionType, FilterType, PngEncoder},
    },
    DynamicImage, ImageDecoder, ImageEncoder, ImageFormat, ImageReader,
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

const MIN_TARGET_QUALITY: u8 = 10;
const MAX_TARGET_QUALITY: u8 = 95;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    code: String,
    message: String,
}

impl CommandError {
    fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageInfo {
    path: String,
    name: String,
    format: String,
    width: u32,
    height: u32,
    size: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompressionRequest {
    path: String,
    destination_dir: Option<String>,
    save_in_new_folder: bool,
    mode: CompressionMode,
    quality: Option<u8>,
    target_bytes: Option<u64>,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
enum CompressionMode {
    Recommended,
    CustomQuality,
    TargetFileSize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompressionResult {
    success: bool,
    original_size: u64,
    new_size: Option<u64>,
    saved_bytes: Option<u64>,
    reduction_percent: Option<f64>,
    output_path: Option<String>,
    warning_code: Option<String>,
    error_code: Option<String>,
    error_message: Option<String>,
}

#[tauri::command]
pub fn inspect_image(path: String) -> Result<ImageInfo, CommandError> {
    inspect_image_path(Path::new(&path))
}

#[tauri::command]
pub fn compress_image(request: CompressionRequest) -> CompressionResult {
    let original_size = fs::metadata(&request.path).map(|m| m.len()).unwrap_or(0);
    match compress_image_inner(&request) {
        Ok((output_path, new_size, warning_code)) => {
            let saved = original_size.saturating_sub(new_size);
            CompressionResult {
                success: true,
                original_size,
                new_size: Some(new_size),
                saved_bytes: Some(saved),
                reduction_percent: Some(savings_percentage(original_size, new_size)),
                output_path: Some(output_path.to_string_lossy().into_owned()),
                warning_code,
                error_code: None,
                error_message: None,
            }
        }
        Err(error) => CompressionResult {
            success: false,
            original_size,
            new_size: None,
            saved_bytes: None,
            reduction_percent: None,
            output_path: None,
            warning_code: None,
            error_code: Some(error.code),
            error_message: Some(error.message),
        },
    }
}

fn inspect_image_path(path: &Path) -> Result<ImageInfo, CommandError> {
    let format = accepted_format(path)?;
    let metadata = fs::metadata(path).map_err(|e| io_error("read_error", path, e))?;
    let image = decode_with_orientation(path)?;
    Ok(ImageInfo {
        path: path.to_string_lossy().into_owned(),
        name: path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("image")
            .to_string(),
        format: format_label(format).to_string(),
        width: image.width(),
        height: image.height(),
        size: metadata.len(),
    })
}

fn compress_image_inner(
    request: &CompressionRequest,
) -> Result<(PathBuf, u64, Option<String>), CommandError> {
    let input = Path::new(&request.path);
    let format = accepted_format(input)?;
    let image = decode_with_orientation(input)?;
    let (encoded, warning) = encode_image(&image, format, request)?;
    let destination = destination_directory(input, request)?;
    fs::create_dir_all(&destination)
        .map_err(|e| io_error("folder_inaccessible", &destination, e))?;
    let output_path = unique_output_path(&destination, input, format);
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output_path)
        .map_err(|e| io_error("write_error", &output_path, e))?;
    output
        .write_all(&encoded)
        .map_err(|e| io_error("write_error", &output_path, e))?;
    output
        .flush()
        .map_err(|e| io_error("write_error", &output_path, e))?;
    Ok((output_path, encoded.len() as u64, warning))
}

fn accepted_format(path: &Path) -> Result<ImageFormat, CommandError> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match extension.as_str() {
        "jpg" | "jpeg" => Ok(ImageFormat::Jpeg),
        "png" => Ok(ImageFormat::Png),
        "webp" => Ok(ImageFormat::WebP),
        _ => Err(CommandError::new(
            "unsupported_format",
            "Only JPG, JPEG, PNG and WebP files are supported.",
        )),
    }
}

fn decode_with_orientation(path: &Path) -> Result<DynamicImage, CommandError> {
    let reader = ImageReader::open(path)
        .map_err(|e| io_error("read_error", path, e))?
        .with_guessed_format()
        .map_err(|e| {
            CommandError::new(
                "invalid_image",
                format!("Cannot identify image format: {e}"),
            )
        })?;
    let mut decoder = reader.into_decoder().map_err(|e| {
        CommandError::new(
            "invalid_image",
            format!("The image is corrupted or unreadable: {e}"),
        )
    })?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut decoded = DynamicImage::from_decoder(decoder).map_err(|e| {
        CommandError::new(
            "invalid_image",
            format!("The image is corrupted or unreadable: {e}"),
        )
    })?;
    decoded.apply_orientation(orientation);
    Ok(decoded)
}

fn encode_image(
    image: &DynamicImage,
    format: ImageFormat,
    request: &CompressionRequest,
) -> Result<(Vec<u8>, Option<String>), CommandError> {
    match format {
        ImageFormat::Jpeg => encode_lossy(image, request, encode_jpeg),
        ImageFormat::WebP => encode_lossy(image, request, encode_webp),
        ImageFormat::Png => {
            let bytes = encode_png(image)?;
            let warning = if request.mode == CompressionMode::TargetFileSize {
                match request.target_bytes {
                    Some(target) if bytes.len() as u64 > target => {
                        Some("target_unreachable_png".to_string())
                    }
                    _ => Some("png_target_not_exact".to_string()),
                }
            } else {
                None
            };
            Ok((bytes, warning))
        }
        _ => Err(CommandError::new(
            "unsupported_format",
            "Unsupported image format.",
        )),
    }
}

fn encode_lossy<F>(
    image: &DynamicImage,
    request: &CompressionRequest,
    encoder: F,
) -> Result<(Vec<u8>, Option<String>), CommandError>
where
    F: Fn(&DynamicImage, u8) -> Result<Vec<u8>, CommandError>,
{
    match request.mode {
        CompressionMode::Recommended => Ok((encoder(image, 82)?, None)),
        CompressionMode::CustomQuality => Ok((
            encoder(image, request.quality.unwrap_or(82).clamp(10, 100))?,
            None,
        )),
        CompressionMode::TargetFileSize => {
            let target = request.target_bytes.ok_or_else(|| {
                CommandError::new("invalid_target", "Choose a valid target file size.")
            })?;
            if target == 0 {
                return Err(CommandError::new(
                    "invalid_target",
                    "Target size must be greater than zero.",
                ));
            }
            let (bytes, reached) = encode_for_target(image, target, &encoder)?;
            Ok((bytes, (!reached).then(|| "target_unreachable".to_string())))
        }
    }
}

fn encode_for_target<F>(
    image: &DynamicImage,
    target: u64,
    encoder: &F,
) -> Result<(Vec<u8>, bool), CommandError>
where
    F: Fn(&DynamicImage, u8) -> Result<Vec<u8>, CommandError>,
{
    let minimum = encoder(image, MIN_TARGET_QUALITY)?;
    if minimum.len() as u64 > target {
        return Ok((minimum, false));
    }
    let mut best = minimum;
    let mut low = MIN_TARGET_QUALITY + 1;
    let mut high = MAX_TARGET_QUALITY;
    while low <= high {
        let quality = low + (high - low) / 2;
        let candidate = encoder(image, quality)?;
        if candidate.len() as u64 <= target {
            best = candidate;
            low = quality.saturating_add(1);
        } else {
            if quality == 0 {
                break;
            }
            high = quality - 1;
        }
    }
    Ok((best, true))
}

fn encode_jpeg(image: &DynamicImage, quality: u8) -> Result<Vec<u8>, CommandError> {
    let mut output = Vec::new();
    JpegEncoder::new_with_quality(&mut output, quality)
        .encode_image(image)
        .map_err(|e| CommandError::new("encode_error", format!("JPEG encoding failed: {e}")))?;
    Ok(output)
}

fn encode_webp(image: &DynamicImage, quality: u8) -> Result<Vec<u8>, CommandError> {
    let rgb = image.to_rgb8();
    let encoder = webp::Encoder::from_rgb(rgb.as_raw(), rgb.width(), rgb.height());
    Ok(encoder.encode(quality as f32).to_vec())
}

fn encode_png(image: &DynamicImage) -> Result<Vec<u8>, CommandError> {
    let rgba = image.to_rgba8();
    let mut output = Vec::new();
    PngEncoder::new_with_quality(&mut output, CompressionType::Best, FilterType::Adaptive)
        .write_image(
            rgba.as_raw(),
            rgba.width(),
            rgba.height(),
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|e| CommandError::new("encode_error", format!("PNG optimization failed: {e}")))?;
    Ok(output)
}

fn destination_directory(
    input: &Path,
    request: &CompressionRequest,
) -> Result<PathBuf, CommandError> {
    let base = request
        .destination_dir
        .as_ref()
        .map(PathBuf::from)
        .or_else(|| input.parent().map(PathBuf::from))
        .ok_or_else(|| {
            CommandError::new(
                "folder_inaccessible",
                "The destination folder is unavailable.",
            )
        })?;
    Ok(if request.save_in_new_folder {
        base.join("FileNova Compressed")
    } else {
        base
    })
}

fn unique_output_path(destination: &Path, input: &Path, format: ImageFormat) -> PathBuf {
    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("image");
    let extension = match format {
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Png => "png",
        ImageFormat::WebP => "webp",
        _ => "img",
    };
    for index in 1.. {
        let suffix = if index == 1 {
            String::new()
        } else {
            format!("-{index}")
        };
        let candidate = destination.join(format!("{stem}-compressed{suffix}.{extension}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!()
}

fn format_label(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Jpeg => "JPG",
        ImageFormat::Png => "PNG",
        ImageFormat::WebP => "WEBP",
        _ => "UNKNOWN",
    }
}

fn savings_percentage(original: u64, compressed: u64) -> f64 {
    if original == 0 || compressed >= original {
        0.0
    } else {
        ((original - compressed) as f64 / original as f64) * 100.0
    }
}

fn io_error(default_code: &str, path: &Path, error: std::io::Error) -> CommandError {
    let code = if error.raw_os_error() == Some(112) {
        "disk_full"
    } else {
        default_code
    };
    CommandError::new(code, format!("{}: {}", path.to_string_lossy(), error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GenericImageView, ImageBuffer, Rgb, Rgba};
    use tempfile::tempdir;

    fn sample_rgb() -> DynamicImage {
        DynamicImage::ImageRgb8(ImageBuffer::from_fn(96, 72, |x, y| {
            Rgb([(x * 2) as u8, (y * 3) as u8, (x + y) as u8])
        }))
    }

    fn write_sample(path: &Path, format: ImageFormat) {
        sample_rgb().save_with_format(path, format).unwrap();
    }

    fn request(path: &Path, destination: &Path) -> CompressionRequest {
        CompressionRequest {
            path: path.to_string_lossy().into_owned(),
            destination_dir: Some(destination.to_string_lossy().into_owned()),
            save_in_new_folder: false,
            mode: CompressionMode::Recommended,
            quality: None,
            target_bytes: None,
        }
    }

    #[test]
    fn compresses_jpeg_and_preserves_original() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("original.jpg");
        write_sample(&input, ImageFormat::Jpeg);
        let before = fs::read(&input).unwrap();
        let result = compress_image(request(&input, dir.path()));
        assert!(result.success);
        assert_eq!(fs::read(&input).unwrap(), before);
        assert!(result
            .output_path
            .unwrap()
            .ends_with("original-compressed.jpg"));
    }

    #[test]
    fn compresses_png_losslessly() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("alpha.png");
        DynamicImage::ImageRgba8(ImageBuffer::from_pixel(64, 64, Rgba([20, 90, 180, 128])))
            .save_with_format(&input, ImageFormat::Png)
            .unwrap();
        let result = compress_image(request(&input, dir.path()));
        assert!(result.success);
        let output = image::open(result.output_path.unwrap()).unwrap();
        assert_eq!(output.dimensions(), (64, 64));
    }

    #[test]
    fn compresses_webp() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("photo.webp");
        write_sample(&input, ImageFormat::WebP);
        let result = compress_image(request(&input, dir.path()));
        assert!(result.success);
        assert_eq!(
            image::open(result.output_path.unwrap())
                .unwrap()
                .dimensions(),
            (96, 72)
        );
    }

    #[test]
    fn creates_unique_duplicate_names() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("photo.jpg");
        write_sample(&input, ImageFormat::Jpeg);
        let first = compress_image(request(&input, dir.path()));
        let second = compress_image(request(&input, dir.path()));
        assert!(first.output_path.unwrap().ends_with("photo-compressed.jpg"));
        assert!(second
            .output_path
            .unwrap()
            .ends_with("photo-compressed-2.jpg"));
    }

    #[test]
    fn rejects_invalid_image() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("broken.jpg");
        fs::write(&input, b"not an image").unwrap();
        assert_eq!(
            inspect_image_path(&input).unwrap_err().code,
            "invalid_image"
        );
    }

    #[test]
    fn calculates_saved_percentage() {
        assert!((savings_percentage(1000, 625) - 37.5).abs() < f64::EPSILON);
        assert_eq!(savings_percentage(100, 120), 0.0);
    }
}
