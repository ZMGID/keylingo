//! Real-pipeline regression gate over the committed visual fixtures.
//!
//! Unlike the frontend metric unit tests (which feed hand-made numbers into the
//! metric functions), this module runs each fixture's fixed ground-truth OCR
//! leaves and its `source.png` through the REAL layout and erase pipeline
//! (`filter_replaceable_spans` + `build_replace_geometry` + `plate_fill`) and
//! compares the actual render-slot anchors against `expected_geometry.json`.
//!
//! The expected anchors are produced by the fixture generator from the exact
//! drawing coordinates (see `scripts/generate-replace-visual-fixtures.mjs`), not
//! from the pipeline, so a genuine layout regression — a whole-block upward
//! shift, a cross-line/cross-column merge, or a moved first-line anchor — makes
//! this test fail instead of silently agreeing with itself.
//!
//! Fully deterministic: no OCR model, no ONNX runtime, no network.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::layout::{build_replace_geometry, filter_replaceable_spans};
use super::mask::{blocks_from_groups, plate_fill};
use crate::rapidocr::{RapidOcrLine, RapidOcrPoint};

#[derive(Deserialize)]
struct CaseFile {
    scene: String,
    #[serde(rename = "sourceImage")]
    source_image: String,
    leaves: String,
    #[serde(rename = "expectedGeometry")]
    expected_geometry: String,
}

#[derive(Deserialize)]
struct LeavesFile {
    leaves: Vec<LeafRecord>,
}

#[derive(Deserialize)]
struct LeafRecord {
    id: String,
    text: String,
    quad: Vec<[f32; 2]>,
}

#[derive(Deserialize)]
struct GeometryFile {
    slots: Vec<ExpectedSlot>,
}

#[derive(Deserialize)]
struct ExpectedSlot {
    id: String,
    anchor: Anchor,
}

#[derive(Deserialize)]
struct Anchor {
    x: f32,
    y: f32,
}

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("tests")
        .join("fixtures")
        .join("replace-translation")
        .join("v1")
}

