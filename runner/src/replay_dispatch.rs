// ABOUTME: Judge / drift / cross_surface step dispatch helpers for scenario replay.
// ABOUTME: Resolves response sources, runs the judge registry, and enforces drift / similarity verdicts.

use std::fs;

use tempfile::{Builder, NamedTempFile};
use tracing::info;

use crate::compile::playwright::ResponseCapture;
use crate::error::{Result, RunnerError};
use crate::oracle::drift::{DriftVerdict, Fingerprint, detect_drift, jaccard_similarity};
use crate::oracle::judge::{JudgeRegistry, enforce_threshold, run_judge};
use crate::parser::step::{CrossSurfaceArgs, JudgeArgs, SkillStep};
use crate::replay::ReplayOptions;

/// Dispatch a `cross_surface:` step: read every response file and fail on the
/// first pair whose Jaccard similarity falls below the configured threshold.
///
/// # Errors
///
/// * [`RunnerError::CrossSurfaceTooFewFiles`] when fewer than two files are listed.
/// * [`RunnerError::Io`] when a response file can't be read.
/// * [`RunnerError::CrossSurfaceMismatch`] when a pair falls below threshold.
pub fn dispatch_cross_surface(args: &CrossSurfaceArgs) -> Result<()> {
    if args.response_files.len() < 2 {
        return Err(RunnerError::CrossSurfaceTooFewFiles {
            count: args.response_files.len(),
        });
    }
    let threshold = args.min_similarity.unwrap_or(0.7);
    let mut bodies: Vec<(String, String)> = Vec::with_capacity(args.response_files.len());
    for path in &args.response_files {
        let body = fs::read_to_string(path).map_err(|source| RunnerError::Io {
            context: format!("read cross_surface.response_files entry `{path}`"),
            source,
        })?;
        bodies.push((path.clone(), body));
    }

    // Compute pairwise Jaccard similarity. Fail on the first pair below threshold.
    for i in 0..bodies.len() {
        for j in (i + 1)..bodies.len() {
            let fp_a = Fingerprint::of(&bodies[i].1);
            let fp_b = Fingerprint::of(&bodies[j].1);
            let sim = jaccard_similarity(&fp_a, &fp_b);
            info!(
                a = %bodies[i].0,
                b = %bodies[j].0,
                similarity = sim,
                threshold,
                "cross_surface pairwise check"
            );
            if sim < threshold {
                return Err(RunnerError::CrossSurfaceMismatch {
                    a: bodies[i].0.clone(),
                    b: bodies[j].0.clone(),
                    observed: sim,
                    threshold,
                });
            }
        }
    }
    info!(
        files = args.response_files.len(),
        threshold, "cross_surface: all pairs above threshold"
    );
    Ok(())
}

/// If `step` is a `judge:` whose response must be scraped from the DOM (it has
/// a `response_selector` but no inline `response_text`/`response_file`) and a
/// web batch is pending to scrape from, create a temp file and a matching
/// [`ResponseCapture`]. Returns `None` (no capture) under `--no-playwright` or
/// when there is nothing to scrape. The returned [`NamedTempFile`] must outlive
/// the judge dispatch so the scraped file isn't deleted before it is read.
///
/// # Errors
///
/// * [`RunnerError::Io`] when the temp capture file can't be created.
pub fn judge_capture_file(
    step: &SkillStep,
    web_buffer: &[SkillStep],
    options: ReplayOptions,
) -> Result<Option<(NamedTempFile, ResponseCapture)>> {
    let SkillStep::Judge(args) = step else {
        return Ok(None);
    };
    if options.skip_playwright
        || web_buffer.is_empty()
        || args.response_text.is_some()
        || args.response_file.is_some()
        || args.response_selector.trim().is_empty()
    {
        return Ok(None);
    }
    let tmp = Builder::new()
        .prefix("mirroir-judge-response-")
        .suffix(".txt")
        .tempfile()
        .map_err(|source| RunnerError::Io {
            context: "create temp file for judge response capture".to_owned(),
            source,
        })?;
    let capture = ResponseCapture {
        selector: args.response_selector.clone(),
        out_path: tmp.path().display().to_string(),
    };
    Ok(Some((tmp, capture)))
}

/// Dispatch a `judge:` step: load the response, run the judge registry,
/// enforce the pass threshold, and optionally run drift detection against a
/// baseline file.
///
/// # Errors
///
/// * [`RunnerError::JudgeDecode`] when no response source is set.
/// * [`RunnerError::Io`] when a response or baseline file can't be read.
/// * Any error from judge registry loading, judging, threshold enforcement.
/// * [`RunnerError::DriftDetected`] when drift is observed against the baseline.
pub async fn dispatch_judge(args: &JudgeArgs) -> Result<()> {
    let response = load_response_text(args)?;
    let registry = JudgeRegistry::load_from_cwd()?;
    let outcome = run_judge(&registry, args, &response).await?;
    enforce_threshold(&args.profile, args, &outcome)?;
    info!(
        profile = %args.profile,
        score = outcome.score,
        pass_threshold = args.pass_threshold,
        "judge passed"
    );

    // Optional drift detection if a baseline file is provided.
    if let Some(drift_config) = &args.response_drift
        && let Some(baseline_path) = args.drift_baseline_file.as_deref()
    {
        let baseline = fs::read_to_string(baseline_path).map_err(|source| RunnerError::Io {
            context: format!("read drift baseline {baseline_path}"),
            source,
        })?;
        match detect_drift(&baseline, &response, drift_config) {
            DriftVerdict::Match {
                fingerprint_similarity,
                levenshtein_pct,
            } => info!(
                fingerprint_similarity,
                levenshtein_pct, "drift check: MATCH"
            ),
            DriftVerdict::Drift { reason, .. } => {
                return Err(RunnerError::DriftDetected { reason });
            }
        }
    }
    Ok(())
}

fn load_response_text(args: &JudgeArgs) -> Result<String> {
    if let Some(text) = &args.response_text {
        return Ok(text.clone());
    }
    if let Some(path) = &args.response_file {
        return fs::read_to_string(path).map_err(|source| RunnerError::Io {
            context: format!("read judge.response_file {path}"),
            source,
        });
    }
    Err(RunnerError::JudgeDecode {
        reason: "no response source: set response_text or response_file".to_owned(),
    })
}
