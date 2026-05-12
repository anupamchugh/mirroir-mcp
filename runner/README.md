# mirroir-run

Cross-platform replayer for [mirroir](https://mirroir.dev) `SkillStep` YAML
scenarios. Reads scenarios authored against mirroir's step grammar and drives
web (Chrome via CDP, Firefox / WebKit via WebDriver), generic process, and
HTTP targets — for Linux + macOS CI without macOS-only AppKit dependencies.

**Status: scaffold.** The design is locked. This commit ships the CLI surface
(arg parsing, logging) only. Each implementation module — parser, runner,
target backends, oracle — lands in a subsequent commit per the build sequence
in the design gist. mirroir's AGENTS.md rule against placeholder code is
honored: modules exist in the codebase only when they carry real implementation.

## Design

Two secret design gists hold the locked specification:

- [Complete planned solution](https://gist.github.com/jfarcand/e4cc69eeddde2ec4988aa20104566c17)
  — every artifact, every browser, every Atmosphere sample, every CLI command,
  every Dravr surface; canonical SAMPLE.md per archetype; full step grammar;
  drift threshold ownership; build sequence.
- [Brainstorm history](https://gist.github.com/jfarcand/7c30b04801ecfb6ba59c6ca1f62506f7)
  — how we got here (Rust vs. Swift, the gas-station/engine split, the
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

## Usage (target shape — not yet implemented)

```bash
mirroir-run --sample atmosphere/samples/spring-boot-ai-chat \
            --scenarios must_pass \
            --report /tmp/run.json
```

The CLI surface above parses and validates today. The actual replay path lands
in subsequent commits: parser → process target → http target → first runnable
scenario → web/chrome target → oracle → drift detection → session-scoped boot →
firefox + webkit targets → atmosphere sample expansion → CLI command coverage →
Dravr surfaces → cross-surface invariant runner support. See §12 of the design
gist for the suggested order.

## Relationship to Swift mirroir

This runner does **not** port mirroir's Swift code. It implements the **shared
schema** — `SkillStep` grammar verbatim + `.compiled.json` cache format —
against which both runtimes operate.

- Swift `Sources/mirroir-mcp/StepExecutor.swift` keeps running iOS / macOS targets.
- Rust `runner/src/` will add web / process / http targets for Linux CI.
- A cross-parser fixture test diffs both implementations' parsed AST against
  the same `mirroir-skills/legacy/testing/expo-go/login-flow.yaml` to catch
  drift between the two parsers.

## License

Apache-2.0 — same as the parent `iphone-mirroir-mcp` project.
