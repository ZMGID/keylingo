//! 替换翻译的擦除层：按翻译组聚块的块级盖板（Youdao-style plate fill）。
//!
//! 早期实现是「字形掩膜 + 确定性填充 / MI-GAN 修复」的最小破坏路线，但字形掩膜
//! 依赖对比度阈值，灰字压深底、反锯齿边缘、压缩振铃都会漏掩膜——而两条填充路径
//! 都承诺掩膜外像素逐字节保留，漏掉的像素必然以鬼影残留（不可用的重叠）。盖板
//! 用一点背景色差换掉整个失败面：覆盖区内不存在"没擦到"的像素，成本是微秒级。
//!
//! 形态与采色的两条实测教训（改动前先读）：
//! - **盖板按翻译组聚块，不按 OCR 行**。逐行小条带在深色卡片上是一排可见的
//!   "灰条"，行间还漏出原背景；同组多行共用一块连续矩形面（含行间隙、含短行
//!   行尾），才是有道那种整段一体的观感。单行组保持贴合多边形的窄条。
//! - **环带不能贴着盖板边采**。边缘 1px 处常有字形反锯齿残边/阴影，实测把
//!   深色卡片 (22,26,33) 的盖板采成了 (47,53,60)。现在环带从盖板外推 2~5px
//!   取多行/列，块内再加行间隙内部采样（离任何 OCR 多边形 >2px 的原图像素，
//!   这是最可信的同表面背景色），全部并入中位数。
//!
//! 其余护栏：上下环带色差明显时按行线性渐变；`protect_separators` 把盖板压过
//! 的表格线按线色回补；盖板最外 1px 与原图羽化，轻噪声背景不露矩形硬边。

use std::collections::HashMap;

use image::RgbImage;

use super::layout::TranslationGroup;
use crate::rapidocr::{RapidOcrLine, RapidOcrPoint};

/// 环带相对盖板边缘的外推距离范围（px）：跳过紧贴边缘的反锯齿残边，
/// 又不至于远到跨进相邻表面（斑马纹相邻行等）。
const PLATE_RING_OFFSETS: std::ops::RangeInclusive<i32> = 2..=5;
/// 上下环带中位色差（RGB 欧氏距离）超过该值时按行做线性渐变，否则整块平色。
const PLATE_GRADIENT_THRESHOLD: f64 = 10.0;
/// 采样池至少要有这么多样本才信任它。
const PLATE_MIN_SAMPLES: usize = 8;
/// 行间隙内部采样离 OCR 多边形的安全距离（px）：避开字形反锯齿。
const PLATE_INTERIOR_SLACK: f32 = 2.0;
/// 羽化只在原像素与盖板色接近时混合（隐藏接缝）；差异大说明原像素是待盖的墨迹，
/// 混进来会变成灰色斑点。
const PLATE_FEATHER_TOLERANCE: f64 = 32.0;

/// 把翻译组映射回 OCR span 聚块：同组的行共用一块盖板，未入任何组的 span
/// 单独成块（按 id 排序保证确定性）。`lens_replace_translate` 与 fixture
/// 门禁共用，保证生产与测试跑同一个聚块逻辑。
pub fn blocks_from_groups(
    groups: &[TranslationGroup],
    spans: &[RapidOcrLine],
) -> Vec<Vec<RapidOcrLine>> {
    let mut by_id: HashMap<&str, &RapidOcrLine> =
        spans.iter().map(|span| (span.id.as_str(), span)).collect();
    let mut blocks = Vec::new();
    for group in groups {
        let leaves: Vec<RapidOcrLine> = group
            .leaf_ids
            .iter()
            .filter_map(|id| by_id.remove(id.as_str()).cloned())
            .collect();
        if !leaves.is_empty() {
            blocks.push(leaves);
        }
    }
    let mut leftovers: Vec<&RapidOcrLine> = by_id.into_values().collect();
    leftovers.sort_by(|a, b| a.id.cmp(&b.id));
    blocks.extend(leftovers.into_iter().map(|span| vec![span.clone()]));
    blocks
}

/// 块级盖板填充。`blocks` 的每个元素是一组属于同一视觉块（翻译组）的 OCR 行。
/// 输出图中：覆盖区内是采样出的背景色（含渐变/羽化/分隔线回补），覆盖区外
/// 逐字节等于原图。纯 CPU、无模型依赖，调用方负责放进阻塞线程池。
pub fn plate_fill(image: &RgbImage, blocks: &[Vec<RapidOcrLine>]) -> RgbImage {
    let (width, height) = image.dimensions();
    let regions: Vec<BlockRegion> = blocks
        .iter()
        .filter_map(|leaves| BlockRegion::build(image, leaves))
        .collect();
    let mut coverage = vec![0u8; width as usize * height as usize];
    for region in &regions {
        region.rasterize(&mut coverage, width, height);
    }
    let coverage = protect_separators(image, coverage, width, height);
    let mut output = image.clone();
    for region in &regions {
        fill_block_plate(image, &mut output, &coverage, region);
    }
    feather_plate_edges(image, &mut output, &coverage, width, height);
    output
}

