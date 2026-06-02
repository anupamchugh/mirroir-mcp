// ABOUTME: SkillStep enum + step argument types — port of mirroir's SwiftParser grammar.
// ABOUTME: Externally tagged so YAML `- launch: "App"` deserializes via the key name as the discriminator.

use std::collections::HashMap;
use std::result::Result as StdResult;

use serde::Deserialize;

/// The grammar mirroir emits and mirroir-run replays.
///
/// Externally tagged: each step's YAML representation is a single-key map where
/// the key names the variant (`launch`, `tap`, `spawn`, `judge`, …). Forward
/// compatibility through the [`Self::Skipped`] variant is reserved for future
/// extension when the runner encounters an unknown step type at parse time.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SkillStep {
    // ── mirroir SkillStep variants (Sources/mirroir-mcp/SkillParser.swift) ──
    /// `- launch: "Expo Go"` — launch an app by name.
    Launch(String),
    /// `- tap: "Email"` — tap a labeled element.
    Tap(String),
    /// `- type: "user@example.com"` — type text into the focused element.
    Type(String),
    /// `- press_key: "return"` or `- press_key: { key, modifiers }`.
    PressKey(PressKeyArgs),
    /// `- swipe: "up"` — perform a directional swipe.
    Swipe(String),
    /// `- wait_for: "Connected"` or `- wait_for: { label, timeout_s }`.
    WaitFor(WaitForArgs),
    /// `- assert_visible: "Welcome"`.
    AssertVisible(String),
    /// `- assert_not_visible: "Error toast"`.
    AssertNotVisible(String),
    /// `- screenshot: "name"`.
    Screenshot(String),
    /// `- home: null` — return to home screen (iOS).
    Home(NullValue),
    /// `- open_url: "https://…"`.
    OpenUrl(String),
    /// `- shake: null` — trigger device shake (iOS).
    Shake(NullValue),
    /// `- scroll_to: "Label"` or `- scroll_to: { label, direction, max_scrolls }`.
    ScrollTo(ScrollToArgs),
    /// `- reset_app: "appName"`.
    ResetApp(String),
    /// `- set_network: "airplane"`.
    SetNetwork(String),
    /// `- measure: { name, action, until, max_seconds }`.
    Measure(MeasureArgs),
    /// `- long_press: "Label"` or `- long_press: { label, duration_ms }`.
    LongPress(LongPressArgs),
    /// `- drag: { from, to }`.
    Drag(DragArgs),
    /// `- target: { kind, browsers?, url?, app? }`.
    Target(TargetArgs),
    /// `- remember: "AI observation text"`.
    Remember(String),

    // ── condition: if_visible / then / else (mirroir-skills legacy YAML) ──
    /// `- condition: { if_visible, then, else? }`.
    Condition(ConditionArgs),

    // ── runner extensions (forward-compat — mirroir parses as .skipped) ──
    /// `- spawn: { id, from?: SAMPLE.md, command?, ... }` — process target.
    Spawn(SpawnArgs),
    /// `- wait_port: { port, timeout_s, expect? }` — process target.
    WaitPort(WaitPortArgs),
    /// `- kill: { id, grace_s?, cleanup? }` — process target.
    Kill(KillArgs),
    /// `- assert_log: { id, pattern, flags? }` — regex grep against captured logs.
    AssertLog(AssertLogArgs),
    /// `- assert_log_clean: { id, deny, allow }` — fail if any deny matches and no allow matches.
    AssertLogClean(AssertLogCleanArgs),
    /// `- judge: { profile, ..., pass_threshold, response_drift }` — LLM oracle (Rust post-hook).
    Judge(JudgeArgs),
    /// `- http: { method, url, expect_status?, expect_body_contains? }`.
    Http(HttpArgs),
    /// `- report: pass | fail | drift` or `{ verdict }`.
    Report(ReportArgs),
    /// `- cross_surface: { response_files: [a, b, ...], min_similarity }` —
    /// compare captured responses from multiple surfaces (web/iOS/http) and
    /// fail when pairwise Jaccard fingerprint similarity drops below
    /// `min_similarity` (default 0.7).
    CrossSurface(CrossSurfaceArgs),
}

