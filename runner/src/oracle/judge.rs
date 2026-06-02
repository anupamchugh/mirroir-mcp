// ABOUTME: LLM judge oracle — scores response text against expectation using a profile registry.
// ABOUTME: OpenAI-compatible chat completions API; works with OpenAI, Anthropic-compatible, vLLM, Ollama.

use std::env;
use std::time::Duration;

use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::error::{Result, RunnerError};
use crate::parser::step::JudgeArgs;

/// Profile registry entry — declares how to reach one LLM provider and which
/// model to use for judging.
#[derive(Debug, Clone)]
pub struct JudgeProfile {
    /// Profile name (referenced from scenario `judge.profile`).
    pub name: &'static str,
    /// Chat-completions base URL (e.g. `https://api.openai.com/v1/chat/completions`).
    pub base_url: &'static str,
    /// Model identifier the provider expects.
    pub model: &'static str,
    /// Environment variable that holds the API key. `None` for local providers
    /// like Ollama that don't authenticate.
    pub api_key_env: Option<&'static str>,
    /// Request timeout in seconds.
    pub timeout_s: u32,
}

/// Built-in profiles. Authoring tools may override by passing a custom
/// [`JudgeRegistry`] to [`run_judge`].
#[must_use]
pub fn builtin_profiles() -> Vec<JudgeProfile> {
    vec![
        // Hosted, fast, cheap. Costs per token but turns around in <1 s typical.
        JudgeProfile {
            name: "fast-ci",
            base_url: "https://api.openai.com/v1/chat/completions",
            model: "gpt-4o-mini",
            api_key_env: Some("OPENAI_API_KEY"),
            timeout_s: 30,
        },
        // Local, deterministic, free (assumes an Ollama daemon at default port).
        JudgeProfile {
            name: "byte-stable",
            base_url: "http://127.0.0.1:11434/v1/chat/completions",
            model: "qwen2.5:0.5b",
            api_key_env: None,
            timeout_s: 60,
        },
        // Local, smaller model for quick iteration.
        JudgeProfile {
            name: "cheap-local",
            base_url: "http://127.0.0.1:11434/v1/chat/completions",
            model: "qwen2.5:0.5b",
            api_key_env: None,
            timeout_s: 30,
        },
    ]
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

/// Outcome of a single judge invocation.
#[derive(Debug, Clone, PartialEq)]
pub struct JudgeOutcome {
    /// Score in `[0, 1]` extracted from the model's reply.
    pub score: f64,
    /// Raw text of the model's reply (for diagnostics).
    pub raw: String,
}

/// Score `response` against the scenario's `judge:` args using the named
/// profile. Returns `Ok(JudgeOutcome)` on transport+decode success; the
/// caller is responsible for thresholding via [`enforce_threshold`].
///
/// # Errors
///
/// * [`RunnerError::JudgeUnknownProfile`] when `args.profile` isn't registered.
/// * [`RunnerError::JudgeMissingApiKey`] when the profile needs an env key.
/// * [`RunnerError::JudgeTransport`] on HTTP transport failure.
/// * [`RunnerError::JudgeDecode`] when the model's reply can't be parsed as a score.
pub async fn run_judge(
    registry: &JudgeRegistry,
    args: &JudgeArgs,
    response: &str,
) -> Result<JudgeOutcome> {
    let profile = registry.resolve(&args.profile)?;
    let api_key = api_key_for(profile)?;
    let prompt = build_prompt(args, response);
    info!(
        profile = profile.name,
        model = profile.model,
        url = profile.base_url,
        response_len = response.len(),
        "running judge"
    );

    let client = Client::builder()
        .timeout(Duration::from_secs(u64::from(profile.timeout_s)))
        .user_agent(format!("mirroir-run/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|source| RunnerError::JudgeTransport {
            url: profile.base_url.to_owned(),
            source,
        })?;
    let req = ChatRequest {
        model: profile.model,
        messages: vec![ChatMessage {
            role: "user",
            content: prompt,
        }],
        temperature: 0.0,
    };
    let mut request = client.post(profile.base_url).json(&req);
    if let Some(key) = api_key {
        request = request.bearer_auth(key);
    }
    let response = request
        .send()
        .await
        .map_err(|source| RunnerError::JudgeTransport {
            url: profile.base_url.to_owned(),
            source,
        })?;
    let body: ChatResponse =
        response
            .json()
            .await
            .map_err(|source| RunnerError::JudgeTransport {
                url: profile.base_url.to_owned(),
                source,
            })?;
    let Some(choice) = body.choices.into_iter().next() else {
        return Err(RunnerError::JudgeDecode {
            reason: "no choices in response".to_owned(),
        });
    };
    let raw = choice.message.content;
    let score = parse_score(&raw)?;
    debug!(score, raw = %raw, "judge returned");
    Ok(JudgeOutcome { score, raw })
}

/// Apply `pass_threshold` + optional tolerance to a [`JudgeOutcome`].
///
/// # Errors
///
/// [`RunnerError::JudgeBelowThreshold`] when `score < threshold - tolerance`.
pub fn enforce_threshold(
    profile_name: &str,
    args: &JudgeArgs,
    outcome: &JudgeOutcome,
) -> Result<()> {
    let tolerance = args.pass_threshold_tolerance.unwrap_or(0.0);
    let effective = args.pass_threshold - tolerance;
    if outcome.score < effective {
        return Err(RunnerError::JudgeBelowThreshold {
            profile: profile_name.to_owned(),
            score: outcome.score,
            threshold: effective,
        });
    }
    Ok(())
}

fn api_key_for(profile: &JudgeProfile) -> Result<Option<String>> {
    let Some(env_name) = profile.api_key_env else {
        return Ok(None);
    };
    match env::var(env_name) {
        Ok(value) if !value.is_empty() => Ok(Some(value)),
        _ => Err(RunnerError::JudgeMissingApiKey {
            profile: profile.name.to_owned(),
            env_var: env_name.to_owned(),
        }),
    }
}

fn build_prompt(args: &JudgeArgs, response: &str) -> String {
    let expected = args
        .expected_signal
        .as_deref()
        .unwrap_or("the response satisfies the scenario's success criteria with high fidelity");
    format!(
        "You are a deterministic test oracle. Given an AI agent's response, score how well it \
         matches the expected outcome. Return ONLY a single decimal number between 0.0 (total \
         failure) and 1.0 (perfect match), with at most three decimal places. Do not include any \
         other text, justification, or punctuation.\n\n\
         Expected outcome: {expected}\n\n\
         Agent response:\n```\n{response}\n```\n\n\
         Score:"
    )
}

/// Extract a score from the model's textual reply.
///
/// Tolerates surrounding whitespace, a trailing period, or a leading
/// "Score:" prefix. Anything that doesn't parse as a finite `[0,1]` float
/// returns [`RunnerError::JudgeDecode`].
fn parse_score(raw: &str) -> Result<f64> {
    let cleaned = raw
        .trim()
        .trim_start_matches("Score:")
        .trim_start_matches("score:")
        .trim()
        .trim_end_matches('.')
        .trim();
    let value: f64 = cleaned.parse().map_err(|_| RunnerError::JudgeDecode {
        reason: format!("could not parse `{cleaned}` as f64"),
    })?;
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(RunnerError::JudgeDecode {
            reason: format!("score {value} out of [0,1] range"),
        });
    }
    Ok(value)
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    temperature: f64,
}

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: String,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatChoiceMessage,
}

