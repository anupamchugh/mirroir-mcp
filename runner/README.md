# mirroir-run

Cross-platform replayer for [mirroir](https://mirroir.dev) `SkillStep` YAML
scenarios. Reads scenarios authored against mirroir's step grammar and drives:

- **Web** — compiles steps to Playwright `.spec.ts` and invokes `npx playwright test` against Chromium / Firefox / WebKit.
- **Process** — spawns subprocesses (server lifecycle, CLI tests), captures logs, asserts log shape, kills cleanly.
- **HTTP** — REST probes against MCP, A2A, and other JSON-RPC endpoints.

Runs on Linux + macOS CI without macOS-only AppKit dependencies. iOS replay
stays on mirroir's existing Swift `StepExecutor` at the parent level.

## Status

Scaffold. The design is locked; implementation proceeds incrementally per the
build sequence in the design gist. Each module ships when it carries real
implementation — no placeholder code per `AGENTS.md`.

## Design

Two secret design gists hold the locked specification:

- [Complete planned solution](https://gist.github.com/jfarcand/e4cc69eeddde2ec4988aa20104566c17)
  — every artifact, every browser, every supported sample type, full step grammar,
  drift threshold ownership, build sequence.
- [Brainstorm history](https://gist.github.com/jfarcand/7c30b04801ecfb6ba59c6ca1f62506f7)
  — how we got here (Rust vs. Swift, the Playwright decision, the
  agent + chrome-devtools-mcp canonical chrome recorder).

## Build

```bash
cd runner
cargo build --release
```

Static-musl Linux binary for CI:

```bash
cargo build --release --target=x86_64-unknown-linux-musl
```

## Usage

Five modes are wired and verified end-to-end:

```bash
# 1) Validate a scenario YAML against the SkillStep grammar.
mirroir-run --validate scenarios/connect-then-broadcast.yaml

# 2) Compile a scenario's web steps to a Playwright spec + config (prints to stdout).
mirroir-run --compile-scenario scenarios/connect-then-broadcast.yaml

# 3) Run a single scenario end-to-end (process / http / web / judge / drift).
MIRROIR_PLAYWRIGHT_HOME=/path/to/playwright \
  mirroir-run --run-scenario scenarios/connect-then-broadcast.yaml

# 4) Drive a full sample (SAMPLE.md + multiple scenarios; supports boot_once).
MIRROIR_PLAYWRIGHT_HOME=/path/to/playwright \
  mirroir-run --sample samples/mega-sample --scenarios must-pass

# 5) Compute drift between two text files (Jaccard + Levenshtein).
mirroir-run --diff-text baseline.txt current.txt --levenshtein-threshold 0.2
```

The `samples/mega-sample/` reference walks every primitive in four scenarios:
cross-browser web (chromium + firefox + webkit), HTTP probe, judge + drift
against a local Ollama instance, and cross-surface equivalence. Run it with
the command above to verify the full pipeline on your machine.

## Documentation

| Topic | Doc |
|---|---|
| Scenario grammar (every `SkillStep` variant, dispatch routing) | [docs/scenario-grammar.md](docs/scenario-grammar.md) |
| `SAMPLE.md` schema (`Session`, `Boot`, `Scenarios`, `boot_once`) | [docs/sample-md-format.md](docs/sample-md-format.md) |
| Judge profile registry + Ollama / OpenAI wire format | [docs/judge-profiles.md](docs/judge-profiles.md) |
| Playwright install + `MIRROIR_PLAYWRIGHT_HOME` walkthrough | [docs/playwright-setup.md](docs/playwright-setup.md) |
| CI lanes, caching, integration into downstream repos | [docs/ci-integration.md](docs/ci-integration.md) |

## Build sequence — fully delivered (13 / 13)

| # | Milestone | Commit |
|---|-----------|--------|
| 1 | Parser (SkillStep grammar + env substitution + `--validate`) | `9101a2d` |
| 2 | Process target (`spawn/kill/wait_port/assert_log{,_clean}`) | `17ce319` |
| 3 | HTTP target (REST probe with status + body assertions) | `78b7c26` |
| 4 | First runnable CLI smoke scenarios (process + http end-to-end) | `78b7c26` |
| 5 | `SAMPLE.md` + `--sample` mode + `from: SAMPLE.md` resolution | `4ba2aa7` |
| 6 | Compile web steps → Playwright `.spec.ts` + config | `63c3a43` |
| 7 | Invoke `npx playwright test` + ingest JSON reporter | `5bedbac` |
| 8 | Oracle profile registry + drift detection + session boot | `a494206` |
| 9 | Judge `:` post-hook (OpenAI-compatible LLM client) | `6779364` |
| 10 | Cross-browser fallback (chromium + firefox + webkit, real) | verified in `a494206` |
| 11 | Sample expansion — `samples/mega-sample/` reference | `3d2c040` |
| 12 | Session-scoped boot (`session.boot_once: true`) | `a494206` |
| 13 | Cross-surface invariants (`cross_surface:` step primitive) | this commit |

Cross-surface implementation note: the runner provides the equivalence-comparison
primitive (`cross_surface:` step + pairwise Jaccard fingerprint). Surfaces feed
their captured responses to it via filesystem paths. The web side captures via
the Playwright spec calling `page.locator(...).textContent()` and writing out;
the iOS side captures via `mirroir-mcp` (Swift) writing its observed AX/OCR
output. The runner is surface-agnostic — both are just files to compare.

## Module layout

```
src/
├── main.rs           # CLI entry; clap arg parsing; dispatches to replay::
├── error.rs          # RunnerError + Result (thiserror, ~25 typed variants)
├── parser/
│   ├── env.rs        # ${VAR} / ${VAR:-default} substitution
│   ├── sample.rs     # SAMPLE.md manifest (extract_yaml_block + Session)
│   ├── scenario.rs   # Scenario top-level (singleton_map_recursive enum form)
│   └── step.rs       # SkillStep enum — 29 variants
├── replay.rs         # scenario dispatch + sample loop + web-batch buffering
├── target/
│   ├── process.rs    # tokio::process registry (spawn/kill/SIGTERM/log capture)
│   └── http.rs       # reqwest probe + status/body assertions
├── compile/
│   ├── playwright.rs # YAML web steps → .spec.ts + playwright.config.ts
│   └── invoke.rs     # spawn `npx playwright test` + parse JSON reporter
└── oracle/
    ├── drift.rs      # Fingerprint + Jaccard + Levenshtein verdict
    └── judge.rs      # OpenAI-compatible HTTP client + profile registry
```

Single crate; no internal sub-crates until an external consumer appears.

## Relationship to Swift mirroir

This runner does **not** port mirroir's Swift code. It implements the **shared
schema** — `SkillStep` grammar verbatim + `.compiled.json` cache format —
against which both runtimes operate.

- Swift `Sources/mirroir-mcp/StepExecutor.swift` keeps running iOS / macOS targets.
- Rust `runner/src/` adds web (via Playwright) / process / http targets for Linux CI.
- A cross-parser fixture test diffs both implementations' parsed AST against
  the same `mirroir-skills/legacy/testing/expo-go/login-flow.yaml` to catch
  drift between the two parsers.

## Discipline

This workspace enforces a strict Rust posture documented in the parent
[`AGENTS.md`](../AGENTS.md#rust-workspace-runner). The headlines:

- `unsafe_code = "deny"`. No FFI; any `unsafe` is a hard fail.
- Clippy `all`/`pedantic`/`nursery` at `deny`. `unwrap_used` / `expect_used` /
  `panic` denied in production code.
- `anyhow!()` macro forbidden — structured `RunnerError` everywhere; convert
  external errors via `#[from]` or `.map_err(|source| RunnerError::Variant { ... })`.
- `anyhow::Context` disallowed via `clippy.toml` `disallowed-methods`.
- Test functions return `Result<()>` and propagate via `?` — no `.unwrap()` /
  `.expect()` / `panic!()` even in test code.
- Every `.rs` file starts with two `// ABOUTME:` header lines. Max 500 lines
  per file.

`scripts/ci/pre-push-validate.sh` and `scripts/ci/architectural-validation.sh`
enforce these mechanically.

## License

Apache-2.0 — same as the parent `iphone-mirroir-mcp` project.
