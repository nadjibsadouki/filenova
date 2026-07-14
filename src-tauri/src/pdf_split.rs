use lopdf::{dictionary, Document, Object, ObjectId};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};
use tauri::Emitter;

static SPLIT_RUNNING: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SplitPdfRequest {
    pub path: String,
    pub destination_dir: Option<String>,
    pub mode: String,
    pub pages: Option<String>,
    pub ranges: Option<String>,
    pub every_n: Option<u32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SplitPdfResult {
    pub success: bool,
    pub files: Vec<SplitOutput>,
    pub total_pages: usize,
    pub destination_dir: Option<String>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SplitOutput {
    pub path: String,
    pub name: String,
    pub pages: usize,
    pub size: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SplitProgress {
    pub completed: usize,
    pub total: usize,
    pub percent: usize,
    pub current_file: String,
}

#[derive(Debug)]
struct SplitError {
    code: String,
    message: String,
}

fn error(code: &str, message: impl Into<String>) -> SplitError {
    SplitError {
        code: code.into(),
        message: message.into(),
    }
}

fn load_pdf(path: &Path) -> Result<Document, SplitError> {
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

fn parse_page_token(token: &str, total: u32) -> Result<Vec<u32>, SplitError> {
    let token = token.trim();
    if token.is_empty() {
        return Err(error("split_invalid_pages", "A page value is empty"));
    }
    let parts: Vec<&str> = token.split('-').map(str::trim).collect();
    if parts.len() > 2 || parts.iter().any(|part| part.is_empty()) {
        return Err(error("split_invalid_pages", "Invalid page or range"));
    }
    let start = parts[0]
        .parse::<u32>()
        .map_err(|_| error("split_invalid_pages", "Invalid page number"))?;
    let end = if parts.len() == 2 {
        parts[1]
            .parse::<u32>()
            .map_err(|_| error("split_invalid_pages", "Invalid range end"))?
    } else {
        start
    };
    if start == 0 || end == 0 {
        return Err(error("split_page_zero", "Page numbers cannot be zero"));
    }
    if start > end {
        return Err(error(
            "split_range_reversed",
            "A range start cannot exceed its end",
        ));
    }
    if end > total {
        return Err(error(
            "split_page_out_of_range",
            "A requested page does not exist",
        ));
    }
    Ok((start..=end).collect())
}

fn extract_groups(request: &SplitPdfRequest, total: u32) -> Result<Vec<Vec<u32>>, SplitError> {
    match request.mode.as_str() {
        "extract" => {
            let expression = request.pages.as_deref().unwrap_or_default();
            let mut seen = HashSet::new();
            let mut pages = Vec::new();
            for token in expression.split(',') {
                for page in parse_page_token(token, total)? {
                    if seen.insert(page) {
                        pages.push(page);
                    }
                }
            }
            if pages.is_empty() {
                return Err(error("split_invalid_pages", "No pages were selected"));
            }
            Ok(vec![pages])
        }
        "everyPage" => Ok((1..=total).map(|page| vec![page]).collect()),
        "ranges" => {
            let groups: Result<Vec<_>, _> = request
                .ranges
                .as_deref()
                .unwrap_or_default()
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(|line| parse_page_token(line, total))
                .collect();
            let groups = groups?;
            if groups.is_empty() {
                return Err(error("split_invalid_ranges", "No ranges were entered"));
            }
            Ok(groups)
        }
        "everyN" => {
            let count = request
                .every_n
                .filter(|count| *count > 0)
                .ok_or_else(|| error("split_invalid_n", "N must be greater than zero"))?;
            let pages: Vec<u32> = (1..=total).collect();
            Ok(pages.chunks(count as usize).map(<[u32]>::to_vec).collect())
        }
        _ => Err(error("split_invalid_mode", "Unknown split mode")),
    }
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

fn document_for_pages(source: &Document, page_numbers: &[u32]) -> Result<Document, SplitError> {
    let source_pages = source.get_pages();
    let mut output = source.clone();
    let pages_id = (output.max_id + 1, 0);
    let catalog_id = (output.max_id + 2, 0);
    let mut kids = Vec::with_capacity(page_numbers.len());

    for page_number in page_numbers {
        let page_id = *source_pages
            .get(page_number)
            .ok_or_else(|| error("split_page_out_of_range", "A requested page does not exist"))?;
        let mut page = source
            .get_object(page_id)
            .map_err(|err| error("corrupted_pdf", err.to_string()))?
            .clone();
        if let Ok(dictionary) = page.as_dict_mut() {
            for key in [b"Resources".as_slice(), b"MediaBox", b"CropBox", b"Rotate"] {
                if !dictionary.has(key) {
                    if let Some(value) = inherited_value(source, page_id, key) {
                        dictionary.set(key, value);
                    }
                }
            }
            dictionary.set("Parent", pages_id);
        } else {
            return Err(error("corrupted_pdf", "A page dictionary is invalid"));
        }
        output.objects.insert(page_id, page);
        kids.push(Object::Reference(page_id));
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
    output.max_id += 2;
    output.renumber_objects();
    output.compress();
    Ok(output)
}

fn safe_stem(path: &Path) -> String {
    let value = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("document");
    let cleaned: String = value
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
        "document".into()
    } else {
        cleaned.into()
    }
}

fn page_label(pages: &[u32]) -> String {
    let mut labels = Vec::new();
    let mut index = 0;
    while index < pages.len() {
        let start = pages[index];
        let mut end = start;
        while index + 1 < pages.len() && pages[index + 1] == end + 1 {
            index += 1;
            end = pages[index];
        }
        labels.push(if start == end {
            start.to_string()
        } else {
            format!("{start}-{end}")
        });
        index += 1;
    }
    labels.join("-")
}

fn desired_names(stem: &str, mode: &str, groups: &[Vec<u32>], directory: &Path) -> Vec<String> {
    match mode {
        "everyPage" => groups
            .iter()
            .map(|pages| format!("{stem}-page-{}", pages[0]))
            .collect(),
        "everyN" => {
            let mut next_part = 1;
            groups
                .iter()
                .map(|_| {
                    while directory
                        .join(format!("{stem}-part-{next_part}.pdf"))
                        .exists()
                    {
                        next_part += 1;
                    }
                    let name = format!("{stem}-part-{next_part}");
                    next_part += 1;
                    name
                })
                .collect()
        }
        _ => groups
            .iter()
            .map(|pages| format!("{stem}-pages-{}", page_label(pages)))
            .collect(),
    }
}

fn write_unique(directory: &Path, desired_stem: &str, bytes: &[u8]) -> Result<PathBuf, SplitError> {
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

fn split_pdf_sync<F>(request: SplitPdfRequest, mut report: F) -> Result<SplitPdfResult, SplitError>
where
    F: FnMut(SplitProgress),
{
    let source_path = PathBuf::from(&request.path);
    let source = load_pdf(&source_path)?;
    let total = source.get_pages().len() as u32;
    if total == 0 {
        return Err(error("corrupted_pdf", "The PDF contains no pages"));
    }
    let groups = extract_groups(&request, total)?;
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

    let names = desired_names(
        &safe_stem(&source_path),
        &request.mode,
        &groups,
        &destination,
    );
    let mut created = Vec::with_capacity(groups.len());
    let output_count = groups.len();
    for (index, (pages, name)) in groups.iter().zip(names).enumerate() {
        report(SplitProgress {
            completed: index,
            total: output_count,
            percent: index * 100 / output_count,
            current_file: format!("{name}.pdf"),
        });
        let result = (|| {
            let mut document = document_for_pages(&source, pages)?;
            let mut bytes = Vec::new();
            document
                .save_to(&mut bytes)
                .map_err(|err| error("write_error", err.to_string()))?;
            write_unique(&destination, &name, &bytes)
        })();
        match result {
            Ok(path) => {
                let size = match fs::metadata(&path) {
                    Ok(metadata) => metadata.len(),
                    Err(err) => {
                        let _ = fs::remove_file(&path);
                        for (created_path, _, _) in &created {
                            let _ = fs::remove_file(created_path);
                        }
                        return Err(error("read_error", err.to_string()));
                    }
                };
                report(SplitProgress {
                    completed: index + 1,
                    total: output_count,
                    percent: (index + 1) * 100 / output_count,
                    current_file: path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("output.pdf")
                        .to_owned(),
                });
                created.push((path, pages.len(), size));
            }
            Err(err) => {
                for (path, _, _) in &created {
                    let _ = fs::remove_file(path);
                }
                return Err(err);
            }
        }
    }
    let total_pages = created.iter().map(|(_, pages, _)| pages).sum();
    let files = created
        .into_iter()
        .map(|(path, pages, size)| SplitOutput {
            name: path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("output.pdf")
                .to_owned(),
            path: path.to_string_lossy().into_owned(),
            pages,
            size,
        })
        .collect();
    Ok(SplitPdfResult {
        success: true,
        files,
        total_pages,
        destination_dir: Some(destination.to_string_lossy().into_owned()),
        error_code: None,
        error_message: None,
    })
}

struct RunningGuard;
impl Drop for RunningGuard {
    fn drop(&mut self) {
        SPLIT_RUNNING.store(false, Ordering::Release);
    }
}

#[tauri::command]
pub async fn split_pdf(window: tauri::Window, request: SplitPdfRequest) -> SplitPdfResult {
    if SPLIT_RUNNING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return SplitPdfResult {
            success: false,
            files: vec![],
            total_pages: 0,
            destination_dir: None,
            error_code: Some("split_already_running".into()),
            error_message: Some("Another PDF split is already running".into()),
        };
    }
    let _guard = RunningGuard;
    let progress_window = window.clone();
    match tauri::async_runtime::spawn_blocking(move || {
        split_pdf_sync(request, |progress| {
            let _ = progress_window.emit("pdf-split-progress", progress);
        })
    })
    .await
    {
        Ok(Ok(result)) => result,
        Ok(Err(err)) => SplitPdfResult {
            success: false,
            files: vec![],
            total_pages: 0,
            destination_dir: None,
            error_code: Some(err.code),
            error_message: Some(err.message),
        },
        Err(err) => SplitPdfResult {
            success: false,
            files: vec![],
            total_pages: 0,
            destination_dir: None,
            error_code: Some("write_error".into()),
            error_message: Some(err.to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_pdf(path: &Path, count: usize) {
        let mut document = Document::with_version("1.5");
        let pages_id = document.new_object_id();
        let mut kids = Vec::new();
        for page in 1..=count {
            let page_id = document.add_object(dictionary! {
                "Type" => "Page", "Parent" => pages_id,
                "MediaBox" => vec![0.into(), 0.into(), (100 + page as i64).into(), 200.into()],
                "Resources" => dictionary! {},
            });
            kids.push(Object::Reference(page_id));
        }
        document.objects.insert(
            pages_id,
            Object::Dictionary(
                dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => count as i64 },
            ),
        );
        let catalog_id =
            document.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        document.trailer.set("Root", catalog_id);
        document.save(path).unwrap();
    }

    fn request(path: &Path, destination: &Path, mode: &str) -> SplitPdfRequest {
        SplitPdfRequest {
            path: path.to_string_lossy().into(),
            destination_dir: Some(destination.to_string_lossy().into()),
            mode: mode.into(),
            pages: None,
            ranges: None,
            every_n: None,
        }
    }

    fn output_page_count(output: &SplitOutput) -> usize {
        Document::load(&output.path).unwrap().get_pages().len()
    }

    #[test]
    fn extracts_a_single_page() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("single-source.pdf");
        make_pdf(&source, 4);
        let mut input = request(&source, temp.path(), "extract");
        input.pages = Some("2".into());
        let result = split_pdf_sync(input, |_| {}).unwrap();
        assert_eq!(result.files.len(), 1);
        assert_eq!(result.total_pages, 1);
        assert_eq!(output_page_count(&result.files[0]), 1);
    }

    #[test]
    fn extracts_multiple_pages_and_a_page_range() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("multiple-source.pdf");
        make_pdf(&source, 7);
        let mut input = request(&source, temp.path(), "extract");
        input.pages = Some("1,3,5-7".into());
        let result = split_pdf_sync(input, |_| {}).unwrap();
        assert_eq!(result.total_pages, 5);
        assert_eq!(output_page_count(&result.files[0]), 5);
    }

    #[test]
    fn creates_one_file_for_each_defined_range() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("ranges-source.pdf");
        make_pdf(&source, 8);
        let mut input = request(&source, temp.path(), "ranges");
        input.ranges = Some("1-2\n3-5\n7-8".into());
        let result = split_pdf_sync(input, |_| {}).unwrap();
        assert_eq!(result.files.len(), 3);
        assert_eq!(result.total_pages, 7);
        assert_eq!(
            result
                .files
                .iter()
                .map(output_page_count)
                .collect::<Vec<_>>(),
            vec![2, 3, 2]
        );
    }

    #[test]
    fn splits_every_page_into_a_separate_file() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("pages-source.pdf");
        make_pdf(&source, 4);
        let result = split_pdf_sync(request(&source, temp.path(), "everyPage"), |_| {}).unwrap();
        assert_eq!(result.files.len(), 4);
        assert!(result.files.iter().all(|output| output.pages == 1));
    }

    #[test]
    fn splits_every_n_pages_with_an_incomplete_last_part() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("parts-source.pdf");
        make_pdf(&source, 7);
        let mut input = request(&source, temp.path(), "everyN");
        input.every_n = Some(3);
        let mut progress = Vec::new();
        let result = split_pdf_sync(input, |event| progress.push(event)).unwrap();
        assert_eq!(
            result
                .files
                .iter()
                .map(|output| output.pages)
                .collect::<Vec<_>>(),
            vec![3, 3, 1]
        );
        assert_eq!(progress.first().unwrap().percent, 0);
        assert_eq!(progress.last().unwrap().percent, 100);
        assert!(!progress.first().unwrap().current_file.is_empty());
    }

    #[test]
    fn rejects_a_page_that_does_not_exist() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("short-source.pdf");
        make_pdf(&source, 2);
        let mut input = request(&source, temp.path(), "extract");
        input.pages = Some("3".into());
        assert_eq!(
            split_pdf_sync(input, |_| {}).unwrap_err().code,
            "split_page_out_of_range"
        );
    }

    #[test]
    fn extracts_pages_in_entered_order_and_removes_duplicates() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("dôcument spécial.pdf");
        make_pdf(&source, 8);
        let original = fs::read(&source).unwrap();
        let mut input = request(&source, temp.path(), "extract");
        input.pages = Some("3,1,3,5-6".into());
        let result = split_pdf_sync(input, |_| {}).unwrap();
        assert_eq!(result.files.len(), 1);
        let output = Document::load(&result.files[0].path).unwrap();
        let widths: Vec<i64> = output
            .get_pages()
            .into_values()
            .map(|id| {
                output
                    .get_object(id)
                    .unwrap()
                    .as_dict()
                    .unwrap()
                    .get(b"MediaBox")
                    .unwrap()
                    .as_array()
                    .unwrap()[2]
                    .as_i64()
                    .unwrap()
            })
            .collect();
        assert_eq!(widths, vec![103, 101, 105, 106]);
        assert_eq!(original, fs::read(source).unwrap());
    }

    #[test]
    fn supports_every_page_ranges_and_every_n() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("document.pdf");
        make_pdf(&source, 5);
        let every_page =
            split_pdf_sync(request(&source, temp.path(), "everyPage"), |_| {}).unwrap();
        assert_eq!(every_page.files.len(), 5);
        let mut ranges = request(&source, temp.path(), "ranges");
        ranges.ranges = Some("1-2\n4-5".into());
        assert_eq!(split_pdf_sync(ranges, |_| {}).unwrap().files.len(), 2);
        let mut every_n = request(&source, temp.path(), "everyN");
        every_n.every_n = Some(2);
        let result = split_pdf_sync(every_n, |_| {}).unwrap();
        assert_eq!(result.files.len(), 3);
        assert_eq!(
            Document::load(&result.files.last().unwrap().path)
                .unwrap()
                .get_pages()
                .len(),
            1
        );
    }

    #[test]
    fn validates_pages_and_never_overwrites() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("document.pdf");
        make_pdf(&source, 3);
        let mut invalid = request(&source, temp.path(), "extract");
        invalid.pages = Some("0,-1,3-2,9".into());
        assert_eq!(
            split_pdf_sync(invalid, |_| {}).unwrap_err().code,
            "split_page_zero"
        );
        let mut first = request(&source, temp.path(), "everyN");
        first.every_n = Some(2);
        split_pdf_sync(first, |_| {}).unwrap();
        let mut second = request(&source, temp.path(), "everyN");
        second.every_n = Some(2);
        let paths = split_pdf_sync(second, |_| {}).unwrap().files;
        assert!(paths[0].path.ends_with("document-part-3.pdf"));
        assert!(paths[1].path.ends_with("document-part-4.pdf"));
    }

    #[test]
    fn rejects_corrupt_and_invalid_requests_without_writing_files() {
        let temp = TempDir::new().unwrap();
        let corrupt = temp.path().join("corrupt.pdf");
        fs::write(&corrupt, b"%PDF invalid").unwrap();
        assert_eq!(
            split_pdf_sync(request(&corrupt, temp.path(), "everyPage"), |_| {})
                .unwrap_err()
                .code,
            "corrupted_pdf"
        );

        let source = temp.path().join("valid.pdf");
        make_pdf(&source, 3);
        for (expression, expected) in [
            ("-1", "split_invalid_pages"),
            ("3-2", "split_range_reversed"),
            ("4", "split_page_out_of_range"),
        ] {
            let mut input = request(&source, temp.path(), "extract");
            input.pages = Some(expression.into());
            assert_eq!(split_pdf_sync(input, |_| {}).unwrap_err().code, expected);
        }
        let mut zero_n = request(&source, temp.path(), "everyN");
        zero_n.every_n = Some(0);
        assert_eq!(
            split_pdf_sync(zero_n, |_| {}).unwrap_err().code,
            "split_invalid_n"
        );
    }
}
