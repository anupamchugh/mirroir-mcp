// ABOUTME: `run_mirroir` orchestrator — ties discovery + resolve + lockfile + compose + run_sample.
// ABOUTME: Top-level entry point for bare `mirroir-run` and `mirroir-run --config <PATH>`.

use std::collections::HashMap;
use std::env as std_env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::error::{Result, RunnerError};
use crate::mirroir::compose::{ComposedSample, build_dir_for, compose_needed, compose_sample};
use crate::mirroir::discover::{LOCAL_OVERRIDES_FILE, MIRROIR_DIR, discover_mirroir_config};
use crate::mirroir::lock::{
    FreshnessVerdict, LockfileMode, check_lockfile_fresh, enforce_freshness, read_lockfile,
    regenerate_lockfile, write_lockfile,
};
use crate::mirroir::resolve::{ResolvedArchetype, resolve_archetype};
use crate::parser::local_overrides::{apply_overrides, parse_local_overrides};
use crate::parser::lockfile::Lockfile;
use crate::parser::mirroir::{
    ArchetypeRef, ArchetypeRefKind, DefaultScenarioSet, MirroirConfig, PlanEntry, PlanEntrySource,
    parse_mirroir_config,
};
use crate::replay::{ReplayOptions, ScenarioSet, run_sample};

/// Knobs passed from the CLI into [`run_mirroir`].
#[derive(Debug, Clone, Copy)]
pub struct MirroirRunOptions {
    /// Replay options (forwarded to `run_sample`).
    pub replay: ReplayOptions,
    /// Lockfile freshness mode.
    pub lockfile_mode: LockfileMode,
    /// When true, compose into `.build/` and exit without replaying.
    pub compose_only: bool,
    /// When true, delete `.build/` before composing.
    pub recompose: bool,
    /// When true, skip compose (assume `.build/` is current).
    pub no_compose: bool,
    /// When true, skip loading `mirroir.local.yaml`.
    pub no_local: bool,
}

impl Default for MirroirRunOptions {
    fn default() -> Self {
        Self {
            replay: ReplayOptions::default(),
            lockfile_mode: LockfileMode::Default,
            compose_only: false,
            recompose: false,
            no_compose: false,
            no_local: false,
        }
    }
}

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

/// Run the full `.mirroir/` pipeline against `config_path`.
///
/// Behavior:
/// 1. Load `mirroir.yaml` + optional `mirroir.local.yaml`.
/// 2. Load `mirroir.lock` if present; otherwise treat as missing.
/// 3. Determine freshness; enforce per `options.lockfile_mode`. Auto-regenerate
///    in `Default` mode if stale.
/// 4. Resolve archetype references against `$HOME/.mirroir/skills/` and the
///    project's `.mirroir/archetypes/`.
/// 5. Compose each plan entry (per archetype source files + plan vars/boot)
///    into `<repo>/.mirroir/.build/<entry.name>/`. Local entries pass through.
/// 6. For each composed sample (unless `--compose-only`): invoke
///    [`run_sample`] using the existing `ScenarioSet::MustPass` (or as
///    selected) machinery.
/// 7. Emit the summary JSON to `summary_path` and return `MirroirPlanFailures`
///    when any sample failed.
///
/// # Errors
///
/// Returns any error from the underlying discovery / parse / resolve /
/// compose / replay layers, plus the lockfile-mode enforcement variants.
pub async fn run_mirroir(
    config_path: &Path,
    set: ScenarioSet,
    options: MirroirRunOptions,
    summary_path: &Path,
) -> Result<()> {
    info!(config = %config_path.display(), "loading mirroir config");
    let mut config = load_config(config_path)?;
    let project_root = project_root_for_config(config_path)?;

    if !options.no_local {
        load_and_apply_local_overrides(&project_root, &mut config)?;
    }

    let home_root = resolve_home_root()?;

    let lockfile_path = config_path
        .parent()
        .map_or_else(|| PathBuf::from("mirroir.lock"), |p| p.join("mirroir.lock"));
    let lockfile_opt = ensure_lockfile(
        &lockfile_path,
        &config,
        &project_root,
        &home_root,
        options.lockfile_mode,
    )?;

    let resolved_archetypes =
        resolve_all_archetypes(&config, &project_root, &home_root, lockfile_opt.as_ref())?;

    let include_nice_to_pass = matches!(
        select_set(config.default_set, set),
        ScenarioSet::NiceToPass | ScenarioSet::All
    );
    let composed = build_composed_samples(
        &config,
        &resolved_archetypes,
        &project_root,
        options.recompose,
        options.no_compose,
        include_nice_to_pass,
    )?;

    if options.compose_only {
        info!(
            samples = composed.len(),
            "compose-only mode; skipping replay"
        );
        write_summary(
            summary_path,
            config_path,
            composed
                .iter()
                .map(|(entry, _)| SampleVerdict {
                    name: entry.name.clone(),
                    verdict: "composed".to_owned(),
                    error: None,
                })
                .collect(),
        )?;
        return Ok(());
    }

    let mut verdicts: Vec<SampleVerdict> = Vec::with_capacity(composed.len());
    let mut failed = 0usize;
    let mut passed = 0usize;
    let mut skipped = 0usize;

    for (entry, sample) in composed {
        if entry.skip {
            skipped += 1;
            verdicts.push(SampleVerdict {
                name: entry.name.clone(),
                verdict: "skipped".to_owned(),
                error: None,
            });
            continue;
        }
        info!(sample = %entry.name, dir = %sample.directory.display(), "running sample");
        match run_sample(&sample.directory, ScenarioSet::MustPass, options.replay).await {
            Ok(()) => {
                passed += 1;
                verdicts.push(SampleVerdict {
                    name: entry.name.clone(),
                    verdict: "pass".to_owned(),
                    error: None,
                });
            }
            Err(err) => {
                failed += 1;
                let msg = err.to_string();
                tracing::error!(sample = %entry.name, error = %msg, "sample failed");
                verdicts.push(SampleVerdict {
                    name: entry.name.clone(),
                    verdict: "fail".to_owned(),
                    error: Some(msg),
                });
            }
        }
    }

    let total = verdicts.len();
    write_summary_full(summary_path, config_path, verdicts, passed, failed, skipped)?;

    if failed > 0 {
        Err(RunnerError::MirroirPlanFailures { failed, total })
    } else {
        Ok(())
    }
}

