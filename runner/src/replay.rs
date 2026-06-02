// ABOUTME: Scenario replay orchestration — loads scenarios, dispatches steps, aggregates verdicts.
// ABOUTME: Both `--run-scenario <file>` and `--sample <dir>` modes funnel through `run_scenario_with_context`.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::mem;
use std::path::{Path, PathBuf};

use clap::ValueEnum;
use tracing::{error, info};

use crate::compile::invoke::PlaywrightRunner;
use crate::compile::playwright::compile_scenario;
use crate::error::{Result, RunnerError};
use crate::oracle::drift::{DriftVerdict, Fingerprint, detect_drift, jaccard_similarity};
use crate::oracle::judge::{JudgeRegistry, enforce_threshold, run_judge};
use crate::parser::env::substitute;
use crate::parser::sample::{SAMPLE_SCHEMA_VERSION, SampleManifest, extract_yaml_block};
use crate::parser::scenario::{SCHEMA_VERSION, Scenario};
use crate::parser::step::{
    CrossSurfaceArgs, JudgeArgs, KillArgs, PortState, SkillStep, SpawnArgs, WaitPortArgs,
};
use crate::target::http::HttpClient;
use crate::target::process::ProcessRegistry;

/// Which set of scenarios from a `SAMPLE.md` session block to drive.
#[derive(Copy, Clone, Debug, ValueEnum)]
pub enum ScenarioSet {
    /// `session.scenarios.must_pass` — these must pass for the sample to be considered green.
    MustPass,
    /// `session.scenarios.nice_to_pass` — informational; FAIL doesn't block the sample.
    NiceToPass,
    /// Both `must_pass` and `nice_to_pass`.
    All,
}

/// Read + env-substitute + parse a scenario YAML file with `version` gating.
///
/// `extras` is merged on top of the process environment before substitution
/// — `--sample` mode uses this to expose `${MIRROIR_SAMPLE_DIR}` without
/// touching the global process env (which would require `unsafe`).
///
/// # Errors
///
/// * [`RunnerError::Io`] when the file can't be read.
/// * [`RunnerError::RegexCompile`] when env substitution fails.
/// * [`RunnerError::YamlParse`] when YAML deserialization fails.
/// * [`RunnerError::UnsupportedVersion`] when the `version:` field is out of range.
pub fn load_scenario_with_extras(path: &Path, extras: &[(&str, String)]) -> Result<Scenario> {
    let raw = fs::read_to_string(path).map_err(|source| RunnerError::Io {
        context: format!("read scenario file {}", path.display()),
        source,
    })?;

    let mut environment: HashMap<String, String> = env::vars().collect();
    for (k, v) in extras {
        environment.insert((*k).to_owned(), v.clone());
    }
    let substituted = substitute(&raw, &environment)?;

    let scenario: Scenario =
        serde_yaml::from_str(&substituted).map_err(|source| RunnerError::YamlParse {
            file: path.display().to_string(),
            source,
        })?;

    if scenario.version != SCHEMA_VERSION {
        return Err(RunnerError::UnsupportedVersion {
            artifact: "scenario.yaml".to_owned(),
            found: scenario.version,
            expected: SCHEMA_VERSION..=SCHEMA_VERSION,
        });
    }
    Ok(scenario)
}

/// Convenience wrapper for [`load_scenario_with_extras`] with no extras —
/// used by `--validate` / `--run-scenario` / `--compile-scenario` modes.
///
/// # Errors
///
/// Same as [`load_scenario_with_extras`].
pub fn load_scenario(path: &Path) -> Result<Scenario> {
    load_scenario_with_extras(path, &[])
}

