use lopdf::{dictionary, Document, Object, ObjectId};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PdfError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PdfInfo {
    pub path: String,
    pub name: String,
    pub size: u64,
    pub pages: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergePdfRequest {
    pub paths: Vec<String>,
    pub destination_dir: Option<String>,
    pub output_name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MergePdfResult {
    pub success: bool,
    pub pdf_count: usize,
    pub total_pages: usize,
    pub new_size: Option<u64>,
    pub output_path: Option<String>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}

fn error(code: &str, message: impl Into<String>) -> PdfError {
    PdfError {
        code: code.into(),
        message: message.into(),
    }
}

fn map_load_error(err: lopdf::Error) -> PdfError {
    match err {
        lopdf::Error::InvalidPassword | lopdf::Error::Decryption(_) => {
            error("password_protected_pdf", "The PDF is password protected")
        }
        lopdf::Error::IO(io) => error("read_error", io.to_string()),
        other => error("corrupted_pdf", other.to_string()),
    }
}

fn load_pdf(path: &Path) -> Result<Document, PdfError> {
    if !path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
    {
        return Err(error("not_pdf", "The selected file is not a PDF"));
    }
    let document = Document::load(path).map_err(map_load_error)?;
    if document.is_encrypted() {
        return Err(error(
            "password_protected_pdf",
            "The PDF is password protected",
        ));
    }
    Ok(document)
}

#[tauri::command]
pub fn inspect_pdf(path: String) -> Result<PdfInfo, PdfError> {
    let source = PathBuf::from(&path);
    let metadata = fs::metadata(&source).map_err(|err| error("read_error", err.to_string()))?;
    let document = load_pdf(&source)?;
    Ok(PdfInfo {
        path,
        name: source
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("document.pdf")
            .to_owned(),
        size: metadata.len(),
        pages: document.get_pages().len(),
    })
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

fn build_merged_document(paths: &[String]) -> Result<(Document, usize), PdfError> {
    let mut imported_objects = BTreeMap::new();
    let mut ordered_pages = Vec::new();
    let mut next_id = 1;

    for path in paths {
        let mut document = load_pdf(Path::new(path))?;
        document.renumber_objects_with(next_id);
        next_id = document.max_id + 1;

        for page_id in document.get_pages().into_values() {
            let mut page = document
                .get_object(page_id)
                .map_err(|err| error("corrupted_pdf", err.to_string()))?
                .clone();
            if let Ok(page_dict) = page.as_dict_mut() {
                for key in [b"Resources".as_slice(), b"MediaBox", b"CropBox", b"Rotate"] {
                    if !page_dict.has(key) {
                        if let Some(value) = inherited_value(&document, page_id, key) {
                            page_dict.set(key, value);
                        }
                    }
                }
            }
            ordered_pages.push((page_id, page));
        }
        imported_objects.extend(document.objects);
    }

    let pages_id = (next_id, 0);
    let catalog_id = (next_id + 1, 0);
    let mut merged = Document::with_version("1.7");

    for (id, object) in imported_objects {
        let object_type = object.type_name().unwrap_or(b"");
        if !matches!(
            object_type,
            b"Catalog" | b"Pages" | b"Page" | b"Outlines" | b"Outline"
        ) {
            merged.objects.insert(id, object);
        }
    }

    let mut kids = Vec::with_capacity(ordered_pages.len());
    for (page_id, mut page) in ordered_pages {
        page.as_dict_mut()
            .map_err(|err| error("corrupted_pdf", err.to_string()))?
            .set("Parent", pages_id);
        merged.objects.insert(page_id, page);
        kids.push(Object::Reference(page_id));
    }
    let total_pages = kids.len();
    merged.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Count" => total_pages as i64,
            "Kids" => kids,
        }),
    );
    merged.objects.insert(
        catalog_id,
        Object::Dictionary(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        }),
    );
    merged.trailer.set("Root", catalog_id);
    merged.max_id = next_id + 1;
    merged.renumber_objects();
    merged.compress();
    Ok((merged, total_pages))
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
        "merged-document".into()
    } else {
        cleaned.into()
    }
}

