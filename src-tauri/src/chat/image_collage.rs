//! 批量读图时的联系表（contact sheet）。
//!
//! 默认**永远分张**。拼图会丢掉单图分辨率，用户要「分析 / 核验 / 读文字」时合一张
//! 等于整批作废，所以张数本身不是门槛。只有同时满足才拼：
//! - 张数 ∈ [[`COLLAGE_MIN_IMAGES`], [`COLLAGE_MAX_IMAGES`]]；
//! - 用户话里**没有**细部意图（分析、核验、逐张、文字、拼写…）——细部一票否决；
//! - 并且：工具显式 `overview: true`，**或**用户话是扫一眼/概览/浏览这类粗看。
//!
//! 格子按近似正方形排（6→3×2，9→3×3），左→右、上→下编号；文件名写在工具结果
//! 文本里（画布上只烙数字，避免 bundled 字体覆盖不了 CJK 文件名）。
//! 任一源图解码失败则整批退回分张，不丢图。

use std::path::Path;

use image::{
    codecs::jpeg::JpegEncoder, imageops::FilterType, DynamicImage, Rgb, RgbImage,
};

use super::image_prep::{flatten_onto_white, MODEL_IMAGE_MAX_DIM};

/// 政策下限：少于此数格子分辨率还够用，不值得拼。4 张并排对比也必须分张。
pub(super) const COLLAGE_MIN_IMAGES: usize = 6;
/// 一面联系表的上限；再多请分批 `read`。
pub(super) const COLLAGE_MAX_IMAGES: usize = 12;

const GAP: u32 = 8;
const LABEL_H: u32 = 28;
const JPEG_QUALITY: u8 = 85;
const BG: Rgb<u8> = Rgb([32, 32, 36]);
const LABEL_BG: Rgb<u8> = Rgb([20, 20, 24]);
const LABEL_FG: Rgb<u8> = Rgb([240, 240, 242]);

pub(super) struct ImageCollage {
    pub jpeg: Vec<u8>,
    pub cols: u32,
    pub rows: u32,
    pub names: Vec<String>,
}

impl ImageCollage {
    pub(super) fn tool_message(&self, zh: bool) -> String {
        let mut legend = String::new();
        for (i, name) in self.names.iter().enumerate() {
            legend.push_str(&format!("{}. {name}\n", i + 1));
        }
        if zh {
            format!(
                "已将 {} 张图片拼成一张 {}×{} 联系表（左→右、上→下编号，见下一条消息）。这是批量概览；核对文字、重音、小物件请再单独 read 某一张。\n\n{legend}",
                self.names.len(),
                self.cols,
                self.rows
            )
        } else {
            format!(
                "Combined {} images into a {}×{} contact sheet (left-to-right, top-to-bottom numbering; see the next message). This is an overview; re-read a single file to check small text or fine details.\n\n{legend}",
                self.names.len(),
                self.cols,
                self.rows
            )
        }
    }
}

/// 细部意图：分析、核验、读字、逐张 QA。命中则无论张数、无论 overview 都不拼。
const DETAIL_MARKERS: &[&str] = &[
    "分析",
    "核验",
    "核对",
    "审查",
    "质检",
    "逐张",
    "逐个",
    "每一张",
    "每一幅",
    "细节",
    "细部",
    "细看",
    "看清",
    "文字",
    "拼写",
    "错字",
    "重音",
    "读字",
    "logo",
    "商标",
    "像素",
    "analyze",
    "analys",
    "inspect",
    "verif",
    "proofread",
    "spelling",
    "per-image",
    "each image",
    "one by one",
    "one-by-one",
];

/// 粗看意图：扫一眼这批、概览、找废图。没有细部标记时才允许自动拼。
const OVERVIEW_MARKERS: &[&str] = &[
    "扫一眼",
    "浏览",
    "概览",
    "过一遍",
    "先过",
    "联系表",
    "缩略",
    "不用看细节",
    "不用细看",
    "批量预览",
    "哪张废",
    "有没有废图",
    "明显废",
    "glance",
    "skim",
    "overview",
    "contact sheet",
    "thumbnail",
    "bird's-eye",
    "birds-eye",
];

fn fold_intent_text(text: &str) -> String {
    text.to_ascii_lowercase()
}

pub(super) fn is_detail_image_read(user_text: Option<&str>) -> bool {
    let Some(text) = user_text.map(str::trim).filter(|t| !t.is_empty()) else {
        return false;
    };
    let folded = fold_intent_text(text);
    DETAIL_MARKERS
        .iter()
        .any(|marker| text.contains(marker) || folded.contains(&marker.to_ascii_lowercase()))
}

pub(super) fn is_overview_image_read(user_text: Option<&str>) -> bool {
    let Some(text) = user_text.map(str::trim).filter(|t| !t.is_empty()) else {
        return false;
    };
    let folded = fold_intent_text(text);
    OVERVIEW_MARKERS
        .iter()
        .any(|marker| text.contains(marker) || folded.contains(&marker.to_ascii_lowercase()))
}

