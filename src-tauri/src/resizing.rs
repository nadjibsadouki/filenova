use image::{
    codecs::{
        jpeg::JpegEncoder,
        png::{CompressionType, FilterType as PngFilterType, PngEncoder},
    },
    imageops::FilterType,
    DynamicImage, GenericImageView, ImageDecoder, ImageEncoder, ImageFormat, ImageReader, Rgb,
    RgbImage, Rgba, RgbaImage,
};
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    fs::{self, OpenOptions},
    io::{Cursor, ErrorKind, Write},
    path::{Path, PathBuf},
};

const MAX_OUTPUT_PIXELS: u64 = 100_000_000;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResizeError {
    code: String,
    message: String,
}

impl ResizeError {
    fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ResizeMode {
    Exact,
    Percentage,
    Maximum,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum AdjustmentMethod {
    Fit,
    Fill,
    Stretch,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ResizeOutputFormat {
    Jpg,
    Png,
    Webp,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResizeRequest {
    path: String,
    destination_dir: Option<String>,
    mode: ResizeMode,
    width: u32,
    height: u32,
    percentage: f64,
    adjustment_method: AdjustmentMethod,
    do_not_enlarge: bool,
    preserve_original_format: bool,
    output_format: ResizeOutputFormat,
    quality: u8,
    background_color: String,
    remove_metadata: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResizeResult {
    success: bool,
    original_width: Option<u32>,
    original_height: Option<u32>,
    final_width: Option<u32>,
    final_height: Option<u32>,
    original_size: u64,
    new_size: Option<u64>,
    final_format: Option<String>,
    output_path: Option<String>,
    error_code: Option<String>,
    error_message: Option<String>,
}

#[tauri::command]
pub async fn resize_image(request: ResizeRequest) -> ResizeResult {
    let path = request.path.clone();
    match tauri::async_runtime::spawn_blocking(move || resize_image_sync(request)).await {
        Ok(result) => result,
        Err(error) => ResizeResult {
            success: false,
            original_width: None,
            original_height: None,
            final_width: None,
            final_height: None,
            original_size: fs::metadata(&path)
                .map(|metadata| metadata.len())
                .unwrap_or(0),
            new_size: None,
            final_format: None,
            output_path: None,
            error_code: Some("processing_error".into()),
            error_message: Some(format!("The resize worker stopped unexpectedly: {error}")),
        },
    }
}

fn resize_image_sync(request: ResizeRequest) -> ResizeResult {
    let input = Path::new(&request.path);
    let original_size = fs::metadata(input)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    let decoded = decode_with_orientation(input);
    let (original_width, original_height) = decoded
        .as_ref()
        .map(|image| (Some(image.width()), Some(image.height())))
        .unwrap_or((None, None));

    match decoded.and_then(|image| resize_inner(input, image, &request)) {
        Ok((output_path, final_width, final_height, new_size, final_format)) => ResizeResult {
            success: true,
            original_width,
            original_height,
            final_width: Some(final_width),
            final_height: Some(final_height),
            original_size,
            new_size: Some(new_size),
            final_format: Some(format_label(final_format).to_string()),
            output_path: Some(output_path.to_string_lossy().into_owned()),
            error_code: None,
            error_message: None,
        },
        Err(error) => ResizeResult {
            success: false,
            original_width,
            original_height,
            final_width: None,
            final_height: None,
            original_size,
            new_size: None,
            final_format: None,
            output_path: None,
            error_code: Some(error.code),
            error_message: Some(error.message),
        },
    }
}

fn resize_inner(
    input: &Path,
    image: DynamicImage,
    request: &ResizeRequest,
) -> Result<(PathBuf, u32, u32, u64, ImageFormat), ResizeError> {
    let source_format = accepted_input_format(input)?;
    let (target_width, target_height) = target_dimensions(&image, request)?;
    validate_output_dimensions(target_width, target_height)?;
    let resized = apply_adjustment(&image, target_width, target_height, request)?;
    let output_format = if request.preserve_original_format {
        source_format
    } else {
        requested_image_format(request.output_format)
    };
    if output_format == ImageFormat::Jpeg
        && request.background_color.eq_ignore_ascii_case("transparent")
    {
        return Err(ResizeError::new(
            "transparency_incompatible_jpg",
            "JPG does not support a transparent background. Choose a solid color or PNG/WebP.",
        ));
    }

    // Re-encoding is required after resizing. The image crate applies orientation to pixels;
    // metadata is intentionally not copied because stale EXIF dimensions/orientation are unsafe.
    let _remove_metadata = request.remove_metadata;
    let encoded = encode_image(&resized, output_format, request)?;
    let destination = request
        .destination_dir
        .as_ref()
        .map(PathBuf::from)
        .or_else(|| input.parent().map(PathBuf::from))
        .ok_or_else(|| {
            ResizeError::new(
                "folder_inaccessible",
                "The destination folder is unavailable.",
            )
        })?;
    fs::create_dir_all(&destination)
        .map_err(|error| io_error("folder_inaccessible", &destination, error))?;
    let output_path = write_unique_output(&destination, input, output_format, &encoded)?;
    Ok((
        output_path,
        resized.width(),
        resized.height(),
        encoded.len() as u64,
        output_format,
    ))
}

fn target_dimensions(
    image: &DynamicImage,
    request: &ResizeRequest,
) -> Result<(u32, u32), ResizeError> {
    let (source_width, source_height) = image.dimensions();
    let (mut width, mut height) = match request.mode {
        ResizeMode::Exact => {
            if request.width == 0 || request.height == 0 {
                return Err(ResizeError::new(
                    "invalid_dimensions",
                    "Width and height must be greater than zero.",
                ));
            }
            (request.width, request.height)
        }
        ResizeMode::Percentage => {
            if !request.percentage.is_finite() || request.percentage <= 0.0 {
                return Err(ResizeError::new(
                    "invalid_percentage",
                    "Percentage must be greater than zero.",
                ));
            }
            (
                ((source_width as f64 * request.percentage / 100.0)
                    .round()
                    .max(1.0)) as u32,
                ((source_height as f64 * request.percentage / 100.0)
                    .round()
                    .max(1.0)) as u32,
            )
        }
        ResizeMode::Maximum => {
            if request.width == 0 || request.height == 0 {
                return Err(ResizeError::new(
                    "invalid_dimensions",
                    "Maximum width and height must be greater than zero.",
                ));
            }
            let mut scale = (request.width as f64 / source_width as f64)
                .min(request.height as f64 / source_height as f64);
            if request.do_not_enlarge {
                scale = scale.min(1.0);
            }
            (
                ((source_width as f64 * scale).round().max(1.0)) as u32,
                ((source_height as f64 * scale).round().max(1.0)) as u32,
            )
        }
    };

    if request.mode != ResizeMode::Maximum
        && request.do_not_enlarge
        && source_width <= width
        && source_height <= height
    {
        width = source_width;
        height = source_height;
    }
    Ok((width, height))
}

fn validate_output_dimensions(width: u32, height: u32) -> Result<(), ResizeError> {
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or_else(|| {
            ResizeError::new(
                "dimensions_too_large",
                "The requested dimensions are too large.",
            )
        })?;
    if pixels > MAX_OUTPUT_PIXELS {
        return Err(ResizeError::new(
            "dimensions_too_large",
            "The requested image exceeds the safe 100 megapixel limit.",
        ));
    }
    Ok(())
}

fn apply_adjustment(
    image: &DynamicImage,
    target_width: u32,
    target_height: u32,
    request: &ResizeRequest,
) -> Result<DynamicImage, ResizeError> {
    match request.adjustment_method {
        AdjustmentMethod::Stretch => {
            Ok(image.resize_exact(target_width, target_height, FilterType::Lanczos3))
        }
        AdjustmentMethod::Fit => {
            let scale = (target_width as f64 / image.width() as f64)
                .min(target_height as f64 / image.height() as f64);
            let content_width = ((image.width() as f64 * scale).round().max(1.0)) as u32;
            let content_height = ((image.height() as f64 * scale).round().max(1.0)) as u32;
            let content = image.resize_exact(content_width, content_height, FilterType::Lanczos3);
            let background = parse_background(&request.background_color)?;
            let mut canvas = RgbaImage::from_pixel(target_width, target_height, Rgba(background));
            let x = i64::from((target_width - content_width) / 2);
            let y = i64::from((target_height - content_height) / 2);
            image::imageops::overlay(&mut canvas, &content.to_rgba8(), x, y);
            Ok(DynamicImage::ImageRgba8(canvas))
        }
        AdjustmentMethod::Fill => {
            let scale = (target_width as f64 / image.width() as f64)
                .max(target_height as f64 / image.height() as f64);
            let scaled_width = ((image.width() as f64 * scale).round().max(1.0)) as u32;
            let scaled_height = ((image.height() as f64 * scale).round().max(1.0)) as u32;
            let scaled = image.resize_exact(scaled_width, scaled_height, FilterType::Lanczos3);
            let x = (scaled_width - target_width) / 2;
            let y = (scaled_height - target_height) / 2;
            Ok(scaled.crop_imm(x, y, target_width, target_height))
        }
    }
}

fn accepted_input_format(path: &Path) -> Result<ImageFormat, ResizeError> {
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
        _ => Err(ResizeError::new(
            "unsupported_resize_format",
            "Supported formats are JPG, JPEG, PNG, WebP, BMP and TIFF.",
        )),
    }
}

fn decode_with_orientation(path: &Path) -> Result<DynamicImage, ResizeError> {
    accepted_input_format(path)?;
    let reader = ImageReader::open(path)
        .map_err(|error| io_error("read_error", path, error))?
        .with_guessed_format()
        .map_err(|error| {
            ResizeError::new("invalid_image", format!("Cannot identify image: {error}"))
        })?;
    let mut decoder = reader.into_decoder().map_err(|error| {
        ResizeError::new(
            "invalid_image",
            format!("The image is corrupted or unreadable: {error}"),
        )
    })?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut decoded = DynamicImage::from_decoder(decoder).map_err(|error| {
        ResizeError::new(
            "invalid_image",
            format!("The image is corrupted or unreadable: {error}"),
        )
    })?;
    decoded.apply_orientation(orientation);
    Ok(decoded)
}

fn requested_image_format(format: ResizeOutputFormat) -> ImageFormat {
    match format {
        ResizeOutputFormat::Jpg => ImageFormat::Jpeg,
        ResizeOutputFormat::Png => ImageFormat::Png,
        ResizeOutputFormat::Webp => ImageFormat::WebP,
    }
}

fn encode_image(
    image: &DynamicImage,
    format: ImageFormat,
    request: &ResizeRequest,
) -> Result<Vec<u8>, ResizeError> {
    match format {
        ImageFormat::Jpeg => {
            let background = parse_background(&request.background_color)?;
            let rgb = flatten_transparency(image, [background[0], background[1], background[2]]);
            let mut output = Vec::new();
            JpegEncoder::new_with_quality(&mut output, request.quality.clamp(10, 100))
                .encode_image(&DynamicImage::ImageRgb8(rgb))
                .map_err(|error| {
                    ResizeError::new("encode_error", format!("JPEG encoding failed: {error}"))
                })?;
            Ok(output)
        }
        ImageFormat::Png => {
            let rgba = image.to_rgba8();
            let mut output = Vec::new();
            PngEncoder::new_with_quality(
                &mut output,
                CompressionType::Best,
                PngFilterType::Adaptive,
            )
            .write_image(
                rgba.as_raw(),
                rgba.width(),
                rgba.height(),
                image::ExtendedColorType::Rgba8,
            )
            .map_err(|error| {
                ResizeError::new("encode_error", format!("PNG encoding failed: {error}"))
            })?;
            Ok(output)
        }
        ImageFormat::WebP => {
            if has_transparency(image) {
                let rgba = image.to_rgba8();
                Ok(
                    webp::Encoder::from_rgba(rgba.as_raw(), rgba.width(), rgba.height())
                        .encode(request.quality.clamp(10, 100) as f32)
                        .to_vec(),
                )
            } else {
                let rgb = image.to_rgb8();
                Ok(
                    webp::Encoder::from_rgb(rgb.as_raw(), rgb.width(), rgb.height())
                        .encode(request.quality.clamp(10, 100) as f32)
                        .to_vec(),
                )
            }
        }
        ImageFormat::Bmp | ImageFormat::Tiff => {
            let mut cursor = Cursor::new(Vec::new());
            image.write_to(&mut cursor, format).map_err(|error| {
                ResizeError::new("encode_error", format!("Image encoding failed: {error}"))
            })?;
            Ok(cursor.into_inner())
        }
        _ => Err(ResizeError::new(
            "unsupported_resize_format",
            "Unsupported output format.",
        )),
    }
}

fn parse_background(value: &str) -> Result<[u8; 4], ResizeError> {
    if value.eq_ignore_ascii_case("transparent") {
        return Ok([0, 0, 0, 0]);
    }
    let value = value.trim().trim_start_matches('#');
    if value.len() != 6 {
        return Err(ResizeError::new(
            "invalid_background_color",
            "Choose a valid background color.",
        ));
    }
    let parsed = u32::from_str_radix(value, 16).map_err(|_| {
        ResizeError::new(
            "invalid_background_color",
            "Choose a valid background color.",
        )
    })?;
    Ok([
        ((parsed >> 16) & 0xff) as u8,
        ((parsed >> 8) & 0xff) as u8,
        (parsed & 0xff) as u8,
        255,
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

fn has_transparency(image: &DynamicImage) -> bool {
    image.color().has_alpha() && image.to_rgba8().pixels().any(|pixel| pixel[3] < 255)
}

fn write_unique_output(
    destination: &Path,
    input: &Path,
    format: ImageFormat,
    bytes: &[u8],
) -> Result<PathBuf, ResizeError> {
    let stem = input
        .file_stem()
        .unwrap_or_else(|| std::ffi::OsStr::new("image"));
    let extension = output_extension(format);
    for index in 1..=10_000u32 {
        let mut name = OsString::from(stem);
        name.push("-resized");
        if index > 1 {
            name.push(format!("-{index}"));
        }
        name.push(".");
        name.push(extension);
        let candidate = destination.join(name);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(mut output) => {
                output
                    .write_all(bytes)
                    .and_then(|_| output.flush())
                    .map_err(|error| io_error("write_error", &candidate, error))?;
                return Ok(candidate);
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(io_error("write_error", &candidate, error)),
        }
    }
    Err(ResizeError::new(
        "too_many_duplicates",
        "Too many resized files use the same name.",
    ))
}

fn output_extension(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Png => "png",
        ImageFormat::WebP => "webp",
        ImageFormat::Bmp => "bmp",
        ImageFormat::Tiff => "tiff",
        _ => "img",
    }
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

fn io_error(default_code: &str, path: &Path, error: std::io::Error) -> ResizeError {
    let code = if error.raw_os_error() == Some(112) {
        "disk_full"
    } else {
        default_code
    };
    ResizeError::new(code, format!("{}: {}", path.to_string_lossy(), error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GenericImageView, ImageBuffer};
    use tempfile::tempdir;

    fn sample_image(width: u32, height: u32) -> DynamicImage {
        DynamicImage::ImageRgba8(ImageBuffer::from_fn(width, height, |x, y| {
            Rgba([(x % 255) as u8, (y % 255) as u8, 160, 255])
        }))
    }

    fn transparent_image(width: u32, height: u32) -> DynamicImage {
        DynamicImage::ImageRgba8(ImageBuffer::from_pixel(
            width,
            height,
            Rgba([40, 120, 220, 80]),
        ))
    }

    fn write_image(path: &Path, image: &DynamicImage, format: ImageFormat) {
        image.save_with_format(path, format).unwrap();
    }

    fn request(path: &Path, destination: &Path) -> ResizeRequest {
        ResizeRequest {
            path: path.to_string_lossy().into_owned(),
            destination_dir: Some(destination.to_string_lossy().into_owned()),
            mode: ResizeMode::Exact,
            width: 120,
            height: 120,
            percentage: 100.0,
            adjustment_method: AdjustmentMethod::Fit,
            do_not_enlarge: false,
            preserve_original_format: false,
            output_format: ResizeOutputFormat::Png,
            quality: 90,
            background_color: "#000000".into(),
            remove_metadata: false,
        }
    }

    #[test]
    fn exact_fit_adds_margins_without_distortion() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("landscape.png");
        write_image(&input, &sample_image(100, 50), ImageFormat::Png);
        let result = resize_image_sync(request(&input, dir.path()));
        assert!(result.success);
        let output = image::open(result.output_path.unwrap()).unwrap().to_rgba8();
        assert_eq!(output.dimensions(), (120, 120));
        assert_eq!(output.get_pixel(5, 5).0, [0, 0, 0, 255]);
        assert_ne!(output.get_pixel(60, 60).0, [0, 0, 0, 255]);
    }

    #[test]
    fn exact_dimensions_match_requested_width_and_height() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("exact.png");
        write_image(&input, &sample_image(160, 90), ImageFormat::Png);
        let mut req = request(&input, dir.path());
        req.width = 73;
        req.height = 51;
        req.adjustment_method = AdjustmentMethod::Stretch;
        let result = resize_image_sync(req);
        assert!(result.success);
        assert_eq!(
            (result.final_width, result.final_height),
            (Some(73), Some(51))
        );
        assert_eq!(
            image::open(result.output_path.unwrap())
                .unwrap()
                .dimensions(),
            (73, 51)
        );
    }

    #[test]
    fn fit_preserves_source_aspect_ratio() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("ratio.png");
        write_image(&input, &sample_image(160, 80), ImageFormat::Png);
        let mut req = request(&input, dir.path());
        req.width = 100;
        req.height = 100;
        req.adjustment_method = AdjustmentMethod::Fit;
        req.background_color = "transparent".into();
        let result = resize_image_sync(req);
        let output = image::open(result.output_path.unwrap()).unwrap().to_rgba8();
        let opaque_rows: Vec<_> = (0..output.height())
            .filter(|&y| (0..output.width()).any(|x| output.get_pixel(x, y)[3] > 0))
            .collect();
        assert_eq!(opaque_rows.len(), 50);
        assert_eq!(output.width(), 100);
    }

    #[test]
    fn fill_crops_to_exact_dimensions() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("wide.png");
        write_image(&input, &sample_image(160, 80), ImageFormat::Png);
        let mut req = request(&input, dir.path());
        req.width = 90;
        req.height = 90;
        req.adjustment_method = AdjustmentMethod::Fill;
        let result = resize_image_sync(req);
        assert!(result.success);
        assert_eq!(
            image::open(result.output_path.unwrap())
                .unwrap()
                .dimensions(),
            (90, 90)
        );
    }

    #[test]
    fn stretch_forces_exact_dimensions() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("stretch.png");
        write_image(&input, &sample_image(80, 40), ImageFormat::Png);
        let mut req = request(&input, dir.path());
        req.width = 111;
        req.height = 77;
        req.adjustment_method = AdjustmentMethod::Stretch;
        let result = resize_image_sync(req);
        assert_eq!(
            image::open(result.output_path.unwrap())
                .unwrap()
                .dimensions(),
            (111, 77)
        );
    }

    #[test]
    fn percentage_resizes_from_oriented_source_dimensions() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("half.png");
        write_image(&input, &sample_image(200, 100), ImageFormat::Png);
        let mut req = request(&input, dir.path());
        req.mode = ResizeMode::Percentage;
        req.percentage = 50.0;
        req.adjustment_method = AdjustmentMethod::Stretch;
        let result = resize_image_sync(req);
        assert_eq!(
            image::open(result.output_path.unwrap())
                .unwrap()
                .dimensions(),
            (100, 50)
        );
    }

    #[test]
    fn maximum_dimensions_preserve_ratio() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("maximum.png");
        write_image(&input, &sample_image(200, 100), ImageFormat::Png);
        let mut req = request(&input, dir.path());
        req.mode = ResizeMode::Maximum;
        req.width = 60;
        req.height = 60;
        let result = resize_image_sync(req);
        assert_eq!(
            image::open(result.output_path.unwrap())
                .unwrap()
                .dimensions(),
            (60, 30)
        );
    }

    #[test]
    fn does_not_enlarge_smaller_images() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("small.png");
        write_image(&input, &sample_image(40, 30), ImageFormat::Png);
        let mut req = request(&input, dir.path());
        req.width = 400;
        req.height = 300;
        req.do_not_enlarge = true;
        req.adjustment_method = AdjustmentMethod::Stretch;
        let result = resize_image_sync(req);
        assert_eq!(
            image::open(result.output_path.unwrap())
                .unwrap()
                .dimensions(),
            (40, 30)
        );
    }

