// ABOUTME: Archetype reference resolution — turn an ArchetypeRef into an on-disk directory + manifest.
// ABOUTME: Walks the project-local → user-installed pack → user-global lookup chain per architecture spec.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Result, RunnerError};
use crate::parser::archetype::{ArchetypeManifest, parse_archetype_manifest};
use crate::parser::mirroir::{ArchetypeRef, ArchetypeRefKind};

/// Filename of the archetype manifest at the root of an archetype directory.
pub const ARCHETYPE_MANIFEST_FILE: &str = "archetype.md";

/// Result of resolving an [`ArchetypeRef`] against the lookup chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedArchetype {
    /// Where the archetype was found (pack / project-local / user-global).
    pub origin: ArchetypeOrigin,
    /// Parsed manifest. The runner uses `provides.flows` for flow validation
    /// and `requires.vars` for compose-time defaults.
    pub manifest: ArchetypeManifest,
    /// Absolute path to the archetype directory on disk.
    pub directory: PathBuf,
}

/// Where a resolved archetype lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchetypeOrigin {
    /// Pack-installed: `~/.mirroir/skills/<pack>/<version>/archetypes/<name>/`.
    Pack {
        /// Pack identifier (e.g., `mirroir-skills`).
        pack: String,
        /// Archetype name within the pack (e.g., `atmosphere/ai-console`).
        name: String,
        /// Resolved exact version (e.g., `1.0.3`).
        version: String,
    },
    /// Project-local archetype under `<project>/.mirroir/<rel-path>`.
    ProjectLocal {
        /// Path as declared in the ref (`./archetypes/foo` etc.).
        path: String,
    },
    /// User-global: `~/.mirroir/archetypes/<name>/<version>/`.
    UserGlobal {
        /// Archetype name as it lives under `~/.mirroir/archetypes/`.
        name: String,
        /// Resolved exact version.
        version: String,
    },
}

/// Resolve an [`ArchetypeRef`] to an on-disk archetype.
///
/// * `project_root` — repository root (the directory containing `.mirroir/`).
/// * `home_root` — value of `$HOME` (or an injected tempdir during tests).
/// * `locked_version` — when `Some`, exact version pin from the lockfile.
///   This overrides any constraint in `archetype_ref.version` and skips the
///   "highest installed match" scan.
///
/// # Errors
///
/// * [`RunnerError::MirroirArchetypeNotFound`] — no on-disk match.
/// * Anything [`parse_archetype_manifest`] returns when reading the archetype's
///   `archetype.md`.
pub fn resolve_archetype(
    archetype_ref: &ArchetypeRef,
    project_root: &Path,
    home_root: &Path,
    locked_version: Option<&str>,
) -> Result<ResolvedArchetype> {
    match archetype_ref.kind {
        ArchetypeRefKind::ProjectLocal => resolve_project_local(archetype_ref, project_root),
        ArchetypeRefKind::Pack => resolve_pack(archetype_ref, home_root, locked_version),
        ArchetypeRefKind::UserGlobal => {
            resolve_user_global(archetype_ref, home_root, locked_version)
        }
    }
}

fn resolve_project_local(
    archetype_ref: &ArchetypeRef,
    project_root: &Path,
) -> Result<ResolvedArchetype> {
    // ArchetypeRef.name carries the literal `./<path>` for project-local refs.
    let mirroir_dir = project_root.join(".mirroir");
    let candidate = mirroir_dir.join(strip_leading_dot_slash(&archetype_ref.name));
    let manifest_path = candidate.join(ARCHETYPE_MANIFEST_FILE);
    if !manifest_path.is_file() {
        return Err(RunnerError::MirroirArchetypeNotFound {
            reference: archetype_ref.name.clone(),
            searched: vec![candidate],
        });
    }
    let manifest = load_manifest_from_path(&manifest_path)?;
    Ok(ResolvedArchetype {
        origin: ArchetypeOrigin::ProjectLocal {
            path: archetype_ref.name.clone(),
        },
        manifest,
        directory: candidate,
    })
}

