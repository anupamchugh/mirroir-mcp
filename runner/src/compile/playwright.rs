// ABOUTME: Compile web SkillSteps into a Playwright `.spec.ts` + emit playwright.config.ts.
// ABOUTME: Non-web steps (spawn/http/judge/etc.) are emitted as comments; Rust dispatches those.

use std::fmt::Write as _;

use crate::error::{Result, RunnerError};
use crate::parser::scenario::Scenario;
use crate::parser::step::{Browser, SkillStep, TargetArgs, TargetKind};

/// Output of [`compile_scenario`].
///
/// `spec_ts` is the full TypeScript source of a Playwright spec file (one
/// `test()` per compiled scenario). `browsers` is the list of browsers the
/// emitted spec expects to be parametrized over — used by the
/// [`emit_playwright_config`] caller to drive `projects:`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaywrightSpec {
    /// Full TypeScript source body for the `.spec.ts` file.
    pub spec_ts: String,
    /// Browsers declared by the scenario's `target:` step (defaults to chrome).
    pub browsers: Vec<Browser>,
}

/// Compile one scenario into a Playwright spec body.
///
/// Requires the scenario to declare a `target: { kind: web, ... }` step. Non-web
/// `kind:` values are rejected at compile time; non-web step variants
/// (`spawn:`, `http:`, etc.) are kept in the emitted spec as comments so a
/// human reader sees the full intent — at runtime, the Rust dispatcher handles
/// them around the Playwright invocation.
///
/// # Errors
///
/// * [`RunnerError::PlaywrightUnsupported`] when the scenario has no
///   `target: { kind: web }` step, or declares a non-web kind.
/// * [`RunnerError::PlaywrightEncode`] if a label, URL, or name can't be
///   encoded as a JS string literal.
/// * [`RunnerError::Format`] for `std::fmt::Write` failure (unreachable for
///   `String` but typed for `?` propagation).
pub fn compile_scenario(scenario: &Scenario) -> Result<PlaywrightSpec> {
    let target = find_web_target(scenario)?;

    let browsers = if target.browsers.is_empty() {
        vec![Browser::Chrome]
    } else {
        target.browsers.clone()
    };

    let mut body = String::new();
    writeln!(body, "import {{ test, expect }} from '@playwright/test';")?;
    writeln!(body)?;
    writeln!(
        body,
        "// Helper: pass through raw CSS and Playwright locator-engine strings"
    )?;
    writeln!(
        body,
        "// (role=, text=, xpath=, css=, id=, data-testid=); otherwise prefer the"
    )?;
    writeln!(
        body,
        "// data-test attribute selector and fall back to visible text."
    )?;
    writeln!(body, "const _by = (page, label) => {{")?;
    writeln!(
        body,
        "  if (/^[\\[#.:>*]/.test(label)) return page.locator(label);"
    )?;
    writeln!(
        body,
        "  if (/^(role|text|xpath|css|id|data-testid)=/.test(label)) return page.locator(label);"
    )?;
    writeln!(body, "  return page")?;
    writeln!(body, "    .locator(`[data-test=\"${{label}}\"]`)")?;
    writeln!(body, "    .or(page.getByText(label, {{ exact: true }}));")?;
    writeln!(body, "}};")?;
    writeln!(body)?;

    let title = js_string_literal(&scenario.name, "scenario name")?;
    writeln!(body, "test({title}, async ({{ page }}) => {{")?;

    if let Some(url) = target.url.as_deref() {
        let lit = js_string_literal(url, "target.url")?;
        writeln!(body, "  await page.goto({lit});")?;
    }

    for step in &scenario.steps {
        emit_step(step, &mut body)?;
    }

    writeln!(body, "}});")?;

    Ok(PlaywrightSpec {
        spec_ts: body,
        browsers,
    })
}

