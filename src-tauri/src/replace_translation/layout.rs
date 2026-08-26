//! Cell-aware and paragraph-aware layout aggregation.
//!
//! OCR leaf polygons remain untouched for erasure. This module only decides how
//! much space translated text may use.

use std::collections::HashMap;

use image::RgbImage;
use serde::Serialize;

use crate::rapidocr::{cmp_f32, median, RapidOcrLine};

#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReplaceBounds {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReplaceRegionKind {
    Cell,
    Line,
    Paragraph,
    Heading,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReplaceTextAlign {
    Left,
    Center,
    Right,
}

/// RapidOCR already emits the leaf-level geometry required by replacement
/// translation. Naming it here makes the boundary explicit without copying
/// the OCR payload into a second, drifting representation.
pub type OcrLeaf = RapidOcrLine;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TranslationGroup {
    pub id: String,
    pub leaf_ids: Vec<String>,
    pub source_text: String,
    pub translated: String,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TextAnchor {
    pub x: f32,
    pub y: f32,
    pub baseline_y: f32,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RenderFlow {
    ExactLine,
    ParagraphFlow,
    CellFlow,
    ScenePatch,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RenderVerticalAlign {
    Top,
    Center,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RenderSlot {
    pub id: String,
    pub group_id: String,
    pub leaf_ids: Vec<String>,
    pub bounds: ReplaceBounds,
    pub anchor: TextAnchor,
    pub flow: RenderFlow,
    pub kind: ReplaceRegionKind,
    pub align: ReplaceTextAlign,
    pub vertical_align: RenderVerticalAlign,
    pub source_font_px: f32,
    pub source_color: String,
}

#[derive(Debug, Clone, Default)]
pub struct ReplaceGeometry {
    pub groups: Vec<TranslationGroup>,
    pub slots: Vec<RenderSlot>,
}

#[derive(Debug, Clone)]
struct TranslationLayoutDraft {
    source_text: String,
    bounds: ReplaceBounds,
    span_ids: Vec<String>,
    kind: ReplaceRegionKind,
    align: ReplaceTextAlign,
}

#[derive(Debug, Clone)]
struct RegionDraft {
    spans: Vec<RapidOcrLine>,
    bounds: ReplaceBounds,
    kind: ReplaceRegionKind,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Separators {
    pub(crate) horizontal: Vec<f32>,
    pub(crate) vertical: Vec<f32>,
}

fn build_translation_layout_drafts(
    image: &RgbImage,
    spans: &[RapidOcrLine],
) -> Vec<TranslationLayoutDraft> {
    if spans.is_empty() {
        return Vec::new();
    }
    let separators = detect_separators(image);
    let mut assigned = vec![false; spans.len()];
    let mut drafts = table_cell_regions(spans, &separators, &mut assigned);
    let remaining: Vec<RapidOcrLine> = spans
        .iter()
        .zip(assigned)
        .filter_map(|(span, assigned)| (!assigned).then_some(span.clone()))
        .collect();
    drafts.extend(fallback_regions(
        image.width(),
        image.height(),
        &remaining,
        &separators,
    ));
    drafts.sort_by(|a, b| cmp_f32(a.bounds.y, b.bounds.y).then(cmp_f32(a.bounds.x, b.bounds.x)));
    drafts.into_iter().map(finalize_region).collect()
}

/// Build translation groups and render slots. Paragraph regions become ONE
/// group (whole-paragraph translation, coherent across source line breaks)
/// with one render slot per source visual line — the frontend flows the
/// translation through those slots (`layoutReplaceTextFlow`), so line geometry
/// never moves and a menu can never collapse into one vertically centred
/// rectangle. Headings and standalone lines stay exact-line groups.
pub fn build_replace_geometry(image: &RgbImage, leaves: &[OcrLeaf]) -> ReplaceGeometry {
    let regions = build_translation_layout_drafts(image, leaves);
    let leaves_by_id: HashMap<&str, &OcrLeaf> =
        leaves.iter().map(|leaf| (leaf.id.as_str(), leaf)).collect();
    let mut geometry = ReplaceGeometry::default();

    for region in regions {
        let region_leaves: Vec<OcrLeaf> = region
            .span_ids
            .iter()
            .filter_map(|id| leaves_by_id.get(id.as_str()).map(|leaf| (*leaf).clone()))
            .collect();

        if region.kind == ReplaceRegionKind::Cell {
            let group_id = format!("r{:04}", geometry.groups.len());
            geometry.groups.push(TranslationGroup {
                id: group_id.clone(),
                leaf_ids: region.span_ids.clone(),
                source_text: region.source_text.clone(),
                translated: region.source_text.clone(),
            });
            geometry.slots.push(render_slot(
                image,
                &group_id,
                0,
                region_leaves,
                region.bounds,
                ReplaceRegionKind::Cell,
                RenderFlow::CellFlow,
                region.align,
                RenderVerticalAlign::Center,
            ));
        } else if region.kind == ReplaceRegionKind::Paragraph {
            // 段落整段成组：全文一次翻译（跨行语义连贯，逐行翻译会把一句话
            // 拦腰截成碎片），译文由前端 layoutReplaceTextFlow 依次流过每行
            // 槽位——行几何不动、整组字号统一；盖板层按组聚块，整段一块
            // 连续面而不是逐行灰条。
            let group_id = format!("r{:04}", geometry.groups.len());
            geometry.groups.push(TranslationGroup {
                id: group_id.clone(),
                leaf_ids: region.span_ids.clone(),
                source_text: region.source_text.clone(),
                translated: region.source_text.clone(),
            });
            for (slot_index, line) in visual_lines(&region_leaves).into_iter().enumerate() {
                let bounds = line_slot_bounds(&line, region.bounds);
                geometry.slots.push(render_slot(
                    image,
                    &group_id,
                    slot_index,
                    line,
                    bounds,
                    ReplaceRegionKind::Paragraph,
                    RenderFlow::ParagraphFlow,
                    ReplaceTextAlign::Left,
                    RenderVerticalAlign::Top,
                ));
            }
        } else {
            // 独立行/标题/菜单行保持精确行几何：没有段落语义时，逐行翻译
            // 且译文绝不流动到别的行位置，是唯一安全的默认。
            for line in visual_lines(&region_leaves) {
                let bounds = line_slot_bounds(&line, region.bounds);
                let group_id = format!("r{:04}", geometry.groups.len());
                let source_text = join_spans(&line);
                geometry.groups.push(TranslationGroup {
                    id: group_id.clone(),
                    leaf_ids: line.iter().map(|leaf| leaf.id.clone()).collect(),
                    source_text: source_text.clone(),
                    translated: source_text,
                });
                geometry.slots.push(render_slot(
                    image,
                    &group_id,
                    0,
                    line,
                    bounds,
                    if region.kind == ReplaceRegionKind::Heading {
                        ReplaceRegionKind::Heading
                    } else {
                        ReplaceRegionKind::Line
                    },
                    RenderFlow::ExactLine,
                    ReplaceTextAlign::Left,
                    RenderVerticalAlign::Top,
                ));
            }
        }
    }
    geometry
}

/// 行槽位边界：贴合该行墨迹，右缘放宽到区域右缘（译文长度可能超过原行）。
fn line_slot_bounds(line: &[OcrLeaf], region: ReplaceBounds) -> ReplaceBounds {
    let raw = spans_bounds(line);
    let x = (raw.x - 2.0).max(region.x);
    let y = (raw.y - 2.0).max(region.y);
    let right = (region.x + region.width).max(raw.x + raw.width + 2.0);
    ReplaceBounds {
        x,
        y,
        width: (right - x).max(raw.width),
        height: (raw.height + 4.0).max(1.0),
    }
}

fn visual_lines(leaves: &[OcrLeaf]) -> Vec<Vec<OcrLeaf>> {
    let mut sorted = leaves.to_vec();
    sorted.sort_by(|a, b| cmp_f32(center_y(a), center_y(b)).then(cmp_f32(a.x, b.x)));
    let median_height = median(sorted.iter().map(|leaf| leaf.height).collect()).unwrap_or(18.0);
    let mut lines: Vec<Vec<OcrLeaf>> = Vec::new();
    for leaf in sorted {
        if lines
            .last()
            .is_some_and(|line| belongs_to_visual_line(line, &leaf, median_height))
        {
            lines.last_mut().expect("visual line exists").push(leaf);
        } else {
            lines.push(vec![leaf]);
        }
    }
    for line in &mut lines {
        line.sort_by(|a, b| cmp_f32(a.x, b.x));
    }
    lines
}

#[allow(clippy::too_many_arguments)]
fn render_slot(
    image: &RgbImage,
    group_id: &str,
    slot_index: usize,
    leaves: Vec<OcrLeaf>,
    bounds: ReplaceBounds,
    kind: ReplaceRegionKind,
    flow: RenderFlow,
    align: ReplaceTextAlign,
    vertical_align: RenderVerticalAlign,
) -> RenderSlot {
    let leaf_bounds = spans_bounds(&leaves);
    // RapidOCR detection boxes are unclip-expanded well past the glyphs, so an
    // anchor/font taken from the box renders translations shifted left and a
    // size too large. Measure the tight ink extent inside each box and prefer
    // it; flat/low-contrast regions fall back to the box.
    let ink: Vec<ReplaceBounds> = leaves
        .iter()
        .filter_map(|leaf| glyph_ink_extent(image, leaf))
        .collect();
    let box_font_px =
        (median(leaves.iter().map(|leaf| leaf.height).collect()).unwrap_or(16.0) * 0.78).max(7.0);
    let source_font_px = median(ink.iter().map(|bounds| bounds.height).collect())
        .map_or(box_font_px, |ink_height| {
            ink_height.min(box_font_px).max(7.0)
        });
    let ink_x = ink
        .iter()
        .map(|bounds| bounds.x)
        .fold(f32::INFINITY, f32::min);
    let ink_y = ink
        .iter()
        .map(|bounds| bounds.y)
        .fold(f32::INFINITY, f32::min);
    let anchor_x = if ink_x.is_finite() {
        ink_x
    } else {
        leaf_bounds.x
    };
    let anchor_y = if ink_y.is_finite() {
        ink_y
    } else {
        leaf_bounds.y
    };
    RenderSlot {
        id: format!("{group_id}-s{slot_index:02}"),
        group_id: group_id.to_string(),
        leaf_ids: leaves.iter().map(|leaf| leaf.id.clone()).collect(),
        bounds,
        anchor: TextAnchor {
            x: anchor_x,
            y: anchor_y,
            baseline_y: anchor_y + source_font_px,
        },
        flow,
        kind,
        align,
        vertical_align,
        source_font_px,
        source_color: estimate_source_color(image, &leaves),
    }
}

/// OCR occasionally reads an avatar/copy icon as a compact single character.
/// Keep those original pixels when the candidate is adjacent to real text or
/// sits at the far right of a much longer text/code line.
pub fn filter_replaceable_spans(image_width: u32, spans: &[RapidOcrLine]) -> Vec<RapidOcrLine> {
    spans
        .iter()
        .filter(|candidate| {
            let text = candidate.text.trim();
            if looks_like_icon_prefixed_short_label(text) {
                return false;
            }
            let compact_single = text.chars().count() == 1
                && candidate.width <= candidate.height * 1.8
                && candidate.height <= 96.0;
            if !compact_single {
                return true;
            }
            let adjacent_to_text = spans.iter().any(|other| {
                other.id != candidate.id
                    && other.text.trim().chars().count() >= 4
                    && vertical_overlap(
                        spans_bounds(std::slice::from_ref(candidate)),
                        spans_bounds(std::slice::from_ref(other)),
                    ) >= 0.35
                    && span_horizontal_gap(candidate, other) <= (candidate.height * 1.5).max(24.0)
            });
            let far_right_copy_icon = candidate.x >= image_width as f32 * 0.65
                && spans.iter().any(|other| {
                    other.id != candidate.id
                        && other.text.trim().chars().count() >= 4
                        && other.x < candidate.x
                        && other.width >= candidate.width * 4.0
                        && vertical_overlap(
                            spans_bounds(std::slice::from_ref(candidate)),
                            spans_bounds(std::slice::from_ref(other)),
                        ) >= 0.45
                });
            !adjacent_to_text && !far_right_copy_icon
        })
        .cloned()
        .collect()
}

fn looks_like_icon_prefixed_short_label(text: &str) -> bool {
    let mut characters = text.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    let suffix: Vec<char> = characters.collect();
    !first.is_ascii()
        && !suffix.is_empty()
        && suffix.len() <= 8
        && suffix
            .iter()
            .all(|character| character.is_ascii_alphabetic())
}

pub(crate) fn detect_separators(image: &RgbImage) -> Separators {
    if image.width() < 8 || image.height() < 8 {
        return Separators::default();
    }
    let width = image.width() as usize;
    let height = image.height() as usize;
    let gray: Vec<u8> = image
        .pixels()
        .map(|pixel| {
            (pixel[0] as u32 * 299 / 1000
                + pixel[1] as u32 * 587 / 1000
                + pixel[2] as u32 * 114 / 1000) as u8
        })
        .collect();
    let mut horizontal = Vec::new();
    for y in 1..height - 1 {
        let longest = longest_run_with_gaps(
            (0..width).map(|x| {
                (gray[(y + 1) * width + x] as i16 - gray[(y - 1) * width + x] as i16).abs() >= 10
            }),
            2,
        );
        if longest >= width / 4 {
            horizontal.push(y as f32);
        }
    }
    let mut vertical = Vec::new();
    for x in 1..width - 1 {
        let longest = longest_run_with_gaps(
            (0..height).map(|y| {
                (gray[y * width + x + 1] as i16 - gray[y * width + x - 1] as i16).abs() >= 10
            }),
            2,
        );
        if longest >= height / 4 {
            vertical.push(x as f32);
        }
    }
    Separators {
        horizontal: merge_nearby_lines(horizontal),
        vertical: merge_nearby_lines(vertical),
    }
}

fn longest_run_with_gaps(values: impl Iterator<Item = bool>, max_gap: usize) -> usize {
    let mut longest = 0usize;
    let mut current = 0usize;
    let mut gap = 0usize;
    for value in values {
        if value {
            current += gap + 1;
            gap = 0;
            longest = longest.max(current);
        } else if current > 0 && gap < max_gap {
            gap += 1;
        } else {
            current = 0;
            gap = 0;
        }
    }
    longest
}

fn merge_nearby_lines(mut values: Vec<f32>) -> Vec<f32> {
    values.sort_by(|a, b| cmp_f32(*a, *b));
    let mut groups: Vec<Vec<f32>> = Vec::new();
    for value in values {
        if groups
            .last()
            .and_then(|group| group.last())
            .is_some_and(|previous| value - previous <= 4.0)
        {
            groups.last_mut().expect("group exists").push(value);
        } else {
            groups.push(vec![value]);
        }
    }
    groups
        .into_iter()
        .map(|group| group.iter().sum::<f32>() / group.len() as f32)
        .collect()
}

fn table_cell_regions(
    spans: &[RapidOcrLine],
    separators: &Separators,
    assigned: &mut [bool],
) -> Vec<RegionDraft> {
    // Two vertical edges only describe one column and are commonly produced by
    // a page border or a code block. Requiring an internal divider prevents a
    // document page from being mistaken for a one-column table.
    if separators.horizontal.len() < 2 || separators.vertical.len() < 3 {
        return Vec::new();
    }
    let mut cells: HashMap<(usize, usize), Vec<(usize, RapidOcrLine)>> = HashMap::new();
    for (index, span) in spans.iter().enumerate() {
        let center_x = span.x + span.width / 2.0;
        let center_y = span.y + span.height / 2.0;
        let Some(column) = containing_interval(&separators.vertical, center_x) else {
            continue;
        };
        let Some(row) = containing_interval(&separators.horizontal, center_y) else {
            continue;
        };
        let left = separators.vertical[column];
        let right = separators.vertical[column + 1];
        let top = separators.horizontal[row];
        let bottom = separators.horizontal[row + 1];
        let overlap_x = (right.min(span.x + span.width) - left.max(span.x)).max(0.0);
        let overlap_y = (bottom.min(span.y + span.height) - top.max(span.y)).max(0.0);
        let overlap = overlap_x * overlap_y;
        if overlap < span.width * span.height * 0.45 {
            continue;
        }
        assigned[index] = true;
        cells
            .entry((row, column))
            .or_default()
            .push((index, span.clone()));
    }

    let mut drafts = Vec::new();
    for ((row, column), entries) in cells {
        let inset = 4.0;
        drafts.push(RegionDraft {
            spans: entries.into_iter().map(|(_, span)| span).collect(),
            bounds: ReplaceBounds {
                x: separators.vertical[column] + inset,
                y: separators.horizontal[row] + inset,
                width: (separators.vertical[column + 1]
                    - separators.vertical[column]
                    - inset * 2.0)
                    .max(1.0),
                height: (separators.horizontal[row + 1] - separators.horizontal[row] - inset * 2.0)
                    .max(1.0),
            },
            kind: ReplaceRegionKind::Cell,
        });
    }
    drafts
}

fn containing_interval(lines: &[f32], value: f32) -> Option<usize> {
    lines
        .windows(2)
        .position(|pair| value > pair[0] && value < pair[1])
}

fn fallback_regions(
    image_width: u32,
    image_height: u32,
    spans: &[RapidOcrLine],
    separators: &Separators,
) -> Vec<RegionDraft> {
    if spans.is_empty() {
        return Vec::new();
    }
    let mut sorted = spans.to_vec();
    sorted.sort_by(|a, b| cmp_f32(center_y(a), center_y(b)).then(cmp_f32(a.x, b.x)));
    let median_height = median(sorted.iter().map(|span| span.height).collect()).unwrap_or(18.0);
    let mut lines: Vec<Vec<RapidOcrLine>> = Vec::new();
    for span in sorted {
        let same_line = lines
            .last()
            .is_some_and(|line| belongs_to_visual_line(line, &span, median_height));
        if same_line {
            lines.last_mut().expect("line exists").push(span);
        } else {
            lines.push(vec![span]);
        }
    }
    for line in &mut lines {
        line.sort_by(|a, b| cmp_f32(a.x, b.x));
    }

    let mut paragraphs: Vec<Vec<Vec<RapidOcrLine>>> = Vec::new();
    for line in lines {
        let force_new_block = starts_list_item(&line) || looks_like_code_line(&line);
        let target = (!force_new_block)
            .then(|| {
                paragraphs
                    .iter()
                    .enumerate()
                    .filter(|(_, paragraph)| {
                        let previous = paragraph.last().expect("paragraph has a line");
                        !looks_like_code_line(previous)
                            && lines_belong_to_paragraph(previous, &line, median_height, separators)
                    })
                    .max_by(|(_, a), (_, b)| {
                        let a_bounds = spans_bounds(a.last().expect("paragraph line"));
                        let b_bounds = spans_bounds(b.last().expect("paragraph line"));
                        let line_x = spans_bounds(&line).x;
                        cmp_f32(a_bounds.y + a_bounds.height, b_bounds.y + b_bounds.height)
                            .then_with(|| {
                                cmp_f32((b_bounds.x - line_x).abs(), (a_bounds.x - line_x).abs())
                            })
                    })
                    .map(|(index, _)| index)
            })
            .flatten();
        if let Some(index) = target {
            paragraphs[index].push(line);
        } else {
            paragraphs.push(vec![line]);
        }
    }

    let flattened: Vec<Vec<RapidOcrLine>> = paragraphs
        .into_iter()
        .map(|paragraph| paragraph.into_iter().flatten().collect())
        .collect();
    flattened
        .iter()
        .enumerate()
        .map(|(index, group)| {
            let raw = spans_bounds(group);
            let next_column = flattened
                .iter()
                .enumerate()
                .filter(|(other_index, other)| {
                    *other_index != index
                        && spans_bounds(other).x > raw.x
                        && vertical_overlap(raw, spans_bounds(other)) > 0.35
                })
                .map(|(_, other)| spans_bounds(other).x)
                .min_by(|a, b| cmp_f32(*a, *b));
            let separator_right = separators
                .vertical
                .iter()
                .copied()
                .filter(|line| *line > raw.x + raw.width)
                .min_by(|a, b| cmp_f32(*a, *b));
            let right = next_column
                .into_iter()
                .chain(separator_right)
                .min_by(|a, b| cmp_f32(*a, *b))
                .map(|value| value - 6.0)
                .unwrap_or(image_width as f32 - 6.0)
                .max(raw.x + raw.width);
            let source_font =
                median(group.iter().map(|span| span.height).collect()).unwrap_or(18.0);
            let kind = if group.len() == 1 && source_font > median_height * 1.3 {
                ReplaceRegionKind::Heading
            } else if group.len() == 1 {
                ReplaceRegionKind::Line
            } else {
                ReplaceRegionKind::Paragraph
            };
            RegionDraft {
                spans: group.clone(),
                bounds: ReplaceBounds {
                    x: (raw.x - 2.0).max(0.0),
                    y: (raw.y - 2.0).max(0.0),
                    width: (right - (raw.x - 2.0).max(0.0)).max(raw.width),
                    height: (raw.height + 4.0)
                        .min(image_height as f32 - (raw.y - 2.0).max(0.0))
                        .max(1.0),
                },
                kind,
            }
        })
        .collect()
}

/// 与 rapidocr 的 `span_belongs_to_line` 相似但阈值刻意不同：这里聚合的是排版区域，
/// 横向间隔上限收得更紧（3× 行高 vs 8×），避免同基线但相距很远的控件被合并；
/// 垂直判定也更严格（overlap ≥ 0.45 或中心差 ≤ 0.35× 行高）。不要与 OCR 行分组统一。
fn belongs_to_visual_line(line: &[RapidOcrLine], span: &RapidOcrLine, median_height: f32) -> bool {
    let bounds = spans_bounds(line);
    let overlap = (bounds.y + bounds.height).min(span.y + span.height) - bounds.y.max(span.y);
    let overlap_ratio = overlap.max(0.0) / bounds.height.min(span.height).max(1.0);
    let center_delta = ((bounds.y + bounds.height / 2.0) - center_y(span)).abs();
    let horizontal_gap = if span.x > bounds.x + bounds.width {
        span.x - (bounds.x + bounds.width)
    } else if bounds.x > span.x + span.width {
        bounds.x - (span.x + span.width)
    } else {
        0.0
    };
    (overlap_ratio >= 0.45 || center_delta <= median_height * 0.35)
        && horizontal_gap <= (median_height * 3.0).max(60.0)
}

fn lines_belong_to_paragraph(
    previous: &[RapidOcrLine],
    current: &[RapidOcrLine],
    median_height: f32,
    separators: &Separators,
) -> bool {
    let a = spans_bounds(previous);
    let b = spans_bounds(current);
    let gap = b.y - (a.y + a.height);
    if gap < -median_height * 0.2 || gap > median_height * 0.4 {
        return false;
    }
    if (a.x - b.x).abs() > median_height * 1.5 {
        return false;
    }
    if (median(previous.iter().map(|span| span.height).collect()).unwrap_or(median_height)
        - median(current.iter().map(|span| span.height).collect()).unwrap_or(median_height))
    .abs()
        > median_height * 0.35
    {
        return false;
    }
    !separators
        .horizontal
        .iter()
        .any(|line| *line > a.y + a.height && *line < b.y)
}

fn line_text(line: &[RapidOcrLine]) -> String {
    line.iter()
        .map(|span| span.text.trim())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn starts_list_item(line: &[RapidOcrLine]) -> bool {
    let text = line_text(line);
    let trimmed = text.trim_start();
    let digit_count = trimmed
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .count();
    if digit_count > 0 {
        let rest = &trimmed[digit_count..];
        if rest.starts_with(". ") || rest.starts_with(") ") {
            return true;
        }
    }
    ["- ", "• ", "· ", "* "]
        .iter()
        .any(|prefix| trimmed.starts_with(prefix))
}

fn looks_like_code_line(line: &[RapidOcrLine]) -> bool {
    let text = line_text(line);
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.chars().any(|character| character == '\n') {
        return false;
    }
    let contains_path = trimmed.contains('/') || trimmed.contains('\\');
    let contains_command_option = trimmed
        .split_whitespace()
        .skip(1)
        .any(|part| part.starts_with('-') && part.len() > 1);
    contains_path && contains_command_option
}

fn finalize_region(mut draft: RegionDraft) -> TranslationLayoutDraft {
    draft
        .spans
        .sort_by(|a, b| cmp_f32(center_y(a), center_y(b)).then(cmp_f32(a.x, b.x)));
    let source_text = join_spans(&draft.spans);
    let span_bounds = spans_bounds(&draft.spans);
    let left_space = span_bounds.x - draft.bounds.x;
    let right_space = draft.bounds.x + draft.bounds.width - (span_bounds.x + span_bounds.width);
    let inferred_align = if (left_space - right_space).abs() <= draft.bounds.width * 0.08
        && left_space > draft.bounds.width * 0.08
    {
        ReplaceTextAlign::Center
    } else if right_space < left_space * 0.35 {
        ReplaceTextAlign::Right
    } else {
        ReplaceTextAlign::Left
    };
    let align = if draft.kind == ReplaceRegionKind::Paragraph {
        ReplaceTextAlign::Left
    } else {
        inferred_align
    };
    TranslationLayoutDraft {
        source_text,
        bounds: draft.bounds,
        span_ids: draft.spans.iter().map(|span| span.id.clone()).collect(),
        kind: draft.kind,
        align,
    }
}

fn join_spans(spans: &[RapidOcrLine]) -> String {
    let mut output = String::new();
    let mut previous: Option<&RapidOcrLine> = None;
    for span in spans {
        if let Some(previous) = previous {
            let same_line = (center_y(previous) - center_y(span)).abs()
                <= previous.height.min(span.height) * 0.45;
            if same_line {
                if needs_space(&output, &span.text) {
                    output.push(' ');
                }
            } else {
                output.push('\n');
            }
        }
        output.push_str(span.text.trim());
        previous = Some(span);
    }
    output
}

fn needs_space(left: &str, right: &str) -> bool {
    let left = left.chars().last();
    let right = right.chars().find(|character| !character.is_whitespace());
    match (left, right) {
        (Some(a), Some(b)) => a.is_ascii_alphanumeric() && b.is_ascii_alphanumeric(),
        _ => false,
    }
}

fn estimate_source_color(image: &RgbImage, spans: &[RapidOcrLine]) -> String {
    let bounds = spans_bounds(spans);
    let x0 = bounds.x.floor().max(0.0) as u32;
    let y0 = bounds.y.floor().max(0.0) as u32;
    let x1 = (bounds.x + bounds.width).ceil().min(image.width() as f32) as u32;
    let y1 = (bounds.y + bounds.height).ceil().min(image.height() as f32) as u32;
    let margin = 3u32;
    let ring_x0 = x0.saturating_sub(margin);
    let ring_y0 = y0.saturating_sub(margin);
    let ring_x1 = (x1 + margin).min(image.width());
    let ring_y1 = (y1 + margin).min(image.height());
    let mut background_luma = Vec::new();
    for y in ring_y0..ring_y1 {
        for x in ring_x0..ring_x1 {
            if x < x0 || x >= x1 || y < y0 || y >= y1 {
                background_luma.push(luma(image.get_pixel(x, y).0));
            }
        }
    }
    let background = median(background_luma).unwrap_or(255.0);
    let mut candidates = Vec::new();
    for y in y0..y1 {
        for x in x0..x1 {
            let pixel = image.get_pixel(x, y).0;
            if (luma(pixel) - background).abs() >= 24.0 {
                candidates.push(pixel);
            }
        }
    }
    if candidates.is_empty() {
        return if background > 128.0 {
            "#202124"
        } else {
            "#f5f5f5"
        }
        .into();
    }
    // 只平均对比度最强的核心墨迹：反锯齿边缘像素占比很高，把它们一起平均
    // 会把浅字深底的字色拉成中灰（实测比原文明显发暗），深字浅底同理发灰。
    let max_deviation = candidates
        .iter()
        .map(|pixel| (luma(*pixel) - background).abs())
        .fold(0.0f32, f32::max);
    let core_threshold = max_deviation * 0.6;
    let mut sum = [0u64; 3];
    let mut count = 0u64;
    for pixel in &candidates {
        if (luma(*pixel) - background).abs() < core_threshold {
            continue;
        }
        for channel in 0..3 {
            sum[channel] += pixel[channel] as u64;
        }
        count += 1;
    }
    let count = count.max(1);
    format!(
        "#{:02x}{:02x}{:02x}",
        sum[0] / count,
        sum[1] / count,
        sum[2] / count
    )
}

fn luma(pixel: [u8; 3]) -> f32 {
    pixel[0] as f32 * 0.299 + pixel[1] as f32 * 0.587 + pixel[2] as f32 * 0.114
}

/// Tight bounding box of the actual glyph ink inside one OCR leaf box.
/// Background is the median luma of a thin ring around the box; ink is any
/// pixel contrasting with it. Returns `None` when the region is flat (no
/// contrast), letting callers keep the OCR box as the fallback geometry.
fn glyph_ink_extent(image: &RgbImage, leaf: &RapidOcrLine) -> Option<ReplaceBounds> {
    let x0 = leaf.x.floor().max(0.0) as u32;
    let y0 = leaf.y.floor().max(0.0) as u32;
    let x1 = ((leaf.x + leaf.width).ceil() as u32).min(image.width());
    let y1 = ((leaf.y + leaf.height).ceil() as u32).min(image.height());
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let margin = 3u32;
    let ring_x0 = x0.saturating_sub(margin);
    let ring_y0 = y0.saturating_sub(margin);
    let ring_x1 = (x1 + margin).min(image.width());
    let ring_y1 = (y1 + margin).min(image.height());
    let mut ring_luma = Vec::new();
    for y in ring_y0..ring_y1 {
        for x in ring_x0..ring_x1 {
            if x < x0 || x >= x1 || y < y0 || y >= y1 {
                ring_luma.push(luma(image.get_pixel(x, y).0));
            }
        }
    }
    let background = median(ring_luma)?;
    let mut min_x = u32::MAX;
    let mut min_y = u32::MAX;
    let mut max_x = 0u32;
    let mut max_y = 0u32;
    let mut count = 0usize;
    for y in y0..y1 {
        for x in x0..x1 {
            if (luma(image.get_pixel(x, y).0) - background).abs() >= 24.0 {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
                count += 1;
            }
        }
    }
    // Require enough ink to trust the measurement; stray antialias pixels or a
    // flat badge fill must not shrink the box to a sliver.
    if count < 12 {
        return None;
    }
    Some(ReplaceBounds {
        x: min_x as f32,
        y: min_y as f32,
        width: (max_x - min_x + 1) as f32,
        height: (max_y - min_y + 1) as f32,
    })
}

fn spans_bounds(spans: &[RapidOcrLine]) -> ReplaceBounds {
    let x = spans
        .iter()
        .map(|span| span.x)
        .fold(f32::INFINITY, f32::min);
    let y = spans
        .iter()
        .map(|span| span.y)
        .fold(f32::INFINITY, f32::min);
    let right = spans
        .iter()
        .map(|span| span.x + span.width)
        .fold(f32::NEG_INFINITY, f32::max);
    let bottom = spans
        .iter()
        .map(|span| span.y + span.height)
        .fold(f32::NEG_INFINITY, f32::max);
    ReplaceBounds {
        x,
        y,
        width: (right - x).max(1.0),
        height: (bottom - y).max(1.0),
    }
}

fn vertical_overlap(a: ReplaceBounds, b: ReplaceBounds) -> f32 {
    let overlap = (a.y + a.height).min(b.y + b.height) - a.y.max(b.y);
    overlap.max(0.0) / a.height.min(b.height).max(1.0)
}

fn span_horizontal_gap(a: &RapidOcrLine, b: &RapidOcrLine) -> f32 {
    if a.x > b.x + b.width {
        a.x - (b.x + b.width)
    } else if b.x > a.x + a.width {
        b.x - (a.x + a.width)
    } else {
        0.0
    }
}

fn center_y(span: &RapidOcrLine) -> f32 {
    span.y + span.height / 2.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rapidocr::RapidOcrPoint;

    fn span(id: &str, text: &str, x: f32, y: f32, width: f32, height: f32) -> RapidOcrLine {
        RapidOcrLine {
            id: id.into(),
            text: text.into(),
            points: vec![
                RapidOcrPoint { x, y },
                RapidOcrPoint { x: x + width, y },
                RapidOcrPoint {
                    x: x + width,
                    y: y + height,
                },
                RapidOcrPoint { x, y: y + height },
            ],
            x,
            y,
            width,
            height,
        }
    }

    fn table_image() -> RgbImage {
        let mut image = RgbImage::from_pixel(200, 120, image::Rgb([255, 255, 255]));
        for x in 10..191 {
            for y in [10, 50, 90, 110] {
                image.put_pixel(x, y, image::Rgb([180, 180, 180]));
            }
        }
        for y in 10..111 {
            for x in [10, 70, 190] {
                image.put_pixel(x, y, image::Rgb([180, 180, 180]));
            }
        }
        image
    }

    #[test]
    fn table_cells_are_hard_merge_boundaries() {
        let image = table_image();
        let spans = vec![
            span("s0", "left", 20.0, 22.0, 25.0, 12.0),
            span("s1", "right one", 85.0, 19.0, 45.0, 12.0),
            span("s2", "right two", 85.0, 34.0, 50.0, 10.0),
            span("s3", "next row", 84.0, 62.0, 52.0, 12.0),
        ];
        let regions = build_translation_layout_drafts(&image, &spans);
        assert_eq!(regions.len(), 3);
        assert!(regions
            .iter()
            .all(|region| region.kind == ReplaceRegionKind::Cell));
        let first_row_right = regions
            .iter()
            .find(|region| region.span_ids.contains(&"s1".to_string()))
            .expect("right cell");
        assert_eq!(first_row_right.span_ids, vec!["s1", "s2"]);
        assert!(first_row_right.bounds.x >= 70.0);
    }

    #[test]
    fn paragraph_lines_merge_but_columns_do_not() {
        let image = RgbImage::from_pixel(300, 160, image::Rgb([255, 255, 255]));
        let spans = vec![
            span("s0", "left first", 10.0, 10.0, 70.0, 14.0),
            span("s1", "left second", 10.0, 29.0, 80.0, 14.0),
            span("s2", "right first", 180.0, 10.0, 80.0, 14.0),
            span("s3", "right second", 180.0, 29.0, 85.0, 14.0),
        ];
        let regions = build_translation_layout_drafts(&image, &spans);
        assert_eq!(regions.len(), 2);
        assert!(regions.iter().all(|region| region.span_ids.len() == 2));
        assert!(regions[0].bounds.x < regions[1].bounds.x);
        assert!(regions[0].bounds.x + regions[0].bounds.width <= regions[1].bounds.x);
    }

    #[test]
    fn numbered_list_items_and_code_lines_form_separate_regions() {
        let image = RgbImage::from_pixel(1400, 900, image::Rgb([255, 255, 255]));
        let spans = vec![
            span(
                "s0",
                "1. Download the latest release",
                120.0,
                160.0,
                700.0,
                54.0,
            ),
            span("s1", "setup.exe.", 160.0, 215.0, 180.0, 50.0),
            span("s2", "2. Install and launch", 120.0, 270.0, 600.0, 54.0),
            span("s3", "run:", 160.0, 325.0, 80.0, 46.0),
            span(
                "s4",
                "xattr -cr /Applications/Kivio.app",
                190.0,
                397.0,
                560.0,
                52.0,
            ),
            span(
                "s5",
                "macOS will ask for permissions",
                160.0,
                508.0,
                800.0,
                49.0,
            ),
            span(
                "s6",
                "and Screen Recording permissions",
                160.0,
                556.0,
                760.0,
                49.0,
            ),
            span(
                "s7",
                "3. Follow the first-run wizard",
                120.0,
                655.0,
                760.0,
                54.0,
            ),
            span("s8", "confirm hotkeys.", 160.0, 708.0, 260.0, 50.0),
        ];
        let regions = build_translation_layout_drafts(&image, &spans);
        let groups: Vec<Vec<String>> = regions
            .iter()
            .map(|region| region.span_ids.clone())
            .collect();
        assert!(
            groups.contains(&vec!["s0".into(), "s1".into()]),
            "{groups:?}"
        );
        assert!(groups.contains(&vec!["s2".into(), "s3".into()]));
        assert!(groups.contains(&vec!["s4".into()]));
        assert!(groups.contains(&vec!["s5".into(), "s6".into()]));
        assert!(groups.contains(&vec!["s7".into(), "s8".into()]));
        assert!(regions
            .iter()
            .filter(|region| region.kind == ReplaceRegionKind::Paragraph)
            .all(|region| region.align == ReplaceTextAlign::Left));
    }

    #[test]
    fn spaced_menu_rows_split_but_wrapped_item_lines_merge() {
        let image = RgbImage::from_pixel(720, 700, image::Rgb([255, 255, 255]));
        let spans = vec![
            span("s0", "ZMGID/kivio", 129.0, 194.0, 167.0, 40.0),
            span("s1", "ZMGID/Longtxt", 129.0, 251.0, 208.0, 43.0),
            span("s2", "hekaixin66-sketch/DFOX-", 129.0, 312.0, 340.0, 37.0),
            span(
                "s3",
                "GenericAgent-product-v1.0.1",
                129.0,
                354.0,
                382.0,
                38.0,
            ),
            span("s4", "ZMGID/nabai", 129.0, 468.0, 180.0, 40.0),
        ];
        let regions = build_translation_layout_drafts(&image, &spans);
        let groups: Vec<Vec<String>> = regions
            .iter()
            .map(|region| region.span_ids.clone())
            .collect();
        assert!(groups.contains(&vec!["s0".into()]));
        assert!(groups.contains(&vec!["s1".into()]));
        assert!(groups.contains(&vec!["s2".into(), "s3".into()]));
        assert!(groups.contains(&vec!["s4".into()]));
    }

    /// 换行段落 = 一个翻译组（整段一次翻译）+ 每行一个 paragraph_flow 槽。
    /// 槽位仍锚定各自源行（行几何不动），前端把整组译文流过这些槽。
    #[test]
    fn wrapped_rows_form_one_paragraph_group_flowing_through_per_line_slots() {
        let image = RgbImage::from_pixel(720, 700, image::Rgb([255, 255, 255]));
        let spans = vec![
            span("s0", "hekaixin66-sketch/DFOX-", 129.0, 312.0, 340.0, 37.0),
            span(
                "s1",
                "GenericAgent-product-v1.0.1",
                129.0,
                354.0,
                382.0,
                38.0,
            ),
        ];
        let geometry = build_replace_geometry(&image, &spans);
        assert_eq!(geometry.groups.len(), 1);
        assert_eq!(geometry.groups[0].leaf_ids, vec!["s0", "s1"]);
        assert_eq!(geometry.slots.len(), 2);
        assert!(geometry
            .slots
            .iter()
            .all(|slot| slot.group_id == geometry.groups[0].id));
        assert_eq!(geometry.slots[0].leaf_ids, vec!["s0"]);
        assert_eq!(geometry.slots[1].leaf_ids, vec!["s1"]);
        assert_eq!(geometry.slots[0].anchor.y, 312.0);
        assert_eq!(geometry.slots[1].anchor.y, 354.0);
        assert!(geometry.slots.iter().all(|slot| {
            slot.flow == RenderFlow::ParagraphFlow
                && slot.vertical_align == RenderVerticalAlign::Top
        }));
    }

    #[test]
    fn conservative_default_never_merges_spaced_menu_rows_into_one_translation_group() {
        let image = RgbImage::from_pixel(720, 700, image::Rgb([255, 255, 255]));
        let spans = vec![
            span("s0", "ZMGID/kivio", 129.0, 194.0, 167.0, 40.0),
            span("s1", "ZMGID/Longtxt", 129.0, 251.0, 208.0, 43.0),
            span("s2", "ZMGID/nabai", 129.0, 468.0, 180.0, 40.0),
        ];
        let geometry = build_replace_geometry(&image, &spans);
        assert_eq!(geometry.groups.len(), 3);
        assert_eq!(geometry.slots.len(), 3);
        assert_eq!(
            geometry
                .slots
                .iter()
                .map(|slot| slot.anchor.y)
                .collect::<Vec<_>>(),
            vec![194.0, 251.0, 468.0]
        );
        assert!(geometry
            .slots
            .iter()
            .all(|slot| slot.flow == RenderFlow::ExactLine));
    }

    #[test]
    fn distant_controls_on_the_same_baseline_do_not_merge() {
        let image = RgbImage::from_pixel(720, 120, image::Rgb([255, 255, 255]));
        let spans = vec![
            span("s0", "Top repositories", 82.0, 34.0, 228.0, 40.0),
            span("s1", "New", 550.0, 29.0, 72.0, 49.0),
        ];
        let regions = build_translation_layout_drafts(&image, &spans);
        assert_eq!(regions.len(), 2);
    }

    #[test]
    fn compact_copy_icon_ocr_is_filtered_next_to_long_code_line() {
        let spans = vec![
            span(
                "s0",
                "xattr -cr /Applications/Kivio.app",
                190.0,
                397.0,
                560.0,
                52.0,
            ),
            span("s1", "C", 1266.0, 394.0, 47.0, 42.0),
            span("s2", "A", 100.0, 500.0, 40.0, 42.0),
        ];
        let filtered = filter_replaceable_spans(1400, &spans);
        assert_eq!(
            filtered
                .iter()
                .map(|span| span.id.as_str())
                .collect::<Vec<_>>(),
            vec!["s0", "s2"]
        );
    }

    #[test]
    fn compact_avatar_ocr_is_filtered_next_to_menu_text() {
        let spans = vec![
            span("s0", "®", 74.0, 323.0, 57.0, 57.0),
            span(
                "s1",
                "GenericAgent-product-v1.0.1",
                129.0,
                354.0,
                382.0,
                38.0,
            ),
            span("s2", "Q", 96.0, 541.0, 15.0, 10.0),
            span("s3", "NousResearch/hermes-agent", 129.0, 528.0, 388.0, 38.0),
        ];
        let filtered = filter_replaceable_spans(720, &spans);
        assert_eq!(
            filtered
                .iter()
                .map(|span| span.id.as_str())
                .collect::<Vec<_>>(),
            vec!["s1", "s3"]
        );
    }

    #[test]
    fn icon_prefixed_short_button_label_is_preserved_as_one_control() {
        let spans = vec![span("s0", "日New", 511.0, 29.0, 111.0, 49.0)];
        assert!(filter_replaceable_spans(720, &spans).is_empty());
    }

    #[test]
    fn region_ids_follow_visual_reading_order() {
        let image = RgbImage::from_pixel(200, 100, image::Rgb([255, 255, 255]));
        let spans = vec![
            span("s1", "bottom", 10.0, 60.0, 40.0, 12.0),
            span("s0", "top", 10.0, 10.0, 30.0, 12.0),
        ];
        let geometry = build_replace_geometry(&image, &spans);
        assert_eq!(geometry.groups[0].id, "r0000");
        assert!(geometry.groups[0].source_text.contains("top"));
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    #[ignore]
    async fn layout_real_fixture() {
        let model_dir = std::env::var_os("KIVIO_RAPIDOCR_MODEL_DIR")
            .map(std::path::PathBuf::from)
            .expect("set KIVIO_RAPIDOCR_MODEL_DIR");
        let image_path = std::env::var_os("KIVIO_REPLACE_LAYOUT_IMAGE")
            .map(std::path::PathBuf::from)
            .expect("set KIVIO_REPLACE_LAYOUT_IMAGE");
        let client =
            crate::rapidocr::RapidOcrClient::with_model_dir(model_dir, reqwest::Client::new());
        let spans = client
            .ocr_image_lines(&image_path, crate::rapidocr::ModelTier::High)
            .await
            .expect("run OCR");
        if std::env::var_os("KIVIO_REPLACE_DEBUG_SPANS").is_some() {
            eprintln!(
                "{}",
                serde_json::to_string_pretty(&spans).expect("serialize spans")
            );
        }
        let image = image::open(&image_path).expect("open image").to_rgb8();
        let spans = filter_replaceable_spans(image.width(), &spans);
        let geometry = build_replace_geometry(&image, &spans);
        eprintln!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "groups": &geometry.groups,
                "slots": &geometry.slots,
            }))
            .expect("serialize geometry")
        );
        assert!(!geometry.groups.is_empty());
        assert!(!geometry.slots.is_empty());
        assert!(geometry.slots.iter().all(|slot| !slot.leaf_ids.is_empty()));
    }
}