/// 盖板相对 OCR 多边形的各向外扩：盖住字形反锯齿与轻微阴影。
fn plate_margin(span_height: f32) -> f32 {
    (span_height * 0.12).clamp(2.0, 6.0)
}

/// 一个视觉块的盖板几何：多行组是整块矩形（含行间隙），单行组是贴合的
/// 膨胀多边形 + 行末补带。
struct BlockRegion {
    polygons: Vec<Vec<RapidOcrPoint>>,
    /// 外接矩形（含外扩），像素坐标半开区间。
    rect: (i32, i32, i32, i32),
    rep_height: f32,
    multi: bool,
}

impl BlockRegion {
    fn build(image: &RgbImage, leaves: &[RapidOcrLine]) -> Option<Self> {
        let mut polygons = Vec::new();
        let mut heights = Vec::new();
        for leaf in leaves {
            let points = normalized_points(leaf);
            if points.len() < 3 {
                continue;
            }
            heights.push(leaf.height);
            polygons.push(points);
        }
        if polygons.is_empty() {
            return None;
        }
        heights.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let rep_height = heights[heights.len() / 2];
        let (horizontal, vertical) = band_margins(rep_height);
        let mut min_x = f32::INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        for polygon in &polygons {
            let (x0, y0, x1, y1) = polygon_bbox(polygon);
            min_x = min_x.min(x0);
            min_y = min_y.min(y0);
            max_x = max_x.max(x1);
            max_y = max_y.max(y1);
        }
        let rect = (
            (min_x - horizontal).floor().max(0.0) as i32,
            (min_y - vertical).floor().max(0.0) as i32,
            (max_x + horizontal).ceil().min(image.width() as f32) as i32,
            (max_y + vertical).ceil().min(image.height() as f32) as i32,
        );
        if rect.2 <= rect.0 || rect.3 <= rect.1 {
            return None;
        }
        let multi = polygons.len() > 1;
        Some(Self {
            polygons,
            rect,
            rep_height,
            multi,
        })
    }

    fn rasterize(&self, data: &mut [u8], width: u32, height: u32) {
        if self.multi {
            let (x0, y0, x1, y1) = self.rect;
            for y in y0.max(0)..y1.min(height as i32) {
                for x in x0.max(0)..x1.min(width as i32) {
                    data[y as usize * width as usize + x as usize] = 255;
                }
            }
            return;
        }
        let polygon = &self.polygons[0];
        rasterize_dilated_polygon(data, width, height, polygon, plate_margin(self.rep_height));
        rasterize_end_bands(data, width, height, polygon, self.rep_height);
    }

    /// 采样点是否离任何 OCR 多边形太近（字形反锯齿区）。
    fn near_any_leaf(&self, x: f32, y: f32, slack: f32) -> bool {
        for polygon in &self.polygons {
            let (bx0, by0, bx1, by1) = polygon_bbox(polygon);
            if x < bx0 - slack || x > bx1 + slack || y < by0 - slack || y > by1 + slack {
                continue;
            }
            if point_in_polygon(x, y, polygon) || distance_to_polygon(x, y, polygon) <= slack {
                return true;
            }
        }
        false
    }
}

/// 行末补带与上下外扩的宽度（沿用旧字形掩膜的采样带标定）。
fn band_margins(span_height: f32) -> (f32, f32) {
    let margin = plate_margin(span_height);
    (
        (span_height * 0.22).clamp(2.0, 14.0).max(margin),
        (span_height * 0.05).clamp(1.0, 3.0).max(margin),
    )
}

/// OCR 框常在行首/行尾标点前几像素截断。给单行盖板左右各补一段水平延伸带，
/// 把框外的标点/反锯齿一并盖住（多行块的整块矩形天然覆盖）。
fn rasterize_end_bands(
    data: &mut [u8],
    width: u32,
    height: u32,
    points: &[RapidOcrPoint],
    span_height: f32,
) {
    let (min_x, min_y, max_x, max_y) = polygon_bbox(points);
    let (horizontal, vertical) = band_margins(span_height);
    let y0 = (min_y - vertical).floor().max(0.0) as u32;
    let y1 = (max_y + vertical).ceil().min(height as f32) as u32;
    for (band_x0, band_x1) in [(min_x - horizontal, min_x), (max_x, max_x + horizontal)] {
        let x0 = band_x0.floor().max(0.0) as u32;
        let x1 = band_x1.ceil().min(width as f32) as u32;
        for y in y0..y1 {
            for x in x0..x1 {
                data[y as usize * width as usize + x as usize] = 255;
            }
        }
    }
}

