// ABOUTME: Judge profile registry — built-in profiles plus oracles/profiles.yaml overlay.
// ABOUTME: Maps a scenario's judge.profile name to an LLM endpoint, model, and timeout.

use std::env;
use std::fs;
use std::path::Path;

use serde::Deserialize;

use crate::error::{Result, RunnerError};

/// Profile registry entry — declares how to reach one LLM provider and which
/// model to use for judging. Owned fields so profiles can be loaded at runtime
/// from `oracles/profiles.yaml` as well as compiled in as built-ins.
#[derive(Debug, Clone)]
pub struct JudgeProfile {
    /// Profile name (referenced from scenario `judge.profile`).
    pub name: String,
    /// Chat-completions base URL (e.g. `https://api.openai.com/v1/chat/completions`).
    pub base_url: String,
    /// Model identifier the provider expects.
    pub model: String,
    /// Environment variable that holds the API key. `None` for local providers
    /// like Ollama that don't authenticate.
    pub api_key_env: Option<String>,
    /// Request timeout in seconds.
    pub timeout_s: u32,
}

/// Built-in profiles. Users may override or add profiles via an
/// `oracles/profiles.yaml` file (see [`JudgeRegistry::load_from`]).
#[must_use]
pub fn builtin_profiles() -> Vec<JudgeProfile> {
    vec![
        // Hosted, fast, cheap. Costs per token but turns around in <1 s typical.
        JudgeProfile {
            name: "fast-ci".to_owned(),
            base_url: "https://api.openai.com/v1/chat/completions".to_owned(),
            model: "gpt-4o-mini".to_owned(),
            api_key_env: Some("OPENAI_API_KEY".to_owned()),
            timeout_s: 30,
        },
        // Local, deterministic, free (assumes an Ollama daemon at default port).
        JudgeProfile {
            name: "byte-stable".to_owned(),
            base_url: "http://127.0.0.1:11434/v1/chat/completions".to_owned(),
            model: "qwen2.5:0.5b".to_owned(),
            api_key_env: None,
            timeout_s: 60,
        },
        // Local, smaller model for quick iteration.
        JudgeProfile {
            name: "cheap-local".to_owned(),
            base_url: "http://127.0.0.1:11434/v1/chat/completions".to_owned(),
            model: "qwen2.5:0.5b".to_owned(),
            api_key_env: None,
            timeout_s: 30,
        },
    ]
}

/// One profile entry as parsed from `oracles/profiles.yaml`.
#[derive(Debug, Deserialize)]
struct ProfileSpec {
    name: String,
    base_url: String,
    model: String,
    #[serde(default)]
    api_key_env: Option<String>,
    #[serde(default = "default_timeout_s")]
    timeout_s: u32,
}

const fn default_timeout_s() -> u32 {
    30
}

/// Top-level shape of `oracles/profiles.yaml`.
#[derive(Debug, Deserialize)]
struct ProfilesFile {
    profiles: Vec<ProfileSpec>,
}

/// Profile registry — maps profile names to [`JudgeProfile`] entries.
#[derive(Debug, Clone)]
pub struct JudgeRegistry {
    profiles: Vec<JudgeProfile>,
}

impl Default for JudgeRegistry {
    fn default() -> Self {
        Self {
            profiles: builtin_profiles(),
        }
    }
}

impl JudgeRegistry {
    /// Build a registry from an explicit profile list. Test-only — production
    /// callers use [`Self::default`] which loads the built-in profile set.
    #[cfg(test)]
    pub fn from_profiles(profiles: Vec<JudgeProfile>) -> Self {
        Self { profiles }
    }

    /// Built-ins overlaid with user profiles from `oracles/profiles.yaml`, if
    /// present under `dir` or `dir/.mirroir/`. A user profile whose `name`
    /// matches a built-in overrides it; new names are appended.
    ///
    /// # Errors
    ///
    /// [`RunnerError::JudgeProfilesParse`] when the file exists but can't be read
    /// or parsed as a `profiles:` list.
    pub fn load_from(dir: &Path) -> Result<Self> {
        let mut registry = Self::default();
        let candidates = [
            dir.join("oracles").join("profiles.yaml"),
            dir.join(".mirroir").join("oracles").join("profiles.yaml"),
        ];
        for path in candidates {
            if path.is_file() {
                registry.overlay_from_file(&path)?;
            }
        }
        Ok(registry)
    }