/// Emit a Playwright config that materialises one `projects:` entry per
/// requested browser. JSON reporter is enabled so the Rust side can ingest
/// per-test verdicts in the invocation chunk.
///
/// # Errors
///
/// [`RunnerError::Format`] on `std::fmt::Write` failure (unreachable for `String`).
pub fn emit_playwright_config(browsers: &[Browser]) -> Result<String> {
    let mut s = String::new();
    writeln!(
        s,
        "import {{ defineConfig, devices }} from '@playwright/test';"
    )?;
    writeln!(s)?;
    writeln!(s, "export default defineConfig({{")?;
    writeln!(
        s,
        "  reporter: [['json', {{ outputFile: 'playwright-report.json' }}]],"
    )?;
    writeln!(s, "  projects: [")?;
    for browser in browsers {
        let (name, device) = match browser {
            Browser::Chrome => ("chromium", "Desktop Chrome"),
            Browser::Firefox => ("firefox", "Desktop Firefox"),
            Browser::Webkit => ("webkit", "Desktop Safari"),
        };
        writeln!(
            s,
            "    {{ name: '{name}', use: {{ ...devices['{device}'] }} }},"
        )?;
    }
    writeln!(s, "  ],")?;
    writeln!(s, "}});")?;
    Ok(s)
}

fn find_web_target(scenario: &Scenario) -> Result<&TargetArgs> {
    for step in &scenario.steps {
        if let SkillStep::Target(t) = step {
            return if matches!(t.kind, TargetKind::Web) {
                Ok(t)
            } else {
                Err(RunnerError::PlaywrightUnsupported {
                    reason: format!(
                        "first target has kind={:?}; only `web` compiles to Playwright",
                        t.kind
                    ),
                })
            };
        }
    }
    Err(RunnerError::PlaywrightUnsupported {
        reason: "scenario has no `target: { kind: web, ... }` step".to_owned(),
    })
}

fn emit_step(step: &SkillStep, out: &mut String) -> Result<()> {
    match step {
        SkillStep::Target(_) => {
            // Target step is consumed by compile_scenario; nothing to emit per step.
        }
        SkillStep::Tap(label) => {
            let s = js_string_literal(label, "tap label")?;
            writeln!(out, "  await _by(page, {s}).click();")?;
        }
        SkillStep::Type(text) => {
            let s = js_string_literal(text, "type text")?;
            writeln!(out, "  await page.keyboard.type({s});")?;
        }
        SkillStep::PressKey(args) => {
            let combo = playwright_key_combo(&args.key, &args.modifiers);
            let s = js_string_literal(&combo, "press_key combo")?;
            writeln!(out, "  await page.keyboard.press({s});")?;
        }
        SkillStep::Swipe(direction) => {
            let (dx, dy) = swipe_delta(direction);
            writeln!(out, "  await page.mouse.wheel({dx}, {dy});")?;
        }
        SkillStep::WaitFor(args) => {
            let s = js_string_literal(&args.label, "wait_for label")?;
            let timeout_ms = args
                .timeout_s
                .map_or(30_000_u64, |secs| u64::from(secs) * 1000);
            writeln!(
                out,
                "  await _by(page, {s}).waitFor({{ state: 'visible', timeout: {timeout_ms} }});"
            )?;
        }
        SkillStep::AssertVisible(label) => {
            let s = js_string_literal(label, "assert_visible label")?;
            writeln!(out, "  await expect(_by(page, {s})).toBeVisible();")?;
        }
        SkillStep::AssertNotVisible(label) => {
            let s = js_string_literal(label, "assert_not_visible label")?;
            writeln!(out, "  await expect(_by(page, {s})).toBeHidden();")?;
        }
        SkillStep::Screenshot(name) => {
            let path = format!("screenshots/{name}.png");
            let s = js_string_literal(&path, "screenshot path")?;
            writeln!(
                out,
                "  await page.screenshot({{ path: {s}, fullPage: true }});"
            )?;
        }
        SkillStep::OpenUrl(url) => {
            let s = js_string_literal(url, "open_url")?;
            writeln!(out, "  await page.goto({s});")?;
        }
        SkillStep::ScrollTo(args) => {
            let s = js_string_literal(&args.label, "scroll_to label")?;
            writeln!(out, "  await _by(page, {s}).scrollIntoViewIfNeeded();")?;
        }
        SkillStep::LongPress(args) => {
            let s = js_string_literal(&args.label, "long_press label")?;
            let delay = args.duration_ms.unwrap_or(1000);
            writeln!(out, "  await _by(page, {s}).click({{ delay: {delay} }});")?;
        }
        SkillStep::Drag(args) => {
            let from = js_string_literal(&args.from, "drag.from")?;
            let to = js_string_literal(&args.to, "drag.to")?;
            writeln!(out, "  await _by(page, {from}).dragTo(_by(page, {to}));")?;
        }
        SkillStep::Remember(note) => {
            // Preserve as a comment so reviewers can see the AI observation
            // intent in the generated spec.
            let s = js_string_literal(note, "remember note")?;
            writeln!(out, "  // remember: {s}")?;
        }
        // iOS-only / native-only / non-web steps: kept as a comment so the
        // generated spec mirrors the scenario shape; the Rust dispatcher
        // handles them around the Playwright invocation.
        SkillStep::Launch(_)
        | SkillStep::Home(_)
        | SkillStep::Shake(_)
        | SkillStep::ResetApp(_)
        | SkillStep::SetNetwork(_)
        | SkillStep::Measure(_)
        | SkillStep::Condition(_)
        | SkillStep::Spawn(_)
        | SkillStep::WaitPort(_)
        | SkillStep::Kill(_)
        | SkillStep::AssertLog(_)
        | SkillStep::AssertLogClean(_)
        | SkillStep::Judge(_)
        | SkillStep::Http(_)
        | SkillStep::Report(_)
        | SkillStep::CrossSurface(_) => {
            writeln!(
                out,
                "  // step (handled outside Playwright): {}",
                step_kind_label(step)
            )?;
        }
    }
    Ok(())
}