/// Erase coverage can spill over table borders and row dividers, which the
/// plate then repaints with cell background — visibly breaking the grid. Reuse
/// the layout separator detector and restore covered pixels along each rule
/// line (±1px for antialiasing) — but only pixels whose original color matches
/// the line's uncovered pixels, so glyph ink crossing a detected line (or a
/// long text row misdetected as one) still gets erased.
fn protect_separators(image: &RgbImage, mut data: Vec<u8>, width: u32, height: u32) -> Vec<u8> {
    const LINE_COLOR_TOLERANCE: f64 = 40.0;
    let separators = super::layout::detect_separators(image);
    let mut restore_line = |pixels: &[(u32, u32)]| {
        let unmasked: Vec<[u8; 3]> = pixels
            .iter()
            .filter(|(x, y)| data[*y as usize * width as usize + *x as usize] == 0)
            .map(|(x, y)| image.get_pixel(*x, *y).0)
            .collect();
        if unmasked.is_empty() {
            return;
        }
        let line_color = median_pixels(&unmasked);
        for (x, y) in pixels {
            let index = *y as usize * width as usize + *x as usize;
            if data[index] != 0
                && color_distance(image.get_pixel(*x, *y).0, line_color) <= LINE_COLOR_TOLERANCE
            {
                data[index] = 0;
            }
        }
    };
    for line_y in &separators.horizontal {
        let center = line_y.round() as i64;
        for y in center - 1..=center + 1 {
            if y < 0 || y >= height as i64 {
                continue;
            }
            let pixels: Vec<(u32, u32)> = (0..width).map(|x| (x, y as u32)).collect();
            restore_line(&pixels);
        }
    }
    for line_x in &separators.vertical {
        let center = line_x.round() as i64;
        for x in center - 1..=center + 1 {
            if x < 0 || x >= width as i64 {
                continue;
            }
            let pixels: Vec<(u32, u32)> = (0..height).map(|y| (x as u32, y)).collect();
            restore_line(&pixels);
        }
    }
    data
}

/// 一个块的盖板着色。采样池 = 四边外推环带（2~5px，避开边缘残边）+ 多行块的
/// 行间隙内部采样；上下环带色差明显时按行线性渐变，否则取全池中位色；池子空了
/// 回退块内主色簇（字形墨迹是少数，主簇即背景）；再拿不到就放弃填充。
fn fill_block_plate(
    image: &RgbImage,
    output: &mut RgbImage,
    coverage: &[u8],
    region: &BlockRegion,
) {
    let width = image.width() as i32;
    let (x0, y0, x1, y1) = region.rect;
    let mut top = Vec::new();
    let mut bottom = Vec::new();
    let mut pool = Vec::new();
    for offset in PLATE_RING_OFFSETS {
        collect_row(image, coverage, x0, x1, y0 - offset, &mut top);
        collect_row(image, coverage, x0, x1, y1 - 1 + offset, &mut bottom);
        collect_col(image, coverage, y0, y1, x0 - offset, &mut pool);
        collect_col(image, coverage, y0, y1, x1 - 1 + offset, &mut pool);
    }
    pool.extend_from_slice(&top);
    pool.extend_from_slice(&bottom);
    if region.multi {
        // 行间隙/短行行尾的原图像素：同表面背景的最可信来源。
        let slack = PLATE_INTERIOR_SLACK.max(plate_margin(region.rep_height) - 2.0);
        let mut y = y0;
        while y < y1 {
            let mut x = x0;
            while x < x1 {
                if !region.near_any_leaf(x as f32 + 0.5, y as f32 + 0.5, slack) {
                    pool.push(image.get_pixel(x as u32, y as u32).0);
                }
                x += 2;
            }
            y += 2;
        }
    }
    let top_color = (top.len() >= PLATE_MIN_SAMPLES).then(|| median_pixels(&top));
    let bottom_color = (bottom.len() >= PLATE_MIN_SAMPLES).then(|| median_pixels(&bottom));
    let flat = if pool.len() >= PLATE_MIN_SAMPLES {
        Some(median_pixels(&pool))
    } else {
        region
            .polygons
            .first()
            .and_then(|polygon| dominant_background(&polygon_samples(image, polygon, 0.0, 0.0)))
            .map(|(color, _)| color)
    };
    let (start, end) = match (top_color, bottom_color) {
        (Some(top_color), Some(bottom_color))
            if color_distance(top_color, bottom_color) > PLATE_GRADIENT_THRESHOLD =>
        {
            (top_color, bottom_color)
        }
        _ => match flat {
            Some(color) => (color, color),
            None => return,
        },
    };
    let row_span = (y1 - 1 - y0).max(1) as f64;
    for y in y0..y1 {
        let t = (y - y0) as f64 / row_span;
        let color = lerp_color(start, end, t);
        for x in x0..x1 {
            let index = y as usize * width as usize + x as usize;
            if coverage[index] != 0 {
                output.put_pixel(x as u32, y as u32, image::Rgb(color));
            }
        }
    }
}