    /// Discover `oracles/profiles.yaml` relative to the current directory.
    ///
    /// # Errors
    ///
    /// Propagates [`RunnerError::JudgeProfilesParse`] from [`Self::load_from`],
    /// or [`RunnerError::Io`] when the current directory can't be read.
    pub fn load_from_cwd() -> Result<Self> {
        let cwd = env::current_dir().map_err(|source| RunnerError::Io {
            context: "resolve current directory for judge profiles".to_owned(),
            source,
        })?;
        Self::load_from(&cwd)
    }

    fn overlay_from_file(&mut self, path: &Path) -> Result<()> {
        let raw = fs::read_to_string(path).map_err(|source| RunnerError::JudgeProfilesParse {
            path: path.to_path_buf(),
            reason: source.to_string(),
        })?;
        let parsed: ProfilesFile =
            serde_yaml::from_str(&raw).map_err(|source| RunnerError::JudgeProfilesParse {
                path: path.to_path_buf(),
                reason: source.to_string(),
            })?;
        for spec in parsed.profiles {
            let profile = JudgeProfile {
                name: spec.name,
                base_url: spec.base_url,
                model: spec.model,
                api_key_env: spec.api_key_env,
                timeout_s: spec.timeout_s,
            };
            if let Some(existing) = self.profiles.iter_mut().find(|p| p.name == profile.name) {
                *existing = profile;
            } else {
                self.profiles.push(profile);
            }
        }
        Ok(())
    }

    /// Look up a profile by name.
    ///
    /// # Errors
    ///
    /// [`RunnerError::JudgeUnknownProfile`] when the name isn't in the registry.
    pub fn resolve(&self, name: &str) -> Result<&JudgeProfile> {
        self.profiles
            .iter()
            .find(|p| p.name == name)
            .ok_or_else(|| RunnerError::JudgeUnknownProfile {
                profile: name.to_owned(),
            })
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;
    use std::fs;
    use std::result::Result as StdResult;

    use super::*;

    type TestResult = StdResult<(), Box<dyn StdError>>;

    #[test]
    fn load_from_overlays_user_profiles_over_builtins() -> TestResult {
        let dir = tempfile::tempdir()?;
        let oracles = dir.path().join("oracles");
        fs::create_dir_all(&oracles)?;
        // One new profile + one override of a built-in (`fast-ci`).
        fs::write(
            oracles.join("profiles.yaml"),
            "profiles:\n\
             \x20 - name: my-judge\n\
             \x20   base_url: https://example.test/v1/chat/completions\n\
             \x20   model: my-model\n\
             \x20   api_key_env: MY_KEY\n\
             \x20   timeout_s: 45\n\
             \x20 - name: fast-ci\n\
             \x20   base_url: https://override.test/v1/chat/completions\n\
             \x20   model: overridden\n",
        )?;
        let registry = JudgeRegistry::load_from(dir.path())?;

        let custom = registry.resolve("my-judge")?;
        if custom.model != "my-model" || custom.timeout_s != 45 {
            return Err(format!("custom profile not loaded: {custom:?}").into());
        }
        let overridden = registry.resolve("fast-ci")?;
        if overridden.model != "overridden" {
            return Err(format!("builtin not overridden: {overridden:?}").into());
        }
        // Untouched built-ins remain.
        registry.resolve("byte-stable")?;
        Ok(())
    }

    #[test]
    fn load_from_without_file_returns_builtins() -> TestResult {
        let dir = tempfile::tempdir()?;
        let registry = JudgeRegistry::load_from(dir.path())?;
        registry.resolve("fast-ci")?;
        Ok(())
    }

    #[test]
    fn builtin_profiles_includes_fast_ci_byte_stable_cheap_local() {
        let registry = JudgeRegistry::default();
        for name in &["fast-ci", "byte-stable", "cheap-local"] {
            assert!(
                registry.resolve(name).is_ok(),
                "missing builtin profile: {name}"
            );
        }
    }
}
