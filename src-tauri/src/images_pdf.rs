use image::{codecs::jpeg::JpegEncoder, DynamicImage, ImageDecoder, ImageReader, Rgb, RgbImage};
use lopdf::{dictionary, Dictionary, Document, Object, Stream};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};
use tauri::Emitter;

static IMAGES_PDF_RUNNING: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImagesPdfRequest {
    pub paths: Vec<String>,
    pub destination_dir: Option<String>,
    pub output_name: String,
    pub page_size: String,
    pub custom_width_mm: f64,
    pub custom_height_mm: f64,
    pub orientation: String,
    pub margin: String,
    pub custom_margin_mm: f64,
    pub fitting: String,
    pub background_color: String,
    pub quality: u8,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImagesPdfResult {
    pub success: bool,
    pub image_count: usize,
    pub page_count: usize,
    pub page_size_used: Option<String>,
    pub output_path: Option<String>,
    pub new_size: Option<u64>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImagesPdfProgress {
    pub completed: usize,
    pub total: usize,
    pub percent: usize,
    pub current_file: String,
}

#[derive(Debug)]
struct ImagesPdfError {
    code: String,
    message: String,
}

fn error(code: &str, message: impl Into<String>) -> ImagesPdfError {
    ImagesPdfError {
        code: code.into(),
        message: message.into(),
    }
}

fn decode_with_orientation(path: &Path) -> Result<DynamicImage, ImagesPdfError> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if !matches!(
        extension.to_ascii_lowercase().as_str(),
        "jpg" | "jpeg" | "png" | "webp" | "bmp" | "tif" | "tiff"
    ) {
        return Err(error(
            "unsupported_conversion_format",
            "Supported formats are JPG, JPEG, PNG, WebP, BMP and TIFF",
        ));
    }
    let reader = ImageReader::open(path)
        .map_err(|err| error("read_error", err.to_string()))?
        .with_guessed_format()
        .map_err(|err| error("invalid_image", err.to_string()))?;
    let mut decoder = reader
        .into_decoder()
        .map_err(|err| error("invalid_image", err.to_string()))?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut image = DynamicImage::from_decoder(decoder)
        .map_err(|err| error("invalid_image", err.to_string()))?;
    image.apply_orientation(orientation);
    if image.width() == 0 || image.height() == 0 {
        return Err(error(
            "images_pdf_invalid_dimensions",
            "Image dimensions must be greater than zero",
        ));
    }
    Ok(image)
}

fn page_size_label(request: &ImagesPdfRequest) -> String {
    match request.page_size.as_str() {
        "a4" => "A4".into(),
        "a3" => "A3".into(),
        "letter" => "Letter".into(),
        "legal" => "Legal".into(),
        "fitImage" => "Fit to Image".into(),
        "custom" => format!(
            "Custom ({} × {} mm)",
            request.custom_width_mm, request.custom_height_mm
        ),
        other => other.to_owned(),
    }
}

fn parse_color(value: &str) -> Result<Rgb<u8>, ImagesPdfError> {
    let hex = value.trim().trim_start_matches('#');
    if hex.len() != 6 {
        return Err(error(
            "images_pdf_invalid_color",
            "Invalid background color",
        ));
    }
    let channel = |range| {
        u8::from_str_radix(&hex[range], 16)
            .map_err(|_| error("images_pdf_invalid_color", "Invalid background color"))
    };
    Ok(Rgb([channel(0..2)?, channel(2..4)?, channel(4..6)?]))
}

fn flatten(image: &DynamicImage, background: Rgb<u8>) -> RgbImage {
    let rgba = image.to_rgba8();
    let mut output = RgbImage::new(rgba.width(), rgba.height());
    for (x, y, pixel) in rgba.enumerate_pixels() {
        let alpha = pixel[3] as u16;
        let inverse = 255 - alpha;
        output.put_pixel(
            x,
            y,
            Rgb([
                ((pixel[0] as u16 * alpha + background[0] as u16 * inverse) / 255) as u8,
                ((pixel[1] as u16 * alpha + background[1] as u16 * inverse) / 255) as u8,
                ((pixel[2] as u16 * alpha + background[2] as u16 * inverse) / 255) as u8,
            ]),
        );
    }
    output
}

fn margin_points(request: &ImagesPdfRequest) -> Result<f64, ImagesPdfError> {
    let value = match request.margin.as_str() {
        "none" => 0.0,
        "small" => 18.0,
        "medium" => 36.0,
        "large" => 54.0,
        "custom" if request.custom_margin_mm >= 0.0 => request.custom_margin_mm * 72.0 / 25.4,
        _ => return Err(error("images_pdf_invalid_margin", "Invalid page margin")),
    };
    Ok(value)
}

fn page_dimensions(
    request: &ImagesPdfRequest,
    image_width: u32,
    image_height: u32,
    margin: f64,
) -> Result<(f64, f64), ImagesPdfError> {
    let (mut width, mut height) = match request.page_size.as_str() {
        "a4" => (595.276, 841.89),
        "a3" => (841.89, 1190.551),
        "letter" => (612.0, 792.0),
        "legal" => (612.0, 1008.0),
        "fitImage" => (
            image_width as f64 * 0.75 + margin * 2.0,
            image_height as f64 * 0.75 + margin * 2.0,
        ),
        "custom" if request.custom_width_mm > 0.0 && request.custom_height_mm > 0.0 => (
            request.custom_width_mm * 72.0 / 25.4,
            request.custom_height_mm * 72.0 / 25.4,
        ),
        _ => return Err(error("images_pdf_invalid_page_size", "Invalid page size")),
    };
    match request.orientation.as_str() {
        "auto" => {
            if (image_width > image_height) != (width > height) {
                std::mem::swap(&mut width, &mut height);
            }
        }
        "portrait" => {
            if width > height {
                std::mem::swap(&mut width, &mut height);
            }
        }
        "landscape" => {
            if height > width {
                std::mem::swap(&mut width, &mut height);
            }
        }
        _ => {
            return Err(error(
                "images_pdf_invalid_orientation",
                "Invalid orientation",
            ))
        }
    }
    if width <= margin * 2.0 || height <= margin * 2.0 {
        return Err(error(
            "images_pdf_invalid_margin",
            "Margins leave no room for the image",
        ));
    }
    Ok((width, height))
}

#[derive(Debug, PartialEq)]
struct Placement {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    clip: bool,
}

fn placement(
    fitting: &str,
    image_width: u32,
    image_height: u32,
    page_width: f64,
    page_height: f64,
    margin: f64,
) -> Result<Placement, ImagesPdfError> {
    let available_width = page_width - margin * 2.0;
    let available_height = page_height - margin * 2.0;
    let (width, height, clip) = match fitting {
        "fit" => {
            let scale =
                (available_width / image_width as f64).min(available_height / image_height as f64);
            (
                image_width as f64 * scale,
                image_height as f64 * scale,
                false,
            )
        }
        "fill" => {
            let scale =
                (available_width / image_width as f64).max(available_height / image_height as f64);
            (
                image_width as f64 * scale,
                image_height as f64 * scale,
                true,
            )
        }
        "stretch" => (available_width, available_height, false),
        _ => return Err(error("images_pdf_invalid_fitting", "Invalid image fitting")),
    };
    Ok(Placement {
        x: margin + (available_width - width) / 2.0,
        y: margin + (available_height - height) / 2.0,
        width,
        height,
        clip,
    })
}

fn safe_stem(value: &str) -> String {
    let trimmed = value
        .trim()
        .trim_end_matches(|c: char| c == '.' || c.is_whitespace());
    let without_extension = trimmed
        .strip_suffix(".pdf")
        .or_else(|| trimmed.strip_suffix(".PDF"))
        .unwrap_or(trimmed);
    let cleaned: String = without_extension
        .chars()
        .map(|c| {
            if "<>:\"/\\|?*".contains(c) || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_end_matches('.');
    if cleaned.is_empty() {
        "images-document".into()
    } else {
        cleaned.into()
    }
}

fn write_unique(directory: &Path, stem: &str, bytes: &[u8]) -> Result<PathBuf, ImagesPdfError> {
    for suffix in 1..=10_000usize {
        let name = if suffix == 1 {
            format!("{stem}.pdf")
        } else {
            format!("{stem}-{suffix}.pdf")
        };
        let path = directory.join(name);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                if let Err(err) = file.write_all(bytes).and_then(|_| file.flush()) {
                    let _ = fs::remove_file(&path);
                    let code = if matches!(err.raw_os_error(), Some(28 | 112)) {
                        "insufficient_space"
                    } else {
                        "write_error"
                    };
                    return Err(error(code, err.to_string()));
                }
                return Ok(path);
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(error("destination_inaccessible", err.to_string())),
        }
    }
    Err(error("write_error", "Unable to create a unique filename"))
}

fn create_pdf<F>(
    request: &ImagesPdfRequest,
    mut report: F,
) -> Result<(Document, usize), ImagesPdfError>
where
    F: FnMut(ImagesPdfProgress),
{
    if request.paths.is_empty() {
        return Err(error("images_pdf_no_images", "No images were selected"));
    }
    if !(10..=100).contains(&request.quality) {
        return Err(error(
            "images_pdf_invalid_quality",
            "Quality must be between 10 and 100",
        ));
    }
    let background = parse_color(&request.background_color)?;
    let margin = margin_points(request)?;
    let mut document = Document::with_version("1.5");
    let pages_id = document.new_object_id();
    let mut kids = Vec::with_capacity(request.paths.len());

    for (index, path_string) in request.paths.iter().enumerate() {
        let path = Path::new(path_string);
        report(ImagesPdfProgress {
            completed: index,
            total: request.paths.len(),
            percent: index * 100 / request.paths.len(),
            current_file: path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("image")
                .to_owned(),
        });
        let image = decode_with_orientation(path)?;
        let rgb = flatten(&image, background);
        let mut jpeg = Vec::new();
        JpegEncoder::new_with_quality(&mut jpeg, request.quality)
            .encode_image(&DynamicImage::ImageRgb8(rgb.clone()))
            .map_err(|err| error("encode_error", err.to_string()))?;
        let (page_width, page_height) =
            page_dimensions(request, rgb.width(), rgb.height(), margin)?;
        let place = placement(
            &request.fitting,
            rgb.width(),
            rgb.height(),
            page_width,
            page_height,
            margin,
        )?;
        let image_id = document.add_object(Stream::new(dictionary! {
            "Type" => "XObject", "Subtype" => "Image", "Width" => rgb.width() as i64,
            "Height" => rgb.height() as i64, "ColorSpace" => "DeviceRGB", "BitsPerComponent" => 8,
            "Filter" => "DCTDecode",
        }, jpeg));
        let image_name = format!("Im{}", index + 1);
        let color = background.0.map(|channel| channel as f64 / 255.0);
        let clip = if place.clip {
            format!(
                "{margin:.3} {margin:.3} {:.3} {:.3} re W n\n",
                page_width - margin * 2.0,
                page_height - margin * 2.0
            )
        } else {
            String::new()
        };
        let content = format!(
            "q\n{:.5} {:.5} {:.5} rg\n0 0 {page_width:.3} {page_height:.3} re f\nq\n{clip}{:.3} 0 0 {:.3} {:.3} {:.3} cm\n/{image_name} Do\nQ\nQ",
            color[0], color[1], color[2], place.width, place.height, place.x, place.y
        );
        let content_id = document.add_object(Stream::new(Dictionary::new(), content.into_bytes()));
        let mut xobjects = Dictionary::new();
        xobjects.set(image_name.as_bytes(), image_id);
        let page_id = document.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), page_width.into(), page_height.into()],
            "Resources" => dictionary! { "XObject" => xobjects }, "Contents" => content_id,
        });
        kids.push(Object::Reference(page_id));
        report(ImagesPdfProgress {
            completed: index + 1,
            total: request.paths.len(),
            percent: (index + 1) * 100 / request.paths.len(),
            current_file: path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("image")
                .to_owned(),
        });
    }
    document.objects.insert(pages_id, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => request.paths.len() as i64 }));
    let catalog_id = document.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    document.trailer.set("Root", catalog_id);
    Ok((document, request.paths.len()))
}