fn collect_row(
    image: &RgbImage,
    coverage: &[u8],
    x0: i32,
    x1: i32,
    y: i32,
    samples: &mut Vec<[u8; 3]>,
) {
    let width = image.width() as i32;
    let height = image.height() as i32;
    if y < 0 || y >= height {
        return;
    }
    for x in x0.max(0)..x1.min(width) {
        let index = y as usize * width as usize + x as usize;
        if coverage[index] == 0 {
            samples.push(image.get_pixel(x as u32, y as u32).0);
        }
    }
}

fn collect_col(
    image: &RgbImage,
    coverage: &[u8],
    y0: i32,
    y1: i32,
    x: i32,
    samples: &mut Vec<[u8; 3]>,
) {
    let width = image.width() as i32;
    let height = image.height() as i32;
    if x < 0 || x >= width {
        return;
    }
    for y in y0.max(0)..y1.min(height) {
        let index = y as usize * width as usize + x as usize;
        if coverage[index] == 0 {
            samples.push(image.get_pixel(x as u32, y as u32).0);
        }
    }
}

/// 盖板最外 1px（存在未覆盖 4 邻居的覆盖像素）与原图混合，隐藏轻噪声背景上的
/// 矩形接缝。原像素与盖板色差异过大（多半是压在边界上的墨迹）时保持纯盖板色。
fn feather_plate_edges(
    image: &RgbImage,
    output: &mut RgbImage,
    coverage: &[u8],
    width: u32,
    height: u32,
) {
    let width_i = width as i32;
    let height_i = height as i32;
    for y in 0..height_i {
        for x in 0..width_i {
            let index = y as usize * width as usize + x as usize;
            if coverage[index] == 0 {
                continue;
            }
            let on_edge = [(0i32, -1i32), (0, 1), (-1, 0), (1, 0)]
                .iter()
                .any(|(dx, dy)| {
                    let next_x = x + dx;
                    let next_y = y + dy;
                    next_x >= 0
                        && next_y >= 0
                        && next_x < width_i
                        && next_y < height_i
                        && coverage[next_y as usize * width as usize + next_x as usize] == 0
                });
            if !on_edge {
                continue;
            }
            let plate = output.get_pixel(x as u32, y as u32).0;
            let original = image.get_pixel(x as u32, y as u32).0;
            if color_distance(plate, original) > PLATE_FEATHER_TOLERANCE {
                continue;
            }
            let blended = [
                ((plate[0] as u16 + original[0] as u16) / 2) as u8,
                ((plate[1] as u16 + original[1] as u16) / 2) as u8,
                ((plate[2] as u16 + original[2] as u16) / 2) as u8,
            ];
            output.put_pixel(x as u32, y as u32, image::Rgb(blended));
        }
    }
}

fn lerp_color(start: [u8; 3], end: [u8; 3], t: f64) -> [u8; 3] {
    [0, 1, 2].map(|channel| {
        (start[channel] as f64 + (end[channel] as f64 - start[channel] as f64) * t).round() as u8
    })
}

fn polygon_bbox(points: &[RapidOcrPoint]) -> (f32, f32, f32, f32) {
    let min_x = points
        .iter()
        .map(|point| point.x)
        .fold(f32::INFINITY, f32::min);
    let min_y = points
        .iter()
        .map(|point| point.y)
        .fold(f32::INFINITY, f32::min);
    let max_x = points
        .iter()
        .map(|point| point.x)
        .fold(f32::NEG_INFINITY, f32::max);
    let max_y = points
        .iter()
        .map(|point| point.y)
        .fold(f32::NEG_INFINITY, f32::max);
    (min_x, min_y, max_x, max_y)
}

