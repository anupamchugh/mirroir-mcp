# `.mirroir/` Dotfile — User Reference

> Canonical architecture reference: [gist.github.com/jfarcand/a0ef5d91043851e70ceeb728553514c4](https://gist.github.com/jfarcand/a0ef5d91043851e70ceeb728553514c4)

This page is the practical guide for authoring + running `.mirroir/` in a
consumer repository. It covers the schema, lockfile workflow, archetype
references, local overrides, and CLI flags.

## TL;DR

```bash
# In your project root, create the dotfile tree:
mkdir -p .mirroir
cat > .mirroir/mirroir.yaml <<'YAML'
version: 1
plan:
  must_pass:
    - name: my-app
      archetypes: [mirroir-skills/atmosphere/ai-console@v1]
      flows: [chat-stream]
      vars: { PORT: "8080" }
      boot:
        command: "./mvnw spring-boot:run"
        boot_ready_port: 8080
YAML

# Install the archetype pack once per machine:
git clone --branch v1.0.0 https://github.com/jfarcand/mirroir-skills \
          ~/.mirroir/skills/mirroir-skills/1.0.0

# Replay (anywhere inside your project root):
mirroir-run
```

## Directory layout

```
<your-repo>/.mirroir/
├── mirroir.yaml          # plan + per-sample config (committed)
├── mirroir.local.yaml    # personal overrides (gitignored)
├── mirroir.lock          # exact archetype resolutions (committed)
├── apps/                 # local samples that don't extend an archetype
│   └── <name>/
│       ├── APP.md
│       ├── SKILL.md
│       ├── SAMPLE.md
│       └── scenarios/<flow>.yaml
├── archetypes/           # project-local archetypes (rare; `./` refs point here)
└── .build/               # composed output (gitignored, regenerated)

~/.mirroir/skills/<pack>/<version>/   # installed archetype packs
```

## Authoring `mirroir.yaml`

Two flavors of plan entry — archetype-extending and local.

### Archetype-extending entry

```yaml
- name: spring-boot-ai-chat
  archetypes: [mirroir-skills/atmosphere/ai-console@v1]
  flows: [chat-stream]
  vars:
    PORT: "8080"
  boot:
    command: "./mvnw -q spring-boot:run -pl samples/spring-boot-ai-chat"
    cwd: "${ATMOSPHERE_HOME:-.}"
    env:
      LLM_MODE: fake
      ATMOSPHERE_AUTH_ENABLED: "false"
    boot_ready_port: 8080
    boot_ready_timeout_s: 120
```

- `archetypes:` is a list (v1: exactly one entry).
- `flows:` picks which of the archetype's `provides.flows` to run.
- `vars:` fills `${VAR}` placeholders in the archetype's source files.
- `boot:` is verbatim per-instance wiring; supports `${VAR}` substitution.

### Local entry

```yaml
- name: legacy-app
  local: apps/legacy-app
  boot:
    command: "./mvnw jetty:run"
    boot_ready_port: 8080
```

Local entries reference a self-contained `.mirroir/apps/<dir>/` tree
with its own APP.md / SKILL.md / SAMPLE.md / scenarios. No archetype,
no compose-time substitution — the runner consumes the tree directly.

### Tiers

- `plan.must_pass[]` — failure blocks the suite (exit non-zero).
- `plan.nice_to_pass[]` — informational; failure does not block.

## Archetype references

Three reference forms, each resolved against a different location:

| Form | Resolves to |
|------|-------------|
| `<pack>/<name>@<version>` | `~/.mirroir/skills/<pack>/<resolved-version>/archetypes/<name>/` |
| `./<path>` | `<repo>/.mirroir/<path>/` |
| `user/<name>@<version>` | `~/.mirroir/archetypes/<name>/<resolved-version>/` |

**Bare `<name>` references are invalid** — always use one of the three
forms above to disambiguate.

Version constraints:
- `@1.2.3` — exact pin
- `@v1` or `@1` — floating major (highest installed 1.x.x)
- `@v1.0` or `@1.0` — floating minor
- `@latest` or omitted — floating any (CI should pin via lockfile)

## Lockfile

`mirroir.lock` records the exact resolution of each archetype ref at
lock time. It is **committed**.

```yaml
version: 1
generated_at: 2026-05-16T18:22:14Z
generated_by: mirroir-run 0.1.2
archetypes:
- ref: mirroir-skills/atmosphere/ai-console@v1
  resolved:
    kind: pack
    pack: mirroir-skills
    name: atmosphere/ai-console
    version: 1.0.0
    source:
      url: https://github.com/jfarcand/mirroir-skills
      tag: v1.0.0
      commit: 0f67289ab...
    checksum: sha256:c3bac34c7bb4...
```

### Modes

| Mode | When | Behavior on stale lockfile |
|------|------|----------------------------|
| Default | `mirroir-run` (no `--locked` / `--frozen`) | Auto-regenerate + log warning |
| `--locked` | CI | Exit non-zero with `MirroirLockfileStale` |
| `--frozen` | Hermetic offline CI | As `--locked` plus forbid network fetch |

### Migration from a stale lockfile

```bash
# Default mode auto-fixes:
mirroir-run                       # regenerates if stale, warns once, runs

# Explicit:
mirroir-run --compose-only        # composes everything (which regenerates lockfile in default mode)
git diff .mirroir/mirroir.lock    # inspect the changes
git add .mirroir/mirroir.lock
git commit -m "chore(mirroir): bump archetype lockfile"
```

## Local overrides (`mirroir.local.yaml`)

A gitignored sibling to `mirroir.yaml`. Merges per-entry by `name`.

```yaml
version: 1
plan:
  must_pass:
    - name: spring-boot-ai-chat
      vars: { PORT: "18080" }       # personal port reassignment
      boot:
        env: { LLM_MODE: real }     # merged into base boot.env per key
    - name: heavy-sample
      skip: true                    # exclude from local runs
```

Merge semantics:

- **Scalars** (strings, numbers, bools) → replace.
- **Maps** (`vars`, `boot.env`) → per-key merge; instance override wins per key.
- **Arrays** (`flows`, `archetypes`) → **replace**, not append.
- `skip: true` excludes from the run.

CI disables overrides via `mirroir-run --no-local`.

## `${VAR}` substitution

Substitution is **post-parse**, on **string-leaves only**. Values with
embedded `:`, `"`, newlines, or `${...}` are opaque to the YAML parser
— no Helm-class quoting footguns.

Resolution chain (highest priority first):
1. Plan entry `vars`
2. Plan entry `boot.env`
3. Suite-level `env`
4. Archetype `requires.vars[].default`
5. Process environment
6. Inline `${VAR:-fallback}`
7. Empty string

Keys are **not** substituted — `${KEY}: value` stays a literal `${KEY}`.

## CLI flags

`.mirroir/` pipeline flags:

```
mirroir-run                              # auto-discover .mirroir/, replay
mirroir-run --config <PATH>              # explicit mirroir.yaml path
mirroir-run --no-local                   # skip mirroir.local.yaml
mirroir-run --scenarios must-pass        # tier selector (config default_set when omitted)
mirroir-run --scenarios all              # include nice-to-pass
mirroir-run --compose-only               # compose without replaying
mirroir-run --recompose                  # delete .build/, recompose, replay
mirroir-run --no-compose                 # use existing .build/, no recompose
mirroir-run --locked                     # CI gate: stale lockfile is an error
mirroir-run --frozen                     # --locked + no network fetch
mirroir-run --report <PATH>              # JSON summary path (default mirroir-run-report.json)
mirroir-run --no-playwright              # skip web steps in scenarios
mirroir-run --skills <PATH>              # mirroir-skills checkout (env MIRROIR_SKILLS)
mirroir-run --levenshtein-threshold <FLOAT>  # drift threshold for --diff-text (default 0.2)
```

The pre-`.mirroir/` modes still work (`--sample`, `--validate`,
`--run-scenario`, `--compile-scenario`, `--diff-text`).

## Summary JSON

After every run, mirroir-run writes a summary to `--report`:

```json
{
  "version": 1,
  "config_path": "/path/.mirroir/mirroir.yaml",
  "generated_at": "2026-05-17T...",
  "samples": [
    { "name": "spring-boot-ai-chat", "verdict": "pass" },
    { "name": "spring-boot-broken", "verdict": "fail", "error": "..." }
  ],
  "totals": { "samples": 21, "passed": 21, "failed": 0, "skipped": 0 }
}
```

`verdict` is `pass` / `fail` / `skipped` / `composed` (the last only
appears under `--compose-only`).

## Compose pipeline + `.build/` cache

When the runner sees an archetype-extending entry, it composes the
archetype + instance into `<repo>/.mirroir/.build/<entry.name>/`:

```
.mirroir/.build/spring-boot-ai-chat/
├── SAMPLE.md                # synthesized from plan entry boot
├── APP.md                   # archetype APP.md with ${VAR} substituted
├── SKILL.md                 # archetype SKILL.md substituted
├── scenarios/
│   └── chat-stream.yaml     # archetype scenario substituted
└── .compose-manifest.json   # sha256s + plan-entry hash for cache invalidation
```

The compose layer is **content-addressed**:

- Fast-path: if every archetype source file's mtime predates the
  manifest's `composed_at`, the cache is trusted as-is.
- Slow-path: any newer mtime triggers a sha256 check. Mismatch →
  recompose.
- Plan-entry edits → recompose (the plan-entry hash is included in the
  manifest).

`.build/` is **always** gitignored. Recompose explicitly with
`mirroir-run --recompose`.

## Project-local archetypes

For organization-specific patterns that don't belong in a public pack:

```
.mirroir/archetypes/my-custom/
├── archetype.md
├── APP.md
├── SKILL.md
└── scenarios/my-flow.yaml
```

Plan entry:

```yaml
- name: my-sample
  archetypes: [./archetypes/my-custom]
  flows: [my-flow]
  boot: { command: "...", boot_ready_port: 8080 }
```

The `./` prefix routes the resolver to the project tree. Versioning is
the repo's own git history; no `@<version>` field is honored.

## Installing archetype packs

```bash
mkdir -p ~/.mirroir/skills
git clone --branch v1.0.0 https://github.com/jfarcand/mirroir-skills \
          ~/.mirroir/skills/mirroir-skills/1.0.0
```

Each installed version is a separate directory; the resolver picks the
right one per `@<version>` constraint + lockfile pin.

## Cross-surface parity (iOS ↔ web)

A `.mirroir/apps/<name>/` sample can hold **two legs of the same flow** and
assert they stay equivalent:

- **Web leg** — the runnable scenario (`scenarios/<flow>.yaml`) with real DOM
  selectors, authored against the running web app (e.g. via the `mirroir-onboard`
  flow). `mirroir-run` replays it on Linux CI.
- **iOS leg** — emitted by `mirroir-mcp`'s `generate_skill … emit=true` from an
  iPhone Mirroring capture: a `--validate`-only `scenarios/<flow>.ios.yaml`
  (a faithful linear walk; the runner has no iOS executor, so it is never
  replayed) plus the cross-surface oracle `baselines/<flow>.ios.txt`.

The two meet in a `scenarios/<flow>.parity.yaml` gate (also emitted) that compares
the iOS baseline against a web baseline by Jaccard similarity:

```yaml
version: 1
name: <flow> — cross-surface parity
steps:
  - cross_surface:
      response_files:
        - "${MIRROIR_SAMPLE_DIR}/baselines/<flow>.web.txt"
        - "${MIRROIR_SAMPLE_DIR}/baselines/<flow>.ios.txt"
      min_similarity: 0.5
```

The runner can **produce** the web baseline itself: give `cross_surface` a
`capture: { selector, to }` and it scrapes that selector's `textContent()` into
`to` during the preceding web batch (the same Playwright mechanism `judge:`
uses) — no hand-authored Playwright spec needed:

```yaml
  - target: { kind: web, url: "http://localhost:3000/" }
  # …navigate to the equivalence point…
  - cross_surface:
      capture: { selector: "main", to: "${MIRROIR_SAMPLE_DIR}/baselines/<flow>.web.txt" }
      response_files:
        - "${MIRROIR_SAMPLE_DIR}/baselines/<flow>.web.txt"
        - "${MIRROIR_SAMPLE_DIR}/baselines/<flow>.ios.txt"
      min_similarity: 0.5
```

Without `capture` the runner only **compares** pre-existing files — write
`baselines/<flow>.web.txt` yourself from the web leg. Either way, until the web
baseline exists the parity step **fails closed** — a missing file is an error,
never a silent pass.

`min_similarity` defaults to `0.5` for iOS↔web phrasing divergence; raise it for
high-entropy screens, and treat low-vocabulary screens (e.g. a bare login form)
with care — a generic token set can clear a low threshold by coincidence.

> Two surfaces, one grammar. The iOS and web legs are written in the same
> `SkillStep` language and tied by `cross_surface`, rather than maintained as two
> bespoke suites. The runner gains no iOS executor (it stays Linux-CI-friendly);
> the iOS leg is a baseline + parity anchor.

## Troubleshooting

- **`MirroirConfigNotFound`** — no `.mirroir/mirroir.yaml` in cwd or any
  ancestor. Run from inside a consumer repo, or pass `--config <PATH>`.
- **`MirroirArchetypeNotFound`** — pack not installed at the required
  version. `git clone --branch <tag>` the pack into
  `~/.mirroir/skills/<pack>/<version>/`.
- **`MirroirLockfileStale`** — `mirroir.yaml` and `mirroir.lock`
  disagree under `--locked`. In dev: `mirroir-run` (regenerates +
  warns). In CI: regenerate locally, commit the lockfile.
- **`MirroirCompositionUnsupported`** — plan entry has `archetypes:`
  with more than one element. v1 supports exactly one ref; cross-archetype
  composition is on the roadmap.
- **`MirroirInvalidArchetypeRef`** — bare name without prefix
  (`dashboard`). Use `mirroir-skills/dashboard@v1`, `./<path>`, or
  `user/dashboard@v1`.
