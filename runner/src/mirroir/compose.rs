// ABOUTME: Compose archetype + plan entry → ready-to-replay `.mirroir/.build/<sample>/` tree.
// ABOUTME: Post-parse ${VAR} substitution on every source file; content-addressed cache (sha256 + mtime fast-path).

use std::collections::HashMap;
use std::env as std_env;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{Result, RunnerError};
use crate::mirroir::resolve::{ArchetypeOrigin, ResolvedArchetype};
use crate::parser::archetype::ArchetypeRequiredVar;
use crate::parser::env::substitute as env_substitute;
use crate::parser::mirroir::{PlanEntry, PlanEntrySource};
use crate::parser::substitute::substitute_value;

/// Directory name (under `.mirroir/`) where composed artifacts land.
pub const BUILD_DIR: &str = ".build";

/// Filename of the per-sample compose-provenance manifest.
const COMPOSE_MANIFEST_FILE: &str = ".compose-manifest.json";

/// Where the composed `<sample>/` tree for `entry` lives.
#[must_use]
pub fn build_dir_for(entry: &PlanEntry, project_root: &Path) -> PathBuf {
    project_root
        .join(".mirroir")
        .join(BUILD_DIR)
        .join(&entry.name)
}

/// Top-level outcome of [`compose_sample`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposedSample {
    /// Directory mirroir-run consumes via `run_sample`. For local: entries this
    /// is the source path (no build/ copy); for archetype-extending entries
    /// it's `<project>/.mirroir/.build/<entry.name>/`.
    pub directory: PathBuf,
    /// True when the sample was authored fully under `.mirroir/apps/<...>` and
    /// compose was a no-op. Useful for diagnostics + telemetry.
    pub local: bool,
}

/// Determine whether the cached `.build/<entry.name>/` is stale relative to
/// its archetype source + plan-entry inputs.
///
/// Fast-path: when the compose manifest's `composed_at` is newer than every
/// archetype source file's `mtime`, the cache is trusted as-is.
/// Slow-path: any source file with a newer mtime triggers a sha256 check
/// against the recorded sums.
///
/// Returns `true` when compose must run, `false` when cache is valid.
///
/// # Errors
///
/// Returns [`RunnerError::Io`] when manifest read fails for an existing
/// `.build/` directory.
pub fn compose_needed(
    entry: &PlanEntry,
    resolved: &ResolvedArchetype,
    project_root: &Path,
) -> Result<bool> {
    let build = build_dir_for(entry, project_root);
    if !build.join(COMPOSE_MANIFEST_FILE).exists() {
        return Ok(true);
    }

    let manifest_path = build.join(COMPOSE_MANIFEST_FILE);
    let raw = fs::read_to_string(&manifest_path).map_err(|source| RunnerError::Io {
        context: format!("read compose manifest {}", manifest_path.display()),
        source,
    })?;
    let Ok(manifest) = serde_json::from_str::<ComposeManifest>(&raw) else {
        // Corrupt manifest → recompose.
        return Ok(true);
    };

    if manifest.instance.plan_entry_sha256 != hash_plan_entry(entry) {
        return Ok(true);
    }

    // Fast-path: compare each recorded source file's mtime to composed_at.
    let composed_at_sys: SystemTime = manifest.composed_at.into();
    let mut needs_sha = false;
    for record in &manifest.archetype.source_files {
        let abs = resolved.directory.join(&record.path);
        let Ok(meta) = fs::metadata(&abs) else {
            // Source file removed → recompose.
            return Ok(true);
        };
        let Ok(mtime) = meta.modified() else {
            // Filesystem without mtime → fall back to slow path.
            needs_sha = true;
            continue;
        };
        if mtime > composed_at_sys {
            needs_sha = true;
        }
    }
    if !needs_sha {
        return Ok(false);
    }

    // Slow-path: sha256 every recorded source file.
    for record in &manifest.archetype.source_files {
        let abs = resolved.directory.join(&record.path);
        let Ok(bytes) = fs::read(&abs) else {
            return Ok(true);
        };
        if sha256_hex(&bytes) != record.sha256 {
            return Ok(true);
        }
    }

    Ok(false)
}