fn polygon_samples(
    image: &RgbImage,
    points: &[RapidOcrPoint],
    horizontal_margin: f32,
    vertical_margin: f32,
) -> Vec<(u32, u32, [u8; 3])> {
    let (polygon_min_x, polygon_min_y, polygon_max_x, polygon_max_y) = polygon_bbox(points);
    let min_x = (polygon_min_x - horizontal_margin).floor().max(0.0) as u32;
    let min_y = (polygon_min_y - vertical_margin).floor().max(0.0) as u32;
    let max_x = (polygon_max_x + horizontal_margin)
        .ceil()
        .min(image.width() as f32) as u32;
    let max_y = (polygon_max_y + vertical_margin)
        .ceil()
        .min(image.height() as f32) as u32;

    let mut samples = Vec::new();
    for y in min_y..max_y {
        for x in min_x..max_x {
            let sample_x = x as f32 + 0.5;
            let sample_y = y as f32 + 0.5;
            let in_horizontal_end_band = sample_y >= polygon_min_y - vertical_margin
                && sample_y <= polygon_max_y + vertical_margin
                && ((sample_x < polygon_min_x && sample_x >= polygon_min_x - horizontal_margin)
                    || (sample_x > polygon_max_x && sample_x <= polygon_max_x + horizontal_margin));
            let near_polygon = distance_to_polygon(sample_x, sample_y, points) <= vertical_margin;
            if !point_in_polygon(sample_x, sample_y, points)
                && !in_horizontal_end_band
                && !near_polygon
            {
                continue;
            }
            let pixel = image.get_pixel(x, y).0;
            samples.push((x, y, pixel));
        }
    }
    samples
}

fn dominant_background(samples: &[(u32, u32, [u8; 3])]) -> Option<([u8; 3], f64)> {
    if samples.is_empty() {
        return None;
    }
    let key_for = |pixel: [u8; 3]| {
        ((pixel[0] as u16 >> 5) << 6) | ((pixel[1] as u16 >> 5) << 3) | (pixel[2] as u16 >> 5)
    };
    let mut counts = HashMap::<u16, usize>::new();
    for (_, _, pixel) in samples {
        *counts.entry(key_for(*pixel)).or_default() += 1;
    }
    // HashMap iteration order is randomized. Resolve equal-size clusters
    // deterministically so the same image cannot alternate between treating a
    // dark glyph cluster and the surrounding background as dominant.
    let (&dominant_key, &dominant_count) =
        counts.iter().max_by_key(|(key, count)| (**count, **key))?;
    let mut channels = [
        Vec::with_capacity(dominant_count),
        Vec::with_capacity(dominant_count),
        Vec::with_capacity(dominant_count),
    ];
    for (_, _, pixel) in samples {
        if key_for(*pixel) != dominant_key {
            continue;
        }
        for channel in 0..3 {
            channels[channel].push(pixel[channel]);
        }
    }
    Some((
        [
            median_u8(&mut channels[0]),
            median_u8(&mut channels[1]),
            median_u8(&mut channels[2]),
        ],
        dominant_count as f64 / samples.len() as f64,
    ))
}

fn median_pixels(samples: &[[u8; 3]]) -> [u8; 3] {
    let mut channels = [
        Vec::with_capacity(samples.len()),
        Vec::with_capacity(samples.len()),
        Vec::with_capacity(samples.len()),
    ];
    for pixel in samples {
        for channel in 0..3 {
            channels[channel].push(pixel[channel]);
        }
    }
    [
        median_u8(&mut channels[0]),
        median_u8(&mut channels[1]),
        median_u8(&mut channels[2]),
    ]
}

fn normalized_points(span: &RapidOcrLine) -> Vec<RapidOcrPoint> {
    if span.points.len() >= 3 {
        span.points.clone()
    } else {
        vec![
            RapidOcrPoint {
                x: span.x,
                y: span.y,
            },
            RapidOcrPoint {
                x: span.x + span.width,
                y: span.y,
            },
            RapidOcrPoint {
                x: span.x + span.width,
                y: span.y + span.height,
            },
            RapidOcrPoint {
                x: span.x,
                y: span.y + span.height,
            },
        ]
    }
}

fn rasterize_dilated_polygon(
    data: &mut [u8],
    width: u32,
    height: u32,
    points: &[RapidOcrPoint],
    radius: f32,
) {
    let (min_x, min_y, max_x, max_y) = polygon_bbox(points);
    let x0 = (min_x - radius).floor().max(0.0) as u32;
    let y0 = (min_y - radius).floor().max(0.0) as u32;
    let x1 = (max_x + radius).ceil().min(width as f32) as u32;
    let y1 = (max_y + radius).ceil().min(height as f32) as u32;
    for y in y0..y1 {
        for x in x0..x1 {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            if point_in_polygon(px, py, points) || distance_to_polygon(px, py, points) <= radius {
                data[y as usize * width as usize + x as usize] = 255;
            }
        }
    }
}

