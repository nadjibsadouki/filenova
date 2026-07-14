use image::{codecs::jpeg::JpegEncoder, DynamicImage, ImageFormat, ImageReader};
use pdfium_render::prelude::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs::{self, OpenOptions},
    io::{Cursor, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        OnceLock,
    },
};
use tauri::{Emitter, Manager};

static CONVERSION_RUNNING: AtomicBool = AtomicBool::new(false);
static PDFIUM: OnceLock<Result<Pdfium, String>> = OnceLock::new();

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PdfImagesError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PdfImagesInfo {
    pub path: String,
    pub name: String,
    pub size: u64,
    pub pages: usize,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PdfToImagesRequest {
    pub path: String,
    pub destination_dir: Option<String>,
    pub page_mode: String,
    pub current_page: Option<usize>,
    pub custom_pages: Option<String>,
    pub format: String,
    pub quality: u8,
    pub dpi: u16,
    pub transparent_background: bool,
    pub password: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PdfImageOutput {
    pub path: String,
    pub name: String,
    pub page: usize,
    pub width: u32,
    pub height: u32,
    pub size: u64,
    pub format: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PdfImageFailure {
    pub page: usize,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PdfToImagesResult {
    pub success: bool,
    pub files: Vec<PdfImageOutput>,
    pub failures: Vec<PdfImageFailure>,
    pub total_pages: usize,
    pub failed_count: usize,
    pub format: String,
    pub dpi: u16,
    pub total_size: u64,
    pub destination_dir: Option<String>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PdfImagesProgress {
    pub completed: usize,
    pub total: usize,
    pub percent: usize,
    pub current_page: usize,
    pub current_file: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PdfImageOpenValidation {
    pub path: String,
    pub size: u64,
    pub format: String,
}

fn error(code: &str, message: impl Into<String>) -> PdfImagesError {
    PdfImagesError {
        code: code.into(),
        message: message.into(),
    }
}

fn map_pdfium_error(err: PdfiumError) -> PdfImagesError {
    match err {
        PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError) => error(
            "pdf_images_password_invalid",
            "A valid password is required to open this PDF",
        ),
        PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::SecurityError) => error(
            "pdf_images_security_error",
            "PDFium rejected this PDF's security settings",
        ),
        PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::FileError) => {
            error("pdf_images_read_error", "The PDF could not be read")
        }
        PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::FormatError) => error(
            "corrupted_pdf",
            "The PDF is corrupted or has an invalid format",
        ),
        other => error("pdf_images_render_error", other.to_string()),
    }
}

fn pdfium_path(app: &tauri::AppHandle) -> Result<PathBuf, PdfImagesError> {
    let mut candidates = Vec::new();
    if let Ok(path) = app
        .path()
        .resolve("pdfium/pdfium.dll", tauri::path::BaseDirectory::Resource)
    {
        candidates.push(path);
    }
    if let Ok(executable) = std::env::current_exe() {
        if let Some(directory) = executable.parent() {
            candidates.push(directory.join("pdfium").join("pdfium.dll"));
            candidates.push(directory.join("pdfium.dll"));
        }
    }
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/pdfium/pdfium.dll"));
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| {
            error(
                "pdfium_unavailable",
                "The bundled PDFium library could not be found",
            )
        })
}

fn bind_pdfium(path: &Path) -> Result<&'static Pdfium, PdfImagesError> {
    match PDFIUM.get_or_init(|| {
        Pdfium::bind_to_library(path)
            .map(Pdfium::new)
            .map_err(|err| err.to_string())
    }) {
        Ok(pdfium) => Ok(pdfium),
        Err(message) => Err(error("pdfium_unavailable", message.clone())),
    }
}

fn validate_pdf_path(path: &Path) -> Result<(), PdfImagesError> {
    if !path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
    {
        return Err(error("not_pdf", "The selected file is not a PDF"));
    }
    if !path.is_file() {
        return Err(error("pdf_images_read_error", "The PDF could not be found"));
    }
    Ok(())
}