fn resolve_pack(
    archetype_ref: &ArchetypeRef,
    home_root: &Path,
    locked_version: Option<&str>,
) -> Result<ResolvedArchetype> {
    let pack =
        archetype_ref
            .pack
            .as_deref()
            .ok_or_else(|| RunnerError::MirroirArchetypeNotFound {
                reference: archetype_ref.name.clone(),
                searched: vec![],
            })?;
    let pack_root = home_root.join(".mirroir").join("skills").join(pack);

    let resolved_version = resolve_version_from_constraint(
        &pack_root,
        archetype_ref.version.as_deref(),
        locked_version,
    )?;
    let archetype_dir = pack_root
        .join(&resolved_version)
        .join("archetypes")
        .join(&archetype_ref.name);
    let manifest_path = archetype_dir.join(ARCHETYPE_MANIFEST_FILE);
    if !manifest_path.is_file() {
        return Err(RunnerError::MirroirArchetypeNotFound {
            reference: format!(
                "{pack}/{name}@{ver}",
                pack = pack,
                name = archetype_ref.name,
                ver = resolved_version,
            ),
            searched: vec![archetype_dir],
        });
    }
    let manifest = load_manifest_from_path(&manifest_path)?;
    Ok(ResolvedArchetype {
        origin: ArchetypeOrigin::Pack {
            pack: pack.to_owned(),
            name: archetype_ref.name.clone(),
            version: resolved_version,
        },
        manifest,
        directory: archetype_dir,
    })
}

fn resolve_user_global(
    archetype_ref: &ArchetypeRef,
    home_root: &Path,
    locked_version: Option<&str>,
) -> Result<ResolvedArchetype> {
    let archetype_root = home_root
        .join(".mirroir")
        .join("archetypes")
        .join(&archetype_ref.name);
    let resolved_version = resolve_version_from_constraint(
        &archetype_root,
        archetype_ref.version.as_deref(),
        locked_version,
    )?;
    let archetype_dir = archetype_root.join(&resolved_version);
    let manifest_path = archetype_dir.join(ARCHETYPE_MANIFEST_FILE);
    if !manifest_path.is_file() {
        return Err(RunnerError::MirroirArchetypeNotFound {
            reference: format!(
                "user/{name}@{ver}",
                name = archetype_ref.name,
                ver = resolved_version
            ),
            searched: vec![archetype_dir],
        });
    }
    let manifest = load_manifest_from_path(&manifest_path)?;
    Ok(ResolvedArchetype {
        origin: ArchetypeOrigin::UserGlobal {
            name: archetype_ref.name.clone(),
            version: resolved_version,
        },
        manifest,
        directory: archetype_dir,
    })
}

fn load_manifest_from_path(path: &Path) -> Result<ArchetypeManifest> {
    let raw = fs::read_to_string(path).map_err(|source| RunnerError::Io {
        context: format!("read archetype manifest {}", path.display()),
        source,
    })?;
    parse_archetype_manifest(&path.display().to_string(), &raw)
}

/// Pick the version to use from the installed directories under `root`.
///
/// Resolution priority (highest wins):
/// 1. `locked_version` — exact pin from the lockfile.
/// 2. `ref_constraint` — version constraint from the ref (`v1`, `1.2`, `1.2.3`).
/// 3. Floating "latest": highest semver-parseable directory.
///
/// Returns `MirroirArchetypeNotFound` when the root doesn't exist, contains no
/// versioned directories, or contains none that satisfy the constraint.
fn resolve_version_from_constraint(
    root: &Path,
    ref_constraint: Option<&str>,
    locked_version: Option<&str>,
) -> Result<String> {
    if let Some(v) = locked_version {
        return Ok(v.to_owned());
    }
    if let Some(v) = ref_constraint
        && let Some(parsed) = parse_exact_version(v)
    {
        return Ok(format!("{}.{}.{}", parsed.0, parsed.1, parsed.2));
    }
    let entries = read_installed_versions(root)?;
    let constraint = Constraint::parse(ref_constraint.unwrap_or(""));
    let mut matches: Vec<InstalledVersion> = entries
        .into_iter()
        .filter_map(|(v, dir)| {
            if constraint.matches(v) {
                Some((v, dir))
            } else {
                None
            }
        })
        .collect();
    matches.sort_by_key(|(v, _)| *v);
    let best = matches
        .pop()
        .ok_or_else(|| RunnerError::MirroirArchetypeNotFound {
            reference: format!(
                "no installed version matching `{}` at {}",
                ref_constraint.unwrap_or("<any>"),
                root.display(),
            ),
            searched: vec![root.to_path_buf()],
        })?;
    Ok(best.1)
}

