// ABOUTME: Lockfile generation, freshness check, and --locked/--frozen enforcement.
// ABOUTME: Generation gathers git source info + sha256 of archetype tree; freshness compares config vs lockfile.

use std::fs;
use std::path::Path;

use crate::error::{Result, RunnerError};
use crate::mirroir::resolve_version::version_satisfies_constraint;
use crate::parser::lockfile::{Lockfile, parse_lockfile, serialize_lockfile};
use crate::parser::mirroir::{ArchetypeRef, ArchetypeRefKind, MirroirConfig, PlanEntrySource};

pub use crate::mirroir::lock_generate::regenerate_lockfile;

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

    // Version-constraint drift: for refs present in both, the recorded
    // `resolved.version` must still satisfy the constraint declared in config.
    // Catches a hand-edited or partially-updated lockfile whose pin no longer
    // matches its own ref's `@<version>` constraint. Project-local refs carry
    // no resolved version (checksum-only) and are skipped.
    for r in collect_plan_archetype_refs(config) {
        let ref_str = format_ref(r);
        let Some(locked) = lockfile.archetypes.iter().find(|a| a.reference == ref_str) else {
            continue;
        };
        let Some(resolved_version) = locked.resolved.version.as_deref() else {
            continue;
        };
        let constraint = r.version.as_deref().unwrap_or("");
        // `None` = unparseable resolved version (non-semver pin); leave to checksum.
        if version_satisfies_constraint(resolved_version, constraint) == Some(false) {
            reasons.push(format!(
                "ref `{ref_str}` is locked at {resolved_version}, which no longer satisfies its config constraint"
            ));
        }
    }

    if reasons.is_empty() {
        FreshnessVerdict::Fresh
    } else {
        FreshnessVerdict::Stale { reasons }
    }
}

/// Collect the structured archetype refs referenced by the plan (both sets).
fn collect_plan_archetype_refs(config: &MirroirConfig) -> Vec<&ArchetypeRef> {
    let mut refs = Vec::new();
    for entry in config
        .plan
        .must_pass
        .iter()
        .chain(config.plan.nice_to_pass.iter())
    {
        if let PlanEntrySource::Archetypes { references } = &entry.source {
            refs.extend(references.iter());
        }
    }
    refs
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

/// Render an [`ArchetypeRef`] back to its canonical `<pack>/<name>[@<version>]`
/// (or `user/<name>` / project-local path) string form.
#[must_use]
pub fn format_ref(r: &ArchetypeRef) -> String {
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

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;
    use std::result::Result as StdResult;

    use chrono::Utc;

    use super::*;
    use crate::parser::lockfile::{LockedArchetype, LockedOrigin, ResolvedRecord};
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
    fn stale_when_locked_version_violates_constraint() -> TestResult {
        // Config ref pins `@v1` (major 1); the lockfile records a resolved
        // version of 2.0.0 — the ref string matches but the pin no longer
        // satisfies the constraint, which the set-diff alone would miss.
        let config = config_with_one_archetype()?;
        let mut lock = empty_lockfile();
        lock.archetypes.push(LockedArchetype {
            reference: "mirroir-skills/foo/bar@v1".to_owned(),
            resolved: ResolvedRecord {
                kind: LockedOrigin::Pack,
                pack: Some("mirroir-skills".to_owned()),
                name: "foo/bar".to_owned(),
                version: Some("2.0.0".to_owned()),
                source: None,
                checksum: "sha256:xx".to_owned(),
            },
        });
        match check_lockfile_fresh(&config, &lock) {
            FreshnessVerdict::Stale { reasons } => {
                assert!(
                    reasons.iter().any(|r| r.contains("no longer satisfies")),
                    "expected constraint-drift reason, got {reasons:?}"
                );
                Ok(())
            }
            FreshnessVerdict::Fresh => Err("expected Stale on version drift, got Fresh".into()),
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
