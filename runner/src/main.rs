// ABOUTME: Entry point for the mirroir-run binary — parses CLI args and dispatches to replay::.
// ABOUTME: Returns std::process::ExitCode; never panics, never uses anyhow!() or anyhow::Result.

//! `mirroir-run` is the cross-platform replayer for mirroir `SkillStep` YAML scenarios.
//!
//! It compiles web steps to Playwright `.spec.ts`, drives generic process and HTTP
//! targets natively in Rust, and evaluates LLM judge steps as a post-hook. See the
//! parent `AGENTS.md` for the Rust discipline this workspace enforces.

use std::env as std_env;
use std::error::Error as StdError;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::process::ExitCode;
use std::result::Result as StdResult;

use clap::Parser;
use tokio::runtime::Builder as TokioRuntimeBuilder;
use tracing::{error, info};
use tracing_subscriber::{EnvFilter, fmt};

mod compile;
mod error;
mod mirroir;
mod oracle;
mod parser;
mod replay;
mod replay_dispatch;
mod replay_sample;
mod target;

use crate::compile::playwright::{compile_scenario, emit_playwright_config};
use crate::error::{Result, RunnerError};
use crate::mirroir::lock::LockfileMode;
use crate::mirroir::run::{MirroirRunOptions, run_mirroir, run_mirroir_autodiscover};
use crate::oracle::drift::{DriftVerdict, detect_drift};
use crate::parser::step::ResponseDriftConfig;
use crate::replay::{ReplayOptions, ScenarioSet, load_scenario, run_sample, run_scenario};

/// Cross-platform replayer for mirroir `SkillStep` YAML scenarios.
#[derive(Parser, Debug)]
#[command(name = "mirroir-run", version, about, long_about = None)]
struct Cli {
    /// Path to a sample directory containing a `SAMPLE.md` contract.
    ///
    /// The runner loads `<dir>/SAMPLE.md`, picks the scenario set named by
    /// `--scenarios`, and runs each scenario YAML (resolved relative to the
    /// sample dir) through the full step dispatcher. Scenarios may use
    /// `spawn: { from: SAMPLE.md, ... }` to inherit boot defaults.
    #[arg(long, conflicts_with_all = ["validate", "run_scenario"])]
    sample: Option<PathBuf>,

    /// Validate a single scenario YAML file against the `SkillStep` grammar and exit.
    ///
    /// Loads the file, applies `${VAR}` substitution from the process environment,
    /// parses as a `Scenario`, and reports the step count. Useful for CI gating of
    /// scenarios authored by the recorder.
    #[arg(long, conflicts_with_all = ["sample", "run_scenario"])]
    validate: Option<PathBuf>,

    /// Execute a single scenario YAML file end-to-end (no sample context).
    ///
    /// Process and HTTP steps are dispatched; web / iOS / judge / etc.
    /// step variants are logged and skipped (they land in subsequent commits
    /// per the build sequence in the design gist —
    /// <https://gist.github.com/jfarcand/e4cc69eeddde2ec4988aa20104566c17>).
    /// Scenarios that use `spawn: { from: SAMPLE.md }` are rejected — use
    /// `--sample` mode for those.
    #[arg(long, conflicts_with_all = ["sample", "validate", "compile_scenario"])]
    run_scenario: Option<PathBuf>,

    /// Compile a scenario's web steps into a Playwright spec + config and
    /// print the resulting TypeScript to stdout.
    ///
    /// Useful for inspecting / vetting what `npx playwright test` will run
    /// before the invocation chunk wires `--sample` end-to-end to Playwright.
    #[arg(long, conflicts_with_all = ["sample", "validate", "run_scenario"])]
    compile_scenario: Option<PathBuf>,

