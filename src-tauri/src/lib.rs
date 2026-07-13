mod compression;

use compression::{compress_image, inspect_image};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![inspect_image, compress_image])
        .run(tauri::generate_context!())
        .expect("error while running FileNova");
}