fn point_in_polygon(x: f32, y: f32, points: &[RapidOcrPoint]) -> bool {
    let mut inside = false;
    let mut previous = points[points.len() - 1];
    for current in points {
        if ((current.y > y) != (previous.y > y))
            && x < (previous.x - current.x) * (y - current.y) / (previous.y - current.y) + current.x
        {
            inside = !inside;
        }
        previous = *current;
    }
    inside
}

fn distance_to_polygon(x: f32, y: f32, points: &[RapidOcrPoint]) -> f32 {
    let mut distance = f32::INFINITY;
    let mut previous = points[points.len() - 1];
    for current in points {
        distance = distance.min(distance_to_segment(x, y, previous, *current));
        previous = *current;
    }
    distance
}

fn distance_to_segment(x: f32, y: f32, start: RapidOcrPoint, end: RapidOcrPoint) -> f32 {
    let dx = end.x - start.x;
    let dy = end.y - start.y;
    let length_sq = dx * dx + dy * dy;
    if length_sq <= f32::EPSILON {
        return ((x - start.x).powi(2) + (y - start.y).powi(2)).sqrt();
    }
    let t = (((x - start.x) * dx + (y - start.y) * dy) / length_sq).clamp(0.0, 1.0);
    let projected_x = start.x + t * dx;
    let projected_y = start.y + t * dy;
    ((x - projected_x).powi(2) + (y - projected_y).powi(2)).sqrt()
}

fn color_distance(a: [u8; 3], b: [u8; 3]) -> f64 {
    let dr = a[0] as f64 - b[0] as f64;
    let dg = a[1] as f64 - b[1] as f64;
    let db = a[2] as f64 - b[2] as f64;
    (dr * dr + dg * dg + db * db).sqrt()
}

