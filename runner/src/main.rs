// ABOUTME: Entry point for the mirroir-run binary — parses CLI args and logs them.
// ABOUTME: Module wiring lands per the build sequence; this commit ships the arg surface only.

use anyhow::Result;
use clap::{Parser, ValueEnum};
use std::path::PathBuf;
use tracing::info;

/// Cross-platform replayer for mirroir SkillStep YAML scenarios.
#[derive(Parser, Debug)]
#[command(name = "mirroir-run", version, about, long_about = None)]
struct Cli {
    /// Path to the sample directory containing SAMPLE.md (Atmosphere) or SURFACE.md (Dravr).
    #[arg(long)]
    sample: PathBuf,

    /// Scenario set to run (resolved against SAMPLE.md `session.scenarios.<set>`).
    #[arg(long, value_enum, default_value_t = ScenarioSet::MustPass)]
    scenarios: ScenarioSet,

    /// Path to the JSON report artifact.
    #[arg(long, default_value = "mirroir-run-report.json")]
    report: PathBuf,

    /// Path to the mirroir-skills checkout (for scenarios/, oracles/, drift-defaults.yaml).
    #[arg(long, env = "MIRROIR_SKILLS")]
    skills: Option<PathBuf>,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum ScenarioSet {
    MustPass,
    NiceToPass,
    All,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    info!(
        sample = ?cli.sample,
        scenarios = ?cli.scenarios,
        report = ?cli.report,
        skills = ?cli.skills,
        "mirroir-run starting"
    );

    Ok(())
}