/// 要不要拼联系表。默认 false：张数到了也不拼。
pub(super) fn should_collage(
    count: usize,
    user_text: Option<&str>,
    overview_requested: bool,
) -> bool {
    if count < COLLAGE_MIN_IMAGES || count > COLLAGE_MAX_IMAGES {
        return false;
    }
    if is_detail_image_read(user_text) {
        return false;
    }
    overview_requested || is_overview_image_read(user_text)
}

/// `n` 张图的列×行：列数取 ceil(sqrt(n))，行数补齐。
pub(super) fn collage_grid(n: usize) -> (u32, u32) {
    let n = n.max(1) as u32;
    let cols = (n as f64).sqrt().ceil() as u32;
    let rows = n.div_ceil(cols);
    (cols.max(1), rows.max(1))
}

/// 从磁盘批量构图。张数不够、解码失败或编码失败都返回 None（调用方分张直喂）。
pub(super) fn collage_from_paths(paths: &[impl AsRef<Path>]) -> Option<ImageCollage> {
    if paths.len() < COLLAGE_MIN_IMAGES || paths.len() > COLLAGE_MAX_IMAGES {
        return None;
    }
    let mut tiles = Vec::with_capacity(paths.len());
    for path in paths {
        let path = path.as_ref();
        let img = image::open(path).ok()?;
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("image")
            .to_string();
        tiles.push((name, flatten_onto_white(img)));
    }
    compose_collage(&tiles)
}

fn compose_collage(tiles: &[(String, RgbImage)]) -> Option<ImageCollage> {
    if tiles.len() < 2 {
        return None;
    }
    let (cols, rows) = collage_grid(tiles.len());
    let canvas = MODEL_IMAGE_MAX_DIM;
    let cell_w = (canvas.saturating_sub(GAP * (cols + 1))) / cols;
    let cell_h = (canvas.saturating_sub(GAP * (rows + 1))) / rows;
    if cell_w < 32 || cell_h < LABEL_H + 32 {
        return None;
    }
    let inner_h = cell_h.saturating_sub(LABEL_H);
    let width = GAP + cols * (cell_w + GAP);
    let height = GAP + rows * (cell_h + GAP);
    let mut canvas = RgbImage::from_pixel(width, height, BG);

    for (i, (_, src)) in tiles.iter().enumerate() {
        let col = (i as u32) % cols;
        let row = (i as u32) / cols;
        let x0 = GAP + col * (cell_w + GAP);
        let y0 = GAP + row * (cell_h + GAP);
        fill_rect(&mut canvas, x0, y0, cell_w, LABEL_H, LABEL_BG);
        blit_index(&mut canvas, x0 + 6, y0 + 6, i + 1);
        let fitted = contain_fit(src, cell_w, inner_h);
        let ox = x0 + (cell_w - fitted.width()) / 2;
        let oy = y0 + LABEL_H + (inner_h - fitted.height()) / 2;
        overlay_rgb(&mut canvas, &fitted, ox, oy);
    }

    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(&mut jpeg, JPEG_QUALITY)
        .encode_image(&DynamicImage::ImageRgb8(canvas))
        .ok()?;
    Some(ImageCollage {
        jpeg,
        cols,
        rows,
        names: tiles.iter().map(|(n, _)| n.clone()).collect(),
    })
}

fn contain_fit(src: &RgbImage, max_w: u32, max_h: u32) -> RgbImage {
    let (sw, sh) = (src.width().max(1), src.height().max(1));
    let scale = (max_w as f64 / sw as f64).min(max_h as f64 / sh as f64).min(1.0);
    let tw = ((sw as f64) * scale).round().max(1.0) as u32;
    let th = ((sh as f64) * scale).round().max(1.0) as u32;
    if tw == sw && th == sh {
        return src.clone();
    }
    image::imageops::resize(src, tw, th, FilterType::Triangle)
}

fn overlay_rgb(dst: &mut RgbImage, src: &RgbImage, x: u32, y: u32) {
    for (px, py, pixel) in src.enumerate_pixels() {
        let dx = x + px;
        let dy = y + py;
        if dx < dst.width() && dy < dst.height() {
            dst.put_pixel(dx, dy, *pixel);
        }
    }
}

fn fill_rect(img: &mut RgbImage, x: u32, y: u32, w: u32, h: u32, color: Rgb<u8>) {
    for py in y..y.saturating_add(h).min(img.height()) {
        for px in x..x.saturating_add(w).min(img.width()) {
            img.put_pixel(px, py, color);
        }
    }
}

