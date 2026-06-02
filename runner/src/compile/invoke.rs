// ABOUTME: Invoke `npx playwright test` against a compiled spec, ingest JSON reporter, return verdict.
// ABOUTME: One-shot per scenario; npx must be on PATH (or supplied via PlaywrightRunner::with_npx).

use std::env;
use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::Deserialize;
use tempfile::TempDir;
use tokio::fs;
use tokio::process::Command;
use tracing::{debug, info};

use crate::compile::playwright::{PlaywrightSpec, emit_playwright_config};
use crate::error::{Result, RunnerError};

/// Per-scenario summary of Playwright test outcomes across all browser projects.
///
/// `passed + failed + skipped + flaky` equals the total result records the JSON
/// reporter wrote — one per (test × browser project × retry) tuple.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlaywrightVerdict {
    /// Number of result records with `status: "passed"`.
    pub passed: usize,
    /// Number of result records with `status: "failed"`.
    pub failed: usize,
    /// Number of result records with `status: "skipped"`.
    pub skipped: usize,
    /// Number of result records with `status: "flaky"` (passed on retry).
    pub flaky: usize,
}

/// Driver for spawning a Playwright invocation.
///
/// Constructed once per process — typically inside [`crate::replay`] when the
/// first web step is encountered. Reuses no state across calls; every
/// [`Self::run`] gets its own tempdir workspace.
pub struct PlaywrightRunner {
    npx: PathBuf,
}

impl PlaywrightRunner {
    /// Resolve `npx` from the process's `PATH` environment variable.
    ///
    /// # Errors
    ///
    /// [`RunnerError::PlaywrightNotInstalled`] when no `npx` is on `PATH`.
    pub fn from_env() -> Result<Self> {
        let path = env::var_os("PATH").unwrap_or_default();
        Self::from_path(&path)
    }

    /// Resolve `npx` from an explicit `PATH` string. Lets callers (and tests)
    /// supply a controlled search path without mutating the process env.
    ///
    /// # Errors
    ///
    /// [`RunnerError::PlaywrightNotInstalled`] when no `npx` is on `path`.
    pub fn from_path(path: &OsStr) -> Result<Self> {
        which_in(path, "npx")
            .map(|npx| Self { npx })
            .ok_or(RunnerError::PlaywrightNotInstalled)
    }

    /// Override the binary used as `npx`. Test-only — production callers go
    /// through [`Self::from_env`] / [`Self::from_path`].
    #[cfg(test)]
    pub fn with_npx(npx: PathBuf) -> Self {
        Self { npx }
    }

    /// Compile + write workspace + invoke Playwright + ingest the JSON reporter.
    ///
    /// # Errors
    ///
    /// * [`RunnerError::PlaywrightWorkspace`] on workspace setup failure.
    /// * [`RunnerError::PlaywrightInvoke`] when npx exits non-zero AND
    ///   no `playwright-report.json` was produced.
    /// * [`RunnerError::PlaywrightReport`] when the report JSON can't be parsed.
    /// * [`RunnerError::PlaywrightTestFailures`] when at least one result has
    ///   `status: "failed"`.
    pub async fn run(&self, spec: &PlaywrightSpec) -> Result<PlaywrightVerdict> {
        let workspace = build_workspace(spec).await?;
        self.spawn_npx(&workspace).await?;
        parse_report(&workspace).await
    }

    async fn spawn_npx(&self, workspace: &Workspace) -> Result<()> {
        let mut cmd = Command::new(&self.npx);
        cmd.arg("-p")
            .arg("@playwright/test")
            .arg("playwright")
            .arg("test")
            .arg("--config")
            .arg(&workspace.config_path)
            .arg(&workspace.spec_path)
            .current_dir(workspace.root())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        debug!(npx = %self.npx.display(), workspace = %workspace.root().display(), "spawning playwright");

        let output = match cmd.output().await {
            Ok(out) => out,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Err(RunnerError::PlaywrightNotInstalled);
            }
            Err(source) => {
                return Err(RunnerError::PlaywrightWorkspace {
                    context: "spawn npx playwright test".to_owned(),
                    source,
                });
            }
        };

