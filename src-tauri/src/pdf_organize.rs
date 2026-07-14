use lopdf::{dictionary, Document, Object, ObjectId};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

static ORGANIZE_RUNNING: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrganizePageOperation {
    pub source_page: u32,
    pub rotation: i32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrganizePdfRequest {
    pub path: String,
    pub destination_dir: Option<String>,
    pub output_name: String,
    pub pages: Vec<OrganizePageOperation>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrganizePdfResult {
    pub success: bool,
    pub initial_pages: usize,
    pub final_pages: usize,
    pub output_path: Option<String>,
    pub new_size: Option<u64>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Debug)]
struct OrganizeError {
    code: String,
    message: String,
}

fn error(code: &str, message: impl Into<String>) -> OrganizeError {
    OrganizeError {
        code: code.into(),
        message: message.into(),
    }
}

fn load_pdf(path: &Path) -> Result<Document, OrganizeError> {
    if !path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
    {
        return Err(error("not_pdf", "The selected file is not a PDF"));
    }
    let document = Document::load(path).map_err(|err| match err {
        lopdf::Error::InvalidPassword | lopdf::Error::Decryption(_) => {
            error("password_protected_pdf", "The PDF is password protected")
        }
        lopdf::Error::IO(io) => error("read_error", io.to_string()),
        other => error("corrupted_pdf", other.to_string()),
    })?;
    if document.is_encrypted() {
        return Err(error(
            "password_protected_pdf",
            "The PDF is password protected",
        ));
    }
    Ok(document)
}

fn inherited_value(document: &Document, page_id: ObjectId, key: &[u8]) -> Option<Object> {
    let mut current = page_id;
    loop {
        let dictionary = document.get_object(current).ok()?.as_dict().ok()?;
        if let Ok(value) = dictionary.get(key) {
            return Some(value.clone());
        }
        current = dictionary.get(b"Parent").ok()?.as_reference().ok()?;
    }
}

fn normalized_rotation(value: i32) -> i32 {
    ((value % 360) + 360) % 360
}

fn build_organized_document(
    source: &Document,
    operations: &[OrganizePageOperation],
) -> Result<Document, OrganizeError> {
    if operations.is_empty() {
        return Err(error(
            "organize_all_pages_deleted",
            "At least one page must remain",
        ));
    }
    let source_pages = source.get_pages();
    let mut output = source.clone();
    let mut next_id = output.max_id + 1;
    let mut kids = Vec::with_capacity(operations.len());
    let pages_id = (next_id + operations.len() as u32, 0);
    let catalog_id = (pages_id.0 + 1, 0);

    for operation in operations {
        let source_id = *source_pages.get(&operation.source_page).ok_or_else(|| {
            error(
                "organize_page_out_of_range",
                "An organized page does not exist in the source PDF",
            )
        })?;
        let mut page = source
            .get_object(source_id)
            .map_err(|err| error("corrupted_pdf", err.to_string()))?
            .clone();
        let dictionary = page
            .as_dict_mut()
            .map_err(|err| error("corrupted_pdf", err.to_string()))?;
        for key in [b"Resources".as_slice(), b"MediaBox", b"CropBox"] {
            if !dictionary.has(key) {
                if let Some(value) = inherited_value(source, source_id, key) {
                    dictionary.set(key, value);
                }
            }
        }
        let source_rotation = inherited_value(source, source_id, b"Rotate")
            .and_then(|value| value.as_i64().ok())
            .unwrap_or(0) as i32;
        dictionary.set(
            "Rotate",
            normalized_rotation(source_rotation + operation.rotation) as i64,
        );
        dictionary.set("Parent", pages_id);
        let new_page_id = (next_id, 0);
        next_id += 1;
        output.objects.insert(new_page_id, page);
        kids.push(Object::Reference(new_page_id));
    }

    output.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Count" => kids.len() as i64,
            "Kids" => kids,
        }),
    );
    output.objects.insert(
        catalog_id,
        Object::Dictionary(dictionary! { "Type" => "Catalog", "Pages" => pages_id }),
    );
    output.trailer.set("Root", catalog_id);
    output.max_id = catalog_id.0;
    output.renumber_objects();
    output.compress();
    Ok(output)
}