/// Build a Playwright key-press string like `"Control+Shift+KeyA"`. Mirroir
/// uses friendly names ("return", "escape", "command"); map them to the
/// equivalents Playwright's keyboard understands.
fn playwright_key_combo(key: &str, modifiers: &[String]) -> String {
    let mut parts: Vec<String> = modifiers.iter().map(|m| map_modifier(m)).collect();
    parts.push(map_key(key));
    parts.join("+")
}

fn map_modifier(name: &str) -> String {
    match name.to_lowercase().as_str() {
        "command" | "cmd" | "meta" => "Meta".to_owned(),
        "control" | "ctrl" => "Control".to_owned(),
        "shift" => "Shift".to_owned(),
        "alt" | "option" => "Alt".to_owned(),
        other => capitalize(other),
    }
}

fn map_key(name: &str) -> String {
    match name.to_lowercase().as_str() {
        "return" | "enter" => "Enter".to_owned(),
        "escape" | "esc" => "Escape".to_owned(),
        "tab" => "Tab".to_owned(),
        "backspace" | "delete" => "Backspace".to_owned(),
        "space" => "Space".to_owned(),
        "arrowup" | "up" => "ArrowUp".to_owned(),
        "arrowdown" | "down" => "ArrowDown".to_owned(),
        "arrowleft" | "left" => "ArrowLeft".to_owned(),
        "arrowright" | "right" => "ArrowRight".to_owned(),
        other => capitalize(other),
    }
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map_or_else(String::new, |c| {
        let rest: String = chars.collect();
        format!("{}{rest}", c.to_uppercase())
    })
}

fn swipe_delta(direction: &str) -> (i32, i32) {
    match direction.to_lowercase().as_str() {
        "up" => (0, -300),
        "down" => (0, 300),
        "left" => (-300, 0),
        "right" => (300, 0),
        _ => (0, 0),
    }
}

fn js_string_literal(s: &str, context: &str) -> Result<String> {
    serde_json::to_string(s).map_err(|source| RunnerError::PlaywrightEncode {
        context: context.to_owned(),
        source,
    })
}