/// Marker type for step verbs whose YAML form carries no arguments.
///
/// Accepts either `null` (the mirroir convention — `home: null`, `shake: null`)
/// or an empty mapping `{}`. Any other shape is rejected at parse time.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NullValue {}

impl<'de> Deserialize<'de> for NullValue {
    fn deserialize<D>(deserializer: D) -> StdResult<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged, deny_unknown_fields)]
        enum Repr {
            Null,
            Empty {},
        }
        let _ = Repr::deserialize(deserializer)?;
        Ok(Self {})
    }
}

/// Arguments for `press_key`. Accepts string shorthand or full record form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PressKeyArgs {
    /// The key name (e.g. `"return"`, `"escape"`, `"tab"`).
    pub key: String,
    /// Modifier names (e.g. `["command", "shift"]`). Empty when the shorthand form is used.
    pub modifiers: Vec<String>,
}

impl<'de> Deserialize<'de> for PressKeyArgs {
    fn deserialize<D>(deserializer: D) -> StdResult<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Shorthand(String),
            Full {
                key: String,
                #[serde(default)]
                modifiers: Vec<String>,
            },
        }
        Ok(match Repr::deserialize(deserializer)? {
            Repr::Shorthand(key) => Self {
                key,
                modifiers: Vec::new(),
            },
            Repr::Full { key, modifiers } => Self { key, modifiers },
        })
    }
}

/// Arguments for `wait_for`. Accepts string shorthand (label only) or full record form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaitForArgs {
    /// The text label or `data-test` identifier to wait for.
    pub label: String,
    /// Optional per-step timeout override, in seconds. `None` lets the runner pick a default.
    pub timeout_s: Option<u32>,
}

impl<'de> Deserialize<'de> for WaitForArgs {
    fn deserialize<D>(deserializer: D) -> StdResult<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Shorthand(String),
            Full {
                label: String,
                #[serde(default)]
                timeout_s: Option<u32>,
            },
        }
        Ok(match Repr::deserialize(deserializer)? {
            Repr::Shorthand(label) => Self {
                label,
                timeout_s: None,
            },
            Repr::Full { label, timeout_s } => Self { label, timeout_s },
        })
    }
}

/// Arguments for `scroll_to`. Accepts string shorthand (label only) or full record form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScrollToArgs {
    /// The text label to scroll to.
    pub label: String,
    /// Scroll direction — defaults to `"up"` per mirroir's convention.
    pub direction: String,
    /// Maximum number of scroll attempts before failing — defaults to `10`.
    pub max_scrolls: u32,
}

impl<'de> Deserialize<'de> for ScrollToArgs {
    fn deserialize<D>(deserializer: D) -> StdResult<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Shorthand(String),
            Full {
                label: String,
                #[serde(default = "default_scroll_direction")]
                direction: String,
                #[serde(default = "default_max_scrolls")]
                max_scrolls: u32,
            },
        }
        Ok(match Repr::deserialize(deserializer)? {
            Repr::Shorthand(label) => Self {
                label,
                direction: default_scroll_direction(),
                max_scrolls: default_max_scrolls(),
            },
            Repr::Full {
                label,
                direction,
                max_scrolls,
            } => Self {
                label,
                direction,
                max_scrolls,
            },
        })
    }
}

fn default_scroll_direction() -> String {
    "up".to_owned()
}

fn default_max_scrolls() -> u32 {
    10
}

/// Arguments for `measure`. `action` is a `type:value` string per mirroir's
/// `docs/tools.md` (e.g. `"tap:Label"`, `"press_key:return"`,
/// `"wait_visible:streaming-caret"`).
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MeasureArgs {
    /// Friendly name reported in the run artifact (e.g. `"first_token_latency"`).
    pub name: String,
    /// Action to perform before timing starts, in `type:value` shorthand.
    pub action: String,
    /// Label / text to wait for after the action; stops the clock.
    pub until: String,
    /// Optional ceiling — fails the step if `until` doesn't appear within this many seconds.
    #[serde(default)]
    pub max_seconds: Option<f64>,
}

/// Arguments for `long_press`. Accepts string shorthand (label only) or full record form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LongPressArgs {
    /// The label of the element to long-press.
    pub label: String,
    /// Optional press duration override, in milliseconds. `None` accepts the runner's default.
    pub duration_ms: Option<u32>,
}