fn fixture_dirs(root: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(root)
        .unwrap_or_else(|error| panic!("read fixtures root {}: {error}", root.display()))
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.join("case.json").is_file())
        .collect();
    dirs.sort();
    dirs
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> T {
    let bytes =
        std::fs::read(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

fn leaf_to_line(leaf: &LeafRecord) -> RapidOcrLine {
    let min_x = leaf
        .quad
        .iter()
        .map(|point| point[0])
        .fold(f32::INFINITY, f32::min);
    let min_y = leaf
        .quad
        .iter()
        .map(|point| point[1])
        .fold(f32::INFINITY, f32::min);
    let max_x = leaf
        .quad
        .iter()
        .map(|point| point[0])
        .fold(f32::NEG_INFINITY, f32::max);
    let max_y = leaf
        .quad
        .iter()
        .map(|point| point[1])
        .fold(f32::NEG_INFINITY, f32::max);
    RapidOcrLine {
        id: leaf.id.clone(),
        text: leaf.text.clone(),
        points: leaf
            .quad
            .iter()
            .map(|point| RapidOcrPoint {
                x: point[0],
                y: point[1],
            })
            .collect(),
        x: min_x,
        y: min_y,
        width: max_x - min_x,
        height: max_y - min_y,
    }
}

/// Max acceptable distance (px) between a real render-slot anchor and the
/// ground-truth drawn anchor. Anchors should reproduce the drawn top-left
/// exactly; a couple of pixels of slack absorbs float noise while still failing
/// on any real shift. Photo text keeps a slightly wider budget.
fn anchor_tolerance(scene: &str) -> f32 {
    match scene {
        "photo" => 4.0,
        _ => 2.0,
    }
}

#[test]
fn fixtures_reproduce_ground_truth_anchors_and_ghost_free_plates() {
    let root = fixtures_root();
    let dirs = fixture_dirs(&root);
    assert!(
        !dirs.is_empty(),
        "no replace-translation fixtures found under {}",
        root.display()
    );

    for dir in dirs {
        let name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("<fixture>")
            .to_string();
        let case: CaseFile = read_json(&dir.join("case.json"));
        let leaves_file: LeavesFile = read_json(&dir.join(&case.leaves));
        let expected: GeometryFile = read_json(&dir.join(&case.expected_geometry));
        let tolerance = anchor_tolerance(&case.scene);

        let image = image::open(dir.join(&case.source_image))
            .unwrap_or_else(|error| panic!("[{name}] open source image: {error}"))
            .to_rgb8();

        let leaves: Vec<RapidOcrLine> = leaves_file.leaves.iter().map(leaf_to_line).collect();
        assert!(!leaves.is_empty(), "[{name}] fixture has no leaves");

        // Run the REAL pipeline read-only.
        let spans = filter_replaceable_spans(image.width(), &leaves);
        let geometry = build_replace_geometry(&image, &spans);
        let filled = plate_fill(&image, &blocks_from_groups(&geometry.groups, &spans));

        // Anti-collapse gate: a list/table must not degenerate into one slot,
        // and no source line may be dropped or invented.
        assert_eq!(
            geometry.slots.len(),
            expected.slots.len(),
            "[{name}] slot count changed (cross-line/cross-column merge or drop): actual {}, expected {}",
            geometry.slots.len(),
            expected.slots.len()
        );

        // Every group has exactly one slot in these fixtures; index anchors by
        // group id so the expected ground-truth ids map onto real slots.
        let mut actual_by_group: HashMap<&str, Vec<(f32, f32)>> = HashMap::new();
        for slot in &geometry.slots {
            actual_by_group
                .entry(slot.group_id.as_str())
                .or_default()
                .push((slot.anchor.x, slot.anchor.y));
        }

        // Anti-"整体上移/跨行合并" gate: each expected line keeps its own drawn
        // top-left anchor, so the first line never drifts.
        for slot in &expected.slots {
            let anchors = actual_by_group.get(slot.id.as_str()).unwrap_or_else(|| {
                panic!(
                    "[{name}] expected group {} missing from pipeline output (groups: {:?})",
                    slot.id,
                    actual_by_group.keys().collect::<Vec<_>>()
                )
            });
            assert_eq!(
                anchors.len(),
                1,
                "[{name}] group {} produced {} slots, expected 1",
                slot.id,
                anchors.len()
            );
            let (ax, ay) = anchors[0];
            let drift = ((ax - slot.anchor.x).powi(2) + (ay - slot.anchor.y).powi(2)).sqrt();
            assert!(
                drift <= tolerance,
                "[{name}] group {} anchor drift {drift:.2}px > {tolerance:.1}px (actual ({ax:.1},{ay:.1}) vs ground truth ({:.1},{:.1}))",
                slot.id,
                slot.anchor.x,
                slot.anchor.y
            );
        }

        // Erase gate: the plate must leave no readable ink inside any leaf box.
        // Compare each pixel against its own ROW median so gradient plates and
        // restored separator rows agree with themselves, while ghost glyphs
        // (which deviate from the surrounding row) get counted as residue.
        for leaf in &spans {
            let x0 = leaf.x.floor().max(0.0) as u32;
            let y0 = leaf.y.floor().max(0.0) as u32;
            let x1 = ((leaf.x + leaf.width).ceil() as u32).min(filled.width());
            let y1 = ((leaf.y + leaf.height).ceil() as u32).min(filled.height());
            assert!(
                x1 > x0 && y1 > y0,
                "[{name}] leaf {} has an empty bounding box",
                leaf.id
            );
            let mut residue = 0usize;
            let mut total = 0usize;
            for y in y0..y1 {
                let mut row: Vec<[u8; 3]> = (x0..x1).map(|x| filled.get_pixel(x, y).0).collect();
                let median = row_median(&mut row);
                for x in x0..x1 {
                    total += 1;
                    if channel_distance(filled.get_pixel(x, y).0, median) > 48.0 {
                        residue += 1;
                    }
                }
            }
            let ratio = residue as f32 / total.max(1) as f32;
            assert!(
                ratio < 0.05,
                "[{name}] leaf {} still shows ghost ink after plate fill ({:.1}% residue)",
                leaf.id,
                ratio * 100.0
            );
        }
    }
}

fn row_median(row: &mut [[u8; 3]]) -> [u8; 3] {
    let mut channels = [Vec::new(), Vec::new(), Vec::new()];
    for pixel in row.iter() {
        for channel in 0..3 {
            channels[channel].push(pixel[channel]);
        }
    }
    [0, 1, 2].map(|channel| {
        channels[channel].sort_unstable();
        channels[channel][channels[channel].len() / 2]
    })
}

fn channel_distance(a: [u8; 3], b: [u8; 3]) -> f64 {
    let dr = a[0] as f64 - b[0] as f64;
    let dg = a[1] as f64 - b[1] as f64;
    let db = a[2] as f64 - b[2] as f64;
    (dr * dr + dg * dg + db * db).sqrt()
}
