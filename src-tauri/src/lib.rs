mod compression;
mod conversion;
mod images_pdf;
mod pdf_merge;
mod pdf_organize;
mod pdf_split;
mod pdf_to_images;
mod resizing;
mod video_compression;
mod video_conversion_plan;
#[allow(dead_code)]
mod video_target_size;
mod video_tools;

use compression::{compress_image, inspect_image};
use conversion::{convert_image, inspect_convertible_image};
use images_pdf::images_to_pdf;
use pdf_merge::{inspect_pdf, merge_pdfs};
use pdf_organize::organize_pdf;
use pdf_split::split_pdf;
use pdf_to_images::{inspect_pdf_for_images, pdf_to_images, validate_pdf_image_output};
use resizing::resize_image;
use video_compression::{
    cancel_video_compression, compress_recommended_video, delete_video_result,
    keep_video_target_result, VideoCompressionState,
};
use video_conversion_plan::plan_video_conversion;
use video_tools::inspect_video;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(VideoCompressionState::default())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            video_tools::log_tool_versions(app.handle());
            Ok(())
        })
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
            split_pdf,
            inspect_pdf_for_images,
            pdf_to_images,
            validate_pdf_image_output,
            inspect_video,
            compress_recommended_video,
            cancel_video_compression,
            delete_video_result,
            keep_video_target_result,
            plan_video_conversion
        ])
        .run(tauri::generate_context!())
        .expect("error while running FileNova");
}