/// Triple of (parsed (major,minor,patch), directory name) yielded from a pack root scan.
type InstalledVersion = ((u32, u32, u32), String);

fn read_installed_versions(root: &Path) -> Result<Vec<InstalledVersion>> {
    let Ok(entries) = fs::read_dir(root) else {
        return Err(RunnerError::MirroirArchetypeNotFound {
            reference: format!("pack/archetype root `{}` does not exist", root.display()),
            searched: vec![root.to_path_buf()],
        });
    };
    let mut versions = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name_os) = path.file_name() else {
            continue;
        };
        let Some(name) = name_os.to_str() else {
            continue;
        };
        if let Some(v) = parse_version_lax(name) {
            versions.push((v, name.to_owned()));
        }
    }
    Ok(versions)
}

/// Parse strings like `1.0.3` or `v1.0.3` into a `(u32, u32, u32)` triple.
/// Returns `None` for incomplete versions (e.g., `1.2`, `v1`).
fn parse_exact_version(s: &str) -> Option<(u32, u32, u32)> {
    let trimmed = s.strip_prefix('v').unwrap_or(s);
    let parts: Vec<&str> = trimmed.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let parts_iter = parts.iter().map(|p| p.parse::<u32>().ok());
    let mut iter = parts_iter;
    let major = iter.next()??;
    let minor = iter.next()??;
    let patch = iter.next()??;
    Some((major, minor, patch))
}

/// Parse a directory name as a version. Accepts `1.0.3`, `v1.0.3`, `1.0`,
/// `v1.0`, `1`, `v1`. Missing components default to 0 so they sort correctly.
fn parse_version_lax(s: &str) -> Option<(u32, u32, u32)> {
    let trimmed = s.strip_prefix('v').unwrap_or(s);
    let mut parts = trimmed.split('.');
    let major = parts.next()?.parse::<u32>().ok()?;
    let minor = parts
        .next()
        .and_then(|p| p.parse::<u32>().ok())
        .unwrap_or(0);
    let patch = parts
        .next()
        .and_then(|p| p.parse::<u32>().ok())
        .unwrap_or(0);
    if parts.next().is_some() {
        return None; // 1.2.3.4 is not a version
    }
    Some((major, minor, patch))
}

/// Version constraint extracted from the `@<v>` portion of an [`ArchetypeRef`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Constraint {
    Any,
    Major(u32),
    Minor(u32, u32),
    Exact(u32, u32, u32),
}

impl Constraint {
    fn parse(s: &str) -> Self {
        let trimmed = s.trim();
        if trimmed.is_empty() || trimmed == "latest" {
            return Self::Any;
        }
        let trimmed = trimmed.strip_prefix('v').unwrap_or(trimmed);
        let parts: Vec<&str> = trimmed.split('.').collect();
        match parts.as_slice() {
            [a] => a.parse::<u32>().ok().map_or(Self::Any, Self::Major),
            [a, b] => match (a.parse::<u32>(), b.parse::<u32>()) {
                (Ok(maj), Ok(min)) => Self::Minor(maj, min),
                _ => Self::Any,
            },
            [a, b, c] => match (a.parse::<u32>(), b.parse::<u32>(), c.parse::<u32>()) {
                (Ok(maj), Ok(min), Ok(patch)) => Self::Exact(maj, min, patch),
                _ => Self::Any,
            },
            _ => Self::Any,
        }
    }