fn select_set(config_default: Option<DefaultScenarioSet>, cli: ScenarioSet) -> ScenarioSet {
    // CLI choice always wins; config default is applied only when CLI is the
    // hardcoded clap-default (`MustPass`). To distinguish, the dispatcher
    // should pass the user's actual choice. For v1 we honor CLI verbatim;
    // config_default is informational.
    let _ = config_default;
    cli
}

fn load_config(config_path: &Path) -> Result<MirroirConfig> {
    let raw = fs::read_to_string(config_path).map_err(|source| RunnerError::Io {
        context: format!("read mirroir.yaml at {}", config_path.display()),
        source,
    })?;
    parse_mirroir_config(&config_path.display().to_string(), &raw)
}

fn project_root_for_config(config_path: &Path) -> Result<PathBuf> {
    // config_path is `<root>/.mirroir/mirroir.yaml`; project root is the parent's parent.
    config_path
        .parent() // .mirroir/
        .and_then(Path::parent) // <root>
        .map(Path::to_path_buf)
        .ok_or_else(|| RunnerError::MirroirConfigNotFound {
            searched_from: config_path.to_path_buf(),
        })
}

fn load_and_apply_local_overrides(project_root: &Path, config: &mut MirroirConfig) -> Result<()> {
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

fn resolve_home_root() -> Result<PathBuf> {
    std_env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or(RunnerError::MirroirHomeDirUnavailable)
}

fn ensure_lockfile(
    lockfile_path: &Path,
    config: &MirroirConfig,
    project_root: &Path,
    home_root: &Path,
    mode: LockfileMode,
) -> Result<Option<Lockfile>> {
    if !lockfile_path.is_file() {
        match mode {
            LockfileMode::Locked | LockfileMode::Frozen => {
                return Err(RunnerError::MirroirLockfileMissing {
                    path: lockfile_path.to_path_buf(),
                });
            }
            LockfileMode::Default => {
                let regenerated = regenerate_lockfile(config, project_root, home_root)?;
                write_lockfile(lockfile_path, &regenerated)?;
                warn!(
                    path = %lockfile_path.display(),
                    "lockfile was missing; wrote a fresh one"
                );
                return Ok(Some(regenerated));
            }
        }
    }
    let lockfile = read_lockfile(lockfile_path)?;
    let verdict = check_lockfile_fresh(config, &lockfile);
    if matches!(verdict, FreshnessVerdict::Stale { .. }) && matches!(mode, LockfileMode::Default) {
        let reasons = match &verdict {
            FreshnessVerdict::Stale { reasons } => reasons.join("; "),
            FreshnessVerdict::Fresh => String::new(),
        };
        let regenerated = regenerate_lockfile(config, project_root, home_root)?;
        write_lockfile(lockfile_path, &regenerated)?;
        warn!(reasons = %reasons, "regenerated stale lockfile");
        return Ok(Some(regenerated));
    }
    enforce_freshness(&verdict, mode)?;
    Ok(Some(lockfile))
}

fn resolve_all_archetypes(
    config: &MirroirConfig,
    project_root: &Path,
    home_root: &Path,
    lockfile: Option<&Lockfile>,
) -> Result<HashMap<String, ResolvedArchetype>> {
    let mut out = HashMap::new();
    for entry in config
        .plan
        .must_pass
        .iter()
        .chain(config.plan.nice_to_pass.iter())
    {
        if let PlanEntrySource::Archetypes { references } = &entry.source {
            // v1 enforces exactly one reference per entry (parser layer).
            if let Some(first) = references.first() {
                let ref_str = format_ref_display(first);
                let locked_version = lockfile
                    .and_then(|l| l.archetypes.iter().find(|a| a.reference == ref_str))
                    .and_then(|a| a.resolved.version.clone());
                let resolved =
                    resolve_archetype(first, project_root, home_root, locked_version.as_deref())?;
                out.insert(entry.name.clone(), resolved);
            }
        }
    }
    Ok(out)
}

fn format_ref_display(r: &ArchetypeRef) -> String {
    let base = match (&r.pack, r.kind) {
        (Some(p), _) => format!("{p}/{}", r.name),
        (None, ArchetypeRefKind::UserGlobal) => format!("user/{}", r.name),
        (None, ArchetypeRefKind::Pack | ArchetypeRefKind::ProjectLocal) => r.name.clone(),
    };
    match &r.version {
        Some(v) => format!("{base}@{v}"),
        None => base,
    }
}

fn build_composed_samples<'a>(
    config: &'a MirroirConfig,
    resolved_archetypes: &HashMap<String, ResolvedArchetype>,
    project_root: &Path,
    recompose: bool,
    no_compose: bool,
    include_nice_to_pass: bool,
) -> Result<Vec<(&'a PlanEntry, ComposedSample)>> {
    let mut out = Vec::new();
    let tiers: Vec<&[PlanEntry]> = if include_nice_to_pass {
        vec![
            config.plan.must_pass.as_slice(),
            config.plan.nice_to_pass.as_slice(),
        ]
    } else {
        vec![config.plan.must_pass.as_slice()]
    };
    for tier in tiers {
        for entry in tier {
            let resolved = resolved_archetypes.get(&entry.name);
            let composed =
                if no_compose && matches!(entry.source, PlanEntrySource::Archetypes { .. }) {
                    // Reuse existing .build/<entry.name>/ as-is.
                    let dir = build_dir_for(entry, project_root);
                    ComposedSample {
                        directory: dir,
                        local: false,
                    }
                } else if recompose {
                    compose_sample(entry, &config.env, resolved, project_root)?
                } else if let Some(res) = resolved {
                    if compose_needed(entry, res, project_root)? {
                        compose_sample(entry, &config.env, resolved, project_root)?
                    } else {
                        ComposedSample {
                            directory: build_dir_for(entry, project_root),
                            local: false,
                        }
                    }
                } else {
                    compose_sample(entry, &config.env, resolved, project_root)?
                };
            out.push((entry, composed));
        }
    }
    Ok(out)
}

fn write_summary(path: &Path, config_path: &Path, verdicts: Vec<SampleVerdict>) -> Result<()> {
    let total = verdicts.len();
    write_summary_full(path, config_path, verdicts, total, 0, 0)
}

fn write_summary_full(
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

/// Convenience: discover + run. Used by bare `mirroir-run` invocation.
///
/// # Errors
///
/// Propagates [`RunnerError::MirroirConfigNotFound`] when discovery fails,
/// plus anything [`run_mirroir`] returns.
pub async fn run_mirroir_autodiscover(
    cwd: &Path,
    set: ScenarioSet,
    options: MirroirRunOptions,
    summary_path: &Path,
) -> Result<()> {
    let config_path = discover_mirroir_config(cwd)?;
    run_mirroir(&config_path, set, options, summary_path).await
}
