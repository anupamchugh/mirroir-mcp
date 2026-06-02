// ABOUTME: Parser for `.mirroir/mirroir.yaml` — the plan that lists samples + archetype refs.
// ABOUTME: Validates exactly-one-of(archetypes, local), plural archetypes (v1: ≤1 entry), and ref syntax.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::Deserialize;

use crate::error::{Result, RunnerError};

/// Highest schema version this binary understands for `mirroir.yaml`.
pub const MIRROIR_SCHEMA_VERSION: u32 = 1;

/// Top-level shape of `<repo>/.mirroir/mirroir.yaml`.
///
/// The plan is the contract between the consumer repository and the runner.
/// Each plan entry declares either a pack-resolved archetype (with per-instance
/// overrides) or a fully-local sample definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirroirConfig {
    /// Schema major version. Currently always `1`.
    pub version: u32,
    /// Human-readable suite name.
    pub name: Option<String>,
    /// Multi-line description.
    pub description: Option<String>,
    /// Default scenario tier when `--scenarios` is omitted.
    pub default_set: Option<DefaultScenarioSet>,
    /// The plan itself — `must_pass` + `nice_to_pass` tiers.
    pub plan: Plan,
    /// Suite-level env merged into every sample's boot (per-sample wins on key conflict).
    pub env: HashMap<String, String>,
}

/// Mirrors the runtime `ScenarioSet` from `replay.rs` but lives in the parser
/// module to keep parsing self-contained. The runner translates between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefaultScenarioSet {
    /// Run only `plan.must_pass`.
    MustPass,
    /// Run only `plan.nice_to_pass`.
    NiceToPass,
    /// Run both, `must_pass` first.
    All,
}

/// Two-tier scenario plan. Each tier is an ordered list of plan entries.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    /// Samples whose failure blocks the suite.
    pub must_pass: Vec<PlanEntry>,
    /// Samples whose failure is informational (do not block).
    pub nice_to_pass: Vec<PlanEntry>,
}

/// One sample in the plan.
///
/// Exactly one of `source = Archetypes` or `source = Local` is set — the
/// raw YAML's `archetypes:` and `local:` fields are validated as mutually
/// exclusive at parse time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanEntry {
    /// Unique name within the plan. Used as the `.mirroir/.build/<name>/` directory.
    pub name: String,
    /// Where the sample's APP/SKILL/scenarios come from.
    pub source: PlanEntrySource,
    /// Subset of the archetype's `provides.flows`. Empty for `Local` sources.
    pub flows: Vec<String>,
    /// Variable substitutions applied to archetype-sourced files.
    pub vars: HashMap<String, String>,
    /// Per-instance boot wiring.
    pub boot: PlanEntryBoot,
    /// True only in `mirroir.local.yaml` overrides — excludes the entry from the run.
    pub skip: bool,
}

/// Either-or origin of a plan entry's artifacts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanEntrySource {
    /// One or more archetype references (v1 enforces exactly one).
    Archetypes {
        /// Parsed references in declaration order.
        references: Vec<ArchetypeRef>,
    },
    /// Path to a self-contained local sample under `<repo>/.mirroir/`.
    Local {
        /// Repo-relative path (relative to `<repo>/.mirroir/`).
        path: PathBuf,
    },
}

/// Boot wiring for one plan entry. Flattens what would otherwise be split
/// across `boot:` (command/cwd/env/timeout) and `session:` (`boot_once`/`ready_port`)
/// in a standalone `SAMPLE.md`. The compose layer separates these back out
/// when emitting `.mirroir/.build/<sample>/SAMPLE.md`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanEntryBoot {
    /// Shell-words-tokenized command line.
    pub command: String,
    /// Working directory for the boot subprocess. Supports `${VAR}` substitution.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Environment overrides for the boot subprocess.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Optional ceiling on the boot subprocess's runtime, in seconds.
    #[serde(default)]
    pub timeout_s: Option<u32>,
    /// When true, boot once across all scenarios in this sample.
    #[serde(default = "default_boot_once")]
    pub boot_once: bool,
    /// TCP port the runner waits on after boot.
    #[serde(default)]
    pub boot_ready_port: Option<u16>,
    /// Boot-ready wait timeout in seconds. Defaults to 60 when port is set.
    #[serde(default)]
    pub boot_ready_timeout_s: Option<u32>,
}