    fn matches(&self, version: (u32, u32, u32)) -> bool {
        match *self {
            Self::Any => true,
            Self::Major(maj) => version.0 == maj,
            Self::Minor(maj, min) => version.0 == maj && version.1 == min,
            Self::Exact(maj, min, patch) => version == (maj, min, patch),
        }
    }
}

fn strip_leading_dot_slash(s: &str) -> &str {
    s.strip_prefix("./").unwrap_or(s)
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;
    use std::fs;
    use std::result::Result as StdResult;

    use super::*;
    use crate::parser::mirroir::ArchetypeRef;

    type TestResult = StdResult<(), Box<dyn StdError>>;

    use std::fmt::Write as _;
    use std::io;

    fn write_archetype(dir: &Path, name: &str, version: &str, flows: &[&str]) -> io::Result<()> {
        fs::create_dir_all(dir)?;
        let mut flows_yaml = String::new();
        for f in flows {
            // `Write` impl on `String` is infallible; the result is folded into a panic-free buffer.
            let _ = writeln!(flows_yaml, "    - {f}");
        }
        let manifest = format!(
            "```yaml\nversion: 1\nname: {name}\narchetype_version: {version}\nprovides:\n  flows:\n{flows_yaml}```\n"
        );
        fs::write(dir.join("archetype.md"), manifest)?;
        Ok(())
    }

    #[test]
    fn resolves_project_local_archetype() -> TestResult {
        let tmp = tempfile::tempdir()?;
        let project = tmp.path();
        let custom = project
            .join(".mirroir")
            .join("archetypes")
            .join("custom-flow");
        write_archetype(&custom, "custom/flow", "0.1.0", &["smoke"])?;

        let r = ArchetypeRef::parse("./archetypes/custom-flow")?;
        let resolved = resolve_archetype(&r, project, Path::new("/nonexistent"), None)?;
        assert!(matches!(
            resolved.origin,
            ArchetypeOrigin::ProjectLocal { .. }
        ));
        assert_eq!(resolved.manifest.name, "custom/flow");
        assert_eq!(resolved.manifest.provides.flows, vec!["smoke"]);
        Ok(())
    }

    #[test]
    fn resolves_pack_archetype_with_exact_version() -> TestResult {
        let tmp = tempfile::tempdir()?;
        let home = tmp.path();
        let dir = home
            .join(".mirroir")
            .join("skills")
            .join("mirroir-skills")
            .join("1.0.3")
            .join("archetypes")
            .join("atmosphere/ai-console");
        write_archetype(&dir, "atmosphere/ai-console", "1.0.3", &["chat-stream"])?;

        let r = ArchetypeRef::parse("mirroir-skills/atmosphere/ai-console@1.0.3")?;
        let resolved = resolve_archetype(&r, Path::new("/proj"), home, None)?;
        match &resolved.origin {
            ArchetypeOrigin::Pack {
                pack,
                name,
                version,
            } => {
                assert_eq!(pack, "mirroir-skills");
                assert_eq!(name, "atmosphere/ai-console");
                assert_eq!(version, "1.0.3");
            }
            other => return Err(format!("expected Pack, got {other:?}").into()),
        }
        Ok(())
    }

    #[test]
    fn resolves_pack_archetype_with_floating_major_picks_highest() -> TestResult {
        let tmp = tempfile::tempdir()?;
        let home = tmp.path();
        for v in &["1.0.0", "1.0.3", "1.1.0", "2.0.0"] {
            let dir = home
                .join(".mirroir/skills/mirroir-skills")
                .join(v)
                .join("archetypes/foo/bar");
            write_archetype(&dir, "foo/bar", v, &["s"])?;
        }
        let r = ArchetypeRef::parse("mirroir-skills/foo/bar@v1")?;
        let resolved = resolve_archetype(&r, Path::new("/proj"), home, None)?;
        match &resolved.origin {
            ArchetypeOrigin::Pack { version, .. } => assert_eq!(version, "1.1.0"),
            other => return Err(format!("expected Pack, got {other:?}").into()),
        }
        Ok(())
    }

    #[test]
    fn resolves_pack_archetype_with_floating_minor() -> TestResult {
        let tmp = tempfile::tempdir()?;
        let home = tmp.path();
        for v in &["1.0.0", "1.0.5", "1.1.0", "1.1.7"] {
            let dir = home
                .join(".mirroir/skills/mirroir-skills")
                .join(v)
                .join("archetypes/foo/bar");
            write_archetype(&dir, "foo/bar", v, &["s"])?;
        }
        let r = ArchetypeRef::parse("mirroir-skills/foo/bar@v1.0")?;
        let resolved = resolve_archetype(&r, Path::new("/proj"), home, None)?;
        match &resolved.origin {
            ArchetypeOrigin::Pack { version, .. } => assert_eq!(version, "1.0.5"),
            other => return Err(format!("expected Pack, got {other:?}").into()),
        }
        Ok(())
    }

    #[test]
    fn locked_version_overrides_ref_constraint() -> TestResult {
        let tmp = tempfile::tempdir()?;
        let home = tmp.path();
        for v in &["1.0.0", "1.0.5", "2.0.0"] {
            let dir = home
                .join(".mirroir/skills/mirroir-skills")
                .join(v)
                .join("archetypes/foo/bar");
            write_archetype(&dir, "foo/bar", v, &["s"])?;
        }
        // Ref says @v2, but locked says use 1.0.5 exactly.
        let r = ArchetypeRef::parse("mirroir-skills/foo/bar@v2")?;
        let resolved = resolve_archetype(&r, Path::new("/proj"), home, Some("1.0.5"))?;
        match &resolved.origin {
            ArchetypeOrigin::Pack { version, .. } => assert_eq!(version, "1.0.5"),
            other => return Err(format!("expected Pack, got {other:?}").into()),
        }
        Ok(())
    }

    #[test]
    fn missing_archetype_returns_not_found() -> TestResult {
        let tmp = tempfile::tempdir()?;
        let r = ArchetypeRef::parse("mirroir-skills/foo/bar@1.0.0")?;
        match resolve_archetype(&r, Path::new("/proj"), tmp.path(), None) {
            Err(RunnerError::MirroirArchetypeNotFound { .. }) => Ok(()),
            other => Err(format!("expected MirroirArchetypeNotFound, got {other:?}").into()),
        }
    }

    #[test]
    fn resolves_user_global_archetype() -> TestResult {
        let tmp = tempfile::tempdir()?;
        let home = tmp.path();
        let dir = home.join(".mirroir/archetypes/internal-dashboard/1.0.0");
        write_archetype(&dir, "internal-dashboard", "1.0.0", &["smoke"])?;

        let r = ArchetypeRef::parse("user/internal-dashboard@1.0.0")?;
        let resolved = resolve_archetype(&r, Path::new("/proj"), home, None)?;
        match &resolved.origin {
            ArchetypeOrigin::UserGlobal { name, version } => {
                assert_eq!(name, "internal-dashboard");
                assert_eq!(version, "1.0.0");
            }
            other => return Err(format!("expected UserGlobal, got {other:?}").into()),
        }
        Ok(())
    }

    #[test]
    fn constraint_parse_handles_v_prefix() {
        assert_eq!(Constraint::parse("v1"), Constraint::Major(1));
        assert_eq!(Constraint::parse("1"), Constraint::Major(1));
        assert_eq!(Constraint::parse("v1.2"), Constraint::Minor(1, 2));
        assert_eq!(Constraint::parse("1.2.3"), Constraint::Exact(1, 2, 3));
        assert_eq!(Constraint::parse(""), Constraint::Any);
        assert_eq!(Constraint::parse("latest"), Constraint::Any);
    }
}