fn create_unique_file(directory: &Path, stem: &str, bytes: &[u8]) -> Result<PathBuf, PdfError> {
    for suffix in 1..=10_000usize {
        let file_name = if suffix == 1 {
            format!("{stem}.pdf")
        } else {
            format!("{stem}-{suffix}.pdf")
        };
        let path = directory.join(file_name);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                if let Err(err) = file.write_all(bytes).and_then(|_| file.flush()) {
                    let _ = fs::remove_file(&path);
                    let code = if err.raw_os_error() == Some(112) {
                        "insufficient_space"
                    } else {
                        "write_error"
                    };
                    return Err(error(code, err.to_string()));
                }
                return Ok(path);
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => {
                let code = if err.raw_os_error() == Some(112) {
                    "insufficient_space"
                } else {
                    "destination_inaccessible"
                };
                return Err(error(code, err.to_string()));
            }
        }
    }
    Err(error(
        "write_error",
        "Unable to create a unique output filename",
    ))
}

fn output_size_with_cleanup<F>(path: &Path, metadata_len: F) -> Result<u64, PdfError>
where
    F: FnOnce(&Path) -> std::io::Result<u64>,
{
    metadata_len(path).map_err(|err| {
        let _ = fs::remove_file(path);
        error("write_error", err.to_string())
    })
}

fn merge_pdfs_sync(request: MergePdfRequest) -> Result<MergePdfResult, PdfError> {
    if request.paths.len() < 2 {
        return Err(error("not_enough_pdfs", "Select at least two PDF files"));
    }
    let destination = request
        .destination_dir
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
                "No destination folder was selected",
            )
        })?;
    if !destination.is_dir() {
        return Err(error(
            "destination_inaccessible",
            "The destination folder is inaccessible",
        ));
    }

    let (mut document, total_pages) = build_merged_document(&request.paths)?;
    let mut bytes = Vec::new();
    document
        .save_to(&mut bytes)
        .map_err(|err| error("write_error", err.to_string()))?;
    let output_path = create_unique_file(&destination, &safe_stem(&request.output_name), &bytes)?;
    let new_size = output_size_with_cleanup(&output_path, |path| {
        fs::metadata(path).map(|metadata| metadata.len())
    })?;
    Ok(MergePdfResult {
        success: true,
        pdf_count: request.paths.len(),
        total_pages,
        new_size: Some(new_size),
        output_path: Some(output_path.to_string_lossy().into_owned()),
        error_code: None,
        error_message: None,
    })
}

