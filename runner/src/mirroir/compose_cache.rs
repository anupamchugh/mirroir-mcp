// ABOUTME: Compose cache freshness — manifest structs, sha256 hashing, and the compose_needed check.
// ABOUTME: Decides whether a cached `.build/<sample>/` tree is stale vs. its archetype + plan inputs.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{Result, RunnerError};
use crate::mirroir::compose::build_dir_for;
use crate::mirroir::resolve::{ArchetypeOrigin, ResolvedArchetype};
use crate::parser::mirroir::PlanEntry;

/// Filename of the per-sample compose-provenance manifest.
pub const COMPOSE_MANIFEST_FILE: &str = ".compose-manifest.json";

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

/// Extract the resolved archetype version, if the origin carries one.
#[must_use]
pub fn archetype_version(resolved: &ResolvedArchetype) -> Option<String> {
    match &resolved.origin {
        ArchetypeOrigin::Pack { version, .. } | ArchetypeOrigin::UserGlobal { version, .. } => {
            Some(version.clone())
        }
        ArchetypeOrigin::ProjectLocal { .. } => None,
    }
}

/// Hex-encoded SHA-256 of `bytes`.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Deterministic hash of the plan-entry fields that affect the composed output.
#[must_use]
pub fn hash_plan_entry(entry: &PlanEntry) -> String {
    // Deterministic JSON-ish representation of the fields that matter for the cache.
    let canonical = format!(
        "name={}|flows={:?}|vars={:?}|boot={:?}",
        entry.name, entry.flows, entry.vars, entry.boot
    );
    sha256_hex(canonical.as_bytes())
}

/// Provenance manifest emitted under `.build/<sample>/.compose-manifest.json`.
#[derive(Debug, Serialize, Deserialize)]
pub struct ComposeManifest {
    /// Manifest schema version.
    pub version: u32,
    /// Timestamp the sample was composed at (fast-path mtime comparison anchor).
    pub composed_at: DateTime<Utc>,
    /// Archetype-side provenance (reference, version, source-file checksums).
    pub archetype: ComposeManifestArchetype,
    /// Instance-side provenance (sample name + plan-entry hash).
    pub instance: ComposeManifestInstance,
}

/// Archetype-side provenance recorded in [`ComposeManifest`].
#[derive(Debug, Serialize, Deserialize)]
pub struct ComposeManifestArchetype {
    /// Debug rendering of the resolved archetype origin.
    pub reference: String,
    /// Resolved archetype version, when the origin carries one.
    pub resolved_version: Option<String>,
    /// Checksummed archetype source files used during compose.
    pub source_files: Vec<SourceFileRecord>,
}

/// Instance-side provenance recorded in [`ComposeManifest`].
#[derive(Debug, Serialize, Deserialize)]
pub struct ComposeManifestInstance {
    /// Composed sample name (matches the plan entry).
    pub name: String,
    /// Hash of the plan entry — a mismatch forces a recompose.
    pub plan_entry_sha256: String,
}

/// A single archetype source file's path + checksum.
#[derive(Debug, Serialize, Deserialize)]
pub struct SourceFileRecord {
    /// Archetype-relative path of the source file.
    pub path: PathBuf,
    /// Hex SHA-256 of the source file's bytes at compose time.
    pub sha256: String,
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::error::Error as StdError;
    use std::io;
    use std::result::Result as StdResult;
    use std::thread::sleep;
    use std::time::Duration;

    use super::*;
    use crate::mirroir::compose::compose_sample;
    use crate::parser::archetype::parse_archetype_manifest;
    use crate::parser::mirroir::{ArchetypeRef, ArchetypeRefKind, PlanEntryBoot, PlanEntrySource};

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
}