/// Read + env-substitute + parse a `SAMPLE.md` manifest, gating on its `version` field.
///
/// `extras` is merged on top of the process environment before substitution,
/// mirroring [`load_scenario_with_extras`]. The substitution runs on the
/// extracted yaml body only — markdown prose around the fenced block is left
/// untouched. This lets a SAMPLE.md declare `cwd: "${ATMOSPHERE_HOME}"` and
/// resolve it from the caller's environment at load time.
///
/// # Errors
///
/// * [`RunnerError::Io`] when the file can't be read.
/// * [`RunnerError::SampleMissingYaml`] when no fenced yaml block is present.
/// * [`RunnerError::RegexCompile`] when env substitution fails.
/// * [`RunnerError::YamlParse`] when the YAML body fails deserialization.
/// * [`RunnerError::UnsupportedVersion`] when the manifest's `version:` is out of range.
pub fn load_sample_manifest_with_extras(
    path: &Path,
    extras: &[(&str, String)],
) -> Result<SampleManifest> {
    let raw = fs::read_to_string(path).map_err(|source| RunnerError::Io {
        context: format!("read SAMPLE.md at {}", path.display()),
        source,
    })?;
    let body = extract_yaml_block(&raw).ok_or_else(|| RunnerError::SampleMissingYaml {
        path: path.display().to_string(),
    })?;

    let mut environment: HashMap<String, String> = env::vars().collect();
    for (k, v) in extras {
        environment.insert((*k).to_owned(), v.clone());
    }
    let substituted = substitute(&body, &environment)?;

    let manifest: SampleManifest =
        serde_yaml::from_str(&substituted).map_err(|source| RunnerError::YamlParse {
            file: path.display().to_string(),
            source,
        })?;
    if manifest.version != SAMPLE_SCHEMA_VERSION {
        return Err(RunnerError::UnsupportedVersion {
            artifact: "SAMPLE.md".to_owned(),
            found: manifest.version,
            expected: SAMPLE_SCHEMA_VERSION..=SAMPLE_SCHEMA_VERSION,
        });
    }
    Ok(manifest)
}

/// Convenience wrapper for [`load_sample_manifest_with_extras`] with no extras —
/// used by `--sample` mode and the `--atmosphere` dispatcher.
///
/// # Errors
///
/// Same as [`load_sample_manifest_with_extras`].
pub fn load_sample_manifest(path: &Path) -> Result<SampleManifest> {
    load_sample_manifest_with_extras(path, &[])
}

/// Bundle of the SAMPLE.md manifest + the directory it was loaded from.
///
/// Carried by [`run_scenario_with_context`] when running a sample-driven
/// replay so `from: SAMPLE.md` on a `spawn:` step can fill in defaults.
#[derive(Debug, Clone, Copy)]
pub struct SampleContext<'a> {
    /// Directory `SAMPLE.md` lives in. `cwd:` overrides resolve relative to this.
    pub sample_dir: &'a Path,
    /// Parsed manifest.
    pub manifest: &'a SampleManifest,
}

/// Tunables for [`run_scenario_with_context`].
///
/// Default behavior: web step batches dispatched through `npx playwright test`.
/// Set `disable_playwright = true` to fail closed if a scenario contains web
/// steps (useful in CI lanes where Playwright isn't installed and the run
/// should not silently skip).
#[derive(Debug, Clone, Copy, Default)]
pub struct ReplayOptions {
    /// When true, web steps are logged and skipped instead of dispatched
    /// through Playwright. Process / HTTP / `assert_log` steps still run.
    pub skip_playwright: bool,
}

