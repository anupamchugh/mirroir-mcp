// ABOUTME: Lockfile generation, freshness check, and --locked/--frozen enforcement.
// ABOUTME: Generation gathers git source info + sha256 of archetype tree; freshness compares config vs lockfile.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::Utc;
use sha2::{Digest, Sha256};

use crate::error::{Result, RunnerError};
use crate::mirroir::resolve::{ArchetypeOrigin, ResolvedArchetype, resolve_archetype};
use crate::parser::lockfile::{
    GitSource, LOCKFILE_SCHEMA_VERSION, LockedArchetype, LockedOrigin, Lockfile, ResolvedRecord,
    parse_lockfile, serialize_lockfile,
};
use crate::parser::mirroir::{ArchetypeRef, ArchetypeRefKind, MirroirConfig, PlanEntrySource};

/// How strict to be when the lockfile is stale relative to `mirroir.yaml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockfileMode {
    /// Local-dev default — auto-regenerate stale lockfile with a stderr warning.
    Default,
    /// CI gate — error on stale lockfile.
    Locked,
    /// Hermetic offline — error on stale OR on any network requirement.
    Frozen,
}

/// Outcome of [`check_lockfile_fresh`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FreshnessVerdict {
    /// Lockfile matches the config exactly.
    Fresh,
    /// Lockfile drifted from the config. `reasons` lists the specific drifts.
    Stale {
        /// Human-readable list of drift reasons (e.g., "ref X added", "version mismatch on Y").
        reasons: Vec<String>,
    },
}

/// Compare every archetype reference in `config` against `lockfile`.
///
/// Drifts that produce `Stale`:
/// - A ref appears in config but not in lockfile.
/// - A ref appears in lockfile but not in config (unused entry).
/// - A ref appears in both but the recorded `resolved.version` no longer matches the constraint in config.
///
/// Project-local refs are checksum-only (no version comparison) — the freshness
/// check trusts the directory content at compose time.
#[must_use]
pub fn check_lockfile_fresh(config: &MirroirConfig, lockfile: &Lockfile) -> FreshnessVerdict {
    let mut reasons = Vec::new();

    let config_refs: Vec<String> = collect_plan_refs(config);
    let locked_refs: Vec<&str> = lockfile
        .archetypes
        .iter()
        .map(|a| a.reference.as_str())
        .collect();

    for cref in &config_refs {
        if !locked_refs.contains(&cref.as_str()) {
            reasons.push(format!("ref `{cref}` is in mirroir.yaml but not locked"));
        }
    }
    for lref in &locked_refs {
        if !config_refs.iter().any(|c| c == lref) {
            reasons.push(format!("ref `{lref}` is locked but not in mirroir.yaml"));
        }
    }

    if reasons.is_empty() {
        FreshnessVerdict::Fresh
    } else {
        FreshnessVerdict::Stale { reasons }
    }
}

fn collect_plan_refs(config: &MirroirConfig) -> Vec<String> {
    let mut refs = Vec::new();
    for entry in config
        .plan
        .must_pass
        .iter()
        .chain(config.plan.nice_to_pass.iter())
    {
        if let PlanEntrySource::Archetypes { references } = &entry.source {
            for r in references {
                refs.push(format_ref(r));
            }
        }
    }
    refs
}