    /// Compute drift between two text files. Prints fingerprint similarity
    /// and normalized Levenshtein distance, with a Drift / Match verdict at
    /// the threshold given by `--levenshtein-threshold` (default 0.2).
    ///
    /// Exits 0 on Match, 1 on Drift. Takes two paths: baseline and current.
    #[arg(
        long,
        num_args = 2,
        value_names = ["BASELINE", "CURRENT"],
        conflicts_with_all = ["sample", "validate", "run_scenario", "compile_scenario"],
    )]
    diff_text: Option<Vec<PathBuf>>,

    /// Levenshtein threshold for `--diff-text` mode. Values above this are
    /// classified as Drift; values at or below are Match.
    #[arg(long, default_value_t = 0.2)]
    levenshtein_threshold: f64,

    /// Scenario set to run. When omitted, the `.mirroir/` pipeline honors the
    /// config's `default_set` (falling back to `must_pass`); the `--sample`
    /// path defaults to `must_pass`.
    #[arg(long, value_enum)]
    scenarios: Option<ScenarioSet>,

    /// Path to the JSON report artifact.
    #[arg(long, default_value = "mirroir-run-report.json")]
    report: PathBuf,

    /// Path to the mirroir-skills checkout (for scenarios/, oracles/, drift-defaults.yaml).
    #[arg(long, env = "MIRROIR_SKILLS")]
    skills: Option<PathBuf>,

    /// Skip web step batches instead of invoking `npx playwright test`.
    ///
    /// Process and HTTP steps still execute; web step batches log a
    /// "skipped" line. Useful when Playwright isn't installed locally and
    /// you want to verify the non-web portion of a scenario.
    #[arg(long)]
    no_playwright: bool,

    /// Path to a `.mirroir/mirroir.yaml` file. Overrides cwd-based discovery.
    ///
    /// When unset, a bare `mirroir-run` invocation walks `cwd ↑` looking for
    /// `<ancestor>/.mirroir/mirroir.yaml` and drives the entire `.mirroir/`
    /// pipeline from that file: parse, archetype resolution against
    /// `~/.mirroir/skills/`, lockfile freshness check, compose into
    /// `.mirroir/.build/`, then replay each sample through the existing
    /// `run_sample` machinery.
    #[arg(long, conflicts_with_all = ["sample", "validate", "run_scenario", "compile_scenario", "diff_text"])]
    config: Option<PathBuf>,

    /// Skip loading `mirroir.local.yaml` (CI-friendly).
    #[arg(long)]
    no_local: bool,

    /// Compose `.mirroir/.build/` and exit without replaying.
    #[arg(long, conflicts_with = "no_compose")]
    compose_only: bool,

    /// Delete `.mirroir/.build/` and recompose from scratch before replaying.
    #[arg(long, conflicts_with = "no_compose")]
    recompose: bool,

    /// Skip compose; reuse the existing `.mirroir/.build/` tree as-is.
    #[arg(long)]
    no_compose: bool,

    /// CI gate: error when `mirroir.lock` is missing or stale vs `mirroir.yaml`.
    #[arg(long, conflicts_with = "frozen")]
    locked: bool,

    /// Like `--locked` plus forbid any network fetch (hermetic offline mode).
    #[arg(long)]
    frozen: bool,
}

fn main() -> ExitCode {
    if let Err(init_err) = init_tracing() {
        eprintln!("mirroir-run: failed to initialize tracing: {init_err}");
        return ExitCode::FAILURE;
    }

    let cli = Cli::parse();
    info!(
        sample = ?cli.sample,
        validate = ?cli.validate,
        run_scenario = ?cli.run_scenario,
        scenarios = ?cli.scenarios,
        report = ?cli.report,
        skills = ?cli.skills,
        "mirroir-run starting"
    );

    let runtime = match TokioRuntimeBuilder::new_multi_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("mirroir-run: failed to build tokio runtime: {err}");
            return ExitCode::FAILURE;
        }
    };

    match runtime.block_on(run(&cli)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            error!(error = %e, "mirroir-run failed");
            ExitCode::FAILURE
        }
    }
}