fn validate_output_image_sync(path: String) -> Result<PdfImageOpenValidation, PdfImagesError> {
    if path.starts_with("file://") || path.starts_with(r"\\?\") {
        return Err(error(
            "open_image_invalid_path",
            "The image must use a normal local filesystem path",
        ));
    }
    let source = PathBuf::from(&path);
    if !source.is_absolute() {
        return Err(error(
            "open_image_invalid_path",
            "The image path must be absolute",
        ));
    }
    let metadata = fs::metadata(&source)
        .map_err(|_| error("open_image_missing", "The created image no longer exists"))?;
    if !metadata.is_file() {
        return Err(error(
            "open_image_invalid_path",
            "The output path is not an image file",
        ));
    }
    if metadata.len() == 0 {
        return Err(error("open_image_empty", "The created image is empty"));
    }
    let reader = ImageReader::open(&source)
        .map_err(|err| error("open_image_decode_failed", err.to_string()))?
        .with_guessed_format()
        .map_err(|err| error("open_image_decode_failed", err.to_string()))?;
    let format = match reader.format() {
        Some(ImageFormat::Jpeg) => "JPG",
        Some(ImageFormat::Png) => "PNG",
        Some(ImageFormat::WebP) => "WEBP",
        _ => {
            return Err(error(
                "open_image_unsupported",
                "Only JPG, PNG and WebP output images can be opened",
            ))
        }
    };
    reader
        .decode()
        .map_err(|err| error("open_image_decode_failed", err.to_string()))?;
    Ok(PdfImageOpenValidation {
        path,
        size: metadata.len(),
        format: format.into(),
    })
}

#[tauri::command]
pub async fn validate_pdf_image_output(
    path: String,
) -> Result<PdfImageOpenValidation, PdfImagesError> {
    tauri::async_runtime::spawn_blocking(move || validate_output_image_sync(path))
        .await
        .map_err(|err| error("open_image_decode_failed", err.to_string()))?
}