fn images_to_pdf_sync<F>(
    request: ImagesPdfRequest,
    report: F,
) -> Result<ImagesPdfResult, ImagesPdfError>
where
    F: FnMut(ImagesPdfProgress),
{
    let destination = request
        .destination_dir
        .as_deref()
        .map(PathBuf::from)
        .or_else(|| {
            request
                .paths
                .first()
                .and_then(|path| Path::new(path).parent())
                .map(Path::to_path_buf)
        })
        .ok_or_else(|| {
            error(
                "destination_inaccessible",
                "No destination folder is available",
            )
        })?;
    if !destination.is_dir() {
        return Err(error(
            "destination_inaccessible",
            "The destination folder is inaccessible",
        ));
    }
    let (mut document, page_count) = create_pdf(&request, report)?;
    let mut bytes = Vec::new();
    document
        .save_to(&mut bytes)
        .map_err(|err| error("write_error", err.to_string()))?;
    let output_path = write_unique(&destination, &safe_stem(&request.output_name), &bytes)?;
    let new_size = fs::metadata(&output_path)
        .map_err(|err| error("read_error", err.to_string()))?
        .len();
    Ok(ImagesPdfResult {
        success: true,
        image_count: request.paths.len(),
        page_count,
        page_size_used: Some(page_size_label(&request)),
        output_path: Some(output_path.to_string_lossy().into_owned()),
        new_size: Some(new_size),
        error_code: None,
        error_message: None,
    })
}