/// Compose `entry` into `<repo>/.mirroir/.build/<entry.name>/` from `resolved`.
///
/// For `PlanEntrySource::Local` entries, compose is a passthrough — the
/// returned [`ComposedSample::directory`] points at the source `.mirroir/<rel>/`
/// directory and no `.build/` tree is created.
///
/// For `PlanEntrySource::Archetypes` entries, compose:
/// 1. Substitutes `${VAR}` placeholders post-parse on every archetype source file.
/// 2. Writes `APP.md`, `SKILL.md`, `scenarios/<flow>.yaml` for each selected flow.
/// 3. Synthesizes a `SAMPLE.md` from the plan entry's boot + flow list.
/// 4. Records source-file sha256s + plan-entry hash in `.compose-manifest.json`.
///
/// # Errors
///
/// * [`RunnerError::Io`] for any read/write/mkdir failure.
/// * [`RunnerError::MirroirSampleMissing`] when a Local entry's path doesn't exist.
/// * [`RunnerError::MirroirComposeFailed`] for malformed archetype-side files.
pub fn compose_sample(
    entry: &PlanEntry,
    suite_env: &HashMap<String, String>,
    resolved: Option<&ResolvedArchetype>,
    project_root: &Path,
) -> Result<ComposedSample> {
    match &entry.source {
        PlanEntrySource::Local { path } => {
            let abs = project_root.join(".mirroir").join(path);
            if !abs.is_dir() {
                return Err(RunnerError::MirroirSampleMissing {
                    sample: entry.name.clone(),
                    expected_path: abs,
                });
            }
            Ok(ComposedSample {
                directory: abs,
                local: true,
            })
        }
        PlanEntrySource::Archetypes { .. } => {
            let resolved = resolved.ok_or_else(|| RunnerError::MirroirComposeFailed {
                sample: entry.name.clone(),
                context: "archetype entry composed without a resolved archetype".to_owned(),
                source: io::Error::other("missing resolved archetype"),
            })?;
            compose_archetype_entry(entry, suite_env, resolved, project_root)
        }
    }
}

fn compose_archetype_entry(
    entry: &PlanEntry,
    suite_env: &HashMap<String, String>,
    resolved: &ResolvedArchetype,
    project_root: &Path,
) -> Result<ComposedSample> {
    let build = build_dir_for(entry, project_root);
    if build.exists() {
        fs::remove_dir_all(&build).map_err(|source| RunnerError::Io {
            context: format!("clear stale build dir {}", build.display()),
            source,
        })?;
    }
    fs::create_dir_all(&build).map_err(|source| RunnerError::Io {
        context: format!("mkdir build dir {}", build.display()),
        source,
    })?;
    fs::create_dir_all(build.join("scenarios")).map_err(|source| RunnerError::Io {
        context: format!("mkdir scenarios dir {}", build.display()),
        source,
    })?;

    let env = build_substitution_env(entry, suite_env, &resolved.manifest.requires.vars);
    let mut source_records = Vec::new();

    // Copy + substitute APP.md and SKILL.md if present in the archetype.
    for fname in ["APP.md", "SKILL.md"] {
        let src = resolved.directory.join(fname);
        if src.is_file() {
            let bytes = fs::read(&src).map_err(|source| RunnerError::Io {
                context: format!("read archetype file {}", src.display()),
                source,
            })?;
            source_records.push(SourceFileRecord {
                path: PathBuf::from(fname),
                sha256: sha256_hex(&bytes),
            });
            let raw = String::from_utf8_lossy(&bytes).into_owned();
            let substituted = substitute_markdown_with_yaml(&raw, &env)?;
            fs::write(build.join(fname), substituted).map_err(|source| RunnerError::Io {
                context: format!("write composed {fname}"),
                source,
            })?;
        }
    }

    // Always include archetype.md as a provenance copy in source_records (not
    // emitted to .build/ — the runner doesn't read it).
    let archetype_md = resolved.directory.join("archetype.md");
    if archetype_md.is_file() {
        let bytes = fs::read(&archetype_md).map_err(|source| RunnerError::Io {
            context: format!("read archetype manifest {}", archetype_md.display()),
            source,
        })?;
        source_records.push(SourceFileRecord {
            path: PathBuf::from("archetype.md"),
            sha256: sha256_hex(&bytes),
        });
    }

    // For each selected flow, substitute its scenario YAML and write under build/scenarios/.
    for flow in &entry.flows {
        let src = resolved
            .directory
            .join("scenarios")
            .join(format!("{flow}.yaml"));
        if !src.is_file() {
            return Err(RunnerError::MirroirComposeFailed {
                sample: entry.name.clone(),
                context: format!(
                    "flow `{flow}` is not provided by archetype `{}` (file {} missing)",
                    resolved.manifest.name,
                    src.display(),
                ),
                source: io::Error::other("flow scenario file not found"),
            });
        }
        let bytes = fs::read(&src).map_err(|source| RunnerError::Io {
            context: format!("read flow scenario {}", src.display()),
            source,
        })?;
        source_records.push(SourceFileRecord {
            path: PathBuf::from("scenarios").join(format!("{flow}.yaml")),
            sha256: sha256_hex(&bytes),
        });
        let substituted = substitute_yaml_text(&String::from_utf8_lossy(&bytes), &env)?;
        let out = build.join("scenarios").join(format!("{flow}.yaml"));
        fs::write(&out, substituted).map_err(|source| RunnerError::Io {
            context: format!("write composed scenario {}", out.display()),
            source,
        })?;
    }

    // Synthesize SAMPLE.md from the plan entry's boot + flow list.
    let sample_md = synthesize_sample_md(entry, &env);
    fs::write(build.join("SAMPLE.md"), sample_md).map_err(|source| RunnerError::Io {
        context: "write composed SAMPLE.md".to_owned(),
        source,
    })?;

    // Write the compose manifest.
    let manifest = ComposeManifest {
        version: 1,
        composed_at: Utc::now(),
        archetype: ComposeManifestArchetype {
            reference: format!("{:?}", resolved.origin),
            resolved_version: archetype_version(resolved),
            source_files: source_records,
        },
        instance: ComposeManifestInstance {
            name: entry.name.clone(),
            plan_entry_sha256: hash_plan_entry(entry),
        },
    };
    let manifest_json =
        serde_json::to_string_pretty(&manifest).map_err(|source| RunnerError::Io {
            context: "serialize compose manifest".to_owned(),
            source: io::Error::other(source.to_string()),
        })?;
    fs::write(build.join(COMPOSE_MANIFEST_FILE), manifest_json).map_err(|source| {
        RunnerError::Io {
            context: "write compose manifest".to_owned(),
            source,
        }
    })?;

    Ok(ComposedSample {
        directory: build,
        local: false,
    })
}