        info!(
            status = ?output.status.code(),
            stdout_bytes = output.stdout.len(),
            stderr_bytes = output.stderr.len(),
            "playwright invocation finished"
        );

        // Defer exit-code interpretation to parse_report: when the JSON
        // reporter has structured failure detail, PlaywrightTestFailures is
        // more informative than the raw exit code. parse_report still errors
        // when the report is empty (total == 0), which catches config errors
        // and other non-test failure paths.
        if !output.status.success() && !workspace.report_path.exists() {
            return Err(RunnerError::PlaywrightInvoke {
                status: output.status.code(),
                stderr_tail: tail_stderr(&output.stderr),
            });
        }
        Ok(())
    }
}

struct Workspace {
    dir: TempDir,
    config_path: PathBuf,
    spec_path: PathBuf,
    report_path: PathBuf,
}

impl Workspace {
    fn root(&self) -> &Path {
        self.dir.path()
    }
}

async fn build_workspace(spec: &PlaywrightSpec) -> Result<Workspace> {
    let dir = TempDir::new().map_err(|source| RunnerError::PlaywrightWorkspace {
        context: "create tempdir".to_owned(),
        source,
    })?;
    let config_path = dir.path().join("playwright.config.ts");
    let spec_path = dir.path().join("scenario.spec.ts");
    let report_path = dir.path().join("playwright-report.json");

    let config = emit_playwright_config(&spec.browsers)?;
    fs::write(&config_path, config)
        .await
        .map_err(|source| RunnerError::PlaywrightWorkspace {
            context: format!("write {}", config_path.display()),
            source,
        })?;
    fs::write(&spec_path, &spec.spec_ts)
        .await
        .map_err(|source| RunnerError::PlaywrightWorkspace {
            context: format!("write {}", spec_path.display()),
            source,
        })?;
    link_playwright_home(dir.path()).await?;
    Ok(Workspace {
        dir,
        config_path,
        spec_path,
        report_path,
    })
}

/// If `MIRROIR_PLAYWRIGHT_HOME` points at a directory containing
/// `node_modules/@playwright/test`, symlink that `node_modules/` into the
/// workspace so the emitted `playwright.config.ts` can resolve its imports.
///
/// Silently no-ops when the env var is unset or the target dir is missing —
/// in those cases the runner relies on `npx` to find Playwright via its own
/// cache (works only when @playwright/test is installed globally).
async fn link_playwright_home(workspace_root: &Path) -> Result<()> {
    let Some(home) = env::var_os("MIRROIR_PLAYWRIGHT_HOME") else {
        return Ok(());
    };
    let src = PathBuf::from(home).join("node_modules");
    if !src.is_dir() {
        debug!(src = %src.display(), "MIRROIR_PLAYWRIGHT_HOME has no node_modules; skipping link");
        return Ok(());
    }
    let dst = workspace_root.join("node_modules");
    #[cfg(unix)]
    {
        fs::symlink(&src, &dst)
            .await
            .map_err(|source| RunnerError::PlaywrightWorkspace {
                context: format!("symlink {} → {}", src.display(), dst.display()),
                source,
            })?;
        debug!(src = %src.display(), dst = %dst.display(), "linked playwright home");
    }
    #[cfg(not(unix))]
    {
        // Non-unix platforms aren't a target right now; left as a no-op.
        let _ = (src, dst);
    }
    Ok(())
}

async fn parse_report(workspace: &Workspace) -> Result<PlaywrightVerdict> {
    let body = fs::read_to_string(&workspace.report_path)
        .await
        .map_err(|source| RunnerError::PlaywrightWorkspace {
            context: format!("read {}", workspace.report_path.display()),
            source,
        })?;
    let parsed: ReporterRoot =
        serde_json::from_str(&body).map_err(|source| RunnerError::PlaywrightReport {
            path: workspace.report_path.display().to_string(),
            source,
        })?;
    let mut verdict = PlaywrightVerdict::default();
    walk_suites(&parsed.suites, &mut verdict);
    let total = verdict.passed + verdict.failed + verdict.skipped + verdict.flaky;
    if verdict.failed > 0 {
        return Err(RunnerError::PlaywrightTestFailures {
            failed: verdict.failed,
            total,
        });
    }
    if total == 0 {
        return Err(RunnerError::PlaywrightTestFailures {
            failed: 0,
            total: 0,
        });
    }
    Ok(verdict)
}