const fn default_boot_once() -> bool {
    true
}

/// A parsed archetype reference. The runner consumes this via the resolver
/// (see Stage 2) to locate the archetype on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchetypeRef {
    /// Which lookup chain to follow at resolve time.
    pub kind: ArchetypeRefKind,
    /// Pack identifier (e.g., `mirroir-skills`). `None` for project-local and user-global refs.
    pub pack: Option<String>,
    /// Archetype name within its origin (e.g., `atmosphere/ai-console`) — or a relative path for project-local refs.
    pub name: String,
    /// Version pin or constraint (e.g., `1.0.3`, `v1`, `1`). `None` for floating-latest or project-local.
    pub version: Option<String>,
}

/// Origin family of an [`ArchetypeRef`]. Drives the resolution chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchetypeRefKind {
    /// Pack-prefixed: `<pack>/<name>[@<version>]` → `~/.mirroir/skills/<pack>/<v>/archetypes/<name>/`.
    Pack,
    /// Project-local: `./<path>` → `<repo>/.mirroir/<path>/`.
    ProjectLocal,
    /// User-global: `user/<name>[@<version>]` → `~/.mirroir/archetypes/<name>/<v>/`.
    UserGlobal,
}

impl ArchetypeRef {
    /// Parse a single archetype reference string.
    ///
    /// Accepts:
    /// - `<pack>/<name>[@<version>]` — pack-prefixed
    /// - `./<path>` — project-local
    /// - `user/<name>[@<version>]` — user-global
    ///
    /// Rejects bare `<name>` (would be ambiguous between pack and user-global).
    ///
    /// # Errors
    ///
    /// Returns [`RunnerError::MirroirInvalidArchetypeRef`] on any of:
    /// - Empty string
    /// - Bare name with no prefix
    /// - `<pack>/` with no name component
    /// - `user/` with no name component
    pub fn parse(input: &str) -> Result<Self> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err(RunnerError::MirroirInvalidArchetypeRef {
                value: input.to_owned(),
                reason: "empty reference".to_owned(),
            });
        }

        // Project-local: anything starting with `./` or `../`.
        if trimmed.starts_with("./") || trimmed.starts_with("../") {
            return Ok(Self {
                kind: ArchetypeRefKind::ProjectLocal,
                pack: None,
                name: trimmed.to_owned(),
                version: None,
            });
        }

        // Split `@<version>` off the right.
        let (path_part, version) = trimmed
            .rsplit_once('@')
            .map_or((trimmed, None), |(left, right)| {
                (left, Some(right.to_owned()))
            });

        // Split prefix from the rest. Need at least `<prefix>/<something>`.
        let Some((prefix, rest)) = path_part.split_once('/') else {
            return Err(RunnerError::MirroirInvalidArchetypeRef {
                value: input.to_owned(),
                reason: "bare names without a prefix are ambiguous; use `<pack>/<name>`, `./<path>`, or `user/<name>`".to_owned(),
            });
        };
        if rest.is_empty() {
            return Err(RunnerError::MirroirInvalidArchetypeRef {
                value: input.to_owned(),
                reason: format!("`{prefix}/` is missing the name component"),
            });
        }

        if prefix == "user" {
            Ok(Self {
                kind: ArchetypeRefKind::UserGlobal,
                pack: None,
                name: rest.to_owned(),
                version,
            })
        } else {
            Ok(Self {
                kind: ArchetypeRefKind::Pack,
                pack: Some(prefix.to_owned()),
                name: rest.to_owned(),
                version,
            })
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Raw deserialization shapes — flat YAML mapping for `mirroir.yaml`.
// Validation is applied in `try_from` impls below.
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirroirConfigRaw {
    version: u32,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    default_set: Option<DefaultScenarioSet>,
    #[serde(default)]
    plan: PlanRaw,
    #[serde(default)]
    env: HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct PlanRaw {
    #[serde(default)]
    must_pass: Vec<PlanEntryRaw>,
    #[serde(default)]
    nice_to_pass: Vec<PlanEntryRaw>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlanEntryRaw {
    name: String,
    #[serde(default)]
    archetypes: Option<Vec<String>>,
    #[serde(default)]
    local: Option<PathBuf>,
    #[serde(default)]
    flows: Vec<String>,
    #[serde(default)]
    vars: HashMap<String, String>,
    boot: PlanEntryBoot,
    #[serde(default)]
    skip: bool,
}

/// Parse a `mirroir.yaml` body into a validated [`MirroirConfig`].
///
/// The input should be the YAML content (no markdown frontmatter
/// stripping — `mirroir.yaml` is a pure YAML file, not a fenced block).
///
/// # Errors
///
/// * [`RunnerError::YamlParse`] when serde rejects the input.
/// * [`RunnerError::UnsupportedVersion`] when `version != 1`.
/// * [`RunnerError::MirroirInvalidArchetypeRef`] for malformed refs.
/// * [`RunnerError::MirroirCompositionUnsupported`] for `archetypes.len() > 1`.
/// * [`RunnerError::MirroirPlanEntryAmbiguous`] when neither/both of
///   `archetypes` and `local` are set.
pub fn parse_mirroir_config(source_label: &str, yaml: &str) -> Result<MirroirConfig> {
    let raw: MirroirConfigRaw =
        serde_yaml::from_str(yaml).map_err(|source| RunnerError::YamlParse {
            file: source_label.to_owned(),
            source,
        })?;

    if raw.version != MIRROIR_SCHEMA_VERSION {
        return Err(RunnerError::UnsupportedVersion {
            artifact: "mirroir.yaml".to_owned(),
            found: raw.version,
            expected: MIRROIR_SCHEMA_VERSION..=MIRROIR_SCHEMA_VERSION,
        });
    }

    let plan = Plan {
        must_pass: raw
            .plan
            .must_pass
            .into_iter()
            .map(PlanEntry::try_from_raw)
            .collect::<Result<Vec<_>>>()?,
        nice_to_pass: raw
            .plan
            .nice_to_pass
            .into_iter()
            .map(PlanEntry::try_from_raw)
            .collect::<Result<Vec<_>>>()?,
    };

    Ok(MirroirConfig {
        version: raw.version,
        name: raw.name,
        description: raw.description,
        default_set: raw.default_set,
        plan,
        env: raw.env,
    })
}

impl PlanEntry {
    fn try_from_raw(raw: PlanEntryRaw) -> Result<Self> {
        let source = match (raw.archetypes, raw.local) {
            (Some(refs), None) => {
                if refs.is_empty() {
                    return Err(RunnerError::MirroirPlanEntryAmbiguous {
                        entry_name: raw.name,
                        reason: "`archetypes:` is present but empty".to_owned(),
                    });
                }
                if refs.len() > 1 {
                    return Err(RunnerError::MirroirCompositionUnsupported {
                        entry_name: raw.name,
                        count: refs.len(),
                    });
                }
                let parsed = refs
                    .iter()
                    .map(|s| ArchetypeRef::parse(s))
                    .collect::<Result<Vec<_>>>()?;
                PlanEntrySource::Archetypes { references: parsed }
            }
            (None, Some(path)) => PlanEntrySource::Local { path },
            (Some(_), Some(_)) => {
                return Err(RunnerError::MirroirPlanEntryAmbiguous {
                    entry_name: raw.name,
                    reason: "both `archetypes:` and `local:` are set; pick one".to_owned(),
                });
            }
            (None, None) => {
                return Err(RunnerError::MirroirPlanEntryAmbiguous {
                    entry_name: raw.name,
                    reason: "neither `archetypes:` nor `local:` is set; one is required".to_owned(),
                });
            }
        };

        Ok(Self {
            name: raw.name,
            source,
            flows: raw.flows,
            vars: raw.vars,
            boot: raw.boot,
            skip: raw.skip,
        })
    }
}

#[cfg(test)]
#[allow(clippy::too_many_lines)]
mod tests {
    use std::error::Error as StdError;
    use std::result::Result as StdResult;

    use super::*;

    type TestResult = StdResult<(), Box<dyn StdError>>;

    #[test]
    fn parse_pack_ref_with_version() -> TestResult {
        let r = ArchetypeRef::parse("mirroir-skills/atmosphere/ai-console@v1")?;
        assert_eq!(r.kind, ArchetypeRefKind::Pack);
        assert_eq!(r.pack.as_deref(), Some("mirroir-skills"));
        assert_eq!(r.name, "atmosphere/ai-console");
        assert_eq!(r.version.as_deref(), Some("v1"));
        Ok(())
    }

    #[test]
    fn parse_pack_ref_without_version() -> TestResult {
        let r = ArchetypeRef::parse("mirroir-skills/atmosphere/ai-console")?;
        assert_eq!(r.kind, ArchetypeRefKind::Pack);
        assert_eq!(r.version, None);
        Ok(())
    }

    #[test]
    fn parse_project_local_ref() -> TestResult {
        let r = ArchetypeRef::parse("./archetypes/my-custom")?;
        assert_eq!(r.kind, ArchetypeRefKind::ProjectLocal);
        assert_eq!(r.pack, None);
        assert_eq!(r.name, "./archetypes/my-custom");
        assert_eq!(r.version, None);
        Ok(())
    }

    #[test]
    fn parse_user_global_ref() -> TestResult {
        let r = ArchetypeRef::parse("user/internal-dashboard@1.0.0")?;
        assert_eq!(r.kind, ArchetypeRefKind::UserGlobal);
        assert_eq!(r.pack, None);
        assert_eq!(r.name, "internal-dashboard");
        assert_eq!(r.version.as_deref(), Some("1.0.0"));
        Ok(())
    }

    #[test]
    fn reject_bare_name() -> TestResult {
        let result = ArchetypeRef::parse("just-a-name");
        let Err(RunnerError::MirroirInvalidArchetypeRef { value, .. }) = result else {
            return Err(format!("expected MirroirInvalidArchetypeRef, got {result:?}").into());
        };
        assert_eq!(value, "just-a-name");
        Ok(())
    }

    #[test]
    fn reject_empty_ref() {
        let result = ArchetypeRef::parse("   ");
        assert!(matches!(
            result,
            Err(RunnerError::MirroirInvalidArchetypeRef { .. })
        ));
    }

    #[test]
    fn reject_prefix_with_no_name() {
        let result = ArchetypeRef::parse("mirroir-skills/");
        assert!(matches!(
            result,
            Err(RunnerError::MirroirInvalidArchetypeRef { .. })
        ));
    }

    #[test]
    fn parses_minimal_mirroir_yaml() -> TestResult {
        let yaml = r#"
version: 1
plan:
  must_pass:
    - name: hello
      local: apps/hello
      boot:
        command: "echo hi"
"#;
        let config = parse_mirroir_config("test", yaml)?;
        assert_eq!(config.plan.must_pass.len(), 1);
        let entry = &config.plan.must_pass[0];
        assert_eq!(entry.name, "hello");
        let PlanEntrySource::Local { path } = &entry.source else {
            return Err(format!("expected Local, got {:?}", entry.source).into());
        };
        assert_eq!(path, &PathBuf::from("apps/hello"));
        assert_eq!(entry.boot.command, "echo hi");
        assert!(entry.boot.boot_once);
        Ok(())
    }

    #[test]
    fn parses_archetype_entry_with_vars() -> TestResult {
        // Note: the cwd value uses a plain ${VAR} (not :-default) to avoid
        // the `literal_string_with_formatting_args` clippy lint catching
        // the literal `:-` inside a raw string at compile time.
        let yaml = r#"
version: 1
name: atmosphere
plan:
  must_pass:
    - name: spring-boot-ai-chat
      archetypes: [mirroir-skills/atmosphere/ai-console@v1]
      flows: [chat-stream]
      vars:
        PORT: "8080"
      boot:
        command: "./mvnw -q spring-boot:run -pl samples/spring-boot-ai-chat"
        cwd: "${ATMOSPHERE_HOME}"
        env:
          LLM_MODE: fake
        boot_ready_port: 8080
        boot_ready_timeout_s: 120
"#;
        let config = parse_mirroir_config("test", yaml)?;
        let entry = &config.plan.must_pass[0];
        assert_eq!(entry.flows, vec!["chat-stream".to_owned()]);
        assert_eq!(entry.vars.get("PORT").map(String::as_str), Some("8080"));
        let PlanEntrySource::Archetypes { references } = &entry.source else {
            return Err(format!("expected Archetypes, got {:?}", entry.source).into());
        };
        assert_eq!(references.len(), 1);
        assert_eq!(references[0].pack.as_deref(), Some("mirroir-skills"));
        assert_eq!(references[0].name, "atmosphere/ai-console");
        assert_eq!(references[0].version.as_deref(), Some("v1"));
        Ok(())
    }

    #[test]
    fn rejects_both_archetypes_and_local() -> TestResult {
        let yaml = r#"
version: 1
plan:
  must_pass:
    - name: bad
      archetypes: [mirroir-skills/foo/bar]
      local: apps/bad
      flows: [x]
      boot:
        command: "true"
"#;
        let Err(RunnerError::MirroirPlanEntryAmbiguous { entry_name, .. }) =
            parse_mirroir_config("test", yaml)
        else {
            return Err("expected MirroirPlanEntryAmbiguous".into());
        };
        assert_eq!(entry_name, "bad");
        Ok(())
    }

    #[test]
    fn rejects_neither_archetypes_nor_local() {
        let yaml = r#"
version: 1
plan:
  must_pass:
    - name: bad
      boot:
        command: "true"
"#;
        assert!(matches!(
            parse_mirroir_config("test", yaml),
            Err(RunnerError::MirroirPlanEntryAmbiguous { .. })
        ));
    }

    #[test]
    fn rejects_multi_archetype_composition_in_v1() -> TestResult {
        let yaml = r#"
version: 1
plan:
  must_pass:
    - name: composed
      archetypes:
        - mirroir-skills/a/one
        - mirroir-skills/b/two
      flows: [x]
      boot:
        command: "true"
"#;
        let Err(RunnerError::MirroirCompositionUnsupported { entry_name, count }) =
            parse_mirroir_config("test", yaml)
        else {
            return Err("expected MirroirCompositionUnsupported".into());
        };
        assert_eq!(entry_name, "composed");
        assert_eq!(count, 2);
        Ok(())
    }

    #[test]
    fn rejects_unknown_top_level_field() {
        let yaml = r"
version: 1
plan:
  must_pass: []
unknown_field: yep
";
        assert!(matches!(
            parse_mirroir_config("test", yaml),
            Err(RunnerError::YamlParse { .. })
        ));
    }

    #[test]
    fn rejects_wrong_version() -> TestResult {
        let yaml = r"
version: 99
plan:
  must_pass: []
";
        let Err(RunnerError::UnsupportedVersion { found, .. }) = parse_mirroir_config("test", yaml)
        else {
            return Err("expected UnsupportedVersion".into());
        };
        assert_eq!(found, 99);
        Ok(())
    }
}