    #[test]
    fn preserves_png_and_webp_transparency() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("transparent source.png");
        write_image(&input, &transparent_image(80, 60), ImageFormat::Png);
        for format in [ResizeOutputFormat::Png, ResizeOutputFormat::Webp] {
            let mut req = request(&input, dir.path());
            req.output_format = format;
            req.width = 40;
            req.height = 40;
            req.adjustment_method = AdjustmentMethod::Fit;
            req.background_color = "transparent".into();
            let result = resize_image_sync(req);
            assert!(result.success);
            let output = image::open(result.output_path.unwrap()).unwrap().to_rgba8();
            assert!(output.get_pixel(10, 10)[3] < 255);
            assert_eq!(output.get_pixel(0, 0)[3], 0);
        }
    }

    #[test]
    fn rejects_transparent_background_for_jpg_output() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("transparent.png");
        write_image(&input, &transparent_image(80, 60), ImageFormat::Png);
        let mut req = request(&input, dir.path());
        req.output_format = ResizeOutputFormat::Jpg;
        req.background_color = "transparent".into();
        let result = resize_image_sync(req);
        assert!(!result.success);
        assert_eq!(
            result.error_code.as_deref(),
            Some("transparency_incompatible_jpg")
        );
    }

    #[test]
    fn creates_unique_names_and_never_changes_original() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("photo.jpg");
        write_image(&input, &sample_image(80, 60), ImageFormat::Jpeg);
        let original = fs::read(&input).unwrap();
        let outputs: Vec<_> = (0..3)
            .map(|_| {
                let mut req = request(&input, dir.path());
                req.preserve_original_format = true;
                resize_image_sync(req).output_path.unwrap()
            })
            .collect();
        assert!(outputs[0].ends_with("photo-resized.jpg"));
        assert!(outputs[1].ends_with("photo-resized-2.jpg"));
        assert!(outputs[2].ends_with("photo-resized-3.jpg"));
        assert_eq!(fs::read(&input).unwrap(), original);
    }

    #[test]
    fn handles_multiple_unicode_and_space_filenames() {
        let dir = tempdir().unwrap();
        let names = [
            "vacances été.png",
            "élève numéro 2.png",
            "image (finale) #1.png",
        ];
        for name in names {
            let input = dir.path().join(name);
            write_image(&input, &sample_image(70, 50), ImageFormat::Png);
            let result = resize_image_sync(request(&input, dir.path()));
            assert!(
                result.success,
                "failed for {name}: {:?}",
                result.error_message
            );
            assert!(Path::new(&result.output_path.unwrap()).exists());
        }
    }

    #[test]
    fn resizes_multiple_files_successfully() {
        let dir = tempdir().unwrap();
        let inputs = ["one.png", "two.png", "three.png"].map(|name| dir.path().join(name));
        for input in &inputs {
            write_image(input, &sample_image(120, 80), ImageFormat::Png);
        }
        let results: Vec<_> = inputs
            .iter()
            .map(|input| resize_image_sync(request(input, dir.path())))
            .collect();
        assert_eq!(results.len(), 3);
        assert!(results.iter().all(|result| result.success));
        assert!(results.iter().all(|result| result
            .output_path
            .as_ref()
            .is_some_and(|path| Path::new(path).exists())));
    }

    #[test]
    fn rejects_corrupted_image_file() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("broken.png");
        fs::write(&input, b"this is not an image").unwrap();
        let result = resize_image_sync(request(&input, dir.path()));
        assert!(!result.success);
        assert_eq!(result.error_code.as_deref(), Some("invalid_image"));
        assert!(result.output_path.is_none());
    }

    #[test]
    fn applies_exif_orientation_before_resizing() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("oriented.jpg");
        let mut jpeg = Vec::new();
        JpegEncoder::new_with_quality(&mut jpeg, 90)
            .encode_image(&sample_image(40, 20).to_rgb8())
            .unwrap();
        let exif: [u8; 32] = [
            b'E', b'x', b'i', b'f', 0, 0, b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0, 0x12, 0x01, 3, 0, 1,
            0, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0,
        ];
        let mut oriented = vec![0xff, 0xd8, 0xff, 0xe1, 0, 34];
        oriented.extend_from_slice(&exif);
        oriented.extend_from_slice(&jpeg[2..]);
        fs::write(&input, oriented).unwrap();
        let decoded = decode_with_orientation(&input).unwrap();
        assert_eq!(decoded.dimensions(), (20, 40));

        let mut req = request(&input, dir.path());
        req.mode = ResizeMode::Percentage;
        req.percentage = 50.0;
        req.adjustment_method = AdjustmentMethod::Stretch;
        let result = resize_image_sync(req);
        assert_eq!(
            image::open(result.output_path.unwrap())
                .unwrap()
                .dimensions(),
            (10, 20)
        );
    }
}