impl<'de> Deserialize<'de> for LongPressArgs {
    fn deserialize<D>(deserializer: D) -> StdResult<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Shorthand(String),
            Full {
                label: String,
                #[serde(default)]
                duration_ms: Option<u32>,
            },
        }
        Ok(match Repr::deserialize(deserializer)? {
            Repr::Shorthand(label) => Self {
                label,
                duration_ms: None,
            },
            Repr::Full { label, duration_ms } => Self { label, duration_ms },
        })
    }
}

/// Arguments for `drag`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DragArgs {
    /// Label of the element to start dragging from.
    pub from: String,
    /// Label of the element to release on.
    pub to: String,
}

/// Arguments for `target` — declares the execution surface for subsequent steps.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct TargetArgs {
    /// Kind of surface — `web` (Playwright), `process` (tokio), `http` (reqwest), `ios` (mirroir-mcp).
    pub kind: TargetKind,
    /// For `kind: web`, the list of browsers to materialize as Playwright projects.
    /// Empty list defaults to `[chromium]` at compile time.
    #[serde(default)]
    pub browsers: Vec<Browser>,
    /// For `kind: web`, the starting URL.
    #[serde(default)]
    pub url: Option<String>,
    /// For `kind: ios`, the application identifier (mirroir-mcp delegates).
    #[serde(default)]
    pub app: Option<String>,
}

/// Target kinds supported by the runner.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    /// Web surface driven via Playwright.
    Web,
    /// Subprocess target (spawn / kill / log capture).
    Process,
    /// HTTP target (REST probes).
    Http,
    /// iOS surface delegated to mirroir-mcp.
    Ios,
    /// macOS app surface (mirroir-mcp existing target).
    Macos,
}

/// Browsers materialized as Playwright projects in the emitted `playwright.config.ts`.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Browser {
    /// Chromium / Chrome / Edge.
    Chrome,
    /// Mozilla Firefox.
    Firefox,
    /// Safari / `WebKit`.
    Webkit,
}

/// Arguments for `condition` — branches based on element visibility.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ConditionArgs {
    /// Label / selector to check for visibility.
    pub if_visible: String,
    /// Steps to execute when the label is visible.
    pub then: Vec<SkillStep>,
    /// Optional steps to execute when the label is not visible.
    #[serde(default, rename = "else")]
    pub else_steps: Option<Vec<SkillStep>>,
}

// ─── Runner extension args ───

/// Arguments for `spawn` — start a subprocess managed by the process target.
/// Either `from: SAMPLE.md` (read the sample's boot block) or an inline
/// `command:` is required at dispatch time; the parser accepts both shapes
/// and lets the dispatcher decide.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct SpawnArgs {
    /// Identifier for this subprocess; later `kill { id: … }` targets the same id.
    pub id: String,
    /// `SAMPLE.md` literal when the spawn config lives in the sample's boot block.
    #[serde(default)]
    pub from: Option<String>,
    /// Inline command line.
    #[serde(default)]
    pub command: Option<String>,
    /// Working directory for the command.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Environment variable overrides.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Optional ceiling on subprocess runtime, in seconds.
    #[serde(default)]
    pub timeout_s: Option<u32>,
    /// Required exit code (defaults to 0 at dispatch time).
    #[serde(default)]
    pub expect_exit: Option<i32>,
    /// When set, capture stdout into the named runner variable for `${var}` substitution.
    #[serde(default)]
    pub capture_stdout: Option<String>,
}

/// Arguments for `wait_port` — block until a TCP port reaches the expected state.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct WaitPortArgs {
    /// TCP port to probe.
    pub port: u16,
    /// Timeout in seconds before the step fails.
    pub timeout_s: u32,
    /// Whether to wait for the port to open or close. Defaults to `Open`.
    #[serde(default = "default_port_expect")]
    pub expect: PortState,
}

/// Expected port state for `wait_port`.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PortState {
    /// Port should be accepting connections.
    Open,
    /// Port should refuse connections (process is gone / not listening).
    Closed,
}

fn default_port_expect() -> PortState {
    PortState::Open
}