fn password_ref(password: &Option<String>) -> Option<&str> {
    password
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn inspect_sync(
    path: String,
    password: Option<String>,
    library: PathBuf,
) -> Result<PdfImagesInfo, PdfImagesError> {
    let source = PathBuf::from(&path);
    validate_pdf_path(&source)?;
    let metadata =
        fs::metadata(&source).map_err(|err| error("pdf_images_read_error", err.to_string()))?;
    let pdfium = bind_pdfium(&library)?;
    let document = pdfium
        .load_pdf_from_file(&source, password_ref(&password))
        .map_err(map_pdfium_error)?;
    Ok(PdfImagesInfo {
        path,
        name: source
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("document.pdf")
            .to_owned(),
        size: metadata.len(),
        pages: document.pages().len().max(0) as usize,
    })
}

#[tauri::command]
pub async fn inspect_pdf_for_images(
    app: tauri::AppHandle,
    path: String,
    password: Option<String>,
) -> Result<PdfImagesInfo, PdfImagesError> {
    let library = pdfium_path(&app)?;
    tauri::async_runtime::spawn_blocking(move || inspect_sync(path, password, library))
        .await
        .map_err(|err| error("pdf_images_read_error", err.to_string()))?
}

fn parse_custom_pages(value: &str, total: usize) -> Result<Vec<usize>, PdfImagesError> {
    if value.trim().is_empty() {
        return Err(error(
            "pdf_images_pages_required",
            "Enter at least one page",
        ));
    }
    let mut pages = Vec::new();
    let mut seen = HashSet::new();
    for raw in value.split(',') {
        let token = raw.trim();
        let parts: Vec<&str> = token.split('-').map(str::trim).collect();
        if parts.is_empty() || parts.len() > 2 || parts.iter().any(|part| part.is_empty()) {
            return Err(error(
                "pdf_images_invalid_range",
                "Use pages and ascending ranges such as 1,3,5-8",
            ));
        }
        let start = parts[0]
            .parse::<usize>()
            .map_err(|_| error("pdf_images_invalid_pages", "Invalid page number"))?;
        let end = if parts.len() == 2 {
            parts[1]
                .parse::<usize>()
                .map_err(|_| error("pdf_images_invalid_range", "Invalid page range"))?
        } else {
            start
        };
        if start == 0 {
            return Err(error(
                "pdf_images_invalid_pages",
                "Page numbers must be greater than zero",
            ));
        }
        if end < start {
            return Err(error(
                "pdf_images_invalid_range",
                "The start of a range cannot exceed its end",
            ));
        }
        if end > total {
            return Err(error(
                "pdf_images_page_out_of_range",
                format!("Pages must be between 1 and {total}"),
            ));
        }
        for page in start..=end {
            if seen.insert(page) {
                pages.push(page);
            }
        }
    }
    Ok(pages)
}

fn selected_pages(
    request: &PdfToImagesRequest,
    total: usize,
) -> Result<Vec<usize>, PdfImagesError> {
    match request.page_mode.as_str() {
        "all" => Ok((1..=total).collect()),
        "current" => {
            let page = request.current_page.unwrap_or(0);
            if page == 0 || page > total {
                Err(error(
                    "pdf_images_page_out_of_range",
                    format!("Pages must be between 1 and {total}"),
                ))
            } else {
                Ok(vec![page])
            }
        }
        "custom" => parse_custom_pages(request.custom_pages.as_deref().unwrap_or_default(), total),
        _ => Err(error(
            "pdf_images_invalid_pages",
            "The page selection mode is invalid",
        )),
    }
}

fn safe_stem(path: &Path) -> String {
    let raw = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("document");
    let cleaned: String = raw
        .chars()
        .map(|character| {
            if "<>:\"/\\|?*".contains(character) || character.is_control() {
                '_'
            } else {
                character
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_end_matches(['.', ' ']);
    if cleaned.is_empty() {
        "document".into()
    } else {
        cleaned.into()
    }
}

fn output_name(stem: &str, page: usize, extension: &str, suffix: usize) -> String {
    if suffix == 1 {
        format!("{stem}-page-{page:03}.{extension}")
    } else {
        format!("{stem}-page-{page:03}-{suffix}.{extension}")
    }
}

fn encode(image: &DynamicImage, format: &str, quality: u8) -> Result<Vec<u8>, PdfImagesError> {
    let mut bytes = Vec::new();
    match format {
        "png" => image.write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png),
        "jpg" => JpegEncoder::new_with_quality(&mut bytes, quality).encode_image(&image.to_rgb8()),
        "webp" => {
            let encoded = webp::Encoder::from_image(image)
                .map_err(|message| error("pdf_images_encode_error", message))?
                .encode(quality as f32);
            bytes.extend_from_slice(&encoded);
            return Ok(bytes);
        }
        _ => {
            return Err(error(
                "unsupported_conversion_format",
                "Only JPG, PNG and WebP are supported",
            ))
        }
    }
    .map_err(|err| error("pdf_images_encode_error", err.to_string()))?;
    Ok(bytes)
}

fn write_unique(
    directory: &Path,
    stem: &str,
    page: usize,
    extension: &str,
    bytes: &[u8],
) -> Result<PathBuf, PdfImagesError> {
    for suffix in 1..=10_000 {
        let path = directory.join(output_name(stem, page, extension, suffix));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                if let Err(err) = file.write_all(bytes).and_then(|_| file.flush()) {
                    let _ = fs::remove_file(&path);
                    return Err(error(
                        if err.raw_os_error() == Some(112) {
                            "pdf_images_insufficient_space"
                        } else {
                            "pdf_images_write_error"
                        },
                        err.to_string(),
                    ));
                }
                return Ok(path);
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => {
                return Err(error(
                    if err.raw_os_error() == Some(112) {
                        "pdf_images_insufficient_space"
                    } else {
                        "pdf_images_destination_inaccessible"
                    },
                    err.to_string(),
                ))
            }
        }
    }
    Err(error(
        "pdf_images_write_error",
        "Unable to create a unique output filename",
    ))
}

fn convert_sync<F>(
    request: PdfToImagesRequest,
    library: PathBuf,
    progress: F,
) -> Result<PdfToImagesResult, PdfImagesError>
where
    F: Fn(PdfImagesProgress),
{
    let source = PathBuf::from(&request.path);
    validate_pdf_path(&source)?;
    if !(10..=100).contains(&request.quality) {
        return Err(error(
            "pdf_images_invalid_quality",
            "Quality must be between 10 and 100",
        ));
    }
    if !(1..=2400).contains(&request.dpi) {
        return Err(error(
            "pdf_images_invalid_dpi",
            "DPI must be between 1 and 2400",
        ));
    }
    if !matches!(request.format.as_str(), "jpg" | "png" | "webp") {
        return Err(error(
            "unsupported_conversion_format",
            "Only JPG, PNG and WebP are supported",
        ));
    }

    let destination = request
        .destination_dir
        .as_deref()
        .map(PathBuf::from)
        .or_else(|| source.parent().map(Path::to_path_buf))
        .ok_or_else(|| {
            error(
                "pdf_images_destination_inaccessible",
                "No destination folder is available",
            )
        })?;
    if !destination.is_dir() {
        return Err(error(
            "pdf_images_destination_inaccessible",
            "The destination folder is inaccessible",
        ));
    }

    let pdfium = bind_pdfium(&library)?;
    let document = pdfium
        .load_pdf_from_file(&source, password_ref(&request.password))
        .map_err(map_pdfium_error)?;
    let pages = selected_pages(&request, document.pages().len().max(0) as usize)?;
    let total = pages.len();
    let stem = safe_stem(&source);
    let mut files = Vec::with_capacity(total);
    let mut failures = Vec::new();

    for (completed, page_number) in pages.into_iter().enumerate() {
        let expected_name = output_name(&stem, page_number, &request.format, 1);
        progress(PdfImagesProgress {
            completed,
            total,
            percent: completed * 100 / total.max(1),
            current_page: page_number,
            current_file: expected_name.clone(),
        });
        let rendered = (|| -> Result<PdfImageOutput, PdfImagesError> {
            let page = document
                .pages()
                .get((page_number - 1) as PdfPageIndex)
                .map_err(map_pdfium_error)?;
            let scale = request.dpi as f64 / 72.0;
            let estimated_width = (page.width().value as f64 * scale).ceil() as u64;
            let estimated_height = (page.height().value as f64 * scale).ceil() as u64;
            let estimated_bytes = estimated_width
                .checked_mul(estimated_height)
                .and_then(|pixels| pixels.checked_mul(4))
                .ok_or_else(|| {
                    error(
                        "pdf_images_insufficient_memory",
                        "The rendered page is too large for memory",
                    )
                })?;
            if estimated_bytes > 1_500_000_000 {
                return Err(error(
                    "pdf_images_insufficient_memory",
                    "The selected resolution requires too much memory",
                ));
            }
            let clear = if request.format == "png" && request.transparent_background {
                PdfColor::new(0, 0, 0, 0)
            } else {
                PdfColor::WHITE
            };
            let bitmap = page
                .render_with_config(
                    &PdfRenderConfig::new()
                        .scale_page_by_factor(request.dpi as f32 / 72.0)
                        .set_clear_color(clear)
                        .render_annotations(true)
                        .render_form_data(true),
                )
                .map_err(map_pdfium_error)?;
            let image = bitmap.as_image().map_err(map_pdfium_error)?;
            let width = image.width();
            let height = image.height();
            if width == 0 || height == 0 {
                return Err(error(
                    "pdf_images_render_error",
                    "PDFium returned invalid page dimensions",
                ));
            }
            let encoded = encode(&image, &request.format, request.quality)?;
            let output = write_unique(&destination, &stem, page_number, &request.format, &encoded)?;
            let size = fs::metadata(&output)
                .map_err(|err| error("pdf_images_write_error", err.to_string()))?
                .len();
            let name = output
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("image")
                .to_owned();
            Ok(PdfImageOutput {
                path: output.to_string_lossy().into_owned(),
                name,
                page: page_number,
                width,
                height,
                size,
                format: request.format.to_uppercase(),
            })
        })();
        match rendered {
            Ok(output) => {
                let current_file = output.name.clone();
                files.push(output);
                progress(PdfImagesProgress {
                    completed: completed + 1,
                    total,
                    percent: (completed + 1) * 100 / total.max(1),
                    current_page: page_number,
                    current_file,
                });
            }
            Err(err) => {
                failures.push(PdfImageFailure {
                    page: page_number,
                    code: err.code,
                    message: err.message,
                });
                progress(PdfImagesProgress {
                    completed: completed + 1,
                    total,
                    percent: (completed + 1) * 100 / total.max(1),
                    current_page: page_number,
                    current_file: expected_name,
                });
            }
        }
    }
    let total_size = files.iter().map(|file| file.size).sum();
    let first_error = failures.first().cloned();
    Ok(PdfToImagesResult {
        success: !files.is_empty(),
        total_pages: files.len(),
        failed_count: failures.len(),
        total_size,
        format: request.format.to_uppercase(),
        dpi: request.dpi,
        files,
        failures,
        destination_dir: Some(destination.to_string_lossy().into_owned()),
        error_code: first_error.as_ref().map(|failure| failure.code.clone()),
        error_message: first_error.map(|failure| failure.message),
    })
}

fn failed_result(err: PdfImagesError, format: String, dpi: u16) -> PdfToImagesResult {
    PdfToImagesResult {
        success: false,
        files: vec![],
        failures: vec![],
        total_pages: 0,
        failed_count: 0,
        format,
        dpi,
        total_size: 0,
        destination_dir: None,
        error_code: Some(err.code),
        error_message: Some(err.message),
    }
}

struct RunningGuard;
impl Drop for RunningGuard {
    fn drop(&mut self) {
        CONVERSION_RUNNING.store(false, Ordering::Release);
    }
}

#[tauri::command]
pub async fn pdf_to_images(
    app: tauri::AppHandle,
    request: PdfToImagesRequest,
) -> PdfToImagesResult {
    let format = request.format.to_uppercase();
    let dpi = request.dpi;
    if CONVERSION_RUNNING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return failed_result(
            error(
                "pdf_images_already_running",
                "Another PDF to Images conversion is already running",
            ),
            format,
            dpi,
        );
    }
    let _guard = RunningGuard;
    let library = match pdfium_path(&app) {
        Ok(path) => path,
        Err(err) => return failed_result(err, format, dpi),
    };
    let event_app = app.clone();
    match tauri::async_runtime::spawn_blocking(move || {
        convert_sync(request, library, |state| {
            let _ = event_app.emit("pdf-images-progress", state);
        })
    })
    .await
    {
        Ok(Ok(result)) => result,
        Ok(Err(err)) => failed_result(err, format, dpi),
        Err(err) => failed_result(
            error("pdf_images_render_error", err.to_string()),
            format,
            dpi,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::GenericImageView;
    use lopdf::{dictionary, Document, Object};
    #[cfg(target_os = "windows")]
    use std::sync::Mutex;

    #[cfg(target_os = "windows")]
    static PDFIUM_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn parses_custom_pages_in_order_and_removes_duplicates() {
        assert_eq!(
            parse_custom_pages("1,3,5-8,3", 10).unwrap(),
            vec![1, 3, 5, 6, 7, 8]
        );
    }

    #[test]
    fn rejects_invalid_and_missing_pages() {
        assert_eq!(
            parse_custom_pages("0,2", 4).unwrap_err().code,
            "pdf_images_invalid_pages"
        );
        assert_eq!(
            parse_custom_pages("2-1", 4).unwrap_err().code,
            "pdf_images_invalid_range"
        );
        assert_eq!(
            parse_custom_pages("5", 4).unwrap_err().code,
            "pdf_images_page_out_of_range"
        );
    }

    #[test]
    fn creates_padded_unique_names_without_overwriting() {
        let directory = tempfile::tempdir().unwrap();
        let first = write_unique(directory.path(), "résumé final", 1, "png", b"one").unwrap();
        let second = write_unique(directory.path(), "résumé final", 1, "png", b"two").unwrap();
        assert!(first.ends_with("résumé final-page-001.png"));
        assert!(second.ends_with("résumé final-page-001-2.png"));
        assert_eq!(fs::read(first).unwrap(), b"one");
    }

    #[test]
    fn validates_jpg_png_and_webp_outputs_in_unicode_paths() {
        let temp = tempfile::Builder::new()
            .prefix("FileNova images ouvertes ")
            .tempdir()
            .unwrap();
        let nested = temp.path().join("résultats avec espaces");
        fs::create_dir(&nested).unwrap();
        let image = DynamicImage::new_rgba8(8, 6);
        for (name, format, expected) in [
            ("épreuve photo.jpg", ImageFormat::Jpeg, "JPG"),
            ("épreuve image.png", ImageFormat::Png, "PNG"),
            ("épreuve web.webp", ImageFormat::WebP, "WEBP"),
        ] {
            let path = nested.join(name);
            image.save_with_format(&path, format).unwrap();
            let exact_path = path.to_string_lossy().into_owned();
            let validation = validate_output_image_sync(exact_path.clone()).unwrap();
            assert_eq!(validation.path, exact_path);
            assert_eq!(validation.format, expected);
            assert!(validation.size > 0);
        }
    }

    #[test]
    fn rejects_missing_empty_and_non_image_open_targets() {
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("missing.png");
        assert_eq!(
            validate_output_image_sync(missing.to_string_lossy().into_owned())
                .unwrap_err()
                .code,
            "open_image_missing"
        );
        let empty = temp.path().join("empty.png");
        fs::write(&empty, []).unwrap();
        assert_eq!(
            validate_output_image_sync(empty.to_string_lossy().into_owned())
                .unwrap_err()
                .code,
            "open_image_empty"
        );
        let text = temp.path().join("not-an-image.png");
        fs::write(&text, b"not an image").unwrap();
        assert_eq!(
            validate_output_image_sync(text.to_string_lossy().into_owned())
                .unwrap_err()
                .code,
            "open_image_decode_failed"
        );
    }

    fn make_pdf(path: &Path, sizes: &[(i64, i64)]) {
        let mut document = Document::with_version("1.7");
        let pages_id = document.new_object_id();
        let pages = sizes
            .iter()
            .map(|(width, height)| {
                document.add_object(dictionary! {
                    "Type" => "Page", "Parent" => pages_id,
                    "MediaBox" => vec![0.into(), 0.into(), (*width).into(), (*height).into()],
                    "Resources" => dictionary! {},
                })
            })
            .collect::<Vec<_>>();
        document.objects.insert(pages_id, Object::Dictionary(dictionary! {
            "Type" => "Pages", "Kids" => pages.iter().copied().map(Object::Reference).collect::<Vec<_>>(), "Count" => pages.len() as i64,
        }));
        let catalog = document.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        document.trailer.set("Root", catalog);
        document.save(path).unwrap();
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn renders_multiple_pages_formats_dpi_custom_sizes_and_preserves_original() {
        let _pdfium_test = PDFIUM_TEST_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("document spécial.pdf");
        let destination = temp.path().join("sortie");
        fs::create_dir(&destination).unwrap();
        make_pdf(&source, &[(200, 300), (400, 200)]);
        let original = fs::read(&source).unwrap();
        let library = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/pdfium/pdfium.dll");
        let result = convert_sync(
            PdfToImagesRequest {
                path: source.to_string_lossy().into_owned(),
                destination_dir: Some(destination.to_string_lossy().into_owned()),
                page_mode: "all".into(),
                current_page: None,
                custom_pages: None,
                format: "png".into(),
                quality: 90,
                dpi: 72,
                transparent_background: true,
                password: None,
            },
            library.clone(),
            |_| {},
        )
        .unwrap();
        assert_eq!(result.files.len(), 2);
        assert_eq!((result.files[0].width, result.files[0].height), (200, 300));
        assert_eq!((result.files[1].width, result.files[1].height), (400, 200));
        assert_eq!(
            image::open(&result.files[0].path).unwrap().dimensions(),
            (200, 300)
        );

        let webp = convert_sync(
            PdfToImagesRequest {
                path: source.to_string_lossy().into_owned(),
                destination_dir: Some(destination.to_string_lossy().into_owned()),
                page_mode: "custom".into(),
                current_page: None,
                custom_pages: Some("2,1,2".into()),
                format: "webp".into(),
                quality: 75,
                dpi: 150,
                transparent_background: false,
                password: None,
            },
            library,
            |_| {},
        )
        .unwrap();
        assert_eq!(
            webp.files.iter().map(|file| file.page).collect::<Vec<_>>(),
            vec![2, 1]
        );
        let jpg = convert_sync(
            PdfToImagesRequest {
                path: source.to_string_lossy().into_owned(),
                destination_dir: Some(destination.to_string_lossy().into_owned()),
                page_mode: "current".into(),
                current_page: Some(1),
                custom_pages: None,
                format: "jpg".into(),
                quality: 60,
                dpi: 300,
                transparent_background: false,
                password: None,
            },
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/pdfium/pdfium.dll"),
            |_| {},
        )
        .unwrap();
        assert_eq!(jpg.files.len(), 1);
        assert!((832..=834).contains(&jpg.files[0].width));
        assert_eq!(jpg.files[0].height, 1250);
        assert_eq!(
            image::open(&jpg.files[0].path).unwrap().dimensions(),
            (jpg.files[0].width, 1250)
        );

        let encrypted_path = temp.path().join("document protégé.pdf");
        let mut encrypted = Document::load(&source).unwrap();
        encrypted.trailer.set(
            "ID",
            Object::Array(vec![
                Object::String(
                    b"filenova-test-id-1".to_vec(),
                    lopdf::StringFormat::Hexadecimal,
                ),
                Object::String(
                    b"filenova-test-id-2".to_vec(),
                    lopdf::StringFormat::Hexadecimal,
                ),
            ]),
        );
        let encryption = lopdf::EncryptionVersion::V2 {
            document: &encrypted,
            owner_password: "owner-secret",
            user_password: "mot-de-passe",
            key_length: 128,
            permissions: lopdf::Permissions::all(),
        };
        let encryption = lopdf::EncryptionState::try_from(encryption).unwrap();
        encrypted.encrypt(&encryption).unwrap();
        encrypted.save(&encrypted_path).unwrap();
        assert_eq!(
            inspect_sync(
                encrypted_path.to_string_lossy().into_owned(),
                None,
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/pdfium/pdfium.dll")
            )
            .unwrap_err()
            .code,
            "pdf_images_password_invalid"
        );
        assert_eq!(
            inspect_sync(
                encrypted_path.to_string_lossy().into_owned(),
                Some("mot-de-passe".into()),
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/pdfium/pdfium.dll")
            )
            .unwrap()
            .pages,
            2
        );
        let protected = convert_sync(
            PdfToImagesRequest {
                path: encrypted_path.to_string_lossy().into_owned(),
                destination_dir: Some(destination.to_string_lossy().into_owned()),
                page_mode: "current".into(),
                current_page: Some(2),
                custom_pages: None,
                format: "png".into(),
                quality: 90,
                dpi: 72,
                transparent_background: false,
                password: Some("mot-de-passe".into()),
            },
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/pdfium/pdfium.dll"),
            |_| {},
        )
        .unwrap();
        assert_eq!(protected.files[0].page, 2);
        assert_eq!(fs::read(source).unwrap(), original);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn renders_a_single_page_pdf() {
        let _pdfium_test = PDFIUM_TEST_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("one page.pdf");
        make_pdf(&source, &[(612, 792)]);
        let original = fs::read(&source).unwrap();
        let result = convert_sync(
            PdfToImagesRequest {
                path: source.to_string_lossy().into_owned(),
                destination_dir: Some(temp.path().to_string_lossy().into_owned()),
                page_mode: "all".into(),
                current_page: None,
                custom_pages: None,
                format: "png".into(),
                quality: 90,
                dpi: 72,
                transparent_background: true,
                password: None,
            },
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/pdfium/pdfium.dll"),
            |_| {},
        )
        .unwrap();
        assert_eq!(result.total_pages, 1);
        assert_eq!(result.failed_count, 0);
        assert_eq!((result.files[0].width, result.files[0].height), (612, 792));
        assert_eq!(fs::read(source).unwrap(), original);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn rejects_a_corrupted_pdf_without_panicking() {
        let _pdfium_test = PDFIUM_TEST_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("corrupted.pdf");
        fs::write(&source, b"%PDF-1.7\nthis is not a valid document").unwrap();
        let error = inspect_sync(
            source.to_string_lossy().into_owned(),
            None,
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/pdfium/pdfium.dll"),
        )
        .unwrap_err();
        assert_eq!(error.code, "corrupted_pdf");
    }
}
