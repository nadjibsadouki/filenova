mod audio_extraction;
mod audio_extraction_plan;
mod compression;
mod conversion;
mod images_pdf;
#[cfg(test)]
mod navigation_contract;
mod pdf_merge;
mod pdf_organize;
mod pdf_split;
mod pdf_to_images;
mod resizing;
mod social_media_export;
#[allow(dead_code)]
mod social_media_plan;
mod trim_video;
#[allow(dead_code)]
mod trim_video_plan;
mod video_compression;
mod video_conversion;
mod video_conversion_plan;
mod video_gif;
#[allow(dead_code)]
mod video_gif_plan;
#[allow(dead_code)]
mod video_target_size;
mod video_tools;

use audio_extraction::{cancel_audio_extraction, extract_audio, AudioExtractionState};
use audio_extraction_plan::{
    get_audio_extraction_capabilities, inspect_audio_source, plan_audio_extraction,
};
use compression::{compress_image, inspect_image};
use conversion::{convert_image, inspect_convertible_image};
use images_pdf::images_to_pdf;
use pdf_merge::{inspect_pdf, merge_pdfs};
use pdf_organize::organize_pdf;
use pdf_split::split_pdf;
use pdf_to_images::{inspect_pdf_for_images, pdf_to_images, validate_pdf_image_output};
use resizing::resize_image;
use social_media_export::{
    cancel_social_media_export, export_social_media_video, SocialExportState,
};
use trim_video::{
    cancel_fast_trim, cancel_precise_trim, fast_trim_video, precise_trim_video, TrimVideoState,
};
use trim_video_plan::allow_trim_video_preview;
use video_compression::{
    cancel_video_compression, compress_recommended_video, delete_video_result,
    keep_video_target_result, VideoCompressionState,
};
use video_conversion::{cancel_video_conversion, convert_video, VideoConversionState};
use video_conversion_plan::{get_video_conversion_capabilities, plan_video_conversion};
use video_gif::{cancel_video_gif, create_video_gif, VideoGifState};
use video_gif_plan::allow_video_gif_preview;
use video_tools::inspect_video;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(VideoCompressionState::default())
        .manage(VideoConversionState::default())
        .manage(AudioExtractionState::default())
        .manage(TrimVideoState::default())
        .manage(VideoGifState::default())
        .manage(SocialExportState::default())
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
            plan_video_conversion,
            get_video_conversion_capabilities,
            convert_video,
            cancel_video_conversion,
            get_audio_extraction_capabilities,
            inspect_audio_source,
            plan_audio_extraction,
            extract_audio,
            cancel_audio_extraction,
            allow_trim_video_preview,
            fast_trim_video,
            cancel_fast_trim,
            precise_trim_video,
            cancel_precise_trim,
            allow_video_gif_preview,
            create_video_gif,
            cancel_video_gif,
            export_social_media_video,
            cancel_social_media_export
        ])
        .run(tauri::generate_context!())
        .expect("error while running FileNova");
}