/// Arguments for `kill` — terminate a previously-spawned subprocess.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct KillArgs {
    /// Identifier of the subprocess to kill (matches a prior `spawn { id }`).
    pub id: String,
    /// Grace period before `SIGKILL`, in seconds. Defaults to `5`.
    #[serde(default = "default_grace_s")]
    pub grace_s: u32,
    /// Optional cleanup shell command to run after the process exits.
    #[serde(default)]
    pub cleanup: Option<String>,
}

fn default_grace_s() -> u32 {
    5
}

/// Arguments for `assert_log` — single-pattern regex check against captured logs.
///
/// The dispatcher polls the log file until the pattern appears or `timeout_s`
/// elapses (Playwright "expect-with-retry" semantics). Omitting `timeout_s`
/// accepts the dispatcher's default.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct AssertLogArgs {
    /// Subprocess id whose log file to scan.
    pub id: String,
    /// Regex pattern. Must match at least one captured line to pass.
    pub pattern: String,
    /// Optional regex flag string (e.g. `"i"`, `"m"`, `"im"`).
    #[serde(default)]
    pub flags: Option<String>,
    /// Optional auto-retry deadline in seconds. The dispatcher polls the log
    /// every ~100ms until the pattern matches or this many seconds elapse.
    /// `None` accepts the dispatcher's default (5 s).
    #[serde(default)]
    pub timeout_s: Option<u32>,
}

/// Arguments for `assert_log_clean` — fail if any `deny` matches a line and no `allow` matches the same line.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct AssertLogCleanArgs {
    /// Subprocess id whose log file to scan.
    pub id: String,
    /// Patterns that, if matched, fail the step.
    pub deny: Vec<LogPattern>,
    /// Patterns that exempt an otherwise-deny-matching line from failing the step.
    #[serde(default)]
    pub allow: Vec<LogPattern>,
}

/// A regex pattern with optional flags, used by log assertions.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct LogPattern {
    /// The regex pattern body.
    pub pattern: String,
    /// Optional flags (e.g. `"i"` for case-insensitive).
    #[serde(default)]
    pub flags: Option<String>,
}

/// Arguments for `judge` — LLM oracle step. Captured at scenario time; evaluated by the Rust post-hook.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct JudgeArgs {
    /// Judge profile name (resolves against `oracles/profiles.yaml`).
    pub profile: String,
    /// SHA-256 of the user-prompt template; pinned for reproducibility.
    pub user_prompt_template_hash: String,
    /// CSS selector that locates the response text in the captured DOM.
    pub response_selector: String,
    /// Minimum score for PASS, before tolerance.
    pub pass_threshold: f64,
    /// Tolerance band around `pass_threshold` to absorb hosted-model stochasticity.
    #[serde(default)]
    pub pass_threshold_tolerance: Option<f64>,
    /// Human-readable signal (not load-bearing — for log readability).
    #[serde(default)]
    pub expected_signal: Option<String>,
    /// Optional drift-detection configuration over the response text.
    #[serde(default)]
    pub response_drift: Option<ResponseDriftConfig>,
    /// Inline response text to judge. Mutually exclusive with `response_file`.
    /// Used when the scenario captures the response in a step preceding the
    /// `judge:` step (e.g. via an `http:` probe or an explicit capture step).
    #[serde(default)]
    pub response_text: Option<String>,
    /// Path to a file containing the response text. Useful when the response
    /// was captured by the Playwright batch and written out via JS.
    #[serde(default)]
    pub response_file: Option<String>,
    /// Path to a baseline file to compare against for drift detection.
    /// Only consulted when `response_drift` is also set.
    #[serde(default)]
    pub drift_baseline_file: Option<String>,
}

/// Configuration for response-text drift detection between runs.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ResponseDriftConfig {
    /// Maximum Levenshtein-pct delta vs. previous-green response. Above triggers DRIFT.
    pub max_levenshtein_pct: f64,
}

/// Arguments for `http` — REST probe with optional response assertions.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct HttpArgs {
    /// HTTP method.
    pub method: HttpMethod,
    /// Target URL.
    pub url: String,
    /// Optional required status code (e.g. `200`).
    #[serde(default)]
    pub expect_status: Option<u16>,
    /// Optional substrings the response body must contain.
    #[serde(default)]
    pub expect_body_contains: Vec<String>,
    /// Optional request timeout in seconds (defaults to 30 s when omitted).
    #[serde(default)]
    pub timeout_s: Option<u32>,
}