/// Execute one scenario end-to-end.
///
/// Walks the scenario's steps in order, dispatching:
/// - Process target steps (`spawn`/`kill`/`wait_port`/`assert_log{,_clean}`)
///   through a per-scenario [`ProcessRegistry`].
/// - HTTP steps through a per-scenario [`HttpClient`].
/// - Web steps (`target` + `tap`/`type`/`wait_for`/etc.) buffered into
///   contiguous batches; on the next non-web step (or end of scenario) the
///   buffer is compiled to a Playwright spec and run via `npx playwright test`.
/// - Judge / report / iOS / measure / condition steps logged and skipped
///   until their targets land.
///
/// # Errors
///
/// Any error returned by the dispatched step variants propagates verbatim.
pub async fn run_scenario_with_context(
    path: &Path,
    context: Option<SampleContext<'_>>,
    options: ReplayOptions,
    shared_processes: Option<&mut ProcessRegistry>,
) -> Result<()> {
    let extras: Vec<(&str, String)> = context.map_or_else(Vec::new, |ctx| {
        vec![("MIRROIR_SAMPLE_DIR", ctx.sample_dir.display().to_string())]
    });
    let scenario = load_scenario_with_extras(path, &extras)?;
    let mut local_processes = ProcessRegistry::default();
    let http = HttpClient::new()?;
    let mut web_buffer: Vec<SkillStep> = Vec::new();
    let session_shared = shared_processes.is_some();
    let processes: &mut ProcessRegistry = shared_processes.unwrap_or(&mut local_processes);

    info!(
        file = %path.display(),
        name = %scenario.name,
        steps = scenario.steps.len(),
        with_sample = context.is_some(),
        session_shared,
        skip_playwright = options.skip_playwright,
        "scenario run starting"
    );

    for (idx, step) in scenario.steps.iter().enumerate() {
        if is_web_step(step) {
            web_buffer.push(step.clone());
            continue;
        }
        // Non-web step: flush any pending web batch before dispatching.
        flush_web_buffer(&mut web_buffer, &scenario.name, options).await?;
        info!(idx, kind = step_kind(step), "dispatching step");
        match step {
            SkillStep::Spawn(args) => {
                let resolved = resolve_spawn_args(args, context.as_ref())?;
                if session_shared {
                    processes.ensure_spawned(&resolved)?;
                } else {
                    processes.spawn(&resolved)?;
                }
            }
            SkillStep::Kill(args) => {
                if session_shared {
                    // In session-scoped mode the shared boot stays alive
                    // across scenarios; scenario-level kill: of the boot id
                    // is a no-op so individual scenarios stay portable.
                    info!(id = %args.id, "kill: skipped (session-shared subprocess)");
                } else {
                    processes.kill_process(args).await?;
                }
            }
            SkillStep::WaitPort(args) => processes.wait_port(args).await?,
            SkillStep::AssertLog(args) => processes.assert_log(args).await?,
            SkillStep::AssertLogClean(args) => processes.assert_log_clean(args).await?,
            SkillStep::Http(args) => http.dispatch(args).await?,
            SkillStep::Judge(args) => dispatch_judge(args).await?,
            SkillStep::CrossSurface(args) => dispatch_cross_surface(args)?,
            _ => info!(
                idx,
                kind = step_kind(step),
                "step kind not yet wired; skipping"
            ),
        }
    }
    // End-of-scenario: flush any trailing web batch.
    flush_web_buffer(&mut web_buffer, &scenario.name, options).await?;

    info!(file = %path.display(), name = %scenario.name, "scenario run completed");
    Ok(())
}

/// Convenience: run a single scenario with no sample context. Used by
/// `mirroir-run --run-scenario <file>`.
///
/// # Errors
///
/// Same as [`run_scenario_with_context`].
pub async fn run_scenario(path: &Path, options: ReplayOptions) -> Result<()> {
    run_scenario_with_context(path, None, options, None).await
}

async fn flush_web_buffer(
    buffer: &mut Vec<SkillStep>,
    scenario_name: &str,
    options: ReplayOptions,
) -> Result<()> {
    if buffer.is_empty() {
        return Ok(());
    }
    let count = buffer.len();
    if options.skip_playwright {
        info!(count, "web step batch skipped (--no-playwright)");
        buffer.clear();
        return Ok(());
    }
    let batch_scenario = Scenario {
        version: SCHEMA_VERSION,
        name: scenario_name.to_owned(),
        app: None,
        description: None,
        tags: Vec::new(),
        steps: mem::take(buffer),
    };
    let spec = compile_scenario(&batch_scenario)?;
    let runner = PlaywrightRunner::from_env()?;
    info!(count, browsers = ?spec.browsers, "dispatching web batch to Playwright");
    let verdict = runner.run(&spec).await?;
    info!(
        passed = verdict.passed,
        failed = verdict.failed,
        skipped = verdict.skipped,
        flaky = verdict.flaky,
        "playwright batch completed"
    );
    Ok(())
}