fn archetype_version(resolved: &ResolvedArchetype) -> Option<String> {
    match &resolved.origin {
        ArchetypeOrigin::Pack { version, .. } | ArchetypeOrigin::UserGlobal { version, .. } => {
            Some(version.clone())
        }
        ArchetypeOrigin::ProjectLocal { .. } => None,
    }
}

fn build_substitution_env(
    entry: &PlanEntry,
    suite_env: &HashMap<String, String>,
    archetype_required_vars: &[ArchetypeRequiredVar],
) -> HashMap<String, String> {
    let mut env: HashMap<String, String> = std_env::vars().collect();
    // Archetype defaults first (lowest priority), then suite env, then entry.boot.env,
    // then entry.vars (highest priority — instance wins).
    for spec in archetype_required_vars {
        if let Some(default) = &spec.default {
            env.insert(spec.name.clone(), default.clone());
        }
    }
    for (k, v) in suite_env {
        env.insert(k.clone(), v.clone());
    }
    for (k, v) in &entry.boot.env {
        env.insert(k.clone(), v.clone());
    }
    for (k, v) in &entry.vars {
        env.insert(k.clone(), v.clone());
    }
    env
}

/// Substitute a markdown file's fenced yaml block while preserving the surrounding prose.
fn substitute_markdown_with_yaml(raw: &str, env: &HashMap<String, String>) -> Result<String> {
    let Some((start_idx, end_idx, body)) = locate_fenced_yaml(raw) else {
        // No yaml block — substitute nothing; return the markdown verbatim.
        return Ok(raw.to_owned());
    };
    let substituted_body = substitute_yaml_text(body, env)?;
    let mut out = String::with_capacity(raw.len() + substituted_body.len());
    out.push_str(&raw[..start_idx]);
    out.push_str(&substituted_body);
    out.push_str(&raw[end_idx..]);
    Ok(out)
}

/// Substitute a plain yaml string: parse → walk → re-serialize.
fn substitute_yaml_text(yaml: &str, env: &HashMap<String, String>) -> Result<String> {
    let mut value: serde_yaml::Value =
        serde_yaml::from_str(yaml).map_err(|source| RunnerError::YamlParse {
            file: "(compose-substitute)".to_owned(),
            source,
        })?;
    substitute_value(&mut value, env)?;
    serde_yaml::to_string(&value).map_err(|source| RunnerError::YamlParse {
        file: "(compose-substitute serialize)".to_owned(),
        source,
    })
}