fn safe_stem(value: &str) -> String {
    let trimmed = value
        .trim()
        .trim_end_matches(|character: char| character == '.' || character.is_whitespace());
    let without_extension = trimmed
        .strip_suffix(".pdf")
        .or_else(|| trimmed.strip_suffix(".PDF"))
        .unwrap_or(trimmed);
    let cleaned: String = without_extension
        .chars()
        .map(|character| {
            if "<>:\"/\\|?*".contains(character) || character.is_control() {
                '_'
            } else {
                character
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_end_matches('.');
    if cleaned.is_empty() {
        "organized-document".into()
    } else {
        cleaned.into()
    }
}

fn write_unique(
    directory: &Path,
    desired_stem: &str,
    bytes: &[u8],
) -> Result<PathBuf, OrganizeError> {
    for suffix in 1..=10_000usize {
        let name = if suffix == 1 {
            format!("{desired_stem}.pdf")
        } else {
            format!("{desired_stem}-{suffix}.pdf")
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

fn organize_pdf_sync(request: OrganizePdfRequest) -> Result<OrganizePdfResult, OrganizeError> {
    let source_path = PathBuf::from(&request.path);
    let source = load_pdf(&source_path)?;
    let initial_pages = source.get_pages().len();
    let destination = request
        .destination_dir
        .as_deref()
        .map(PathBuf::from)
        .or_else(|| source_path.parent().map(Path::to_path_buf))
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
    let final_pages = request.pages.len();
    let mut document = build_organized_document(&source, &request.pages)?;
    let mut bytes = Vec::new();
    document
        .save_to(&mut bytes)
        .map_err(|err| error("write_error", err.to_string()))?;
    let output_path = write_unique(&destination, &safe_stem(&request.output_name), &bytes)?;
    let new_size = fs::metadata(&output_path)
        .map_err(|err| error("read_error", err.to_string()))?
        .len();
    Ok(OrganizePdfResult {
        success: true,
        initial_pages,
        final_pages,
        output_path: Some(output_path.to_string_lossy().into_owned()),
        new_size: Some(new_size),
        error_code: None,
        error_message: None,
    })
}

struct RunningGuard;
impl Drop for RunningGuard {
    fn drop(&mut self) {
        ORGANIZE_RUNNING.store(false, Ordering::Release);
    }
}

#[tauri::command]
pub async fn organize_pdf(request: OrganizePdfRequest) -> OrganizePdfResult {
    if ORGANIZE_RUNNING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return OrganizePdfResult {
            success: false,
            initial_pages: 0,
            final_pages: 0,
            output_path: None,
            new_size: None,
            error_code: Some("organize_already_running".into()),
            error_message: Some("Another organization is already running".into()),
        };
    }
    let _guard = RunningGuard;
    match tauri::async_runtime::spawn_blocking(move || organize_pdf_sync(request)).await {
        Ok(Ok(result)) => result,
        Ok(Err(err)) => OrganizePdfResult {
            success: false,
            initial_pages: 0,
            final_pages: 0,
            output_path: None,
            new_size: None,
            error_code: Some(err.code),
            error_message: Some(err.message),
        },
        Err(err) => OrganizePdfResult {
            success: false,
            initial_pages: 0,
            final_pages: 0,
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
    use tempfile::TempDir;

    fn make_pdf(path: &Path) {
        let mut document = Document::with_version("1.5");
        let pages_id = document.new_object_id();
        let mut kids = Vec::new();
        for (index, (width, height)) in [(200, 300), (400, 250), (600, 700)].into_iter().enumerate()
        {
            let page_id = document.add_object(dictionary! {
                "Type" => "Page", "Parent" => pages_id,
                "MediaBox" => vec![0.into(), 0.into(), width.into(), height.into()],
                "Resources" => dictionary! {}, "Rotate" => if index == 1 { 90 } else { 0 },
            });
            kids.push(Object::Reference(page_id));
        }
        document.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => 3 }),
        );
        let catalog_id =
            document.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        document.trailer.set("Root", catalog_id);
        document.save(path).unwrap();
    }

    fn request(source: &Path, destination: &Path, pages: Vec<(u32, i32)>) -> OrganizePdfRequest {
        OrganizePdfRequest {
            path: source.to_string_lossy().into(),
            destination_dir: Some(destination.to_string_lossy().into()),
            output_name: "résultat organisé".into(),
            pages: pages
                .into_iter()
                .map(|(source_page, rotation)| OrganizePageOperation {
                    source_page,
                    rotation,
                })
                .collect(),
        }
    }

    #[test]
    fn reorders_rotates_deletes_and_duplicates_pages() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("dôcument source.pdf");
        make_pdf(&source);
        let result = organize_pdf_sync(request(
            &source,
            temp.path(),
            vec![(3, 90), (1, 180), (3, -90)],
        ))
        .unwrap();
        assert_eq!(result.initial_pages, 3);
        assert_eq!(result.final_pages, 3);
        let output = Document::load(result.output_path.unwrap()).unwrap();
        let details: Vec<(i64, i64)> = output
            .get_pages()
            .into_values()
            .map(|id| {
                let dict = output.get_object(id).unwrap().as_dict().unwrap();
                let media = dict.get(b"MediaBox").unwrap().as_array().unwrap();
                (
                    media[2].as_i64().unwrap(),
                    dict.get(b"Rotate").unwrap().as_i64().unwrap(),
                )
            })
            .collect();
        assert_eq!(details, vec![(600, 90), (200, 180), (600, 270)]);
    }

    #[test]
    fn preserves_original_and_creates_unique_names() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source.pdf");
        make_pdf(&source);
        let original = fs::read(&source).unwrap();
        let first = organize_pdf_sync(request(&source, temp.path(), vec![(1, 0)]))
            .unwrap()
            .output_path
            .unwrap();
        let second = organize_pdf_sync(request(&source, temp.path(), vec![(1, 0)]))
            .unwrap()
            .output_path
            .unwrap();
        assert!(first.ends_with("résultat organisé.pdf"));
        assert!(second.ends_with("résultat organisé-2.pdf"));
        assert_eq!(original, fs::read(source).unwrap());
    }

    #[test]
    fn rejects_empty_invalid_and_corrupt_inputs() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source.pdf");
        make_pdf(&source);
        assert_eq!(
            organize_pdf_sync(request(&source, temp.path(), vec![]))
                .unwrap_err()
                .code,
            "organize_all_pages_deleted"
        );
        assert_eq!(
            organize_pdf_sync(request(&source, temp.path(), vec![(4, 0)]))
                .unwrap_err()
                .code,
            "organize_page_out_of_range"
        );
        let corrupt = temp.path().join("corrupt.pdf");
        fs::write(&corrupt, b"%PDF invalid").unwrap();
        assert_eq!(
            organize_pdf_sync(request(&corrupt, temp.path(), vec![(1, 0)]))
                .unwrap_err()
                .code,
            "corrupted_pdf"
        );
    }
}
