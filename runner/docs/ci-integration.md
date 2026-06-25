# CI integration

How downstream consumers (e.g., Atmosphere samples, mirroir-skills test
suites) call `mirroir-run` from their own CI. The reference is
[`.github/workflows/runner.yml`](../../.github/workflows/runner.yml) in
this repository.

## Four-lane shape

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

1. Implement non-unix `send_sigterm` in `runner/src/target/process.rs`
   (currently a no-op stub).
2. Add `windows-latest` to the matrix.
3. Adjust Ollama install (Windows uses the `.exe` installer).

Tracking the Windows port: open a GitHub issue with the `os:windows`
label when you need it.

## Sample expansion: driving 25 Atmosphere samples

The Atmosphere project hosts 25 samples; each that wants `mirroir-run`
coverage drops a `SAMPLE.md` next to its source. The Atmosphere
repo's CI can then run:

```yaml
- run: mirroir-run --sample samples/spring-boot-chat --scenarios must-pass
- run: mirroir-run --sample samples/spring-boot-quarkus-chat --scenarios must-pass
```

…or a single matrix job iterating over sample directories. See
[sample-md-format.md](sample-md-format.md) for the `SAMPLE.md` schema.

Atmosphere-specific sample integration lives in the Atmosphere repo's
own CI; this runner's job is to be the binary they invoke, with stable
exit codes and structured logs.
