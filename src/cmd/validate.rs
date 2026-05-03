//! `utm-dev validate` — golden-image validation for screenshots.
//!
//! Compares an actual screenshot against a golden template, returns a match
//! percentage, and (on mismatch) writes a red/green diff PNG showing exactly
//! which pixels drifted. Pairs with `utm-dev screenshot` for UI regression.
//!
//! Resizes `actual` to golden's dimensions via Lanczos3 before comparing, so
//! resolution differences across machines don't trip the diff. Allows ±16/255
//! drift per channel — survives anti-aliasing and font-rendering noise.
//!
//! Pattern adapted from ewe-studios/ewe_platform foundation_testbed.

use anyhow::{Context, Result, bail};
use image::{Rgba, RgbaImage};
use std::path::Path;

pub struct ValidationResult {
    pub match_pct: f64,
    pub passed: bool,
    pub diff_path: Option<String>,
}

/// Compare `actual` to `golden`, fail if match drops below `tolerance` (0–100).
/// On mismatch, write a diff PNG to `diff_output` if provided.
pub fn run(actual: &Path, golden: &Path, tolerance: f64, diff_output: Option<&Path>) -> Result<()> {
    let result = validate(actual, golden, tolerance, diff_output)?;
    println!(
        "{} match: {:.2}% (tolerance {:.1}%)",
        if result.passed { "✓" } else { "✗" },
        result.match_pct,
        tolerance,
    );
    if let Some(diff) = &result.diff_path {
        println!("  diff written: {diff}");
    }
    if !result.passed {
        bail!(
            "validation failed: {:.2}% < {:.1}%",
            result.match_pct,
            tolerance
        );
    }
    Ok(())
}

fn validate(
    actual: &Path,
    golden: &Path,
    tolerance: f64,
    diff_output: Option<&Path>,
) -> Result<ValidationResult> {
    let golden_img =
        image::open(golden).with_context(|| format!("loading golden image {golden:?}"))?;
    let actual_img =
        image::open(actual).with_context(|| format!("loading actual image {actual:?}"))?;

    let (gw, gh) = (golden_img.width(), golden_img.height());
    let actual_resized = actual_img.resize_exact(gw, gh, image::imageops::FilterType::Lanczos3);

    let golden_rgba = golden_img.to_rgba8();
    let actual_rgba = actual_resized.to_rgba8();

    let total_pixels = u64::from(gw) * u64::from(gh);
    let mut matching: u64 = 0;
    let mut diff_img = diff_output.map(|_| RgbaImage::new(gw, gh));

    for y in 0..gh {
        for x in 0..gw {
            let gp = golden_rgba.get_pixel(x, y);
            let ap = actual_rgba.get_pixel(x, y);
            let matches =
                gp.0.iter()
                    .zip(ap.0.iter())
                    .all(|(g, a)| (i16::from(*g) - i16::from(*a)).abs() < 16);

            if matches {
                matching += 1;
                if let Some(d) = diff_img.as_mut() {
                    d.put_pixel(x, y, Rgba([0, 255, 0, 255]));
                }
            } else if let Some(d) = diff_img.as_mut() {
                d.put_pixel(x, y, Rgba([255, 0, 0, 255]));
            }
        }
    }

    let match_pct = (matching as f64 / total_pixels as f64) * 100.0;
    let passed = match_pct >= tolerance;

    let diff_path = if !passed && let (Some(d), Some(out)) = (diff_img, diff_output) {
        d.save(out)
            .with_context(|| format!("saving diff image {out:?}"))?;
        Some(out.to_string_lossy().into_owned())
    } else {
        None
    };

    Ok(ValidationResult {
        match_pct,
        passed,
        diff_path,
    })
}
