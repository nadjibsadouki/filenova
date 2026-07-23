use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProcessingResultAvailability {
    file_exists: bool,
    parent_exists: bool,
    parent_path: Option<String>,
}

fn result_availability(path: &str) -> ProcessingResultAvailability {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return ProcessingResultAvailability {
            file_exists: false,
            parent_exists: false,
            parent_path: None,
        };
    }

    let result_path = PathBuf::from(trimmed);
    let parent = result_path.parent().filter(|value| !value.as_os_str().is_empty());
    ProcessingResultAvailability {
        file_exists: result_path.is_file(),
        parent_exists: parent.is_some_and(Path::is_dir),
        parent_path: parent.map(|value| value.to_string_lossy().into_owned()),
    }
}

#[tauri::command]
pub fn check_processing_result_availability(path: String) -> ProcessingResultAvailability {
    result_availability(&path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn checks_existing_unicode_file_and_parent_without_modifying_it() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("dossier avec espaces é عربي 你好");
        fs::create_dir(&directory).unwrap();
        let file = directory.join("résultat نهائي 你好.txt");
        let contents = b"FileNova history availability";
        fs::write(&file, contents).unwrap();

        let availability = result_availability(&file.to_string_lossy());
        let expected_parent = directory.to_string_lossy().into_owned();

        assert!(availability.file_exists);
        assert!(availability.parent_exists);
        assert_eq!(
            availability.parent_path.as_deref(),
            Some(expected_parent.as_str())
        );
        assert_eq!(fs::read(&file).unwrap(), contents);
    }

    #[test]
    fn reports_missing_file_but_keeps_an_existing_parent_available() {
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("fichier déplacé.mp4");

        let availability = result_availability(&missing.to_string_lossy());
        let expected_parent = temp.path().to_string_lossy().into_owned();

        assert!(!availability.file_exists);
        assert!(availability.parent_exists);
        assert_eq!(
            availability.parent_path.as_deref(),
            Some(expected_parent.as_str())
        );
    }

    #[test]
    fn treats_shell_metacharacters_as_plain_path_text_and_never_creates_or_deletes_files() {
        let temp = tempfile::tempdir().unwrap();
        let sentinel = temp.path().join("sentinel.txt");
        fs::write(&sentinel, b"keep").unwrap();
        let suspicious = temp.path().join("missing & calc.exe; $(echo injected).mp4");

        let availability = result_availability(&suspicious.to_string_lossy());

        assert!(!availability.file_exists);
        assert!(availability.parent_exists);
        assert_eq!(fs::read(&sentinel).unwrap(), b"keep");
        assert!(!suspicious.exists());
    }

    #[test]
    fn empty_or_parentless_paths_are_unavailable_without_panicking() {
        assert_eq!(
            result_availability(""),
            ProcessingResultAvailability {
                file_exists: false,
                parent_exists: false,
                parent_path: None,
            }
        );
    }
}
