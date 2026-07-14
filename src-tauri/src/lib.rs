mod compression;
mod conversion;
mod images_pdf;
mod pdf_merge;
mod pdf_organize;
mod pdf_split;
mod resizing;

use compression::{compress_image, inspect_image};
use conversion::{convert_image, inspect_convertible_image};
use images_pdf::images_to_pdf;
use pdf_merge::{inspect_pdf, merge_pdfs};
use pdf_organize::organize_pdf;
use pdf_split::split_pdf;
use resizing::resize_image;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            inspect_image,
            compress_image,
            inspect_convertible_image,
            convert_image,
            images_to_pdf,
            resize_image,
            inspect_pdf,
            merge_pdfs,
            organize_pdf,
            split_pdf
        ])
        .run(tauri::generate_context!())
        .expect("error while running FileNova");
}
