// ABOUTME: I/O helpers for run_mirroir — config + local-override loading and run-summary JSON emission.
// ABOUTME: Keeps the orchestrator file lean; these helpers touch the filesystem and the summary schema.

use std::env as std_env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{Result, RunnerError};
use crate::mirroir::discover::{LOCAL_OVERRIDES_FILE, MIRROIR_DIR};
use crate::parser::local_overrides::{apply_overrides, parse_local_overrides};
use crate::parser::mirroir::{MirroirConfig, parse_mirroir_config};

/// Per-sample outcome line in the summary JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SampleVerdict {
    /// Plan entry name.
    pub name: String,
    /// `pass` / `fail` / `skipped`.
    pub verdict: String,
    /// Optional error message when the sample failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Top-level summary written to `--report` (`mirroir-run-report.json` by default).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSummary {
    /// Schema version of this summary JSON.
    pub version: u32,
    /// Absolute path to the `.mirroir/mirroir.yaml` that was loaded.
    pub config_path: PathBuf,
    /// Generation timestamp.
    pub generated_at: DateTime<Utc>,
    /// Per-sample outcomes in plan order.
    pub samples: Vec<SampleVerdict>,
    /// Aggregate counts.
    pub totals: RunTotals,
}

/// Aggregate counts for [`RunSummary`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RunTotals {
    /// Samples discovered in the plan.
    pub samples: usize,
    /// Samples whose `run_sample` returned `Ok`.
    pub passed: usize,
    /// Samples whose `run_sample` returned `Err`.
    pub failed: usize,
    /// Samples that were skipped (`skip: true` in overrides).
    pub skipped: usize,
}

/// Read + parse the `mirroir.yaml` at `config_path`.
///
/// # Errors
///
/// * [`RunnerError::Io`] when the file can't be read.
/// * Anything [`parse_mirroir_config`] returns.
pub fn load_config(config_path: &Path) -> Result<MirroirConfig> {
    let raw = fs::read_to_string(config_path).map_err(|source| RunnerError::Io {
        context: format!("read mirroir.yaml at {}", config_path.display()),
        source,
    })?;
    parse_mirroir_config(&config_path.display().to_string(), &raw)
}

/// Derive the project root from the config path.
///
/// `config_path` is `<root>/.mirroir/mirroir.yaml`; the project root is the
/// parent's parent.
///
/// # Errors
///
/// [`RunnerError::MirroirConfigNotFound`] when the path has too few components.
pub fn project_root_for_config(config_path: &Path) -> Result<PathBuf> {
    // config_path is `<root>/.mirroir/mirroir.yaml`; project root is the parent's parent.
    config_path
        .parent() // .mirroir/
        .and_then(Path::parent) // <root>
        .map(Path::to_path_buf)
        .ok_or_else(|| RunnerError::MirroirConfigNotFound {
            searched_from: config_path.to_path_buf(),
        })
}

/// Load `mirroir.local.yaml` (if present) and apply its overrides to `config`.
///
/// # Errors
///
/// * [`RunnerError::Io`] when the overrides file can't be read.
/// * Anything [`parse_local_overrides`] / [`apply_overrides`] returns.
pub fn load_and_apply_local_overrides(
    project_root: &Path,
    config: &mut MirroirConfig,
) -> Result<()> {
    let path = project_root.join(MIRROIR_DIR).join(LOCAL_OVERRIDES_FILE);
    if !path.is_file() {
        return Ok(());
    }
    let raw = fs::read_to_string(&path).map_err(|source| RunnerError::Io {
        context: format!("read local overrides at {}", path.display()),
        source,
    })?;
    let overrides = parse_local_overrides(&path.display().to_string(), &raw)?;
    apply_overrides(config, overrides)
}

/// Resolve the user's home root from `$HOME`.
///
/// # Errors
///
/// [`RunnerError::MirroirHomeDirUnavailable`] when `$HOME` is unset.
pub fn resolve_home_root() -> Result<PathBuf> {
    std_env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or(RunnerError::MirroirHomeDirUnavailable)
}

/// Write a summary JSON treating every verdict as `passed` (compose-only mode).
///
/// # Errors
///
/// Same as [`write_summary_full`].
pub fn write_summary(path: &Path, config_path: &Path, verdicts: Vec<SampleVerdict>) -> Result<()> {
    let total = verdicts.len();
    write_summary_full(path, config_path, verdicts, total, 0, 0)
}

/// Write the full run-summary JSON with explicit pass/fail/skip counts.
///
/// # Errors
///
/// [`RunnerError::Io`] when serialization or the write fails.
pub fn write_summary_full(
    path: &Path,
    config_path: &Path,
    verdicts: Vec<SampleVerdict>,
    passed: usize,
    failed: usize,
    skipped: usize,
) -> Result<()> {
    let total = verdicts.len();
    let summary = RunSummary {
        version: 1,
        config_path: config_path.to_path_buf(),
        generated_at: Utc::now(),
        samples: verdicts,
        totals: RunTotals {
            samples: total,
            passed,
            failed,
            skipped,
        },
    };
    let json = serde_json::to_string_pretty(&summary).map_err(|source| RunnerError::Io {
        context: "serialize mirroir summary".to_owned(),
        source: io::Error::other(source.to_string()),
    })?;
    fs::write(path, json).map_err(|source| RunnerError::Io {
        context: format!("write summary at {}", path.display()),
        source,
    })
}
