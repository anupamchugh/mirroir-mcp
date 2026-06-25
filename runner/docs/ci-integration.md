# CI integration

How downstream consumers (e.g., Atmosphere samples, mirroir-skills test
suites) call `mirroir-run` from their own CI. The reference is
[`.github/workflows/runner.yml`](../../.github/workflows/runner.yml) in
this repository.

## Job shape

`runner.yml` defines six jobs. Four are test lanes; two are guard jobs
(`runner-deny` runs `cargo deny check`, `publish-rehearsal` runs
`cargo publish --dry-run --locked`).

```
push / PR                               nightly Sun 03:00 UTC + manual
   │                                              │
   ▼                                              ▼
┌─────────────┐ ┌─────────────┐ ┌─────────────┐ ┌──────────────────────┐
│ runner-fast │ │runner-smoke │ │  runner-e2e │ │ runner-e2e-allbrowsers│
│  ~3 min     │→│  ~30 s      │→│  ~5 min     │ │  ~12 min              │
│  fmt clippy │ │ process http│ │ Playwright  │ │ chrome+firefox+webkit │
│  test diff  │ │ cross_surf  │ │  + Ollama   │ │  + Ollama             │
└─────────────┘ └─────────────┘ └─────────────┘ └──────────────────────┘
   linux + macos    linux + macos    linux + macos    linux + macos

┌─────────────┐ ┌──────────────────────┐
│ runner-deny │ │  publish-rehearsal   │
│ cargo deny  │ │ cargo publish        │
│   check     │ │  --dry-run --locked  │
└─────────────┘ └──────────────────────┘
     linux              linux
```

Lanes downstream of `runner-fast` use the `needs:` keyword so they only
spin up runners after the fast lane is green — saves runner-minutes on
PRs that have a basic regression.

## Path filters

The workflow only triggers when files under `runner/**` or the workflow
file itself change. This keeps Swift-only commits from spending runner
time on Rust CI and vice versa.

## Caching strategy

| Cache | Key | What it stores |
|---|---|---|
| Cargo registry + git + `runner/target` | `Swatinem/rust-cache@v2`, shared-key per lane | crates.io index, downloaded crates, incremental compilation artifacts |
| Playwright browsers | `playwright-${{ runner.os }}-chromium-v1` | `~/.cache/ms-playwright` (Linux), `~/Library/Caches/ms-playwright` (macOS) |
| Ollama models | `ollama-${{ runner.os }}-qwen2.5:0.5b-v1` | `~/.ollama/models` (~400 MB for the qwen2.5:0.5b judge model) |

Bump the trailing `-v1` suffix to force a cache rotation when the
underlying tool's binary digest changes (e.g., Playwright minor-version
bump).

## Why Ollama and not OpenAI

`mirroir-run`'s `byte-stable` judge profile targets a local Ollama daemon.
CI installs Ollama on every fresh runner and uses it for judge scoring. No
secrets, no per-run cost, byte-stable across reruns at `temperature=0`.

The `fast-ci` profile (OpenAI `gpt-4o-mini`) is documented and supported
for local users who prefer hosted scoring, but never invoked in CI to keep
the workflow free of remote-LLM dependencies.

### Profile trust boundary

Built-in profiles and the user's home config
(`~/.mirroir/oracles/profiles.yaml`) are trusted: they may set a profile's
`base_url` / `api_key_env` / `model` / `timeout_s` and define new profiles.
Repo-local config (`<repo>/oracles/profiles.yaml` and
`<repo>/.mirroir/oracles/profiles.yaml`) is untrusted: it may only tune
`model` / `timeout_s` of an existing profile. A repo-local `base_url` /
`api_key_env` change or a brand-new profile is ignored with a warning, so a
checked-out repository cannot redirect judge prompts and API keys to an
attacker-controlled host.

Independently, the judge's `user_prompt_template_hash` is verified on every
run: a scenario that pins a stale hash hard-fails with
`RunnerError::JudgeTemplateMismatch`, so a changed oracle template can never
silently invalidate a scenario's reproducibility guarantee.

## Consuming the runner from another repo's CI

```yaml
- name: Install mirroir-run
  run: |
    git clone --depth 1 https://github.com/jfarcand/iphone-mirroir-mcp.git /tmp/mirroir-mcp
    cd /tmp/mirroir-mcp/runner
    cargo build --release --bin mirroir-run
    sudo cp target/release/mirroir-run /usr/local/bin/

- name: Install Playwright + Ollama (see runner-e2e in the upstream workflow)
  run: |
    # ... same install + cache pattern as runner.yml ...

- name: Drive your sample
  run: mirroir-run --sample path/to/your/sample
```

A future release will publish prebuilt binaries (musl-static Linux, dylib
macOS) under GitHub Releases so this `git clone + cargo build` step can
collapse to a `curl | tar` install.

## Exit codes

| Exit | Meaning |
|---|---|
| 0 | All scenarios in the chosen set passed |
| 1 | One or more scenarios failed; check stderr for the typed `RunnerError` |
| 64-71 | Reserved (per `sysexits.h` convention; not currently used by the runner) |

CI lanes should treat any non-zero exit as a test failure. The runner
never returns successfully with a partial pass — `must_pass` scenarios
must all be green.

### Report artifact

The `.mirroir/` pipeline writes a JSON run summary to the path given by
`--report` (default `mirroir-run-report.json`). CI can upload it as a
build artifact for post-mortem. The shape is `RunSummary`:

```json
{
  "version": 1,
  "config_path": "/abs/path/.mirroir/mirroir.yaml",
  "generated_at": "2026-06-19T03:00:00Z",
  "samples": [ /* per-sample SampleVerdict entries, in plan order */ ],
  "totals": { "samples": 3, "passed": 3, "failed": 0, "skipped": 0 }
}
```

## Verbose logging in CI

Set `RUST_LOG=debug` to see the dispatcher's per-step trace:

```yaml
- run: RUST_LOG=debug ./target/release/mirroir-run --sample samples/foo
```

Output is line-oriented `tracing-subscriber` formatted; pipe through
`grep` / `awk` to slice. The `runner-e2e` lane tails the Ollama daemon
log on failure for post-mortem.

## Matrix expansion

The current workflow runs on `[ubuntu-latest, macos-latest]`. To add
Windows (not currently supported — `nix` SIGTERM handling is unix-only):

1. Implement the non-unix `send_group_sigterm` / `send_group_sigkill`
   stubs in `runner/src/target/process_log.rs` (currently no-ops under
   `#[cfg(not(unix))]`; `process.rs` only calls them).
2. Add `windows-latest` to the matrix.
3. Adjust Ollama install (Windows uses the `.exe` installer).

Tracking the Windows port: open a GitHub issue with the `os:windows`
label when you need it.

## Sample expansion: driving many samples

This repository ships a single end-to-end fixture,
[`runner/samples/mega-sample`](../samples/mega-sample), exercised by the
`runner-e2e` lane. A downstream consumer drops a `SAMPLE.md` next to each
source it wants `mirroir-run` coverage for, then runs each from its own
CI:

```yaml
- run: mirroir-run --sample path/to/sample-a --scenarios must-pass
- run: mirroir-run --sample path/to/sample-b --scenarios must-pass
```

…or a single matrix job iterating over sample directories. See
[sample-md-format.md](sample-md-format.md) for the `SAMPLE.md` schema.

Consumer-specific sample integration lives in the consumer repo's own CI;
this runner's job is to be the binary they invoke, with stable exit codes
and structured logs.
