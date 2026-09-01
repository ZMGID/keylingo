//! 模型入口的图片降采样（治本层）。
//!
//! 所有「图片进模型上下文」的路径（用户附件 / `read` 读图直喂 / 辅助视觉 / MCP 图片
//! artifact）在 base64 编码前统一过这里：超过 [`MODEL_IMAGE_MAX_DIM`] 的缩到边界内、
//! 超过 [`MODEL_IMAGE_TARGET_BYTES`] 的按 JPEG 质量阶梯重压。对齐 pi（2000px/4.5MB
//! base64、质量阶梯）与 Claude Code（2000px/3.75MB 原始字节）的入口收口；Codex 只做
//! 了入口缩放没做历史预算，堆积事故（openai/codex#28316）是 `compaction.rs::
//! prune_image_parts` 存在的原因——入口收口后每张图有界，历史层预算才从「每轮必砍、
//! 把模型没看过的图挤出去」退化成极端情况的保险。
//!
//! 解不出来的字节（webp/gif 等未编译进 `image` crate 的格式、损坏文件）返回 None，
//! 调用方原样直通——与引入本模块前的行为一致，绝不因降采样失败丢图。

use std::io::Cursor;

use image::{codecs::jpeg::JpegEncoder, imageops::FilterType, DynamicImage, RgbImage};

/// 喂给模型的图片最长边上限。pi / Claude Code 同为 2000，Codex 为 2048；
/// Anthropic 服务端超过 1568px 也会自己内部降采样，2000 已是「略大保画质」的取值。
pub(super) const MODEL_IMAGE_MAX_DIM: u32 = 2000;

/// 重压后的目标字节数（原始编码字节，非 base64；base64 后约 ×4/3 ≈ 2.7MB）。
/// 2000px 的 JPEG q80 通常只有几百 KB，这个上限只有病态输入（噪声图/超高熵）才会逼近。
pub(super) const MODEL_IMAGE_TARGET_BYTES: usize = 2 * 1024 * 1024;

/// JPEG 质量阶梯（对齐 pi 的 80→…→40 与 Claude Code 的 80→60→40→20）。
const JPEG_QUALITY_LADDER: [u8; 4] = [80, 60, 40, 20];

/// 入口降采样：返回 `Some((jpeg_bytes, "image/jpeg"))` 表示已处理，`None` 表示
/// 原字节直接可用（尺寸/体积都在界内，或根本解不出来——直通是唯一安全的降级）。
pub(super) fn prepare_image_bytes_for_model(bytes: &[u8]) -> Option<(Vec<u8>, &'static str)> {
    // 先只读头部拿尺寸，避免为了「本来就合规」的图付一次全量解码。
    let (width, height) = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()?;
    let within_dims = width <= MODEL_IMAGE_MAX_DIM && height <= MODEL_IMAGE_MAX_DIM;
    if within_dims && bytes.len() <= MODEL_IMAGE_TARGET_BYTES {
        return None;
    }

    let decoded = image::load_from_memory(bytes).ok()?;
    let resized = if within_dims {
        decoded
    } else {
        // Triangle 与 Codex 同款：比 Lanczos 快数倍，缩小场景画质差异不可见。
        decoded.resize(MODEL_IMAGE_MAX_DIM, MODEL_IMAGE_MAX_DIM, FilterType::Triangle)
    };
    let rgb = flatten_onto_white(resized);

    let mut smallest: Option<Vec<u8>> = None;
    for quality in JPEG_QUALITY_LADDER {
        let mut buf = Vec::new();
        JpegEncoder::new_with_quality(&mut buf, quality)
            .encode_image(&rgb)
            .ok()?;
        if buf.len() <= MODEL_IMAGE_TARGET_BYTES {
            return Some((buf, "image/jpeg"));
        }
        if smallest.as_ref().is_none_or(|s| buf.len() < s.len()) {
            smallest = Some(buf);
        }
    }
    // 阶梯走完仍超目标（理论上 2000px q20 不会发生）：发最小的那份，别把图整个丢掉。
    smallest.map(|buf| (buf, "image/jpeg"))
}

