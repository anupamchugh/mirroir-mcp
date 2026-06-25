# Judge profile registry

The `judge:` step scores a captured response against an "expected signal"
using an LLM. Source of truth: `runner/src/oracle/judge.rs`. CI uses
**Ollama** exclusively — no remote LLM, no secret keys.

## Built-in profiles

| Name | Base URL | Model | API key env | Timeout |
|---|---|---|---|---|
| `fast-ci` | `https://api.openai.com/v1/chat/completions` | `gpt-4o-mini` | `OPENAI_API_KEY` | 30 s |
| `byte-stable` | `http://127.0.0.1:11434/v1/chat/completions` | `qwen2.5:0.5b` | *(none — local)* | 60 s |
| `cheap-local` | `http://127.0.0.1:11434/v1/chat/completions` | `qwen2.5:0.5b` | *(none — local)* | 30 s |

CI (`runner-e2e` lane) installs Ollama, starts the daemon, pulls
`qwen2.5:0.5b`, and points scenarios at the `byte-stable` profile. The
`fast-ci` profile is provided for users who want hosted scoring locally;
it is not exercised by CI to keep runs free of remote-LLM dependencies.

## Wire format

All profiles speak the **OpenAI chat-completions** API. Anthropic's API can be
adapted via a base-URL override; Ollama natively exposes a compatible endpoint
at `/v1/chat/completions`.

Request shape the runner sends:

```json
{
  "model": "qwen2.5:0.5b",
  "messages": [{"role": "user", "content": "<prompt>"}],
  "temperature": 0.0
}
```

`temperature: 0.0` keeps scoring deterministic across runs.

## Prompt template

The runner builds the prompt verbatim from this template
(`runner/src/oracle/judge.rs::build_prompt`):

```
You are a deterministic test oracle. Given an AI agent's response,
score how well it matches the expected outcome. Return ONLY a single
decimal number between 0.0 (total failure) and 1.0 (perfect match),
with at most three decimal places. Do not include any other text,
justification, or punctuation.

Expected outcome: <expected_signal | default fallback>

Agent response:
```
<response_text>
```

Score:
```

The `user_prompt_template_hash` field on the YAML `judge:` step is *for
authors* — pin the SHA-256 of your expected template + signal so future
template tweaks are visible in diffs. The runner does not verify the hash
today (planned: bump-detection that warns when the on-disk template hash
differs from the pinned value).

## Score parsing

The runner is tolerant of typical model formatting:

- leading/trailing whitespace
- a leading `Score:` / `score:` prefix
- a trailing period

Anything that doesn't parse as a finite `f64` in `[0, 1]` is rejected
with `RunnerError::JudgeDecode`. Out-of-range values (e.g., `1.5`) also
reject — protects against hallucinated scores.

## Threshold logic

```yaml
- judge:
    profile: byte-stable
    pass_threshold: 0.8
    pass_threshold_tolerance: 0.05    # optional
    ...
```

Effective threshold = `pass_threshold - tolerance`. If `pass_threshold = 0.8`
and `tolerance = 0.05`, scores ≥ 0.75 pass. The tolerance band absorbs
hosted-model stochasticity (less relevant for local Ollama at temperature=0,
but kept for consistency).

`RunnerError::JudgeBelowThreshold` is raised when `score < effective`.

## Adding a custom profile

The built-in registry covers the common cases. To add your own:

1. Edit `runner/src/oracle/judge.rs::builtin_profiles()`.
2. Add a `JudgeProfile` entry with `name`, `base_url`, `model`,
   `api_key_env` (or `None` for local), `timeout_s`.
3. Scenarios reference the new name via `judge.profile: <name>`.

For full custom-registry support (load from YAML at runtime), see the
roadmap in `runner/README.md`. For now, profiles are compiled in to keep
the trust boundary tight — no untrusted YAML can redirect judge scoring
to a different endpoint mid-run.

## Local development setup (matches CI)

```bash
# 1. Install Ollama (macOS)
brew install ollama

# 1. Install Ollama (Linux)
curl -fsSL https://ollama.com/install.sh | sh

# 2. Start the daemon
ollama serve &

# 3. Pull the CI-pinned model
ollama pull qwen2.5:0.5b

# 4. Sanity probe
curl -sf http://127.0.0.1:11434/api/version | jq .
```

Once Ollama is up, scenarios using `profile: byte-stable` (or `cheap-local`)
work without any additional flags.

## Drift detection alongside `judge:`

When the scenario also sets `response_drift` + `drift_baseline_file`, the
runner runs drift detection **after** the judge passes — it's a second
gate on top of LLM scoring. Implementation lives in
`runner/src/oracle/drift.rs` (Jaccard fingerprint + normalized Levenshtein
distance, pure functions, no I/O).

Drift verdict:

- `Match` when `levenshtein_pct ≤ max_levenshtein_pct`.
- `Drift` otherwise; the runner returns `RunnerError::DriftDetected`.

See [scenario-grammar.md](scenario-grammar.md) for the `response_drift`
field shape.
