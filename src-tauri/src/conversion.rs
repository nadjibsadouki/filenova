use image::{
    codecs::{
        jpeg::JpegEncoder,
        png::{CompressionType, FilterType, PngEncoder},
    },
    DynamicImage, ImageDecoder, ImageEncoder, ImageFormat, ImageReader, Rgb, RgbImage,
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{Cursor, Write},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversionError {
    code: String,
    message: String,
}

impl ConversionError {
    fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConvertibleImageInfo {
    path: String,
    name: String,
    format: String,
    width: u32,
    height: u32,
    size: u64,
    has_transparency: bool,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    Jpg,
    Png,
    Webp,
    Bmp,
    Tiff,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversionRequest {
    path: String,
    destination_dir: Option<String>,
    target_format: OutputFormat,
    quality: u8,
    optimize_png: bool,
    background_color: String,
    confirm_transparency: bool,
    keep_dimensions: bool,
    remove_metadata: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversionResult {
    success: bool,
    source_format: Option<String>,
    target_format: String,
    original_size: u64,
    new_size: Option<u64>,
    output_path: Option<String>,
    error_code: Option<String>,
    error_message: Option<String>,
}

#[tauri::command]
pub fn inspect_convertible_image(path: String) -> Result<ConvertibleImageInfo, ConversionError> {
    inspect_path(Path::new(&path))
}

#[tauri::command]
pub fn convert_image(request: ConversionRequest) -> ConversionResult {
    let input = Path::new(&request.path);
    let original_size = fs::metadata(input).map(|m| m.len()).unwrap_or(0);
    let source_format = accepted_input_format(input)
        .ok()
        .map(format_label)
        .map(str::to_string);
    let target_format = output_label(request.target_format).to_string();
    match convert_inner(&request) {
        Ok((path, size)) => ConversionResult {
            success: true,
            source_format,
            target_format,
            original_size,
            new_size: Some(size),
            output_path: Some(path.to_string_lossy().into_owned()),
            error_code: None,
            error_message: None,
        },
        Err(error) => ConversionResult {
            success: false,
            source_format,
            target_format,
            original_size,
            new_size: None,
            output_path: None,
            error_code: Some(error.code),
            error_message: Some(error.message),
        },
    }
}

fn inspect_path(path: &Path) -> Result<ConvertibleImageInfo, ConversionError> {
    let format = accepted_input_format(path)?;
    let metadata = fs::metadata(path).map_err(|error| io_error("read_error", path, error))?;
    let image = decode_with_orientation(path)?;
    Ok(ConvertibleImageInfo {
        path: path.to_string_lossy().into_owned(),
        name: path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("image")
            .to_string(),
        format: format_label(format).to_string(),
        width: image.width(),
        height: image.height(),
        size: metadata.len(),
        has_transparency: has_transparency(&image),
    })
}

fn convert_inner(request: &ConversionRequest) -> Result<(PathBuf, u64), ConversionError> {
    let input = Path::new(&request.path);
    accepted_input_format(input)?;
    let image = decode_with_orientation(input)?;
    if request.target_format == OutputFormat::Jpg
        && has_transparency(&image)
        && !request.confirm_transparency
    {
        return Err(ConversionError::new(
            "transparency_confirmation_required",
            "JPEG does not support transparency. Confirm a background color before converting.",
        ));
    }

    // Conversion never changes pixel dimensions. Re-encoding pixel data also removes metadata.
    let _keep_dimensions = request.keep_dimensions;
    let _remove_metadata = request.remove_metadata;
    let bytes = encode_output(&image, request)?;
    let destination = request
        .destination_dir
        .as_ref()
        .map(PathBuf::from)
        .or_else(|| input.parent().map(PathBuf::from))
        .ok_or_else(|| {
            ConversionError::new(
                "folder_inaccessible",
                "The destination folder is unavailable.",
            )
        })?;
    fs::create_dir_all(&destination)
        .map_err(|error| io_error("folder_inaccessible", &destination, error))?;
    let output_path = unique_output_path(&destination, input, request.target_format);
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output_path)
        .map_err(|error| io_error("write_error", &output_path, error))?;
    output
        .write_all(&bytes)
        .map_err(|error| io_error("write_error", &output_path, error))?;
    output
        .flush()
        .map_err(|error| io_error("write_error", &output_path, error))?;
    Ok((output_path, bytes.len() as u64))
}

fn accepted_input_format(path: &Path) -> Result<ImageFormat, ConversionError> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match extension.as_str() {
        "jpg" | "jpeg" => Ok(ImageFormat::Jpeg),
        "png" => Ok(ImageFormat::Png),
        "webp" => Ok(ImageFormat::WebP),
        "bmp" => Ok(ImageFormat::Bmp),
        "tif" | "tiff" => Ok(ImageFormat::Tiff),
        _ => Err(ConversionError::new(
            "unsupported_conversion_format",
            "Supported formats are JPG, JPEG, PNG, WebP, BMP and TIFF.",
        )),
    }
}

fn decode_with_orientation(path: &Path) -> Result<DynamicImage, ConversionError> {
    let reader = ImageReader::open(path)
        .map_err(|error| io_error("read_error", path, error))?
        .with_guessed_format()
        .map_err(|error| {
            ConversionError::new("invalid_image", format!("Cannot identify image: {error}"))
        })?;
    let mut decoder = reader.into_decoder().map_err(|error| {
        ConversionError::new(
            "invalid_image",
            format!("The image is corrupted or unreadable: {error}"),
        )
    })?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut image = DynamicImage::from_decoder(decoder).map_err(|error| {
        ConversionError::new(
            "invalid_image",
            format!("The image is corrupted or unreadable: {error}"),
        )
    })?;
    image.apply_orientation(orientation);
    Ok(image)
}

fn encode_output(
    image: &DynamicImage,
    request: &ConversionRequest,
) -> Result<Vec<u8>, ConversionError> {
    match request.target_format {
        OutputFormat::Jpg => encode_jpeg(
            image,
            request.quality.clamp(10, 100),
            &request.background_color,
        ),
        OutputFormat::Png => encode_png(image, request.optimize_png),
        OutputFormat::Webp => encode_webp(image, request.quality.clamp(10, 100)),
        OutputFormat::Bmp => write_dynamic(image, ImageFormat::Bmp, "BMP"),
        OutputFormat::Tiff => write_dynamic(image, ImageFormat::Tiff, "TIFF"),
    }
}

fn encode_jpeg(
    image: &DynamicImage,
    quality: u8,
    background: &str,
) -> Result<Vec<u8>, ConversionError> {
    let rgb = if has_transparency(image) {
        flatten_transparency(image, parse_color(background)?)
    } else {
        image.to_rgb8()
    };
    let mut output = Vec::new();
    JpegEncoder::new_with_quality(&mut output, quality)
        .encode_image(&DynamicImage::ImageRgb8(rgb))
        .map_err(|error| {
            ConversionError::new("encode_error", format!("JPEG encoding failed: {error}"))
        })?;
    Ok(output)
}

fn encode_png(image: &DynamicImage, optimize: bool) -> Result<Vec<u8>, ConversionError> {
    let rgba = image.to_rgba8();
    let mut output = Vec::new();
    let compression = if optimize {
        CompressionType::Best
    } else {
        CompressionType::Default
    };
    PngEncoder::new_with_quality(&mut output, compression, FilterType::Adaptive)
        .write_image(
            rgba.as_raw(),
            rgba.width(),
            rgba.height(),
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|error| {
            ConversionError::new("encode_error", format!("PNG encoding failed: {error}"))
        })?;
    Ok(output)
}

fn encode_webp(image: &DynamicImage, quality: u8) -> Result<Vec<u8>, ConversionError> {
    if has_transparency(image) {
        let rgba = image.to_rgba8();
        Ok(
            webp::Encoder::from_rgba(rgba.as_raw(), rgba.width(), rgba.height())
                .encode(quality as f32)
                .to_vec(),
        )
    } else {
        let rgb = image.to_rgb8();
        Ok(
            webp::Encoder::from_rgb(rgb.as_raw(), rgb.width(), rgb.height())
                .encode(quality as f32)
                .to_vec(),
        )
    }
}

fn write_dynamic(
    image: &DynamicImage,
    format: ImageFormat,
    label: &str,
) -> Result<Vec<u8>, ConversionError> {
    let mut cursor = Cursor::new(Vec::new());
    image.write_to(&mut cursor, format).map_err(|error| {
        ConversionError::new("encode_error", format!("{label} encoding failed: {error}"))
    })?;
    Ok(cursor.into_inner())
}

fn has_transparency(image: &DynamicImage) -> bool {
    image.color().has_alpha() && image.to_rgba8().pixels().any(|pixel| pixel[3] < 255)
}

fn parse_color(color: &str) -> Result<[u8; 3], ConversionError> {
    let value = color.trim().trim_start_matches('#');
    if value.len() != 6 {
        return Err(ConversionError::new(
            "invalid_background_color",
            "Choose a valid background color.",
        ));
    }
    let parsed = u32::from_str_radix(value, 16).map_err(|_| {
        ConversionError::new(
            "invalid_background_color",
            "Choose a valid background color.",
        )
    })?;
    Ok([
        ((parsed >> 16) & 0xff) as u8,
        ((parsed >> 8) & 0xff) as u8,
        (parsed & 0xff) as u8,
    ])
}

fn flatten_transparency(image: &DynamicImage, background: [u8; 3]) -> RgbImage {
    let rgba = image.to_rgba8();
    RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let pixel = rgba.get_pixel(x, y);
        let alpha = pixel[3] as u16;
        Rgb([
            ((pixel[0] as u16 * alpha + background[0] as u16 * (255 - alpha)) / 255) as u8,
            ((pixel[1] as u16 * alpha + background[1] as u16 * (255 - alpha)) / 255) as u8,
            ((pixel[2] as u16 * alpha + background[2] as u16 * (255 - alpha)) / 255) as u8,
        ])
    })
}