fn median_u8(values: &mut [u8]) -> u8 {
    values.sort_unstable();
    values[values.len() / 2]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(points: Vec<RapidOcrPoint>) -> RapidOcrLine {
        RapidOcrLine {
            id: "s0000".into(),
            text: "text".into(),
            points,
            x: 10.0,
            y: 10.0,
            width: 20.0,
            height: 20.0,
        }
    }

    fn rect_span(id: &str, x: f32, y: f32, width: f32, height: f32) -> RapidOcrLine {
        RapidOcrLine {
            id: id.into(),
            text: "text".into(),
            points: Vec::new(),
            x,
            y,
            width,
            height,
        }
    }

    fn single(item: RapidOcrLine) -> Vec<Vec<RapidOcrLine>> {
        vec![vec![item]]
    }

    #[test]
    fn plate_erases_glyph_including_antialias_halo() {
        let mut image = RgbImage::from_pixel(40, 40, image::Rgb([245, 245, 245]));
        for y in 12..28 {
            for x in 18..22 {
                image.put_pixel(x, y, image::Rgb([20, 20, 20]));
            }
        }
        // 模拟字形掩膜时代会漏掉的反锯齿边缘：中等对比像素紧贴笔画。
        for y in 12..28 {
            image.put_pixel(17, y, image::Rgb([180, 180, 180]));
            image.put_pixel(22, y, image::Rgb([180, 180, 180]));
        }
        let output = plate_fill(&image, &single(span(Vec::new())));
        for y in 12..28 {
            for x in 17..23 {
                let pixel = output.get_pixel(x, y).0;
                assert!(
                    pixel.iter().all(|channel| *channel >= 235),
                    "residual ink at ({x},{y}): {pixel:?}"
                );
            }
        }
    }

    #[test]
    fn rotated_polygon_plate_covers_ink_but_not_far_corner() {
        let item = span(vec![
            RapidOcrPoint { x: 20.0, y: 10.0 },
            RapidOcrPoint { x: 30.0, y: 20.0 },
            RapidOcrPoint { x: 20.0, y: 30.0 },
            RapidOcrPoint { x: 10.0, y: 20.0 },
        ]);
        let mut image = RgbImage::from_pixel(40, 40, image::Rgb([245, 245, 245]));
        for y in 18..23 {
            for x in 18..23 {
                image.put_pixel(x, y, image::Rgb([20, 20, 20]));
            }
        }
        // 远角哨兵：多边形外扩不应该把远处像素也盖掉。
        image.put_pixel(2, 2, image::Rgb([10, 200, 30]));
        let output = plate_fill(&image, &single(item));
        assert!(output.get_pixel(20, 20).0.iter().all(|c| *c >= 235));
        assert_eq!(output.get_pixel(2, 2).0, [10, 200, 30]);
    }

    #[test]
    fn plate_is_clipped_at_image_boundary() {
        let item = rect_span("s0000", -5.0, -5.0, 12.0, 12.0);
        let mut image = RgbImage::from_pixel(10, 10, image::Rgb([245, 245, 245]));
        for y in 0..4 {
            for x in 0..4 {
                image.put_pixel(x, y, image::Rgb([20, 20, 20]));
            }
        }
        let output = plate_fill(&image, &single(item));
        assert!(output.get_pixel(1, 1).0.iter().all(|c| *c >= 200));
    }

    #[test]
    fn plate_tracks_vertical_gradient() {
        let mut image = RgbImage::from_fn(40, 40, |_, y| {
            let value = (y * 6).min(240) as u8;
            image::Rgb([value, value, value])
        });
        for y in 12..28 {
            for x in 14..26 {
                image.put_pixel(x, y, image::Rgb([0, 0, 0]));
            }
        }
        let output = plate_fill(&image, &single(span(Vec::new())));
        // 行 y=20 的真实背景值是 120；渐变盖板应当落在附近而不是整块平色。
        let repaired = output.get_pixel(20, 20).0[0] as i32;
        assert!(
            (120 - 30..=120 + 30).contains(&repaired),
            "gradient plate at y=20 should be near 120, got {repaired}"
        );
        // 顶部与底部各自贴近本行背景。
        let top = output.get_pixel(20, 12).0[0] as i32;
        let bottom = output.get_pixel(20, 27).0[0] as i32;
        assert!(
            bottom > top + 40,
            "plate lost the gradient: top {top}, bottom {bottom}"
        );
    }

    #[test]
    fn adjacent_blocks_take_their_own_background() {
        // 上半白、下半深灰的"斑马纹"：两行文字各在一个色带里，各自成块。
        let mut image = RgbImage::from_fn(120, 80, |_, y| {
            if y < 40 {
                image::Rgb([250, 250, 250])
            } else {
                image::Rgb([60, 60, 60])
            }
        });
        for x in 20..100 {
            for y in 14..26 {
                image.put_pixel(x, y, image::Rgb([20, 20, 20]));
            }
            for y in 54..66 {
                image.put_pixel(x, y, image::Rgb([230, 230, 230]));
            }
        }
        let top_span = rect_span("s0000", 18.0, 13.0, 84.0, 14.0);
        let bottom_span = rect_span("s0001", 18.0, 53.0, 84.0, 14.0);
        let output = plate_fill(&image, &[vec![top_span], vec![bottom_span]]);
        assert!(output.get_pixel(60, 20).0.iter().all(|c| *c >= 235));
        assert!(output.get_pixel(60, 60).0.iter().all(|c| *c <= 80));
    }

    #[test]
    fn end_band_covers_punctuation_beyond_ocr_box() {
        let mut image = RgbImage::from_pixel(80, 40, image::Rgb([255, 255, 255]));
        // OCR 框右缘之外 3px 的标点墨迹。
        for y in 14..24 {
            for x in 31..34 {
                image.put_pixel(x, y, image::Rgb([20, 20, 20]));
            }
        }
        let output = plate_fill(&image, &single(span(Vec::new())));
        assert!(output.get_pixel(32, 18).0.iter().all(|c| *c >= 235));
    }

    #[test]
    fn plate_never_changes_pixels_far_outside_coverage() {
        let image = RgbImage::from_fn(60, 60, |x, y| image::Rgb([x as u8 * 3, y as u8 * 3, 200]));
        let output = plate_fill(&image, &single(span(Vec::new())));
        // span bbox 10..30 + 最大水平带 14 + 环带/羽化余量,取 x/y >= 50 的区域必然原样。
        for y in 50..60u32 {
            for x in 50..60u32 {
                assert_eq!(image.get_pixel(x, y), output.get_pixel(x, y));
            }
        }
    }

    #[test]
    fn full_image_span_falls_back_to_dominant_cluster() {
        // span 覆盖整图：没有任何环带可采，回退 span 内主色簇。
        let mut image = RgbImage::from_pixel(30, 30, image::Rgb([240, 240, 240]));
        for y in 10..20 {
            for x in 10..20 {
                image.put_pixel(x, y, image::Rgb([15, 15, 15]));
            }
        }
        let item = rect_span("s0000", 0.0, 0.0, 30.0, 30.0);
        let output = plate_fill(&image, &single(item));
        assert!(output.get_pixel(15, 15).0.iter().all(|c| *c >= 230));
    }

    #[test]
    fn separator_rule_survives_plate() {
        let mut image = RgbImage::from_pixel(200, 100, image::Rgb([255, 255, 255]));
        for x in 0..200 {
            image.put_pixel(x, 50, image::Rgb([180, 180, 180]));
        }
        for y in 40..48 {
            for x in 30..170 {
                image.put_pixel(x, y, image::Rgb([20, 20, 20]));
            }
        }
        let item = rect_span("s0000", 28.0, 38.0, 144.0, 14.0);
        let output = plate_fill(&image, &single(item));
        // 文字被抹掉。
        assert!(output.get_pixel(100, 44).0.iter().all(|c| *c >= 235));
        // 盖板压过的分隔线像素被回补为线色（羽化最多把它调亮一半）。
        let rule = output.get_pixel(100, 50).0;
        assert!(
            rule.iter().all(|c| (170..=230).contains(c)),
            "separator was repainted: {rule:?}"
        );
    }

    /// 复刻实测翻车现场：深色卡片上的浅灰多行段落，字形边缘带反锯齿残边。
    /// 断言三件事：行间隙一起被盖住（整段一块连续面）、盖板色贴近真实背景
    /// （不被残边采样带亮）、整块颜色均匀（无逐行条带）。
    #[test]
    fn paragraph_block_paints_one_continuous_background_plate() {
        let card = [20u8, 24, 30];
        let mut image = RgbImage::from_pixel(220, 90, image::Rgb(card));
        let lines = [
            rect_span("s0000", 10.0, 10.0, 180.0, 14.0),
            rect_span("s0001", 10.0, 30.0, 180.0, 14.0),
            rect_span("s0002", 10.0, 50.0, 140.0, 14.0),
        ];
        for line in &lines {
            let x0 = line.x as u32;
            let x1 = (line.x + line.width) as u32;
            let y0 = line.y as u32;
            let y1 = (line.y + line.height) as u32;
            for y in y0..y1 {
                for x in x0..x1 {
                    // 文字主体 + 大量中等对比的反锯齿像素。
                    let pixel = if (x + y) % 3 == 0 {
                        [140, 146, 155]
                    } else if (x + y) % 3 == 1 {
                        [70, 75, 82]
                    } else {
                        card
                    };
                    image.put_pixel(x, y, image::Rgb(pixel));
                }
            }
        }
        let output = plate_fill(&image, &[lines.to_vec()]);
        let near_card = |pixel: [u8; 3]| {
            pixel
                .iter()
                .zip(card.iter())
                .all(|(a, b)| (*a as i32 - *b as i32).abs() <= 8)
        };
        // 行间隙也被盖住且是背景色（整段一块面，不漏原图）。
        assert!(
            near_card(output.get_pixel(100, 27).0),
            "line gap not plated with card color: {:?}",
            output.get_pixel(100, 27).0
        );
        // 行内文字被背景色盖掉，没被残边带亮。
        assert!(
            near_card(output.get_pixel(100, 17).0),
            "plate color drifted bright: {:?}",
            output.get_pixel(100, 17).0
        );
        // 短行行尾的剩余区域同样并入整块。
        assert!(
            near_card(output.get_pixel(170, 57).0),
            "short-line tail not plated: {:?}",
            output.get_pixel(170, 57).0
        );
        // 无逐行条带：不同行上的盖板颜色一致。
        let line1 = output.get_pixel(100, 17).0;
        let line2 = output.get_pixel(100, 37).0;
        assert!(
            color_distance(line1, line2) <= 6.0,
            "per-line banding: {line1:?} vs {line2:?}"
        );
    }

    #[test]
    fn blocks_from_groups_reunites_group_leaves_and_keeps_leftovers() {
        let spans = vec![
            rect_span("s0000", 10.0, 10.0, 100.0, 14.0),
            rect_span("s0001", 10.0, 30.0, 100.0, 14.0),
            rect_span("s0002", 10.0, 60.0, 100.0, 14.0),
        ];
        let group = TranslationGroup {
            id: "g0".into(),
            leaf_ids: vec!["s0000".into(), "s0001".into()],
            source_text: String::new(),
            translated: String::new(),
        };
        let blocks = blocks_from_groups(&[group], &spans);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].len(), 2);
        assert_eq!(blocks[1].len(), 1);
        assert_eq!(blocks[1][0].id, "s0002");
    }
}