/// Top-level fallible entry point. All module-level dispatch hangs off this function
/// so the binary's `main()` only handles the `Result` → `ExitCode` translation.
async fn run(cli: &Cli) -> Result<()> {
    if let Some(path) = &cli.validate {
        let scenario = load_scenario(path)?;
        info!(
            file = %path.display(),
            name = %scenario.name,
            steps = scenario.steps.len(),
            tags = ?scenario.tags,
            "scenario validated"
        );
        return Ok(());
    }

    let options = ReplayOptions {
        skip_playwright: cli.no_playwright,
    };

    if let Some(path) = &cli.run_scenario {
        return run_scenario(path, options).await;
    }

    if let Some(path) = &cli.compile_scenario {
        let scenario = load_scenario(path)?;
        let spec = compile_scenario(&scenario)?;
        let config = emit_playwright_config(&spec.browsers)?;
        // Stdout for piping to a file; tracing keeps the metadata separate.
        info!(
            file = %path.display(),
            name = %scenario.name,
            browsers = ?spec.browsers,
            "compiled scenario to Playwright spec"
        );
        println!("// === playwright.config.ts ===\n{config}");
        println!("// === <scenario-name>.spec.ts ===\n{}", spec.spec_ts);
        return Ok(());
    }

    if let Some(paths) = &cli.diff_text {
        return run_diff_text(paths, cli.levenshtein_threshold);
    }

    if let Some(dir) = &cli.sample {
        return run_sample(dir, cli.scenarios.unwrap_or(ScenarioSet::MustPass), options).await;
    }

    // .mirroir/ pipeline — explicit --config OR bare invocation with autodiscovery.
    let mirroir_options = MirroirRunOptions {
        replay: options,
        lockfile_mode: lockfile_mode_from_cli(cli),
        compose_only: cli.compose_only,
        recompose: cli.recompose,
        no_compose: cli.no_compose,
        no_local: cli.no_local,
    };
    if let Some(path) = &cli.config {
        return run_mirroir(path, cli.scenarios, mirroir_options, &cli.report).await;
    }

    let cwd = std_env::current_dir().map_err(|source| RunnerError::Io {
        context: "read current working directory".to_owned(),
        source,
    })?;
    run_mirroir_autodiscover(&cwd, cli.scenarios, mirroir_options, &cli.report).await
}

fn lockfile_mode_from_cli(cli: &Cli) -> LockfileMode {
    if cli.frozen {
        LockfileMode::Frozen
    } else if cli.locked {
        LockfileMode::Locked
    } else {
        LockfileMode::Default
    }
}

/// Implements `mirroir-run --diff-text <baseline> <current>`.
///
/// Reads both files, computes drift, prints a one-line verdict to stdout,
/// and returns `Err(RunnerError::DriftDetected)` when drift exceeded the
/// configured threshold (so the binary exits non-zero).
fn run_diff_text(paths: &[PathBuf], threshold: f64) -> Result<()> {
    // Clap enforces num_args = 2; defensive read.
    let [baseline_path, current_path] = match paths {
        [b, c] => [b.as_path(), c.as_path()],
        _ => {
            return Err(RunnerError::Io {
                context: "--diff-text requires exactly two paths".to_owned(),
                source: io::Error::new(io::ErrorKind::InvalidInput, "expected two paths"),
            });
        }
    };
    let baseline = fs::read_to_string(baseline_path).map_err(|source| RunnerError::Io {
        context: format!("read baseline {}", baseline_path.display()),
        source,
    })?;
    let current = fs::read_to_string(current_path).map_err(|source| RunnerError::Io {
        context: format!("read current {}", current_path.display()),
        source,
    })?;
    let config = ResponseDriftConfig {
        max_levenshtein_pct: threshold,
    };
    let verdict = detect_drift(&baseline, &current, &config);
    match verdict {
        DriftVerdict::Match {
            fingerprint_similarity,
            levenshtein_pct,
        } => {
            println!(
                "MATCH fingerprint={fingerprint_similarity:.3} levenshtein={levenshtein_pct:.3}"
            );
            Ok(())
        }
        DriftVerdict::Drift {
            fingerprint_similarity,
            levenshtein_pct,
            reason,
        } => {
            println!(
                "DRIFT fingerprint={fingerprint_similarity:.3} levenshtein={levenshtein_pct:.3}"
            );
            Err(RunnerError::DriftDetected { reason })
        }
    }
}

fn init_tracing() -> StdResult<(), Box<dyn StdError + Send + Sync>> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    fmt::fmt().with_env_filter(filter).try_init()
}