#[derive(Deserialize)]
struct ChatChoiceMessage {
    content: String,
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;
    use std::result::Result as StdResult;
    use std::sync::Arc;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::Mutex;
    use tokio::task::JoinHandle;

    use super::*;

    type TestResult = StdResult<(), Box<dyn StdError>>;

    /// Spin a stub HTTP server that returns the canned response body for the
    /// next connection. Returns `(port, JoinHandle)`.
    async fn stub_server(
        canned_json: &'static str,
    ) -> StdResult<(u16, JoinHandle<()>), Box<dyn StdError>> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            canned_json.len(),
            canned_json
        );
        let response = Arc::new(Mutex::new(response));
        let handle = tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 8192];
                let _ = sock.read(&mut buf).await;
                let resp = response.lock().await;
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.shutdown().await;
            }
        });
        Ok((port, handle))
    }

    fn judge_args(profile: &str) -> JudgeArgs {
        JudgeArgs {
            profile: profile.to_owned(),
            user_prompt_template_hash: "sha256:test".to_owned(),
            response_selector: "[data-test=reply]".to_owned(),
            pass_threshold: 0.8,
            pass_threshold_tolerance: Some(0.05),
            expected_signal: Some("response correctly summarises the topic".to_owned()),
            response_drift: None,
            response_text: None,
            response_file: None,
            drift_baseline_file: None,
        }
    }

    fn stub_profile(name: &'static str, base_url: &'static str) -> JudgeProfile {
        JudgeProfile {
            name,
            base_url,
            model: "stub",
            api_key_env: None,
            timeout_s: 5,
        }
    }

    #[tokio::test]
    async fn run_judge_extracts_score_from_response() -> TestResult {
        let canned = r#"{"choices":[{"message":{"content":"0.95"}}]}"#;
        let (port, server) = stub_server(canned).await?;
        let base_url: &'static str =
            Box::leak(format!("http://127.0.0.1:{port}/v1/chat/completions").into_boxed_str());
        let registry = JudgeRegistry::from_profiles(vec![stub_profile("stub", base_url)]);
        let outcome = run_judge(&registry, &judge_args("stub"), "Some response").await?;
        if (outcome.score - 0.95).abs() > 1e-9 {
            return Err(format!("wrong score: {}", outcome.score).into());
        }
        server.await?;
        Ok(())
    }

    #[tokio::test]
    async fn run_judge_tolerates_surrounding_whitespace_and_period() -> TestResult {
        let canned = r#"{"choices":[{"message":{"content":"  0.7.  "}}]}"#;
        let (port, server) = stub_server(canned).await?;
        let base_url: &'static str =
            Box::leak(format!("http://127.0.0.1:{port}/v1/chat/completions").into_boxed_str());
        let registry = JudgeRegistry::from_profiles(vec![stub_profile("stub", base_url)]);
        let outcome = run_judge(&registry, &judge_args("stub"), "x").await?;
        if (outcome.score - 0.7).abs() > 1e-9 {
            return Err(format!("wrong score: {}", outcome.score).into());
        }
        server.await?;
        Ok(())
    }

    #[tokio::test]
    async fn run_judge_returns_decode_error_for_bad_score() -> TestResult {
        let canned = r#"{"choices":[{"message":{"content":"definitely not a number"}}]}"#;
        let (port, server) = stub_server(canned).await?;
        let base_url: &'static str =
            Box::leak(format!("http://127.0.0.1:{port}/v1/chat/completions").into_boxed_str());
        let registry = JudgeRegistry::from_profiles(vec![stub_profile("stub", base_url)]);
        let res = run_judge(&registry, &judge_args("stub"), "x").await;
        if !matches!(res, Err(RunnerError::JudgeDecode { .. })) {
            return Err(format!("expected JudgeDecode, got {res:?}").into());
        }
        server.await?;
        Ok(())
    }

    #[tokio::test]
    async fn run_judge_unknown_profile() -> TestResult {
        let registry = JudgeRegistry::default();
        let res = run_judge(&registry, &judge_args("does-not-exist"), "x").await;
        if !matches!(res, Err(RunnerError::JudgeUnknownProfile { .. })) {
            return Err(format!("expected JudgeUnknownProfile, got {res:?}").into());
        }
        Ok(())
    }

    #[test]
    fn enforce_threshold_passes_within_tolerance() -> TestResult {
        let args = JudgeArgs {
            pass_threshold: 0.9,
            pass_threshold_tolerance: Some(0.05),
            ..judge_args("stub")
        };
        let outcome = JudgeOutcome {
            score: 0.86,
            raw: "0.86".to_owned(),
        };
        enforce_threshold("stub", &args, &outcome)?;
        Ok(())
    }

    #[test]
    fn enforce_threshold_fails_below_tolerance() -> TestResult {
        let args = JudgeArgs {
            pass_threshold: 0.9,
            pass_threshold_tolerance: Some(0.05),
            ..judge_args("stub")
        };
        let outcome = JudgeOutcome {
            score: 0.5,
            raw: "0.5".to_owned(),
        };
        let res = enforce_threshold("stub", &args, &outcome);
        let Err(RunnerError::JudgeBelowThreshold {
            score, threshold, ..
        }) = res
        else {
            return Err(format!("expected JudgeBelowThreshold, got {res:?}").into());
        };
        if (score - 0.5).abs() > 1e-9 || (threshold - 0.85).abs() > 1e-9 {
            return Err(format!("wrong values: score={score} threshold={threshold}").into());
        }
        Ok(())
    }

    #[test]
    fn parse_score_rejects_out_of_range() -> TestResult {
        let res = parse_score("1.5");
        if !matches!(res, Err(RunnerError::JudgeDecode { .. })) {
            return Err(format!("expected JudgeDecode, got {res:?}").into());
        }
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