fn step_kind_label(step: &SkillStep) -> &'static str {
    match step {
        SkillStep::Launch(_) => "launch",
        SkillStep::Home(_) => "home",
        SkillStep::Shake(_) => "shake",
        SkillStep::ResetApp(_) => "reset_app",
        SkillStep::SetNetwork(_) => "set_network",
        SkillStep::Measure(_) => "measure",
        SkillStep::Condition(_) => "condition",
        SkillStep::Spawn(_) => "spawn",
        SkillStep::WaitPort(_) => "wait_port",
        SkillStep::Kill(_) => "kill",
        SkillStep::AssertLog(_) => "assert_log",
        SkillStep::AssertLogClean(_) => "assert_log_clean",
        SkillStep::Judge(_) => "judge",
        SkillStep::Http(_) => "http",
        SkillStep::Report(_) => "report",
        SkillStep::CrossSurface(_) => "cross_surface",
        // Web-handled variants are emitted, not commented; this fn is only
        // called for the non-web branch above.
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;
    use std::result::Result as StdResult;

    use super::*;

    type TestResult = StdResult<(), Box<dyn StdError>>;

    fn compile(yaml: &str) -> StdResult<PlaywrightSpec, Box<dyn StdError>> {
        use serde_yaml::Deserializer;
        use serde_yaml::with::singleton_map_recursive;
        let de = Deserializer::from_str(yaml);
        let scenario: Scenario = singleton_map_recursive::deserialize(de)?;
        Ok(compile_scenario(&scenario)?)
    }

    fn assert_contains(haystack: &str, needle: &str) -> TestResult {
        if haystack.contains(needle) {
            Ok(())
        } else {
            Err(format!("expected `{needle}` in output, got:\n{haystack}").into())
        }
    }

    #[test]
    fn compiles_simple_web_scenario_with_tap_type_assert() -> TestResult {
        let yaml = r#"
version: 1
name: connect-and-broadcast
steps:
  - target: { kind: web, browsers: [chrome, firefox], url: "http://localhost:8081/" }
  - wait_for: "Connected"
  - tap: "prompt-input"
  - type: "hello"
  - tap: "send"
  - assert_visible: "delivered"
"#;
        let spec = compile(yaml)?;
        assert_eq!(spec.browsers, vec![Browser::Chrome, Browser::Firefox]);
        let s = &spec.spec_ts;
        assert_contains(s, "import { test, expect } from '@playwright/test';")?;
        assert_contains(s, "test(\"connect-and-broadcast\"")?;
        assert_contains(s, "await page.goto(\"http://localhost:8081/\")")?;
        assert_contains(s, "await _by(page, \"Connected\").waitFor")?;
        assert_contains(s, "await _by(page, \"prompt-input\").click();")?;
        assert_contains(s, "await page.keyboard.type(\"hello\");")?;
        assert_contains(s, "await _by(page, \"send\").click();")?;
        assert_contains(s, "await expect(_by(page, \"delivered\")).toBeVisible();")?;
        Ok(())
    }

    #[test]
    fn by_helper_passes_through_locator_engine_prefixes() -> TestResult {
        let yaml = r#"
version: 1
name: role-targeting
steps:
  - target: { kind: web, browsers: [chrome], url: "http://localhost/" }
  - tap: "role=button[name=\"Submit\"]"
  - assert_visible: "role=heading[name=\"Settings\"]"
"#;
        let spec = compile(yaml)?;
        let s = &spec.spec_ts;
        // The helper recognizes Playwright locator-engine prefixes and passes them
        // straight to page.locator (role=/text=/xpath=/css=/id=/data-testid=).
        assert_contains(
            s,
            "if (/^(role|text|xpath|css|id|data-testid)=/.test(label)) return page.locator(label);",
        )?;
        assert_contains(
            s,
            "await _by(page, \"role=button[name=\\\"Submit\\\"]\").click();",
        )?;
        Ok(())
    }

    #[test]
    fn compiles_press_key_with_modifiers() -> TestResult {
        let yaml = r#"
version: 1
name: press-key
steps:
  - target: { kind: web, browsers: [chrome], url: "http://localhost/" }
  - press_key: { key: "return", modifiers: ["command", "shift"] }
"#;
        let spec = compile(yaml)?;
        assert_contains(
            &spec.spec_ts,
            "await page.keyboard.press(\"Meta+Shift+Enter\");",
        )?;
        Ok(())
    }

    #[test]
    fn compiles_non_web_steps_as_comments() -> TestResult {
        let yaml = r#"
version: 1
name: mixed
steps:
  - target: { kind: web, browsers: [chrome], url: "http://localhost/" }
  - spawn: { id: server, command: "echo hi" }
  - http: { method: GET, url: "http://x/" }
  - judge: { profile: fast-ci, user_prompt_template_hash: "sha256:abc", response_selector: "[data-test=reply]", pass_threshold: 0.9 }
  - tap: "Send"
"#;
        let spec = compile(yaml)?;
        let s = &spec.spec_ts;
        assert_contains(s, "// step (handled outside Playwright): spawn")?;
        assert_contains(s, "// step (handled outside Playwright): http")?;
        assert_contains(s, "// step (handled outside Playwright): judge")?;
        assert_contains(s, "await _by(page, \"Send\").click();")?;
        Ok(())
    }

    #[test]
    fn missing_target_step_returns_error() -> TestResult {
        let yaml = r#"
version: 1
name: no-target
steps:
  - tap: "Send"
"#;
        let res = compile(yaml);
        let Err(boxed) = res else {
            return Err("expected PlaywrightUnsupported".into());
        };
        if !boxed
            .to_string()
            .contains("no `target: { kind: web, ... }`")
        {
            return Err(format!("wrong error: {boxed}").into());
        }
        Ok(())
    }

    #[test]
    fn non_web_target_returns_error() -> TestResult {
        let yaml = r#"
version: 1
name: ios-target
steps:
  - target: { kind: ios, app: "Expo Go" }
  - tap: "Email"
"#;
        let res = compile(yaml);
        let Err(boxed) = res else {
            return Err("expected PlaywrightUnsupported".into());
        };
        if !boxed.to_string().contains("only `web` compiles") {
            return Err(format!("wrong error: {boxed}").into());
        }
        Ok(())
    }

    #[test]
    fn emits_playwright_config_with_all_three_browsers() -> TestResult {
        let cfg = emit_playwright_config(&[Browser::Chrome, Browser::Firefox, Browser::Webkit])?;
        assert_contains(&cfg, "import { defineConfig, devices }")?;
        assert_contains(&cfg, "name: 'chromium'")?;
        assert_contains(&cfg, "name: 'firefox'")?;
        assert_contains(&cfg, "name: 'webkit'")?;
        assert_contains(&cfg, "devices['Desktop Chrome']")?;
        assert_contains(&cfg, "devices['Desktop Firefox']")?;
        assert_contains(&cfg, "devices['Desktop Safari']")?;
        assert_contains(&cfg, "playwright-report.json")?;
        Ok(())
    }

    #[test]
    fn defaults_browsers_to_chrome_when_unspecified() -> TestResult {
        let yaml = r#"
version: 1
name: defaults
steps:
  - target: { kind: web, url: "http://x/" }
"#;
        let spec = compile(yaml)?;
        assert_eq!(spec.browsers, vec![Browser::Chrome]);
        Ok(())
    }

    #[test]
    fn js_string_literal_escapes_special_chars() -> TestResult {
        assert_eq!(js_string_literal("hello", "ctx")?, "\"hello\"");
        assert_eq!(js_string_literal("a\"b", "ctx")?, "\"a\\\"b\"");
        assert_eq!(js_string_literal("line\nfeed", "ctx")?, "\"line\\nfeed\"");
        Ok(())
    }

    #[test]
    fn swipe_emits_mouse_wheel_in_each_direction() -> TestResult {
        for (dir, expected) in [
            ("up", "wheel(0, -300)"),
            ("down", "wheel(0, 300)"),
            ("left", "wheel(-300, 0)"),
            ("right", "wheel(300, 0)"),
        ] {
            let yaml = format!(
                "version: 1\nname: swipe\nsteps:\n  - target: {{ kind: web, browsers: [chrome], url: \"http://x/\" }}\n  - swipe: \"{dir}\"\n"
            );
            let spec = compile(&yaml)?;
            assert_contains(&spec.spec_ts, expected).map_err(|e| format!("dir={dir}: {e}"))?;
        }
        Ok(())
    }
}