/// HTTP method for the `http` step.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    /// `GET`.
    Get,
    /// `POST`.
    Post,
    /// `PUT`.
    Put,
    /// `DELETE`.
    Delete,
    /// `HEAD`.
    Head,
    /// `PATCH`.
    Patch,
}

/// Arguments for `report` — emit a final verdict for the scenario.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportArgs {
    /// Verdict to emit.
    pub verdict: ReportVerdict,
}

/// Scenario verdicts producible by an explicit `report` step.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReportVerdict {
    /// Scenario passed.
    Pass,
    /// Scenario failed.
    Fail,
    /// Scenario passed structurally but drifted from baseline.
    Drift,
    /// Cross-surface invariant verified.
    CrossSurfacePass,
}

/// Arguments for `cross_surface` — verify pairwise equivalence of N captured
/// responses (one per surface). Surfaces are *runner-agnostic*: each entry is
/// a path to a file the surface wrote, so iOS scenarios (via `mirroir-mcp`),
/// web scenarios (via Playwright capture), or HTTP probes can all feed into
/// the same equivalence check.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct CrossSurfaceArgs {
    /// Filesystem paths whose contents are compared pairwise.
    pub response_files: Vec<String>,
    /// Minimum pairwise fingerprint similarity in `[0, 1]`. Defaults to 0.7.
    #[serde(default)]
    pub min_similarity: Option<f64>,
}