/// Find the inclusive byte range of the body of the first fenced `yaml` block.
fn locate_fenced_yaml(markdown: &str) -> Option<(usize, usize, &str)> {
    let bytes = markdown.as_bytes();
    let needle = b"```yaml\n";
    let yml_needle = b"```yml\n";
    let mut start_fence_idx = None;
    let mut i = 0;
    while i + needle.len() <= bytes.len() {
        if bytes[i..i + needle.len()] == *needle {
            start_fence_idx = Some(i + needle.len());
            break;
        }
        if i + yml_needle.len() <= bytes.len() && bytes[i..i + yml_needle.len()] == *yml_needle {
            start_fence_idx = Some(i + yml_needle.len());
            break;
        }
        i += 1;
    }
    let start = start_fence_idx?;
    // Closing fence: a line that starts with ```.
    let mut j = start;
    while j < bytes.len() {
        if bytes[j] == b'`' && j + 2 < bytes.len() && &bytes[j..j + 3] == b"```" {
            // Walk back to start of line.
            let mut line_start = j;
            while line_start > 0 && bytes[line_start - 1] != b'\n' {
                line_start -= 1;
            }
            if line_start == j || markdown[line_start..j].chars().all(char::is_whitespace) {
                return Some((start, line_start, &markdown[start..line_start]));
            }
        }
        j += 1;
    }
    None
}

fn synthesize_sample_md(entry: &PlanEntry, env: &HashMap<String, String>) -> String {
    // Writing to a String via `std::fmt::Write` is infallible; discard the Result.
    let mut yaml = String::new();
    let _ = writeln!(yaml, "version: 1");
    let _ = writeln!(yaml, "name: {}", yaml_scalar(&entry.name));
    let _ = writeln!(yaml, "session:");
    let _ = writeln!(yaml, "  boot_once: {}", entry.boot.boot_once);
    if let Some(port) = entry.boot.boot_ready_port {
        let _ = writeln!(yaml, "  boot_ready_port: {port}");
    }
    if let Some(timeout) = entry.boot.boot_ready_timeout_s {
        let _ = writeln!(yaml, "  boot_ready_timeout_s: {timeout}");
    }
    let _ = writeln!(yaml, "  boot:");
    let _ = writeln!(yaml, "    command: {}", yaml_scalar(&entry.boot.command));
    if let Some(cwd) = &entry.boot.cwd {
        // Substitute ${VAR} in cwd before emitting so SAMPLE.md is fully concrete.
        let cwd_resolved = expand_env_in_scalar(cwd, env);
        let _ = writeln!(yaml, "    cwd: {}", yaml_scalar(&cwd_resolved));
    }
    if let Some(timeout) = entry.boot.timeout_s {
        let _ = writeln!(yaml, "    timeout_s: {timeout}");
    }
    if !entry.boot.env.is_empty() {
        let _ = writeln!(yaml, "    env:");
        let mut keys: Vec<&String> = entry.boot.env.keys().collect();
        keys.sort();
        for k in keys {
            let v = &entry.boot.env[k];
            let _ = writeln!(yaml, "      {}: {}", yaml_scalar(k), yaml_scalar(v));
        }
    }
    let _ = writeln!(yaml, "  scenarios:");
    let _ = writeln!(yaml, "    must_pass:");
    if entry.flows.is_empty() {
        let _ = writeln!(yaml, "      []");
    } else {
        for flow in &entry.flows {
            let _ = writeln!(yaml, "      - scenarios/{flow}.yaml");
        }
    }

    let mut out = String::new();
    let _ = writeln!(out, "# {name}", name = entry.name);
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "Composed by mirroir-run. Do not hand-edit; regenerate with `mirroir-run --recompose`."
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "```yaml");
    out.push_str(&yaml);
    let _ = writeln!(out, "```");
    out
}

fn expand_env_in_scalar(s: &str, env: &HashMap<String, String>) -> String {
    env_substitute(s, env).unwrap_or_else(|_| s.to_owned())
}