struct RunningGuard;
impl Drop for RunningGuard {
    fn drop(&mut self) {
        IMAGES_PDF_RUNNING.store(false, Ordering::Release);
    }
}

#[tauri::command]
pub async fn images_to_pdf(window: tauri::Window, request: ImagesPdfRequest) -> ImagesPdfResult {
    if IMAGES_PDF_RUNNING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return ImagesPdfResult {
            success: false,
            image_count: 0,
            page_count: 0,
            page_size_used: None,
            output_path: None,
            new_size: None,
            error_code: Some("images_pdf_already_running".into()),
            error_message: Some("Another PDF creation is already running".into()),
        };
    }
    let _guard = RunningGuard;
    let progress_window = window.clone();
    match tauri::async_runtime::spawn_blocking(move || {
        images_to_pdf_sync(request, |progress| {
            let _ = progress_window.emit("images-pdf-progress", progress);
        })
    })
    .await
    {
        Ok(Ok(result)) => result,
        Ok(Err(err)) => ImagesPdfResult {
            success: false,
            image_count: 0,
            page_count: 0,
            page_size_used: None,
            output_path: None,
            new_size: None,
            error_code: Some(err.code),
            error_message: Some(err.message),
        },
        Err(err) => ImagesPdfResult {
            success: false,
            image_count: 0,
            page_count: 0,
            page_size_used: None,
            output_path: None,
            new_size: None,
            error_code: Some("write_error".into()),
            error_message: Some(err.to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};
    use tempfile::TempDir;

    fn request(paths: Vec<String>, destination: &Path, fitting: &str) -> ImagesPdfRequest {
        ImagesPdfRequest {
            paths,
            destination_dir: Some(destination.to_string_lossy().into()),
            output_name: "images document".into(),
            page_size: "a4".into(),
            custom_width_mm: 210.0,
            custom_height_mm: 297.0,
            orientation: "auto".into(),
            margin: "small".into(),
            custom_margin_mm: 10.0,
            fitting: fitting.into(),
            background_color: "#ffffff".into(),
            quality: 90,
        }
    }

    fn make_png(path: &Path, width: u32, height: u32, color: Rgba<u8>) {
        RgbaImage::from_pixel(width, height, color)
            .save(path)
            .unwrap();
    }

    fn object_number(object: &Object) -> f64 {
        match object {
            Object::Integer(value) => *value as f64,
            Object::Real(value) => *value as f64,
            _ => panic!("expected a PDF number"),
        }
    }

    fn page_dimensions_from_pdf(document: &Document, page: u32) -> (f64, f64) {
        let page_id = document.get_pages()[&page];
        let media = document
            .get_object(page_id)
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"MediaBox")
            .unwrap()
            .as_array()
            .unwrap();
        (object_number(&media[2]), object_number(&media[3]))
    }

    fn embedded_image_widths(document: &Document) -> Vec<i64> {
        document
            .get_pages()
            .into_values()
            .map(|page_id| {
                let page = document.get_object(page_id).unwrap().as_dict().unwrap();
                let resources = page.get(b"Resources").unwrap().as_dict().unwrap();
                let xobjects = resources.get(b"XObject").unwrap().as_dict().unwrap();
                let image_id = xobjects.iter().next().unwrap().1.as_reference().unwrap();
                document
                    .get_object(image_id)
                    .unwrap()
                    .as_stream()
                    .unwrap()
                    .dict
                    .get(b"Width")
                    .unwrap()
                    .as_i64()
                    .unwrap()
            })
            .collect()
    }

    #[test]
    fn converts_one_image_to_one_pdf_page() {
        let temp = TempDir::new().unwrap();
        let image = temp.path().join("single.png");
        make_png(&image, 30, 50, Rgba([10, 20, 30, 255]));
        let result = images_to_pdf_sync(
            request(vec![image.to_string_lossy().into()], temp.path(), "fit"),
            |_| {},
        )
        .unwrap();
        assert_eq!((result.image_count, result.page_count), (1, 1));
        assert_eq!(
            Document::load(result.output_path.unwrap())
                .unwrap()
                .get_pages()
                .len(),
            1
        );
    }

    #[test]
    fn converts_multiple_images_in_the_requested_order() {
        let temp = TempDir::new().unwrap();
        let wide = temp.path().join("wide.png");
        let narrow = temp.path().join("narrow.png");
        make_png(&wide, 120, 30, Rgba([255, 0, 0, 255]));
        make_png(&narrow, 25, 90, Rgba([0, 0, 255, 255]));
        let result = images_to_pdf_sync(
            request(
                vec![
                    narrow.to_string_lossy().into(),
                    wide.to_string_lossy().into(),
                ],
                temp.path(),
                "fit",
            ),
            |_| {},
        )
        .unwrap();
        let document = Document::load(result.output_path.unwrap()).unwrap();
        assert_eq!(document.get_pages().len(), 2);
        assert_eq!(embedded_image_widths(&document), vec![25, 120]);
    }

    #[test]
    fn applies_a4_and_fit_to_image_page_sizes() {
        let temp = TempDir::new().unwrap();
        let image = temp.path().join("portrait.png");
        make_png(&image, 40, 80, Rgba([1, 2, 3, 255]));
        let mut a4 = request(vec![image.to_string_lossy().into()], temp.path(), "fit");
        a4.output_name = "a4".into();
        let a4_result = images_to_pdf_sync(a4, |_| {}).unwrap();
        let a4_pdf = Document::load(a4_result.output_path.unwrap()).unwrap();
        let (width, height) = page_dimensions_from_pdf(&a4_pdf, 1);
        assert!((width - 595.276).abs() < 0.01 && (height - 841.89).abs() < 0.01);

        let mut fitted = request(vec![image.to_string_lossy().into()], temp.path(), "fit");
        fitted.page_size = "fitImage".into();
        fitted.output_name = "fitted".into();
        let fitted_result = images_to_pdf_sync(fitted, |_| {}).unwrap();
        let fitted_pdf = Document::load(fitted_result.output_path.unwrap()).unwrap();
        let (width, height) = page_dimensions_from_pdf(&fitted_pdf, 1);
        assert!((width - 66.0).abs() < 0.01 && (height - 96.0).abs() < 0.01);
    }

    #[test]
    fn applies_portrait_landscape_and_margin_options() {
        let temp = TempDir::new().unwrap();
        let image = temp.path().join("landscape.png");
        make_png(&image, 100, 30, Rgba([5, 6, 7, 255]));
        let mut portrait = request(vec![image.to_string_lossy().into()], temp.path(), "fit");
        portrait.orientation = "portrait".into();
        portrait.output_name = "portrait".into();
        let portrait_pdf = Document::load(
            images_to_pdf_sync(portrait, |_| {})
                .unwrap()
                .output_path
                .unwrap(),
        )
        .unwrap();
        let portrait_size = page_dimensions_from_pdf(&portrait_pdf, 1);
        assert!(portrait_size.1 > portrait_size.0);

        let mut landscape = request(vec![image.to_string_lossy().into()], temp.path(), "fit");
        landscape.orientation = "landscape".into();
        landscape.margin = "custom".into();
        landscape.custom_margin_mm = 12.7;
        landscape.output_name = "landscape".into();
        assert!((margin_points(&landscape).unwrap() - 36.0).abs() < 0.001);
        let landscape_pdf = Document::load(
            images_to_pdf_sync(landscape, |_| {})
                .unwrap()
                .output_path
                .unwrap(),
        )
        .unwrap();
        let landscape_size = page_dimensions_from_pdf(&landscape_pdf, 1);
        assert!(landscape_size.0 > landscape_size.1);
    }

    #[test]
    fn flattens_transparent_png_on_the_selected_background() {
        let source = DynamicImage::ImageRgba8(RgbaImage::from_pixel(1, 1, Rgba([255, 0, 0, 128])));
        let flattened = flatten(&source, Rgb([0, 0, 0]));
        let pixel = flattened.get_pixel(0, 0);
        assert!(pixel[0] >= 127 && pixel[0] <= 128);
        assert_eq!((pixel[1], pixel[2]), (0, 0));
    }

    #[test]
    fn rejects_an_invalid_image_file() {
        let temp = TempDir::new().unwrap();
        let invalid = temp.path().join("invalid.png");
        fs::write(&invalid, b"not an image").unwrap();
        let result = images_to_pdf_sync(
            request(vec![invalid.to_string_lossy().into()], temp.path(), "fit"),
            |_| {},
        );
        assert_eq!(result.unwrap_err().code, "invalid_image");
    }

    #[test]
    fn placement_handles_fit_fill_and_stretch() {
        let fit = placement("fit", 200, 100, 600.0, 800.0, 50.0).unwrap();
        assert_eq!((fit.width, fit.height, fit.clip), (500.0, 250.0, false));
        let fill = placement("fill", 200, 100, 600.0, 800.0, 50.0).unwrap();
        assert_eq!((fill.width, fill.height, fill.clip), (1400.0, 700.0, true));
        let stretch = placement("stretch", 200, 100, 600.0, 800.0, 50.0).unwrap();
        assert_eq!((stretch.width, stretch.height), (500.0, 700.0));
    }

    #[test]
    fn applies_exif_orientation_before_creating_the_page() {
        let temp = TempDir::new().unwrap();
        let input = temp.path().join("oriented image.jpg");
        let source = RgbImage::from_pixel(40, 20, Rgb([120, 80, 40]));
        let mut jpeg = Vec::new();
        JpegEncoder::new_with_quality(&mut jpeg, 90)
            .encode_image(&DynamicImage::ImageRgb8(source))
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
        assert_eq!((decoded.width(), decoded.height()), (20, 40));
    }

    #[test]
    fn creates_ordered_pages_flattens_transparency_and_preserves_originals() {
        let temp = TempDir::new().unwrap();
        let first = temp.path().join("première image.png");
        let second = temp.path().join("deuxième image.png");
        RgbaImage::from_pixel(40, 80, Rgba([255, 0, 0, 128]))
            .save(&first)
            .unwrap();
        RgbaImage::from_pixel(100, 30, Rgba([0, 0, 255, 255]))
            .save(&second)
            .unwrap();
        let original = fs::read(&first).unwrap();
        let result = images_to_pdf_sync(
            request(
                vec![
                    second.to_string_lossy().into(),
                    first.to_string_lossy().into(),
                ],
                temp.path(),
                "fit",
            ),
            |_| {},
        )
        .unwrap();
        let pdf = Document::load(result.output_path.unwrap()).unwrap();
        assert_eq!(pdf.get_pages().len(), 2);
        assert_eq!(original, fs::read(first).unwrap());
    }

    #[test]
    fn never_overwrites_existing_pdf() {
        let temp = TempDir::new().unwrap();
        let image = temp.path().join("image.png");
        RgbaImage::from_pixel(20, 20, Rgba([0, 0, 0, 255]))
            .save(&image)
            .unwrap();
        let make = || request(vec![image.to_string_lossy().into()], temp.path(), "fit");
        let first = images_to_pdf_sync(make(), |_| {})
            .unwrap()
            .output_path
            .unwrap();
        let second = images_to_pdf_sync(make(), |_| {})
            .unwrap()
            .output_path
            .unwrap();
        assert!(first.ends_with("images document.pdf"));
        assert!(second.ends_with("images document-2.pdf"));
    }
}