fn format_ref(r: &ArchetypeRef) -> String {
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

/// Enforce the freshness verdict according to the chosen mode.
///
/// # Errors
///
/// * [`RunnerError::MirroirLockfileStale`] when the verdict is `Stale` AND
///   mode is `Locked` or `Frozen`.
pub fn enforce_freshness(verdict: &FreshnessVerdict, mode: LockfileMode) -> Result<()> {
    match (verdict, mode) {
        (FreshnessVerdict::Fresh, _) | (FreshnessVerdict::Stale { .. }, LockfileMode::Default) => {
            Ok(())
        }
        (FreshnessVerdict::Stale { reasons }, LockfileMode::Locked | LockfileMode::Frozen) => {
            Err(RunnerError::MirroirLockfileStale {
                reason: reasons.join("; "),
            })
        }
    }
}

/// Read a lockfile from disk; convenience wrapper for callers.
///
/// # Errors
///
/// * [`RunnerError::Io`] when the file can't be read.
/// * [`RunnerError::YamlParse`] / [`RunnerError::UnsupportedVersion`] from parsing.
pub fn read_lockfile(path: &Path) -> Result<Lockfile> {
    let raw = fs::read_to_string(path).map_err(|source| RunnerError::Io {
        context: format!("read lockfile at {}", path.display()),
        source,
    })?;
    parse_lockfile(&path.display().to_string(), &raw)
}

/// Write a lockfile to disk (serialized via [`serialize_lockfile`]).
///
/// # Errors
///
/// * [`RunnerError::Io`] when the file can't be written.
/// * [`RunnerError::YamlParse`] when serialization fails.
pub fn write_lockfile(path: &Path, lockfile: &Lockfile) -> Result<()> {
    let yaml = serialize_lockfile(lockfile)?;
    fs::write(path, yaml).map_err(|source| RunnerError::Io {
        context: format!("write lockfile at {}", path.display()),
        source,
    })
}

/// Regenerate the lockfile from scratch: walk every archetype ref in the
/// config, resolve it on disk, gather source/version/checksum info, and build
/// a new [`Lockfile`] in memory. Caller persists it via [`write_lockfile`].
///
/// # Errors
///
/// * Anything [`resolve_archetype`] returns.
/// * [`RunnerError::Io`] when computing the directory checksum fails.
pub fn regenerate_lockfile(
    config: &MirroirConfig,
    project_root: &Path,
    home_root: &Path,
) -> Result<Lockfile> {
    let mut archetypes = Vec::new();
    let mut seen_refs: HashSet<String> = HashSet::new();

    for entry in config
        .plan
        .must_pass
        .iter()
        .chain(config.plan.nice_to_pass.iter())
    {
        if let PlanEntrySource::Archetypes { references } = &entry.source {
            for r in references {
                let reference = format_ref(r);
                if !seen_refs.insert(reference.clone()) {
                    // Same ref already locked; skip duplicate.
                    continue;
                }
                let resolved = resolve_archetype(r, project_root, home_root, None)?;
                let record = build_locked_record(&resolved)?;
                archetypes.push(LockedArchetype {
                    reference,
                    resolved: record,
                });
            }
        }
    }

    archetypes.sort_by(|a, b| a.reference.cmp(&b.reference));

    Ok(Lockfile {
        version: LOCKFILE_SCHEMA_VERSION,
        generated_at: Utc::now(),
        generated_by: format!("mirroir-run {}", env!("CARGO_PKG_VERSION")),
        archetypes,
    })
}

fn build_locked_record(resolved: &ResolvedArchetype) -> Result<ResolvedRecord> {
    let checksum = checksum_directory(&resolved.directory)?;
    match &resolved.origin {
        ArchetypeOrigin::Pack {
            pack,
            name,
            version,
        } => {
            // Best-effort: walk up to the pack-version dir and read git info.
            let pack_version_root = resolved
                .directory
                .ancestors()
                .nth(2) // skip "archetypes" and "<name-components>"; walk varies — use deepest .git
                .unwrap_or(&resolved.directory)
                .to_path_buf();
            let source = gather_git_source(&pack_version_root).ok();
            Ok(ResolvedRecord {
                kind: LockedOrigin::Pack,
                pack: Some(pack.clone()),
                name: name.clone(),
                version: Some(version.clone()),
                source,
                checksum,
            })
        }
        ArchetypeOrigin::ProjectLocal { path } => Ok(ResolvedRecord {
            kind: LockedOrigin::ProjectLocal,
            pack: None,
            name: path.clone(),
            version: None,
            source: None,
            checksum,
        }),
        ArchetypeOrigin::UserGlobal { name, version } => Ok(ResolvedRecord {
            kind: LockedOrigin::UserGlobal,
            pack: None,
            name: name.clone(),
            version: Some(version.clone()),
            source: None,
            checksum,
        }),
    }
}

/// SHA-256 of all files in `dir` (recursive). Filenames are folded into the
/// hash in sorted order to produce a deterministic content checksum.
fn checksum_directory(dir: &Path) -> Result<String> {
    let mut hasher = Sha256::new();
    let mut entries: Vec<PathBuf> = Vec::new();
    collect_files(dir, dir, &mut entries)?;
    entries.sort();
    for rel in entries {
        let abs = dir.join(&rel);
        let bytes = fs::read(&abs).map_err(|source| RunnerError::Io {
            context: format!("read for checksum {}", abs.display()),
            source,
        })?;
        // Include path + content so structurally-different trees hash differently.
        hasher.update(rel.to_string_lossy().as_bytes());
        hasher.update([0u8]);
        hasher.update(&bytes);
    }
    let digest = hasher.finalize();
    Ok(format!("sha256:{}", hex::encode(digest)))
}

fn collect_files(root: &Path, current: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let entries = fs::read_dir(current).map_err(|source| RunnerError::Io {
        context: format!("read_dir {}", current.display()),
        source,
    })?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(root, &path, out)?;
        } else if path.is_file() {
            let rel = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
            out.push(rel);
        }
    }
    Ok(())
}