fn tail_stderr(buf: &[u8]) -> String {
    const MAX_BYTES: usize = 4096;
    let slice = if buf.len() <= MAX_BYTES {
        buf
    } else {
        &buf[buf.len() - MAX_BYTES..]
    };
    String::from_utf8_lossy(slice).into_owned()
}

/// Minimal `which` — walk `path`, return the first directory containing an
/// executable of `name`. Returns `None` when no such file exists.
fn which_in(path: &OsStr, name: &str) -> Option<PathBuf> {
    for dir in env::split_paths(path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[derive(Deserialize)]
struct ReporterRoot {
    #[serde(default)]
    suites: Vec<ReporterSuite>,
}

#[derive(Deserialize)]
struct ReporterSuite {
    #[serde(default)]
    specs: Vec<ReporterSpec>,
    #[serde(default)]
    suites: Vec<Self>,
}

#[derive(Deserialize)]
struct ReporterSpec {
    #[serde(default)]
    tests: Vec<ReporterTest>,
}

#[derive(Deserialize)]
struct ReporterTest {
    #[serde(default)]
    results: Vec<ReporterResult>,
}

#[derive(Deserialize)]
struct ReporterResult {
    status: String,
}

fn walk_suites(suites: &[ReporterSuite], verdict: &mut PlaywrightVerdict) {
    for suite in suites {
        for spec in &suite.specs {
            for t in &spec.tests {
                for r in &t.results {
                    match r.status.as_str() {
                        "passed" | "expected" => verdict.passed += 1,
                        "failed" | "unexpected" => verdict.failed += 1,
                        "skipped" => verdict.skipped += 1,
                        "flaky" => verdict.flaky += 1,
                        _ => {}
                    }
                }
            }
        }
        walk_suites(&suite.suites, verdict);
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;
    use std::os::unix::fs::PermissionsExt;
    use std::result::Result as StdResult;

    use super::*;
    use crate::parser::step::Browser;

    type TestResult = StdResult<(), Box<dyn StdError>>;

    fn sample_spec() -> PlaywrightSpec {
        PlaywrightSpec {
            spec_ts: "// stub\ntest('s', async () => {});\n".to_owned(),
            browsers: vec![Browser::Chrome],
        }
    }

    /// Build a stub `npx` shell script that writes a canned JSON report to
    /// the workspace's expected path and returns the requested exit code.
    async fn make_stub_npx(
        dir: &Path,
        canned_json: &str,
        exit_code: i32,
    ) -> StdResult<PathBuf, Box<dyn StdError>> {
        let json_path = dir.join("canned-report.json");
        fs::write(&json_path, canned_json).await?;
        let script_path = dir.join("fake-npx");
        // We don't know the workspace cwd at script-write time; the script
        // shells out via $PWD which Command sets via current_dir().
        let script = format!(
            "#!/bin/sh\n\
             cp '{}' \"$PWD/playwright-report.json\"\n\
             exit {}\n",
            json_path.display(),
            exit_code
        );
        fs::write(&script_path, script).await?;
        let mut perms = fs::metadata(&script_path).await?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms).await?;
        Ok(script_path)
    }

    #[tokio::test]
    async fn run_returns_verdict_on_all_passing_report() -> TestResult {
        let scratch = TempDir::new()?;
        let canned =
            r#"{"suites":[{"specs":[{"tests":[{"results":[{"status":"passed"}]}]}],"suites":[]}]}"#;
        let stub = make_stub_npx(scratch.path(), canned, 0).await?;
        let runner = PlaywrightRunner::with_npx(stub);
        let verdict = runner.run(&sample_spec()).await?;
        assert_eq!(verdict.passed, 1);
        assert_eq!(verdict.failed, 0);
        Ok(())
    }

    #[tokio::test]
    async fn run_returns_failure_when_report_has_failed_test() -> TestResult {
        let scratch = TempDir::new()?;
        let canned =
            r#"{"suites":[{"specs":[{"tests":[{"results":[{"status":"failed"}]}]}],"suites":[]}]}"#;
        let stub = make_stub_npx(scratch.path(), canned, 1).await?;
        let runner = PlaywrightRunner::with_npx(stub);
        let res = runner.run(&sample_spec()).await;
        let Err(RunnerError::PlaywrightTestFailures { failed, total }) = res else {
            return Err(format!("expected PlaywrightTestFailures, got {res:?}").into());
        };
        if failed != 1 || total != 1 {
            return Err(format!("wrong counts: failed={failed} total={total}").into());
        }
        Ok(())
    }

    #[tokio::test]
    async fn invoke_error_when_no_report_and_nonzero_exit() -> TestResult {
        let scratch = TempDir::new()?;
        // Stub that exits non-zero and does NOT write any report.json.
        let script_path = scratch.path().join("fake-npx");
        let script = "#!/bin/sh\necho 'simulated failure' >&2\nexit 17\n";
        fs::write(&script_path, script).await?;
        let mut perms = fs::metadata(&script_path).await?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms).await?;

        let runner = PlaywrightRunner::with_npx(script_path);
        let res = runner.run(&sample_spec()).await;
        let Err(RunnerError::PlaywrightInvoke {
            status,
            stderr_tail,
            ..
        }) = res
        else {
            return Err(format!("expected PlaywrightInvoke, got {res:?}").into());
        };
        if status != Some(17) {
            return Err(format!("expected status 17, got {status:?}").into());
        }
        if !stderr_tail.contains("simulated failure") {
            return Err(format!("stderr_tail missing message: {stderr_tail}").into());
        }
        Ok(())
    }

    #[test]
    fn from_path_returns_not_installed_when_npx_absent() -> TestResult {
        // An empty path string yields no candidate directories.
        let res = PlaywrightRunner::from_path(OsStr::new(""));
        if !matches!(res, Err(RunnerError::PlaywrightNotInstalled)) {
            return Err(format!("expected PlaywrightNotInstalled, got {:?}", res.err()).into());
        }
        // A path pointing at a directory with no `npx` also yields the error.
        let empty_dir = TempDir::new()?;
        let res2 = PlaywrightRunner::from_path(empty_dir.path().as_os_str());
        if !matches!(res2, Err(RunnerError::PlaywrightNotInstalled)) {
            return Err(format!("expected PlaywrightNotInstalled, got {:?}", res2.err()).into());
        }
        Ok(())
    }

    #[tokio::test]
    async fn from_path_resolves_stub_npx_when_present() -> TestResult {
        let dir = TempDir::new()?;
        // Plant a stub binary named `npx`.
        let stub = make_stub_npx(dir.path(), "{}", 0).await?;
        let renamed = dir.path().join("npx");
        fs::rename(&stub, &renamed).await?;
        // If from_path errored we'd never get here; that itself is the assertion.
        let _runner = PlaywrightRunner::from_path(dir.path().as_os_str())?;
        Ok(())
    }

    #[test]
    fn walk_suites_counts_nested_suites() -> TestResult {
        let json = r#"{
            "suites": [
              {
                "specs": [{"tests": [{"results": [{"status": "passed"}]}]}],
                "suites": [
                  {"specs": [{"tests": [{"results": [{"status": "failed"}, {"status": "flaky"}]}]}], "suites": []}
                ]
              }
            ]
        }"#;
        let root: ReporterRoot = serde_json::from_str(json)?;
        let mut v = PlaywrightVerdict::default();
        walk_suites(&root.suites, &mut v);
        assert_eq!(v.passed, 1);
        assert_eq!(v.failed, 1);
        assert_eq!(v.flaky, 1);
        Ok(())
    }
}
