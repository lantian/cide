//! Machine-local project overrides. Missing values inherit the global launch configuration.
use crate::{ClaudeCli, ClaudeEnvVar, CodexCli, ConsoleHarness};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct OpencodeSettings {
    pub cli: OpencodeCli,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct OpencodeCli {
    pub binary: String,
    pub args: Vec<String>,
    pub env: Vec<ClaudeEnvVar>,
    pub inject: OpencodeInjections,
}

impl Default for OpencodeCli {
    fn default() -> Self {
        Self {
            binary: "opencode".into(),
            args: vec![],
            env: vec![],
            inject: OpencodeInjections::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct OpencodeInjections {
    pub events: bool,
    pub mcp_config: bool,
    pub instructions: bool,
    pub resume: bool,
    pub fork: bool,
}
impl Default for OpencodeInjections {
    fn default() -> Self {
        Self {
            events: true,
            mcp_config: true,
            instructions: true,
            resume: true,
            fork: true,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
#[ts(export)]
pub struct ProjectHarnessSettings {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub console_harness: Option<ConsoleHarness>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub claude: Option<ClaudeCli>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub codex: Option<CodexCli>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub opencode: Option<OpencodeCli>,
}

/// A field operation, so concurrent edits to different launch configurations cannot erase one another.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", tag = "field", content = "value")]
#[ts(export)]
pub enum ProjectHarnessEdit {
    ConsoleHarness(Option<ConsoleHarness>),
    Claude(Option<ClaudeCli>),
    Codex(Option<CodexCli>),
    Opencode(Option<OpencodeCli>),
}
impl ProjectHarnessSettings {
    pub fn apply(&mut self, edit: ProjectHarnessEdit) {
        match edit {
            ProjectHarnessEdit::ConsoleHarness(v) => self.console_harness = v,
            ProjectHarnessEdit::Claude(v) => self.claude = v,
            ProjectHarnessEdit::Codex(v) => self.codex = v,
            ProjectHarnessEdit::Opencode(v) => self.opencode = v,
        }
    }
    pub fn resolve(&self, global: &crate::Settings) -> crate::Settings {
        let mut settings = global.clone();
        if let Some(v) = self.console_harness {
            settings.console_harness = v;
        }
        if let Some(v) = &self.claude {
            settings.claude.cli = v.clone();
        }
        if let Some(v) = &self.codex {
            settings.codex.cli = v.clone();
        }
        if let Some(v) = &self.opencode {
            settings.opencode.cli = v.clone();
        }
        settings
    }
}