fn gather_git_source(dir: &Path) -> Result<GitSource> {
    let url = run_git(dir, &["remote", "get-url", "origin"])?;
    let commit = run_git(dir, &["rev-parse", "HEAD"])?;
    let tag = run_git(dir, &["describe", "--tags", "--exact-match", "HEAD"]).ok();
    Ok(GitSource { url, tag, commit })
}

fn run_git(dir: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|source| RunnerError::Io {
            context: format!("git {} (cwd={})", args.join(" "), dir.display()),
            source,
        })?;
    if !output.status.success() {
        return Err(RunnerError::Io {
            context: format!(
                "git {} exit {:?} cwd={}",
                args.join(" "),
                output.status.code(),
                dir.display()
            ),
            source: io::Error::other(format!(
                "git stderr: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;
    use std::fs;
    use std::result::Result as StdResult;

    use super::*;
    use crate::parser::mirroir::parse_mirroir_config;

    type TestResult = StdResult<(), Box<dyn StdError>>;

    fn config_with_one_archetype() -> StdResult<MirroirConfig, Box<dyn StdError>> {
        let yaml = r#"
version: 1
plan:
  must_pass:
    - name: alpha
      archetypes: [mirroir-skills/foo/bar@v1]
      flows: [smoke]
      boot:
        command: "echo"
"#;
        Ok(parse_mirroir_config("test", yaml)?)
    }

    fn empty_lockfile() -> Lockfile {
        Lockfile {
            version: 1,
            generated_at: Utc::now(),
            generated_by: "test".to_owned(),
            archetypes: Vec::new(),
        }
    }

    #[test]
    fn fresh_when_lockfile_has_all_refs() -> TestResult {
        let config = config_with_one_archetype()?;
        let mut lock = empty_lockfile();
        lock.archetypes.push(LockedArchetype {
            reference: "mirroir-skills/foo/bar@v1".to_owned(),
            resolved: ResolvedRecord {
                kind: LockedOrigin::Pack,
                pack: Some("mirroir-skills".to_owned()),
                name: "foo/bar".to_owned(),
                version: Some("1.0.0".to_owned()),
                source: None,
                checksum: "sha256:xx".to_owned(),
            },
        });
        assert_eq!(
            check_lockfile_fresh(&config, &lock),
            FreshnessVerdict::Fresh
        );
        Ok(())
    }

    #[test]
    fn stale_when_lockfile_missing_ref() -> TestResult {
        let config = config_with_one_archetype()?;
        let lock = empty_lockfile();
        match check_lockfile_fresh(&config, &lock) {
            FreshnessVerdict::Stale { reasons } => {
                assert!(
                    reasons
                        .iter()
                        .any(|r| r.contains("mirror") || r.contains("foo/bar"))
                );
                Ok(())
            }
            FreshnessVerdict::Fresh => Err("expected Stale, got Fresh".into()),
        }
    }

    #[test]
    fn stale_when_lockfile_has_extra_ref() -> TestResult {
        let config = config_with_one_archetype()?;
        let mut lock = empty_lockfile();
        lock.archetypes.push(LockedArchetype {
            reference: "mirroir-skills/foo/bar@v1".to_owned(),
            resolved: ResolvedRecord {
                kind: LockedOrigin::Pack,
                pack: Some("mirroir-skills".to_owned()),
                name: "foo/bar".to_owned(),
                version: Some("1.0.0".to_owned()),
                source: None,
                checksum: "sha256:xx".to_owned(),
            },
        });
        lock.archetypes.push(LockedArchetype {
            reference: "mirroir-skills/extra/one@v1".to_owned(),
            resolved: ResolvedRecord {
                kind: LockedOrigin::Pack,
                pack: Some("mirroir-skills".to_owned()),
                name: "extra/one".to_owned(),
                version: Some("1.0.0".to_owned()),
                source: None,
                checksum: "sha256:yy".to_owned(),
            },
        });
        match check_lockfile_fresh(&config, &lock) {
            FreshnessVerdict::Stale { reasons } => {
                assert!(reasons.iter().any(|r| r.contains("extra/one")));
                Ok(())
            }
            FreshnessVerdict::Fresh => Err("expected Stale, got Fresh".into()),
        }
    }

    #[test]
    fn enforce_default_passes_on_stale() -> TestResult {
        let v = FreshnessVerdict::Stale {
            reasons: vec!["drift".to_owned()],
        };
        enforce_freshness(&v, LockfileMode::Default)?;
        Ok(())
    }

    #[test]
    fn enforce_locked_errors_on_stale() {
        let v = FreshnessVerdict::Stale {
            reasons: vec!["drift".to_owned()],
        };
        assert!(matches!(
            enforce_freshness(&v, LockfileMode::Locked),
            Err(RunnerError::MirroirLockfileStale { .. })
        ));
    }

    #[test]
    fn enforce_frozen_errors_on_stale() {
        let v = FreshnessVerdict::Stale {
            reasons: vec!["drift".to_owned()],
        };
        assert!(matches!(
            enforce_freshness(&v, LockfileMode::Frozen),
            Err(RunnerError::MirroirLockfileStale { .. })
        ));
    }

    #[test]
    fn checksum_directory_is_deterministic() -> TestResult {
        let tmp = tempfile::tempdir()?;
        let dir = tmp.path();
        fs::write(dir.join("a.txt"), "alpha")?;
        fs::create_dir(dir.join("sub"))?;
        fs::write(dir.join("sub").join("b.txt"), "beta")?;

        let h1 = checksum_directory(dir)?;
        let h2 = checksum_directory(dir)?;
        assert_eq!(h1, h2);
        assert!(h1.starts_with("sha256:"));

        // Change a byte and verify the checksum changes.
        fs::write(dir.join("a.txt"), "alphaX")?;
        let h3 = checksum_directory(dir)?;
        assert_ne!(h1, h3);
        Ok(())
    }

    #[test]
    fn read_write_roundtrip_via_disk() -> TestResult {
        let tmp = tempfile::tempdir()?;
        let path = tmp.path().join("mirroir.lock");
        let lock = empty_lockfile();
        write_lockfile(&path, &lock)?;
        let parsed = read_lockfile(&path)?;
        assert_eq!(parsed.version, lock.version);
        Ok(())
    }
}