#[tauri::command]
pub async fn merge_pdfs(request: MergePdfRequest) -> MergePdfResult {
    let pdf_count = request.paths.len();
    match tauri::async_runtime::spawn_blocking(move || merge_pdfs_sync(request)).await {
        Ok(Ok(result)) => result,
        Ok(Err(err)) => MergePdfResult {
            success: false,
            pdf_count,
            total_pages: 0,
            new_size: None,
            output_path: None,
            error_code: Some(err.code),
            error_message: Some(err.message),
        },
        Err(err) => MergePdfResult {
            success: false,
            pdf_count,
            total_pages: 0,
            new_size: None,
            output_path: None,
            error_code: Some("write_error".into()),
            error_message: Some(err.to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_pdf(path: &Path, sizes: &[(i64, i64)]) {
        let mut document = Document::with_version("1.5");
        let pages_id = document.new_object_id();
        let mut kids = Vec::new();
        for (width, height) in sizes {
            let page_id = document.add_object(dictionary! {
                "Type" => "Page", "Parent" => pages_id,
                "MediaBox" => vec![0.into(), 0.into(), (*width).into(), (*height).into()],
                "Resources" => dictionary! {},
            });
            kids.push(Object::Reference(page_id));
        }
        document.objects.insert(
            pages_id,
            Object::Dictionary(
                dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => sizes.len() as i64 },
            ),
        );
        let catalog_id =
            document.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        document.trailer.set("Root", catalog_id);
        document.compress();
        document.save(path).unwrap();
    }

    #[test]
    fn inspects_and_merges_multiple_pdfs_in_order() {
        let temp = TempDir::new().unwrap();
        let first = temp.path().join("premier échantillon.pdf");
        let second = temp.path().join("second.pdf");
        make_pdf(&first, &[(200, 300), (400, 500)]);
        make_pdf(&second, &[(600, 700)]);
        assert_eq!(
            inspect_pdf(first.to_string_lossy().into()).unwrap().pages,
            2
        );
        let result = merge_pdfs_sync(MergePdfRequest {
            paths: vec![
                first.to_string_lossy().into(),
                second.to_string_lossy().into(),
            ],
            destination_dir: Some(temp.path().to_string_lossy().into()),
            output_name: "résultat final".into(),
        })
        .unwrap();
        assert_eq!(result.total_pages, 3);
        let output = Document::load(result.output_path.unwrap()).unwrap();
        let sizes: Vec<(i64, i64)> = output
            .get_pages()
            .into_values()
            .map(|id| {
                let media = output
                    .get_object(id)
                    .unwrap()
                    .as_dict()
                    .unwrap()
                    .get(b"MediaBox")
                    .unwrap()
                    .as_array()
                    .unwrap();
                (media[2].as_i64().unwrap(), media[3].as_i64().unwrap())
            })
            .collect();
        assert_eq!(sizes, vec![(200, 300), (400, 500), (600, 700)]);
    }

    #[test]
    fn never_overwrites_outputs_or_originals() {
        let temp = TempDir::new().unwrap();
        let first = temp.path().join("one.pdf");
        let second = temp.path().join("two.pdf");
        make_pdf(&first, &[(100, 100)]);
        make_pdf(&second, &[(200, 200)]);
        let original = fs::read(&first).unwrap();
        let request = || MergePdfRequest {
            paths: vec![
                first.to_string_lossy().into(),
                second.to_string_lossy().into(),
            ],
            destination_dir: Some(temp.path().to_string_lossy().into()),
            output_name: "merged-document.pdf".into(),
        };
        let a = merge_pdfs_sync(request()).unwrap().output_path.unwrap();
        let b = merge_pdfs_sync(request()).unwrap().output_path.unwrap();
        assert!(a.ends_with("merged-document.pdf"));
        assert!(b.ends_with("merged-document-2.pdf"));
        assert_eq!(original, fs::read(first).unwrap());
    }

    #[test]
    fn rejects_invalid_inputs() {
        let temp = TempDir::new().unwrap();
        let text = temp.path().join("fake.txt");
        fs::write(&text, b"not a pdf").unwrap();
        assert_eq!(
            inspect_pdf(text.to_string_lossy().into()).unwrap_err().code,
            "not_pdf"
        );
        let corrupt = temp.path().join("broken.pdf");
        fs::write(&corrupt, b"%PDF broken").unwrap();
        assert_eq!(
            inspect_pdf(corrupt.to_string_lossy().into())
                .unwrap_err()
                .code,
            "corrupted_pdf"
        );
        assert_eq!(
            merge_pdfs_sync(MergePdfRequest {
                paths: vec![],
                destination_dir: None,
                output_name: "x".into()
            })
            .unwrap_err()
            .code,
            "not_enough_pdfs"
        );
    }

    #[test]
    fn removes_output_when_metadata_read_fails() {
        let temp = TempDir::new().unwrap();
        let output = temp.path().join("merged.pdf");
        fs::write(&output, b"complete output").unwrap();
        let result = output_size_with_cleanup(&output, |_| {
            Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "metadata failure",
            ))
        });
        assert_eq!(result.unwrap_err().code, "write_error");
        assert!(!output.exists());
    }
}