impl<'de> Deserialize<'de> for ReportArgs {
    fn deserialize<D>(deserializer: D) -> StdResult<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Shorthand(ReportVerdict),
            Full { verdict: ReportVerdict },
        }
        Ok(match Repr::deserialize(deserializer)? {
            Repr::Shorthand(v) => Self { verdict: v },
            Repr::Full { verdict } => Self { verdict },
        })
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use serde_yaml::Deserializer;
    use serde_yaml::with::singleton_map_recursive;

    use super::*;

    type TestResult = StdResult<(), Box<dyn StdError>>;

    fn parse(yaml: &str) -> StdResult<SkillStep, serde_yaml::Error> {
        singleton_map_recursive::deserialize(Deserializer::from_str(yaml))
    }

    fn fail<T>(reason: String) -> StdResult<T, Box<dyn StdError>> {
        Err(reason.into())
    }

    #[test]
    fn launch_string() -> TestResult {
        assert_eq!(
            parse("launch: \"Expo Go\"")?,
            SkillStep::Launch("Expo Go".to_owned())
        );
        Ok(())
    }

    #[test]
    fn tap_string() -> TestResult {
        assert_eq!(parse("tap: \"Email\"")?, SkillStep::Tap("Email".to_owned()));
        Ok(())
    }

    #[test]
    fn wait_for_shorthand_and_full() -> TestResult {
        let short = parse("wait_for: \"Welcome\"")?;
        let full = parse("wait_for: { label: \"Welcome\", timeout_s: 30 }")?;
        let SkillStep::WaitFor(short_args) = short else {
            return fail("expected WaitFor, got other variant".to_owned());
        };
        let SkillStep::WaitFor(full_args) = full else {
            return fail("expected WaitFor, got other variant".to_owned());
        };
        assert_eq!(short_args.label, "Welcome");
        assert!(short_args.timeout_s.is_none());
        assert_eq!(full_args.label, "Welcome");
        assert_eq!(full_args.timeout_s, Some(30));
        Ok(())
    }

    #[test]
    fn home_unit_variant_with_null() -> TestResult {
        assert!(matches!(parse("home: null")?, SkillStep::Home(_)));
        Ok(())
    }

    #[test]
    fn condition_with_then_and_else() -> TestResult {
        let yaml = r#"
condition:
  if_visible: "Invalid"
  then:
    - tap: "Sign Up"
  else:
    - wait_for: "Welcome"
"#;
        let SkillStep::Condition(c) = parse(yaml)? else {
            return fail("expected Condition variant".to_owned());
        };
        assert_eq!(c.if_visible, "Invalid");
        assert_eq!(c.then.len(), 1);
        let Some(else_steps) = c.else_steps else {
            return fail("expected else_steps to be Some".to_owned());
        };
        assert_eq!(else_steps.len(), 1);
        Ok(())
    }

    #[test]
    fn spawn_with_id_and_from() -> TestResult {
        let SkillStep::Spawn(args) = parse("spawn: { id: server, from: SAMPLE.md }")? else {
            return fail("expected Spawn variant".to_owned());
        };
        assert_eq!(args.id, "server");
        assert_eq!(args.from.as_deref(), Some("SAMPLE.md"));
        assert!(args.command.is_none());
        Ok(())
    }

    #[test]
    fn wait_port_with_defaults() -> TestResult {
        let SkillStep::WaitPort(args) = parse("wait_port: { port: 8081, timeout_s: 120 }")? else {
            return fail("expected WaitPort variant".to_owned());
        };
        assert_eq!(args.port, 8081);
        assert_eq!(args.timeout_s, 120);
        assert_eq!(args.expect, PortState::Open);
        Ok(())
    }

    #[test]
    fn judge_full_shape() -> TestResult {
        let yaml = r#"
judge:
  profile: fast-ci
  user_prompt_template_hash: "sha256:abc"
  response_selector: "[data-test=message-agent]"
  pass_threshold: 0.9
  pass_threshold_tolerance: 0.05
  response_drift:
    max_levenshtein_pct: 0.2
"#;
        let SkillStep::Judge(args) = parse(yaml)? else {
            return fail("expected Judge variant".to_owned());
        };
        assert_eq!(args.profile, "fast-ci");
        assert!((args.pass_threshold - 0.9).abs() < f64::EPSILON);
        assert_eq!(args.pass_threshold_tolerance, Some(0.05));
        assert!(args.response_drift.is_some());
        Ok(())
    }

    #[test]
    fn target_web_with_browsers() -> TestResult {
        let yaml = r#"
target:
  kind: web
  browsers: [chrome, firefox, webkit]
  url: "http://localhost:8081/"
"#;
        let SkillStep::Target(args) = parse(yaml)? else {
            return fail("expected Target variant".to_owned());
        };
        assert_eq!(args.kind, TargetKind::Web);
        assert_eq!(
            args.browsers,
            vec![Browser::Chrome, Browser::Firefox, Browser::Webkit]
        );
        assert_eq!(args.url.as_deref(), Some("http://localhost:8081/"));
        Ok(())
    }

    #[test]
    fn assert_log_clean_with_deny_and_allow() -> TestResult {
        let yaml = r#"
assert_log_clean:
  id: server
  deny:
    - { pattern: "^\\s*ERROR\\b", flags: "im" }
    - { pattern: "BeanInstantiationException" }
  allow:
    - { pattern: "Disabled retry on ERROR\\.RATE_LIMIT" }
"#;
        let SkillStep::AssertLogClean(args) = parse(yaml)? else {
            return fail("expected AssertLogClean variant".to_owned());
        };
        assert_eq!(args.id, "server");
        assert_eq!(args.deny.len(), 2);
        assert_eq!(args.allow.len(), 1);
        assert_eq!(args.deny[0].flags.as_deref(), Some("im"));
        Ok(())
    }

    #[test]
    fn http_get_with_assertions() -> TestResult {
        let yaml = r#"
http:
  method: GET
  url: "http://localhost:8081/api/transport-info"
  expect_status: 200
  expect_body_contains: ["webtransport", "websocket"]
"#;
        let SkillStep::Http(args) = parse(yaml)? else {
            return fail("expected Http variant".to_owned());
        };
        assert_eq!(args.method, HttpMethod::Get);
        assert_eq!(args.expect_status, Some(200));
        assert_eq!(
            args.expect_body_contains,
            vec!["webtransport".to_owned(), "websocket".to_owned()]
        );
        Ok(())
    }

    #[test]
    fn report_shorthand_and_full() -> TestResult {
        let short = parse("report: pass")?;
        let full = parse("report: { verdict: drift }")?;
        assert!(matches!(
            short,
            SkillStep::Report(ReportArgs {
                verdict: ReportVerdict::Pass
            })
        ));
        assert!(matches!(
            full,
            SkillStep::Report(ReportArgs {
                verdict: ReportVerdict::Drift
            })
        ));
        Ok(())
    }
}