/// JPEG 无 alpha 通道：带透明的图（电商白底/透明底产品图很常见）合成到白底，
/// 而不是 `to_rgb8()` 直接丢弃 alpha 让透明区变成任意残留色。
pub(super) fn flatten_onto_white(img: DynamicImage) -> RgbImage {
    if !img.color().has_alpha() {
        return img.to_rgb8();
    }
    let rgba = img.to_rgba8();
    let mut rgb = RgbImage::new(rgba.width(), rgba.height());
    for (dst, src) in rgb.pixels_mut().zip(rgba.pixels()) {
        let alpha = src[3] as u32;
        for channel in 0..3 {
            dst[channel] = ((src[channel] as u32 * alpha + 255 * (255 - alpha)) / 255) as u8;
        }
    }
    rgb
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_bytes(img: &DynamicImage) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        buf.into_inner()
    }

    fn decoded_dims(bytes: &[u8]) -> (u32, u32) {
        let img = image::load_from_memory(bytes).unwrap();
        (img.width(), img.height())
    }

    #[test]
    fn small_image_passes_through_untouched() {
        let img = DynamicImage::ImageRgb8(RgbImage::from_pixel(400, 300, image::Rgb([10, 20, 30])));
        assert!(prepare_image_bytes_for_model(&png_bytes(&img)).is_none());
    }

    #[test]
    fn oversized_dimensions_get_resized_to_bound() {
        let img = DynamicImage::ImageRgb8(RgbImage::from_fn(3000, 1200, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 99])
        }));
        let (jpeg, mime) =
            prepare_image_bytes_for_model(&png_bytes(&img)).expect("must be processed");
        assert_eq!(mime, "image/jpeg");
        let (w, h) = decoded_dims(&jpeg);
        assert!(w <= MODEL_IMAGE_MAX_DIM && h <= MODEL_IMAGE_MAX_DIM, "got {w}x{h}");
        // 等比：3000x1200 → 2000x800
        assert_eq!((w, h), (2000, 800));
        assert!(jpeg.len() <= MODEL_IMAGE_TARGET_BYTES);
    }

    #[test]
    fn oversized_bytes_within_dims_get_recompressed() {
        // 伪随机噪声让 PNG 无损压缩失效，1300x1300 噪声 PNG > 2MB 但尺寸合规。
        let mut seed = 0x2545f491u32;
        let img = DynamicImage::ImageRgb8(RgbImage::from_fn(1300, 1300, |_, _| {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            image::Rgb([(seed >> 8) as u8, (seed >> 16) as u8, (seed >> 24) as u8])
        }));
        let png = png_bytes(&img);
        assert!(png.len() > MODEL_IMAGE_TARGET_BYTES, "fixture must exceed target");

        let (jpeg, _) = prepare_image_bytes_for_model(&png).expect("must be recompressed");
        assert!(jpeg.len() < png.len());
        // 尺寸不该被动：只超字节不超边长。
        assert_eq!(decoded_dims(&jpeg), (1300, 1300));
    }

    #[test]
    fn transparent_pixels_flatten_to_white_not_black() {
        let rgba = image::RgbaImage::from_pixel(2400, 100, image::Rgba([0, 0, 0, 0]));
        let (jpeg, _) = prepare_image_bytes_for_model(&png_bytes(&DynamicImage::ImageRgba8(rgba)))
            .expect("oversized width forces processing");
        let img = image::load_from_memory(&jpeg).unwrap().to_rgb8();
        let px = img.get_pixel(img.width() / 2, img.height() / 2);
        // JPEG 有量化误差，留余量断言「接近白」。
        assert!(px[0] > 240 && px[1] > 240 && px[2] > 240, "flattened pixel was {px:?}");
    }

    #[test]
    fn undecodable_bytes_pass_through() {
        assert!(prepare_image_bytes_for_model(b"not an image at all").is_none());
    }

    #[test]
    fn nine_product_shots_fit_history_budget_after_prep() {
        // 复现用户会话：9 张电商成图（3000×2000）一次读进上下文。
        // 入口收口后 9 张合计必须落在历史保险预算内；再收一档——连旧的 4MB
        // 硬砍预算也装得下，证明「刚读就被挤出」的死循环对典型成图不会再触发。
        const OLD_HARD_BUDGET: usize = 4 * 1024 * 1024;
        const HISTORY_BASE64_BUDGET: usize = 16 * 1024 * 1024;
        let mut total_b64 = 0usize;
        for i in 0..9u32 {
            let img = DynamicImage::ImageRgb8(RgbImage::from_fn(3000, 2000, |x, y| {
                image::Rgb([
                    ((x / 3 + i * 40) % 256) as u8,
                    ((y / 2 + i * 70) % 256) as u8,
                    80 + (i * 12) as u8,
                ])
            }));
            let png = png_bytes(&img);
            let payload = match prepare_image_bytes_for_model(&png) {
                Some((processed, mime)) => {
                    assert_eq!(mime, "image/jpeg");
                    processed
                }
                None => panic!("3000×2000 shot must be resized"),
            };
            let (w, h) = decoded_dims(&payload);
            assert!(w <= MODEL_IMAGE_MAX_DIM && h <= MODEL_IMAGE_MAX_DIM, "got {w}x{h}");
            assert!(payload.len() <= MODEL_IMAGE_TARGET_BYTES);
            total_b64 += payload.len() * 4 / 3;
        }
        assert!(
            total_b64 < OLD_HARD_BUDGET,
            "9 prepared product shots ({total_b64} B64 bytes) should fit the old 4MB prune budget"
        );
        assert!(total_b64 < HISTORY_BASE64_BUDGET);
    }
}
