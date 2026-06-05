// ABOUTME: Per-step Playwright emission — maps each web SkillStep to its `.spec.ts` line(s).
// ABOUTME: Owns key/modifier/swipe translation and the JS string-literal encoder shared with the compiler.

use std::fmt::Write as _;

use crate::error::{Result, RunnerError};
use crate::parser::step::SkillStep;

/// Emit the Playwright statement(s) for a single web `step` into `out`.
///
/// Web-handled variants (`tap`/`type`/`wait_for`/…) emit `page`/`_by` calls;
/// every other variant is emitted as a comment so the generated spec mirrors
/// the scenario shape while the Rust dispatcher handles it around Playwright.
///
/// # Errors
///
/// * [`RunnerError::PlaywrightEncode`] when a label / text / path can't be
///   encoded as a JS string literal.
/// * [`RunnerError::Format`] for `std::fmt::Write` failure (unreachable for
///   `String` but typed for `?` propagation).
pub fn emit_step(step: &SkillStep, out: &mut String) -> Result<()> {
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

/// Encode `s` as a JSON/JS string literal (double-quoted, escapes applied).
///
/// # Errors
///
/// [`RunnerError::PlaywrightEncode`] when `serde_json` fails to encode `s`.
pub fn js_string_literal(s: &str, context: &str) -> Result<String> {
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

    #[test]
    fn js_string_literal_escapes_special_chars() -> TestResult {
        assert_eq!(js_string_literal("hello", "ctx")?, "\"hello\"");
        assert_eq!(js_string_literal("a\"b", "ctx")?, "\"a\\\"b\"");
        assert_eq!(js_string_literal("line\nfeed", "ctx")?, "\"line\\nfeed\"");
        Ok(())
    }
}