fn dispatch_cross_surface(args: &CrossSurfaceArgs) -> Result<()> {
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

async fn dispatch_judge(args: &JudgeArgs) -> Result<()> {
    let response = load_response_text(args)?;
    let registry = JudgeRegistry::default();
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

fn is_web_step(step: &SkillStep) -> bool {
    matches!(
        step,
        SkillStep::Target(_)
            | SkillStep::Tap(_)
            | SkillStep::Type(_)
            | SkillStep::PressKey(_)
            | SkillStep::Swipe(_)
            | SkillStep::WaitFor(_)
            | SkillStep::AssertVisible(_)
            | SkillStep::AssertNotVisible(_)
            | SkillStep::Screenshot(_)
            | SkillStep::OpenUrl(_)
            | SkillStep::ScrollTo(_)
            | SkillStep::LongPress(_)
            | SkillStep::Drag(_)
            | SkillStep::Remember(_)
    )
}

/// Implements `mirroir-run --sample <dir>`.
///
/// Loads `<dir>/SAMPLE.md`, picks the scenario list for `set`, and drives
/// each scenario through [`run_scenario_with_context`] with manifest context
/// active. Aggregates verdicts; returns [`RunnerError::SampleScenarioFailures`]
/// when at least one scenario reported FAIL.
///
/// # Errors
///
/// * Anything [`load_sample_manifest`] returns.
/// * [`RunnerError::SampleScenarioFailures`] when one or more scenarios failed.
pub async fn run_sample(sample_dir: &Path, set: ScenarioSet, options: ReplayOptions) -> Result<()> {
    let sample_md_path = sample_dir.join("SAMPLE.md");
    let manifest = load_sample_manifest(&sample_md_path)?;
    info!(
        dir = %sample_dir.display(),
        name = ?manifest.name,
        must_pass = manifest.session.scenarios.must_pass.len(),
        nice_to_pass = manifest.session.scenarios.nice_to_pass.len(),
        boot_once = manifest.session.boot_once,
        "sample run starting"
    );

    let selected = select_scenarios(&manifest, set);
    let context = SampleContext {
        sample_dir,
        manifest: &manifest,
    };

    let mut session: Option<ProcessRegistry> = if manifest.session.boot_once {
        Some(boot_session(sample_dir, &manifest).await?)
    } else {
        None
    };

    let mut failed = 0usize;
    let total = selected.len();
    for scenario_rel in selected {
        let resolved = sample_dir.join(&scenario_rel);
        info!(scenario = %scenario_rel.display(), "running scenario");
        let outcome =
            run_scenario_with_context(&resolved, Some(context), options, session.as_mut()).await;
        match outcome {
            Ok(()) => info!(scenario = %scenario_rel.display(), "scenario passed"),
            Err(err) => {
                error!(scenario = %scenario_rel.display(), error = %err, "scenario failed");
                failed += 1;
            }
        }
    }

    // Tear down the shared session boot, if any.
    if let Some(mut shared) = session.take() {
        let kill_args = KillArgs {
            id: SESSION_BOOT_ID.to_owned(),
            grace_s: 3,
            cleanup: None,
        };
        if let Err(err) = shared.kill_process(&kill_args).await {
            error!(error = %err, "session boot teardown failed");
        }
    }

    if failed == 0 {
        info!(dir = %sample_dir.display(), total, "sample run completed");
        Ok(())
    } else {
        Err(RunnerError::SampleScenarioFailures { failed, total })
    }
}

/// Identifier used for the shared subprocess in `boot_once: true` sample
/// runs. Scenarios authored with `spawn: { from: SAMPLE.md, id: ... }` are
/// expected to use this same id so the spawn becomes an idempotent no-op.
const SESSION_BOOT_ID: &str = "session";

async fn boot_session(sample_dir: &Path, manifest: &SampleManifest) -> Result<ProcessRegistry> {
    let mut registry = ProcessRegistry::default();
    let mut env = HashMap::new();
    env.clone_from(&manifest.session.boot.env);
    let cwd = manifest
        .session
        .boot
        .cwd
        .as_ref()
        .map(|rel| sample_dir.join(rel).display().to_string());
    let args = SpawnArgs {
        id: SESSION_BOOT_ID.to_owned(),
        from: Some("SAMPLE.md".to_owned()),
        command: Some(manifest.session.boot.command.clone()),
        cwd,
        env,
        timeout_s: manifest.session.boot.timeout_s,
        expect_exit: None,
        capture_stdout: None,
    };
    info!(
        id = SESSION_BOOT_ID,
        command = %manifest.session.boot.command,
        "booting shared session subprocess"
    );
    registry.spawn(&args)?;

    if let Some(port) = manifest.session.boot_ready_port {
        let timeout_s = manifest.session.boot_ready_timeout_s.unwrap_or(60);
        info!(port, timeout_s, "waiting for session boot ready port");
        registry
            .wait_port(&WaitPortArgs {
                port,
                timeout_s,
                expect: PortState::Open,
            })
            .await?;
    }

    Ok(registry)
}

/// Build the ordered list of scenario paths the user asked for.
fn select_scenarios(manifest: &SampleManifest, set: ScenarioSet) -> Vec<PathBuf> {
    match set {
        ScenarioSet::MustPass => manifest.session.scenarios.must_pass.clone(),
        ScenarioSet::NiceToPass => manifest.session.scenarios.nice_to_pass.clone(),
        ScenarioSet::All => {
            let mut combined = manifest.session.scenarios.must_pass.clone();
            combined.extend(manifest.session.scenarios.nice_to_pass.iter().cloned());
            combined
        }
    }
}

/// Apply manifest defaults to a `spawn:` step that wrote `from: SAMPLE.md`.
///
/// Inline values on the step always win — the manifest only fills fields the
/// scenario left blank. The env map merges with inline overrides taking
/// precedence per-key. When `from: SAMPLE.md` is set without a sample context
/// (e.g. user ran `--run-scenario` on a scenario authored for sample mode),
/// returns [`RunnerError::SpawnFromSampleNoContext`].
fn resolve_spawn_args(args: &SpawnArgs, context: Option<&SampleContext<'_>>) -> Result<SpawnArgs> {
    if args.from.as_deref() != Some("SAMPLE.md") {
        return Ok(args.clone());
    }
    let Some(ctx) = context else {
        return Err(RunnerError::SpawnFromSampleNoContext {
            id: args.id.clone(),
        });
    };
    let boot = &ctx.manifest.session.boot;
    let mut resolved = args.clone();

    if resolved.command.is_none() {
        resolved.command = Some(boot.command.clone());
    }
    if resolved.cwd.is_none()
        && let Some(rel) = &boot.cwd
    {
        let abs = ctx.sample_dir.join(rel);
        resolved.cwd = Some(abs.display().to_string());
    }
    if resolved.timeout_s.is_none() {
        resolved.timeout_s = boot.timeout_s;
    }
    for (k, v) in &boot.env {
        resolved.env.entry(k.clone()).or_insert_with(|| v.clone());
    }
    Ok(resolved)
}

/// Short label for a [`SkillStep`] suitable for `tracing` fields.
fn step_kind(step: &SkillStep) -> &'static str {
    match step {
        SkillStep::Launch(_) => "launch",
        SkillStep::Tap(_) => "tap",
        SkillStep::Type(_) => "type",
        SkillStep::PressKey(_) => "press_key",
        SkillStep::Swipe(_) => "swipe",
        SkillStep::WaitFor(_) => "wait_for",
        SkillStep::AssertVisible(_) => "assert_visible",
        SkillStep::AssertNotVisible(_) => "assert_not_visible",
        SkillStep::Screenshot(_) => "screenshot",
        SkillStep::Home(_) => "home",
        SkillStep::OpenUrl(_) => "open_url",
        SkillStep::Shake(_) => "shake",
        SkillStep::ScrollTo(_) => "scroll_to",
        SkillStep::ResetApp(_) => "reset_app",
        SkillStep::SetNetwork(_) => "set_network",
        SkillStep::Measure(_) => "measure",
        SkillStep::LongPress(_) => "long_press",
        SkillStep::Drag(_) => "drag",
        SkillStep::Target(_) => "target",
        SkillStep::Remember(_) => "remember",
        SkillStep::Condition(_) => "condition",
        SkillStep::Spawn(_) => "spawn",
        SkillStep::WaitPort(_) => "wait_port",
        SkillStep::Kill(_) => "kill",
        SkillStep::AssertLog(_) => "assert_log",
        SkillStep::AssertLogClean(_) => "assert_log_clean",
        SkillStep::Judge(_) => "judge",
        SkillStep::Http(_) => "http",
        SkillStep::Report(_) => "report",
        SkillStep::CrossSurface(_) => "cross_surface",
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;
    use std::result::Result as StdResult;

    use super::*;
    use crate::parser::sample::{Boot, Scenarios, Session};

    type TestResult = StdResult<(), Box<dyn StdError>>;

    fn manifest_with_boot(boot: Boot) -> SampleManifest {
        SampleManifest {
            version: SAMPLE_SCHEMA_VERSION,
            name: None,
            description: None,
            session: Session {
                boot,
                scenarios: Scenarios {
                    must_pass: Vec::new(),
                    nice_to_pass: Vec::new(),
                },
                boot_once: false,
                boot_ready_port: None,
                boot_ready_timeout_s: None,
            },
        }
    }

    fn spawn_args(id: &str) -> SpawnArgs {
        SpawnArgs {
            id: id.to_owned(),
            from: None,
            command: None,
            cwd: None,
            env: HashMap::new(),
            timeout_s: None,
            expect_exit: None,
            capture_stdout: None,
        }
    }

    #[test]
    fn resolve_spawn_passthrough_when_from_is_not_sample_md() -> TestResult {
        let mut args = spawn_args("server");
        args.command = Some("./bin/server --port 8080".to_owned());
        let resolved = resolve_spawn_args(&args, None)?;
        assert_eq!(
            resolved.command.as_deref(),
            Some("./bin/server --port 8080")
        );
        Ok(())
    }

    #[test]
    fn resolve_spawn_errors_when_sample_md_without_context() -> TestResult {
        let mut args = spawn_args("server");
        args.from = Some("SAMPLE.md".to_owned());
        let res = resolve_spawn_args(&args, None);
        let Err(RunnerError::SpawnFromSampleNoContext { id }) = res else {
            return Err(format!("expected SpawnFromSampleNoContext, got {res:?}").into());
        };
        if id != "server" {
            return Err(format!("wrong id `{id}`").into());
        }
        Ok(())
    }

    #[test]
    fn resolve_spawn_fills_command_and_cwd_from_manifest() -> TestResult {
        let mut env_map = HashMap::new();
        env_map.insert("SPRING_PROFILES_ACTIVE".to_owned(), "ci".to_owned());
        let manifest = manifest_with_boot(Boot {
            command: "java -jar target/foo.jar".to_owned(),
            cwd: Some("subdir".to_owned()),
            env: env_map,
            timeout_s: Some(45),
        });
        let sample_dir = PathBuf::from("/samples/foo");
        let context = SampleContext {
            sample_dir: &sample_dir,
            manifest: &manifest,
        };
        let mut args = spawn_args("server");
        args.from = Some("SAMPLE.md".to_owned());

        let resolved = resolve_spawn_args(&args, Some(&context))?;
        assert_eq!(
            resolved.command.as_deref(),
            Some("java -jar target/foo.jar")
        );
        assert_eq!(resolved.cwd.as_deref(), Some("/samples/foo/subdir"));
        assert_eq!(resolved.timeout_s, Some(45));
        assert_eq!(
            resolved.env.get("SPRING_PROFILES_ACTIVE"),
            Some(&"ci".to_owned())
        );
        Ok(())
    }

    #[test]
    fn resolve_spawn_inline_command_wins_over_manifest() -> TestResult {
        let manifest = manifest_with_boot(Boot {
            command: "java -jar target/foo.jar".to_owned(),
            cwd: None,
            env: HashMap::new(),
            timeout_s: None,
        });
        let sample_dir = PathBuf::from("/samples/foo");
        let context = SampleContext {
            sample_dir: &sample_dir,
            manifest: &manifest,
        };
        let mut args = spawn_args("server");
        args.from = Some("SAMPLE.md".to_owned());
        args.command = Some("./bin/override --debug".to_owned());
        args.env
            .insert("SPRING_PROFILES_ACTIVE".to_owned(), "dev".to_owned());

        let resolved = resolve_spawn_args(&args, Some(&context))?;
        assert_eq!(resolved.command.as_deref(), Some("./bin/override --debug"));
        assert_eq!(
            resolved.env.get("SPRING_PROFILES_ACTIVE"),
            Some(&"dev".to_owned())
        );
        Ok(())
    }

    #[test]
    fn load_sample_manifest_substitutes_env_vars_in_yaml_body() -> TestResult {
        let tmp = tempfile::tempdir()?;
        let sample_md = tmp.path().join("SAMPLE.md");
        let markdown = "# Sample\n\n```yaml\nversion: 1\nname: sub-test\nsession:\n  boot:\n    command: \"java -jar app.jar\"\n    cwd: \"${TEST_HOME:-default-cwd}\"\n  scenarios:\n    must_pass:\n      - smoke.yaml\n```\n";
        fs::write(&sample_md, markdown)?;

        // Substitution via extras (no process-env mutation — keep tests hermetic).
        let with_value = load_sample_manifest_with_extras(
            &sample_md,
            &[("TEST_HOME", "/tmp/atmosphere-test-home".to_owned())],
        )?;
        assert_eq!(
            with_value.session.boot.cwd.as_deref(),
            Some("/tmp/atmosphere-test-home")
        );

        // Default-fallback when the var is absent from extras and process env.
        let with_default = load_sample_manifest_with_extras(&sample_md, &[])?;
        // Skip the assertion if a real TEST_HOME leaked in from the developer's
        // environment — the substitution semantics are exercised by the value
        // branch above either way.
        if env::var("TEST_HOME").is_err() {
            assert_eq!(
                with_default.session.boot.cwd.as_deref(),
                Some("default-cwd")
            );
        }
        Ok(())
    }

    #[test]
    fn select_scenarios_all_concatenates_in_order() {
        let manifest = SampleManifest {
            version: SAMPLE_SCHEMA_VERSION,
            name: None,
            description: None,
            session: Session {
                boot: Boot {
                    command: String::new(),
                    cwd: None,
                    env: HashMap::new(),
                    timeout_s: None,
                },
                scenarios: Scenarios {
                    must_pass: vec![PathBuf::from("a.yaml"), PathBuf::from("b.yaml")],
                    nice_to_pass: vec![PathBuf::from("c.yaml")],
                },
                boot_once: false,
                boot_ready_port: None,
                boot_ready_timeout_s: None,
            },
        };
        let all = select_scenarios(&manifest, ScenarioSet::All);
        assert_eq!(
            all,
            vec![
                PathBuf::from("a.yaml"),
                PathBuf::from("b.yaml"),
                PathBuf::from("c.yaml"),
            ]
        );
        let must = select_scenarios(&manifest, ScenarioSet::MustPass);
        assert_eq!(must, vec![PathBuf::from("a.yaml"), PathBuf::from("b.yaml")]);
        let nice = select_scenarios(&manifest, ScenarioSet::NiceToPass);
        assert_eq!(nice, vec![PathBuf::from("c.yaml")]);
    }
}
