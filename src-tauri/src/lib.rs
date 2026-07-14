mod compression;
mod conversion;

use compression::{compress_image, inspect_image};
use conversion::{convert_image, inspect_convertible_image};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            inspect_image,
            compress_image,
            inspect_convertible_image,
            convert_image
        ])
        .run(tauri::generate_context!())
        .expect("error while running FileNova");
}