/// Quote a string as a YAML scalar — always quoted so embedded `:` / `#` /
/// leading whitespace are safe. Quotes any internal `"` via `\"`.
fn yaml_scalar(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

fn hash_plan_entry(entry: &PlanEntry) -> String {
    // Deterministic JSON-ish representation of the fields that matter for the cache.
    let canonical = format!(
        "name={}|flows={:?}|vars={:?}|boot={:?}",
        entry.name, entry.flows, entry.vars, entry.boot
    );
    sha256_hex(canonical.as_bytes())
}

/// Provenance manifest emitted under `.build/<sample>/.compose-manifest.json`.
#[derive(Debug, Serialize, Deserialize)]
struct ComposeManifest {
    version: u32,
    composed_at: DateTime<Utc>,
    archetype: ComposeManifestArchetype,
    instance: ComposeManifestInstance,
}

#[derive(Debug, Serialize, Deserialize)]
struct ComposeManifestArchetype {
    reference: String,
    resolved_version: Option<String>,
    source_files: Vec<SourceFileRecord>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ComposeManifestInstance {
    name: String,
    plan_entry_sha256: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct SourceFileRecord {
    path: PathBuf,
    sha256: String,
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;
    use std::fs;
    use std::result::Result as StdResult;

    use std::thread::sleep;
    use std::time::Duration;

    use super::*;
    use crate::mirroir::resolve::{ArchetypeOrigin, ResolvedArchetype};
    use crate::parser::archetype::parse_archetype_manifest;
    use crate::parser::mirroir::{
        ArchetypeRef, ArchetypeRefKind, PlanEntry, PlanEntryBoot, PlanEntrySource,
    };

    type TestResult = StdResult<(), Box<dyn StdError>>;

    fn write_archetype_tree(dir: &Path) -> io::Result<()> {
        fs::create_dir_all(dir.join("scenarios"))?;
        fs::write(
            dir.join("archetype.md"),
            "```yaml\nversion: 1\nname: test/sample\narchetype_version: 1.0.0\nprovides:\n  flows:\n    - chat-stream\n```\n",
        )?;
        fs::write(
            dir.join("APP.md"),
            "# Test App\n\n```yaml\nversion: 1\napp: test\nsurface: web\nurl: \"http://127.0.0.1:${PORT}/\"\n```\n\nProse follows.",
        )?;
        fs::write(
            dir.join("scenarios/chat-stream.yaml"),
            "version: 1\nname: chat-stream\nsteps:\n  - target: { kind: web, url: \"http://127.0.0.1:${PORT}/console/\" }\n  - type: \"${MESSAGE:-hello}\"\n",
        )?;
        Ok(())
    }

    fn fixture_resolved(dir: &Path) -> StdResult<ResolvedArchetype, Box<dyn StdError>> {
        let raw = fs::read_to_string(dir.join("archetype.md"))?;
        let manifest = parse_archetype_manifest("archetype.md", &raw)?;
        Ok(ResolvedArchetype {
            origin: ArchetypeOrigin::Pack {
                pack: "test-pack".to_owned(),
                name: "test/sample".to_owned(),
                version: "1.0.0".to_owned(),
            },
            manifest,
            directory: dir.to_path_buf(),
        })
    }

    fn fixture_entry(name: &str, port: &str) -> PlanEntry {
        let archetype_ref = ArchetypeRef {
            kind: ArchetypeRefKind::Pack,
            pack: Some("test-pack".to_owned()),
            name: "test/sample".to_owned(),
            version: Some("1.0.0".to_owned()),
        };
        let mut vars = HashMap::new();
        vars.insert("PORT".to_owned(), port.to_owned());
        PlanEntry {
            name: name.to_owned(),
            source: PlanEntrySource::Archetypes {
                references: vec![archetype_ref],
            },
            flows: vec!["chat-stream".to_owned()],
            vars,
            boot: PlanEntryBoot {
                command: "echo hi".to_owned(),
                cwd: Some("${CWD_FALLBACK:-/tmp}".to_owned()),
                env: HashMap::new(),
                timeout_s: None,
                boot_once: true,
                boot_ready_port: Some(port.parse::<u16>().unwrap_or(8080)),
                boot_ready_timeout_s: Some(120),
            },
            skip: false,
        }
    }

    #[test]
    fn compose_writes_scenarios_with_substituted_vars() -> TestResult {
        let tmp = tempfile::tempdir()?;
        let project = tmp.path().join("proj");
        fs::create_dir_all(&project)?;
        let archetype_dir = tmp.path().join("arch");
        write_archetype_tree(&archetype_dir)?;

        let entry = fixture_entry("sample-a", "8080");
        let resolved = fixture_resolved(&archetype_dir)?;
        let suite_env = HashMap::new();
        let composed = compose_sample(&entry, &suite_env, Some(&resolved), &project)?;
        assert!(!composed.local);

        let scenario = fs::read_to_string(composed.directory.join("scenarios/chat-stream.yaml"))?;
        assert!(scenario.contains("http://127.0.0.1:8080/console/"));
        assert!(scenario.contains("type: hello"));
        Ok(())
    }

    #[test]
    fn compose_emits_sample_md_with_boot_wiring() -> TestResult {
        let tmp = tempfile::tempdir()?;
        let project = tmp.path().join("proj");
        fs::create_dir_all(&project)?;
        let archetype_dir = tmp.path().join("arch");
        write_archetype_tree(&archetype_dir)?;
        let entry = fixture_entry("sample-b", "9090");
        let resolved = fixture_resolved(&archetype_dir)?;
        let composed = compose_sample(&entry, &HashMap::new(), Some(&resolved), &project)?;
        let sample_md = fs::read_to_string(composed.directory.join("SAMPLE.md"))?;
        assert!(sample_md.contains("name: \"sample-b\""));
        assert!(sample_md.contains("command: \"echo hi\""));
        assert!(sample_md.contains("boot_ready_port: 9090"));
        assert!(sample_md.contains("- scenarios/chat-stream.yaml"));
        Ok(())
    }

    #[test]
    fn compose_needed_false_on_clean_cache() -> TestResult {
        let tmp = tempfile::tempdir()?;
        let project = tmp.path().join("proj");
        fs::create_dir_all(&project)?;
        let archetype_dir = tmp.path().join("arch");
        write_archetype_tree(&archetype_dir)?;
        let entry = fixture_entry("sample-c", "7070");
        let resolved = fixture_resolved(&archetype_dir)?;
        let _ = compose_sample(&entry, &HashMap::new(), Some(&resolved), &project)?;
        assert!(!compose_needed(&entry, &resolved, &project)?);
        Ok(())
    }

    #[test]
    fn compose_needed_true_after_source_edit() -> TestResult {
        let tmp = tempfile::tempdir()?;
        let project = tmp.path().join("proj");
        fs::create_dir_all(&project)?;
        let archetype_dir = tmp.path().join("arch");
        write_archetype_tree(&archetype_dir)?;
        let entry = fixture_entry("sample-d", "5050");
        let resolved = fixture_resolved(&archetype_dir)?;
        let _ = compose_sample(&entry, &HashMap::new(), Some(&resolved), &project)?;

        // Mutate the archetype source: append bytes, ensures different content + mtime.
        sleep(Duration::from_millis(10));
        let mut existing = fs::read_to_string(archetype_dir.join("APP.md"))?;
        existing.push_str("\nappended\n");
        fs::write(archetype_dir.join("APP.md"), existing)?;

        assert!(compose_needed(&entry, &resolved, &project)?);
        Ok(())
    }

    #[test]
    fn compose_needed_true_after_plan_entry_change() -> TestResult {
        let tmp = tempfile::tempdir()?;
        let project = tmp.path().join("proj");
        fs::create_dir_all(&project)?;
        let archetype_dir = tmp.path().join("arch");
        write_archetype_tree(&archetype_dir)?;
        let resolved = fixture_resolved(&archetype_dir)?;

        let entry_v1 = fixture_entry("sample-e", "1111");
        let _ = compose_sample(&entry_v1, &HashMap::new(), Some(&resolved), &project)?;

        let entry_v2 = fixture_entry("sample-e", "2222"); // different PORT
        assert!(compose_needed(&entry_v2, &resolved, &project)?);
        Ok(())
    }

    #[test]
    fn local_entry_returns_source_path_directly() -> TestResult {
        let tmp = tempfile::tempdir()?;
        let project = tmp.path();
        let apps_dir = project.join(".mirroir/apps/legacy");
        fs::create_dir_all(&apps_dir)?;
        fs::write(apps_dir.join("SAMPLE.md"), "version: 1\nplan: {}")?;

        let entry = PlanEntry {
            name: "legacy".to_owned(),
            source: PlanEntrySource::Local {
                path: PathBuf::from("apps/legacy"),
            },
            flows: vec![],
            vars: HashMap::new(),
            boot: PlanEntryBoot {
                command: "echo".to_owned(),
                cwd: None,
                env: HashMap::new(),
                timeout_s: None,
                boot_once: true,
                boot_ready_port: None,
                boot_ready_timeout_s: None,
            },
            skip: false,
        };
        let composed = compose_sample(&entry, &HashMap::new(), None, project)?;
        assert!(composed.local);
        assert_eq!(composed.directory, apps_dir);
        Ok(())
    }
}
