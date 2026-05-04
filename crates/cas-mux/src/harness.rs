use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Supported interactive harnesses for factory panes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SupervisorCli {
    Claude,
    Codex,
}

impl SupervisorCli {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    pub fn capabilities(self) -> HarnessCapabilities {
        match self {
            Self::Claude => HarnessCapabilities {
                supports_hooks: true,
                supports_subagents: true,
                supports_textbox_submit: true,
                tool_prefix: "mcp__cas__",
            },
            Self::Codex => HarnessCapabilities {
                supports_hooks: false,
                supports_subagents: false,
                supports_textbox_submit: false,
                tool_prefix: "mcp__cs__",
            },
        }
    }
}

impl FromStr for SupervisorCli {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "claude" => Ok(Self::Claude),
            "codex" => Ok(Self::Codex),
            _ => Err(format!("unsupported harness: {s}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HarnessCapabilities {
    pub supports_hooks: bool,
    pub supports_subagents: bool,
    pub supports_textbox_submit: bool,
    pub tool_prefix: &'static str,
}
