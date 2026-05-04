//! Per-worker configuration types.
//!
//! [`WorkerSpec`] is the resolved, per-worker view of `{cli, model, effort}`.
//! It is produced by the cascade resolver in `cas-factory` and consumed at
//! spawn time by `Mux::factory`.

use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::SupervisorCli;

/// Reasoning effort level, shared across backends.
///
/// - Claude: passed as `--effort <level>` (see [`Effort::as_claude_arg`]).
/// - Codex: passed as `--config model_reasoning_effort=<level>` (see
///   [`Effort::as_codex_config`]).
///
/// Both backends accept the same vocabulary: `minimal|low|medium|high|xhigh`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    Minimal,
    Low,
    Medium,
    High,
    /// Extra-high; serialised/parsed as `"xhigh"`.
    #[serde(rename = "xhigh")]
    XHigh,
}

impl Effort {
    /// The string passed to Claude's `--effort` flag.
    pub fn as_claude_arg(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
        }
    }

    /// The value half of Codex's `--config model_reasoning_effort=<v>`.
    pub fn as_codex_config(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
        }
    }
}

impl FromStr for Effort {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "minimal" => Ok(Self::Minimal),
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "xhigh" | "x-high" => Ok(Self::XHigh),
            other => Err(format!(
                "unsupported effort level {other:?}; expected one of minimal|low|medium|high|xhigh"
            )),
        }
    }
}

impl std::fmt::Display for Effort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_claude_arg())
    }
}

/// Resolved per-worker configuration.
///
/// Produced by `cas_factory::spec_resolver::resolve_specs` after applying the
/// 5-layer config cascade, and consumed at spawn time.
///
/// `None` in any field means "use the backend's own default", not "still
/// needs resolution".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerSpec {
    /// Optional name for this worker slot (e.g. `"alice"`).
    /// `None` means the factory assigns a generated name at spawn time.
    pub name: Option<String>,
    /// CLI backend (Claude or Codex).
    pub cli: SupervisorCli,
    /// Model name (e.g. `"claude-opus-4-5"` or `"gpt-5.5"`).
    /// `None` = use the backend's own default.
    pub model: Option<String>,
    /// Reasoning effort. `None` = use the backend's own default.
    pub effort: Option<Effort>,
}

impl WorkerSpec {
    /// Construct the built-in default spec: Claude / no model / High effort.
    pub fn builtin_default() -> Self {
        Self {
            name: None,
            cli: SupervisorCli::Claude,
            model: None,
            effort: Some(Effort::High),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effort_roundtrip_all_variants() {
        let cases = [
            (Effort::Minimal, "minimal"),
            (Effort::Low, "low"),
            (Effort::Medium, "medium"),
            (Effort::High, "high"),
            (Effort::XHigh, "xhigh"),
        ];
        for (variant, s) in cases {
            assert_eq!(variant.as_claude_arg(), s, "claude_arg mismatch for {variant:?}");
            assert_eq!(variant.as_codex_config(), s, "codex_config mismatch for {variant:?}");
            assert_eq!(
                s.parse::<Effort>().unwrap(),
                variant,
                "parse mismatch for {s:?}"
            );
            assert_eq!(variant.to_string(), s, "display mismatch for {variant:?}");
        }
    }

    #[test]
    fn effort_parse_xhigh_alias() {
        assert_eq!("x-high".parse::<Effort>().unwrap(), Effort::XHigh);
    }

    #[test]
    fn effort_parse_case_insensitive() {
        assert_eq!("HIGH".parse::<Effort>().unwrap(), Effort::High);
        assert_eq!("  Low  ".parse::<Effort>().unwrap(), Effort::Low);
    }

    #[test]
    fn effort_parse_invalid() {
        assert!("extreme".parse::<Effort>().is_err());
        assert!("".parse::<Effort>().is_err());
    }

    #[test]
    fn worker_spec_builtin_default() {
        let spec = WorkerSpec::builtin_default();
        assert_eq!(spec.cli, SupervisorCli::Claude);
        assert_eq!(spec.model, None);
        assert_eq!(spec.effort, Some(Effort::High));
        assert_eq!(spec.name, None);
    }
}
