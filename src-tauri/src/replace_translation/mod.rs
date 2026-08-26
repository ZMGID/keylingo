pub mod layout;
pub mod mask;

#[cfg(test)]
mod visual_fixtures;

use std::io::Cursor;

use image::{DynamicImage, ImageFormat, RgbImage};

/// RGB → PNG 的唯一编码入口（替换翻译清理图走这里）。
pub fn encode_rgb_png(image: RgbImage) -> Result<Vec<u8>, String> {
    let mut cursor = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(image)
        .write_to(&mut cursor, ImageFormat::Png)
        .map_err(|error| format!("encode cleaned image: {error}"))?;
    Ok(cursor.into_inner())
}

#[cfg(all(test, target_os = "macos"))]
mod e2e_tests {
    #[tokio::test]
    #[ignore]
    async fn replace_pipeline_real_fixture() {
        let model_dir = std::env::var_os("KIVIO_REPLACE_MODEL_DIR")
            .map(std::path::PathBuf::from)
            .expect("set KIVIO_REPLACE_MODEL_DIR");
        let image_path = std::env::var_os("KIVIO_REPLACE_IMAGE")
            .map(std::path::PathBuf::from)
            .expect("set KIVIO_REPLACE_IMAGE");
        let output_path = std::env::var_os("KIVIO_REPLACE_OUTPUT")
            .map(std::path::PathBuf::from)
            .expect("set KIVIO_REPLACE_OUTPUT");
        let manager = crate::offline_models::OfflineModelManager::with_model_dir(
            model_dir,
            reqwest::Client::new(),
        );
        let ocr = crate::rapidocr::RapidOcrClient::new(manager.clone());
        let spans = ocr
            .ocr_image_lines(&image_path, crate::rapidocr::ModelTier::High)
            .await
            .expect("run OCR");
        let image = image::open(&image_path).expect("open image").to_rgb8();
        let spans = super::layout::filter_replaceable_spans(image.width(), &spans);
        let geometry = super::layout::build_replace_geometry(&image, &spans);
        let started = std::time::Instant::now();
        let filled = super::mask::plate_fill(&image, &spans);
        let png = super::encode_rgb_png(filled).expect("encode plate fill");
        eprintln!(
            "replace pipeline: {} spans, {} regions, plate fill {:?}",
            spans.len(),
            geometry.groups.len(),
            started.elapsed()
        );
        std::fs::write(output_path, png).expect("write result");
    }
}
