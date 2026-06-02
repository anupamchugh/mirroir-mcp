# Playwright setup

`mirroir-run` compiles scenario web steps to a Playwright `.spec.ts` and
runs them via `npx playwright test`. This document covers the one-time
installation and how `MIRROIR_PLAYWRIGHT_HOME` connects the two.

Source of truth: `runner/src/compile/invoke.rs`.

## One-time setup (local dev)

```bash
# 1. Pick a stable location for Playwright + node_modules. Anywhere works;
#    using /opt is conventional on Linux, $HOME elsewhere.
export MIRROIR_PLAYWRIGHT_HOME=$HOME/.cache/mirroir-playwright
mkdir -p "$MIRROIR_PLAYWRIGHT_HOME"
cd "$MIRROIR_PLAYWRIGHT_HOME"

# 2. Initialize a minimal Node package + install @playwright/test.
npm init -y > /dev/null
npm install --no-save @playwright/test

# 3. Install browsers. chromium is what the mega-sample needs; firefox + webkit
#    are required for cross-browser scenarios.
npx playwright install chromium                          # ~120 MB
npx playwright install firefox webkit                    # +~500 MB

# 4. Optional: persist MIRROIR_PLAYWRIGHT_HOME in your shell rc.
echo 'export MIRROIR_PLAYWRIGHT_HOME=$HOME/.cache/mirroir-playwright' >> ~/.zshrc
```

## How `MIRROIR_PLAYWRIGHT_HOME` is consumed

When the runner is about to invoke `npx playwright test`, it:

1. Creates a temporary workspace via `TempDir::new()`.
2. Writes the emitted `playwright.config.ts` + `scenario.spec.ts` into it.
3. If `MIRROIR_PLAYWRIGHT_HOME` is set and points at a directory containing
   `node_modules/`, **symlinks** that `node_modules/` into the workspace.
4. Spawns `npx playwright test --config=… scenario.spec.ts` with `cwd` set
   to the workspace.

Result: Node's module resolution from the workspace's `playwright.config.ts`
finds `@playwright/test` via the symlink. No copying, no per-run `npm install`.

If `MIRROIR_PLAYWRIGHT_HOME` is unset, the runner still attempts `npx`, but
relies on `@playwright/test` being globally resolvable. That works only when
the user has installed it globally — most setups should set
`MIRROIR_PLAYWRIGHT_HOME`.

## CI setup

The `runner-e2e` lane in `.github/workflows/runner.yml` follows the same
steps:

```yaml
env:
  MIRROIR_PLAYWRIGHT_HOME: /tmp/mirroir-pw

- name: Cache Playwright browsers
  uses: actions/cache@v4
  with:
    path: |
      ~/.cache/ms-playwright           # Linux
      ~/Library/Caches/ms-playwright   # macOS
    key: playwright-${{ runner.os }}-chromium-v1

- name: Install Playwright + chromium
  run: |
    mkdir -p "$MIRROIR_PLAYWRIGHT_HOME"
    cd "$MIRROIR_PLAYWRIGHT_HOME"
    npm init -y > /dev/null
    npm install --no-save --silent @playwright/test
    npx playwright install chromium
```

The Playwright browsers cache is keyed by OS — `actions/cache@v4` keys are
global, so a Linux entry would otherwise collide with the macOS entry.

## What the runner emits

Given this scenario:

```yaml
version: 1
name: smoke
steps:
  - target: { kind: web, browsers: [chrome, firefox, webkit], url: "http://localhost:8081/" }
  - wait_for: "Connected"
  - tap: "Send"
  - assert_visible: "delivered"
```

The runner generates:

```typescript
// playwright.config.ts
import { defineConfig, devices } from '@playwright/test';
export default defineConfig({
  reporter: [['json', { outputFile: 'playwright-report.json' }]],
  projects: [
    { name: 'chromium', use: { ...devices['Desktop Chrome'] } },
    { name: 'firefox',  use: { ...devices['Desktop Firefox'] } },
    { name: 'webkit',   use: { ...devices['Desktop Safari'] } },
  ],
});

// scenario.spec.ts
import { test, expect } from '@playwright/test';
const _by = (page, label) =>
  page.locator(`[data-test="${label}"]`).or(page.getByText(label, { exact: true }));

test("smoke", async ({ page }) => {
  await page.goto("http://localhost:8081/");
  await _by(page, "Connected").waitFor({ state: 'visible', timeout: 30000 });
  await _by(page, "Send").click();
  await expect(_by(page, "delivered")).toBeVisible();
});
```

You can preview the emitted TS for any scenario via:

```bash
mirroir-run --compile-scenario path/to/scenario.yaml
```

## Reporter ingest

`mirroir-run` reads `playwright-report.json` after invocation and aggregates:

```rust
pub struct PlaywrightVerdict {
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub flaky: usize,
}
```

A non-zero `failed` count maps to `RunnerError::PlaywrightTestFailures`.
The runner exits the scenario with that error; in `--sample` mode the
overall verdict is aggregated as `SampleScenarioFailures`.

## Skipping Playwright

For lanes that don't have Playwright installed (e.g., the `runner-smoke`
CI lane), pass `--no-playwright`:

```bash
mirroir-run --run-scenario foo.yaml --no-playwright
```

Web batches log `web step batch skipped (--no-playwright)` and the runner
continues with process / http / oracle steps.

## Selector strategy

The `_by(page, label)` helper in every emitted spec resolves a mirroir
label by trying, in order:

1. `[data-test="<label>"]` attribute selector.
2. `page.getByText(<label>, { exact: true })` — visible text exact match.

Authors who want a different strategy can override the spec emission via
`--compile-scenario` and hand-edit (the planned alternative is a config
option `selector_strategy: data-test|text|aria|css`).
