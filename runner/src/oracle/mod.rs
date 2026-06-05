// ABOUTME: Oracle module — drift detection, LLM judge scoring, and the judge profile registry.
// ABOUTME: Both consume `judge:` step args + scenario response text to produce verdicts.

pub mod drift;
pub mod judge;
pub mod judge_profiles;