fn unique_output_path(destination: &Path, input: &Path, format: OutputFormat) -> PathBuf {
    let stem = input
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("image");
    let extension = output_extension(format);
    for index in 1.. {
        let suffix = if index == 1 {
            String::new()
        } else {
            format!("-{index}")
        };
        let candidate = destination.join(format!("{stem}-converted{suffix}.{extension}"));
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
        ImageFormat::Bmp => "BMP",
        ImageFormat::Tiff => "TIFF",
        _ => "UNKNOWN",
    }
}

fn output_label(format: OutputFormat) -> &'static str {
    match format {
        OutputFormat::Jpg => "JPG",
        OutputFormat::Png => "PNG",
        OutputFormat::Webp => "WEBP",
        OutputFormat::Bmp => "BMP",
        OutputFormat::Tiff => "TIFF",
    }
}

fn output_extension(format: OutputFormat) -> &'static str {
    match format {
        OutputFormat::Jpg => "jpg",
        OutputFormat::Png => "png",
        OutputFormat::Webp => "webp",
        OutputFormat::Bmp => "bmp",
        OutputFormat::Tiff => "tiff",
    }
}

fn io_error(default_code: &str, path: &Path, error: std::io::Error) -> ConversionError {
    let code = if error.raw_os_error() == Some(112) {
        "disk_full"
    } else {
        default_code
    };
    ConversionError::new(code, format!("{}: {}", path.to_string_lossy(), error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GenericImageView, ImageBuffer, Rgba};
    use tempfile::tempdir;

    fn opaque_image() -> DynamicImage {
        DynamicImage::ImageRgb8(ImageBuffer::from_fn(80, 60, |x, y| {
            Rgb([(x * 3) as u8, (y * 4) as u8, 120])
        }))
    }

    fn transparent_image() -> DynamicImage {
        DynamicImage::ImageRgba8(ImageBuffer::from_pixel(40, 30, Rgba([255, 0, 0, 0])))
    }

    fn write_image(path: &Path, format: ImageFormat, image: &DynamicImage) {
        image.save_with_format(path, format).unwrap();
    }

    fn request(path: &Path, destination: &Path, target_format: OutputFormat) -> ConversionRequest {
        ConversionRequest {
            path: path.to_string_lossy().into_owned(),
            destination_dir: Some(destination.to_string_lossy().into_owned()),
            target_format,
            quality: 85,
            optimize_png: true,
            background_color: "#ffffff".into(),
            confirm_transparency: true,
            keep_dimensions: true,
            remove_metadata: true,
        }
    }

    #[test]
    fn converts_jpg_to_png() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("photo.jpg");
        write_image(&input, ImageFormat::Jpeg, &opaque_image());
        let result = convert_image(request(&input, dir.path(), OutputFormat::Png));
        assert!(result.success);
        assert_eq!(
            image::open(result.output_path.unwrap())
                .unwrap()
                .dimensions(),
            (80, 60)
        );
    }

    #[test]
    fn converts_png_to_jpg() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("photo.png");
        write_image(&input, ImageFormat::Png, &opaque_image());
        let result = convert_image(request(&input, dir.path(), OutputFormat::Jpg));
        assert!(result.success);
        assert!(result.output_path.unwrap().ends_with("photo-converted.jpg"));
    }

    #[test]
    fn flattens_transparent_png_to_selected_jpg_background() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("transparent.png");
        write_image(&input, ImageFormat::Png, &transparent_image());
        let mut req = request(&input, dir.path(), OutputFormat::Jpg);
        req.background_color = "#00ff00".into();
        let result = convert_image(req);
        assert!(result.success);
        let pixel = image::open(result.output_path.unwrap())
            .unwrap()
            .to_rgb8()
            .get_pixel(10, 10)
            .0;
        assert!(pixel[1] > 240 && pixel[0] < 15 && pixel[2] < 15);
    }

    #[test]
    fn converts_jpg_to_webp() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("photo.jpg");
        write_image(&input, ImageFormat::Jpeg, &opaque_image());
        assert!(convert_image(request(&input, dir.path(), OutputFormat::Webp)).success);
    }

    #[test]
    fn converts_webp_to_png() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("photo.webp");
        write_image(&input, ImageFormat::WebP, &opaque_image());
        assert!(convert_image(request(&input, dir.path(), OutputFormat::Png)).success);
    }

    #[test]
    fn converts_multiple_files() {
        let dir = tempdir().unwrap();
        let inputs = [
            dir.path().join("one.jpg"),
            dir.path().join("two.jpg"),
            dir.path().join("three.jpg"),
        ];
        for input in &inputs {
            write_image(input, ImageFormat::Jpeg, &opaque_image());
        }
        let results: Vec<_> = inputs
            .iter()
            .map(|input| convert_image(request(input, dir.path(), OutputFormat::Png)))
            .collect();
        assert!(results.iter().all(|result| result.success));
    }

    #[test]
    fn rejects_invalid_file() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("broken.tiff");
        fs::write(&input, b"broken").unwrap();
        assert_eq!(inspect_path(&input).unwrap_err().code, "invalid_image");
    }

    #[test]
    fn creates_duplicate_names() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("photo.jpg");
        write_image(&input, ImageFormat::Jpeg, &opaque_image());
        let first = convert_image(request(&input, dir.path(), OutputFormat::Png));
        let second = convert_image(request(&input, dir.path(), OutputFormat::Png));
        assert!(first.output_path.unwrap().ends_with("photo-converted.png"));
        assert!(second
            .output_path
            .unwrap()
            .ends_with("photo-converted-2.png"));
    }

    #[test]
    fn preserves_original_file() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("original.bmp");
        write_image(&input, ImageFormat::Bmp, &opaque_image());
        let before = fs::read(&input).unwrap();
        assert!(convert_image(request(&input, dir.path(), OutputFormat::Tiff)).success);
        assert_eq!(before, fs::read(&input).unwrap());
    }
}