/// 5×7 点阵数字 0–9。行内低位在左。
const DIGITS: [[u8; 7]; 10] = [
    [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
    [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
    [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111],
    [0b01110, 0b10001, 0b00001, 0b00110, 0b00001, 0b10001, 0b01110],
    [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
    [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110],
    [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
    [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
    [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
    [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100],
];

fn blit_index(img: &mut RgbImage, mut x: u32, y: u32, n: usize) {
    let text = n.to_string();
    for ch in text.chars() {
        if let Some(d) = ch.to_digit(10) {
            blit_digit(img, x, y, d as usize);
            x += 18;
        }
    }
}

fn blit_digit(img: &mut RgbImage, x: u32, y: u32, digit: usize) {
    let glyph = DIGITS[digit.min(9)];
    const S: u32 = 2;
    for (row, bits) in glyph.iter().enumerate() {
        for col in 0..5u32 {
            if bits & (1 << (4 - col)) != 0 {
                fill_rect(
                    img,
                    x + col * S,
                    y + row as u32 * S,
                    S,
                    S,
                    LABEL_FG,
                );
            }
        }
    }
}

/// 给单测用：把 RGB 图画成 PNG 再走磁盘构图路径之外的内存构图。
#[cfg(test)]
fn rgb_png(img: &RgbImage) -> Vec<u8> {
    use std::io::Cursor;
    let mut buf = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(img.clone())
        .write_to(&mut buf, image::ImageFormat::Png)
        .unwrap();
    buf.into_inner()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, c: [u8; 3]) -> RgbImage {
        RgbImage::from_pixel(w, h, Rgb(c))
    }

    #[test]
    fn grid_is_nearly_square() {
        assert_eq!(collage_grid(4), (2, 2));
        assert_eq!(collage_grid(5), (3, 2));
        assert_eq!(collage_grid(6), (3, 2));
        assert_eq!(collage_grid(8), (3, 3));
        assert_eq!(collage_grid(9), (3, 3));
        assert_eq!(collage_grid(12), (4, 3));
    }

    #[test]
    fn below_min_count_does_not_collage() {
        let tiles = vec![("a.png".into(), solid(80, 80, [10, 20, 30]))];
        assert!(compose_collage(&tiles).is_none());
        assert!(!should_collage(4, Some("扫一眼这批图"), false));
        assert!(!should_collage(9, Some("帮我看看这些图"), false));
        assert!(!should_collage(9, None, false));
    }

    #[test]
    fn analysis_never_collages_even_with_overview_flag() {
        let q = Some("分析这些图片，核对 logo 拼写和葡语重音");
        assert!(!should_collage(9, q, true));
        assert!(!should_collage(9, Some("逐张核验成品"), false));
        assert!(!should_collage(12, Some("Please analyze each image and verify spelling"), true));
    }

    #[test]
    fn overview_intent_or_flag_collages_from_six() {
        assert!(should_collage(6, Some("扫一眼这批有没有废图"), false));
        assert!(should_collage(9, None, true));
        assert!(should_collage(9, Some("先浏览一下整体"), false));
        assert!(!should_collage(5, Some("扫一眼"), true));
    }

    #[test]
    fn four_images_make_2x2_jpeg() {
        let tiles: Vec<(String, RgbImage)> = ["a", "b", "c", "d"]
            .iter()
            .map(|n| (format!("{n}.png"), solid(400, 300, [200, 80, 40])))
            .collect();
        let sheet = compose_collage(&tiles).expect("4 images should collage");
        assert_eq!((sheet.cols, sheet.rows), (2, 2));
        assert_eq!(sheet.names, vec!["a.png", "b.png", "c.png", "d.png"]);
        assert!(sheet.jpeg.len() > 100);
        let decoded = image::load_from_memory(&sheet.jpeg).unwrap();
        assert!(decoded.width() <= MODEL_IMAGE_MAX_DIM);
        assert!(decoded.height() <= MODEL_IMAGE_MAX_DIM);
        let msg = sheet.tool_message(true);
        assert!(msg.contains("2×2"));
        assert!(msg.contains("1. a.png"));
        assert!(msg.contains("单独 read"));
    }

    #[test]
    fn nine_images_make_3x3() {
        let tiles: Vec<(String, RgbImage)> = (1..=9)
            .map(|i| (format!("H{i}.png"), solid(640, 480, [i as u8 * 20, 40, 90])))
            .collect();
        let sheet = compose_collage(&tiles).expect("9 images should collage");
        assert_eq!((sheet.cols, sheet.rows), (3, 3));
        assert_eq!(sheet.names.len(), 9);
    }

    #[test]
    fn collage_from_paths_skips_undecodable() {
        let dir = std::env::temp_dir().join(format!("kivio-collage-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut paths = Vec::new();
        for i in 0..6 {
            let p = dir.join(format!("{i}.png"));
            if i == 2 {
                std::fs::write(&p, b"not-an-image").unwrap();
            } else {
                std::fs::write(&p, rgb_png(&solid(40, 40, [1, 2, 3]))).unwrap();
            }
            paths.push(p);
        }
        assert!(collage_from_paths(&paths).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
