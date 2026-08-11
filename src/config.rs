//! User configuration (`~/.config/co-review/config.toml`).
//!
//! Everything here has a sensible default, so the file is entirely optional. It
//! exists to satisfy two explicit asks: a **configurable prompt** (default: run
//! the builtin `code-review` skill) and support for **agents other than Claude**.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Token replaced with the review prompt inside an agent command template. If a
/// command contains no such token, the prompt is appended as the final argument.
pub const PROMPT_TOKEN: &str = "{prompt}";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Name of the agent to use when `--agent` is not given.
    pub default_agent: String,
    /// Fraction of the workspace width given to the agent (left) pane. The
    /// navigator gets the rest. `0.6` means 60% agent / 40% navigator.
    pub agent_pane_ratio: f32,
    /// How to create the review checkout: a shared `git worktree` (fast, default)
    /// or a fresh `clone`.
    pub checkout_mode: CheckoutMode,
    /// Named agent definitions. Always contains at least `claude`, `codex`,
    /// `gemini`, and `cursor` unless the user overrides them.
    pub agents: BTreeMap<String, AgentConfig>,
    /// The prompt handed to the agent. `{pr}` is replaced with the PR reference
    /// (e.g. `#123`) and `{protocol}` with the path to the protocol file.
    pub prompt: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CheckoutMode {
    #[default]
    Worktree,
    Clone,
}

/// How to launch a particular agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// The Herdr agent kind, passed to `herdr agent start --kind`. Governs how
    /// Herdr tracks the agent's state. Defaults to the agent's map key.
    #[serde(default)]
    pub kind: Option<String>,
    /// The command to launch, as argv. A `{prompt}` token is substituted with the
    /// review prompt; otherwise the prompt is appended as the last argument.
    pub command: Vec<String>,
}

impl AgentConfig {
    /// Build the concrete argv for launching this agent with `prompt`.
    pub fn build_command(&self, prompt: &str) -> Vec<String> {
        let mut out = Vec::with_capacity(self.command.len() + 1);
        let mut substituted = false;
        for tok in &self.command {
            if tok.contains(PROMPT_TOKEN) {
                out.push(tok.replace(PROMPT_TOKEN, prompt));
                substituted = true;
            } else {
                out.push(tok.clone());
            }
        }
        if !substituted {
            out.push(prompt.to_string());
        }
        out
    }
}

impl Default for Config {
    fn default() -> Self {
        let mut agents = BTreeMap::new();
        // The default set of well-known coding agents. Each is launched
        // interactively with the review prompt as its opening message.
        for name in ["claude", "codex", "gemini", "cursor", "amp", "opencode"] {
            agents.insert(
                name.to_string(),
                AgentConfig {
                    kind: Some(name.to_string()),
                    command: vec![name.to_string()],
                },
            );
        }
        Config {
            default_agent: "claude".to_string(),
            agent_pane_ratio: 0.6,
            checkout_mode: CheckoutMode::Worktree,
            agents,
            prompt: crate::protocol::DEFAULT_PROMPT.to_string(),
        }
    }
}

impl Config {
    /// Load config from the standard path, falling back to defaults if the file
    /// does not exist. User-provided agents are merged over the defaults so a
    /// partial file does not wipe out the built-in agent set.
    pub fn load() -> Result<Config> {
        let path = crate::paths::config_path()?;
        Config::load_from(&path)
    }

    pub fn load_from(path: &std::path::Path) -> Result<Config> {
        if !path.is_file() {
            return Ok(Config::default());
        }
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading config {}", path.display()))?;
        let mut cfg: Config =
            toml::from_str(&text).with_context(|| format!("parsing config {}", path.display()))?;
        // Ensure the built-in agents remain available even if the user only added
        // one of their own.
        for (name, agent) in Config::default().agents {
            cfg.agents.entry(name).or_insert(agent);
        }
        Ok(cfg)
    }

    /// Look up an agent config by name.
    pub fn agent(&self, name: &str) -> Option<&AgentConfig> {
        self.agents.get(name)
    }

    /// Clamp the pane ratio into a usable range so a bad config can't produce a
    /// zero-width pane.
    pub fn clamped_ratio(&self) -> f32 {
        self.agent_pane_ratio.clamp(0.2, 0.8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_has_claude() {
        let c = Config::default();
        assert_eq!(c.default_agent, "claude");
        assert!(c.agent("claude").is_some());
        assert!(!c.prompt.is_empty());
    }

    #[test]
    fn build_command_appends_prompt_when_no_token() {
        let a = AgentConfig {
            kind: None,
            command: vec!["claude".into()],
        };
        assert_eq!(a.build_command("hello"), vec!["claude", "hello"]);
    }

    #[test]
    fn build_command_substitutes_token() {
        let a = AgentConfig {
            kind: None,
            command: vec!["myagent".into(), "--task".into(), "{prompt}".into()],
        };
        assert_eq!(
            a.build_command("do it"),
            vec!["myagent", "--task", "do it"]
        );
    }

    #[test]
    fn partial_config_keeps_builtin_agents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
default_agent = "codex"
[agents.mytool]
command = ["mytool", "run"]
"#,
        )
        .unwrap();
        let cfg = Config::load_from(&path).unwrap();
        assert_eq!(cfg.default_agent, "codex");
        assert!(cfg.agent("mytool").is_some());
        // built-in still present
        assert!(cfg.agent("claude").is_some());
    }

    #[test]
    fn ratio_is_clamped() {
        let wide = Config {
            agent_pane_ratio: 0.95,
            ..Default::default()
        };
        assert!((wide.clamped_ratio() - 0.8).abs() < f32::EPSILON);
        let narrow = Config {
            agent_pane_ratio: 0.01,
            ..Default::default()
        };
        assert!((narrow.clamped_ratio() - 0.2).abs() < f32::EPSILON);
    }
}
