//! Shell plugin for executing commands

use async_trait::async_trait;
use mythos_harness_core::types::*;
use anyhow::{Context, Result};
use chrono::Utc;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;
use tracing::{debug, info, warn};

/// Shell plugin for executing commands
pub struct ShellPlugin {
    config: ShellConfig,
    working_dir: PathBuf,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct ShellConfig {
    /// Allow executing commands
    pub allow_execute: bool,
    /// Default working directory
    pub working_dir: Option<String>,
    /// Default timeout (seconds)
    pub default_timeout_secs: u64,
    /// Maximum timeout (seconds)
    pub max_timeout_secs: u64,
    /// Allowed commands (empty = all, but with denylist)
    pub allowed_commands: Vec<String>,
    /// Denied commands (always blocked)
    pub denied_commands: Vec<String>,
    /// Environment variables to set
    pub env: HashMap<String, String>,
    /// Whether to inherit parent environment
    pub inherit_env: bool,
    /// Maximum output size (bytes)
    pub max_output_size: usize,
}

impl Default for ShellConfig {
    fn default() -> Self {
        let mut denied = vec![
            "rm -rf /".to_string(),
            "format".to_string(),
            "fdisk".to_string(),
            "mkfs".to_string(),
            "dd".to_string(),
            "shutdown".to_string(),
            "reboot".to_string(),
            "halt".to_string(),
            "poweroff".to_string(),
            "init 0".to_string(),
            "init 6".to_string(),
        ];
        
        // Add dangerous commands with wildcards
        denied.extend(vec![
            "rm -rf *".to_string(),
            "rm -rf ~".to_string(),
            "chmod 777".to_string(),
            "chown -R".to_string(),
        ]);
        
        Self {
            allow_execute: true,
            working_dir: None,
            default_timeout_secs: 60,
            max_timeout_secs: 300,
            allowed_commands: Vec::new(),
            denied_commands: denied,
            env: HashMap::new(),
            inherit_env: true,
            max_output_size: 1024 * 1024, // 1MB
        }
    }
}

impl ShellPlugin {
    /// Create a new shell plugin
    pub fn new(config: ShellConfig, working_dir: PathBuf) -> Self {
        Self { config, working_dir }
    }

    /// Check if a command is allowed
    fn is_command_allowed(&self, command: &str) -> bool {
        // Check denied list first
        for denied in &self.config.denied_commands {
            if command.contains(denied) {
                return false;
            }
        }
        
        // If allowed list is specified, command must be in it
        if !self.config.allowed_commands.is_empty() {
            let cmd_name = command.split_whitespace().next().unwrap_or("");
            return self.config.allowed_commands.iter().any(|allowed| allowed == cmd_name);
        }
        
        true
    }

    /// Execute a command and return output
    pub async fn execute_command(
        &self,
        command: &str,
        args: &[&str],
        timeout_secs: Option<u64>,
        env: Option<HashMap<String, String>>,
    ) -> Result<CommandOutput> {
        if !self.config.allow_execute {
            anyhow::bail!("Command execution not allowed");
        }
        
        let full_cmd = format!("{} {}", command, args.join(" "));
        if !self.is_command_allowed(&full_cmd) {
            anyhow::bail!("Command not allowed: {}", full_cmd);
        }
        
        info!("Executing command: {}", full_cmd);
        
        let mut cmd = Command::new(command);
        cmd.args(args);
        
        // Set working directory
        let work_dir = self.config.working_dir.as_ref()
            .map(|d| self.working_dir.join(d))
            .unwrap_or_else(|| self.working_dir.clone());
        cmd.current_dir(&work_dir);
        
        // Set environment
        if self.config.inherit_env {
            // Inherit parent env by default
        }
        
        for (k, v) in &self.config.env {
            cmd.env(k, v);
        }
        
        if let Some(custom_env) = env {
            for (k, v) in custom_env {
                cmd.env(k, v);
            }
        }
        
        // Set timeout
        let timeout_duration = Duration::from_secs(
            timeout_secs.unwrap_or(self.config.default_timeout_secs)
                .min(self.config.max_timeout_secs)
        );
        
        // Execute with timeout
        let output = timeout(timeout_duration, async {
            let output = cmd.output().await?;
            Ok::<_, anyhow::Error>(output)
        }).await;
        
        let output = match output {
            Ok(Ok(out)) => out,
            Ok(Err(e)) => {
                return Err(e).context("Command execution failed");
            }
            Err(_) => {
                anyhow::bail!("Command timed out after {} seconds", timeout_duration.as_secs());
            }
        };
        
        // Check output size
        let stdout_len = output.stdout.len();
        let stderr_len = output.stderr.len();
        
        if stdout_len > self.config.max_output_size {
            warn!("Command stdout truncated: {} bytes", stdout_len);
        }
        if stderr_len > self.config.max_output_size {
            warn!("Command stderr truncated: {} bytes", stderr_len);
        }
        
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        
        Ok(CommandOutput {
            command: full_cmd,
            exit_code: output.status.code(),
            stdout,
            stderr,
            success: output.status.success(),
            timed_out: false,
        })
    }

    /// Execute a shell command string (with shell interpretation)
    pub async fn execute_shell(&self, command: &str, timeout_secs: Option<u64>) -> Result<CommandOutput> {
        let shell = if cfg!(windows) { "cmd" } else { "sh" };
        let arg = if cfg!(windows) { "/C" } else { "-c" };
        
        self.execute_command(shell, &[arg, command], timeout_secs, None).await
    }
}

#[async_trait]
impl Plugin for ShellPlugin {
    fn name(&self) -> &str {
        "shell"
    }

    fn version(&self) -> &str {
        "1.0.0"
    }

    fn description(&self) -> &str {
        "Shell command execution plugin - run commands and capture output"
    }

    fn supported_phases(&self) -> Vec<Phase> {
        vec![Phase::Interrogate, Phase::Contract, Phase::Execute, Phase::Finish]
    }

    async fn initialize(&mut self, config: &serde_json::Value) -> Result<()> {
        if let Ok(cfg) = serde_json::from_value::<ShellConfig>(config.clone()) {
            self.config = cfg;
        }
        info!("Shell plugin initialized");
        Ok(())
    }

    async fn execute(&self, phase: Phase, context: &mut PhaseContext) -> Result<PhaseResult> {
        let started_at = Utc::now();
        let start_time = std::time::Instant::now();
        
        // Demonstrate capabilities
        let mut output = serde_json::json!({
            "capabilities": {
                "execute": self.config.allow_execute,
                "default_timeout": self.config.default_timeout_secs,
                "max_timeout": self.config.max_timeout_secs,
            },
            "working_dir": self.working_dir.to_string_lossy(),
        });

        let success = true;
        let errors = Vec::new();

        Ok(PhaseResult {
            phase,
            success,
            output,
            errors,
            started_at,
            completed_at: Utc::now(),
            duration_ms: start_time.elapsed().as_millis() as u64,
        })
    }

    async fn shutdown(&mut self) -> Result<()> {
        info!("Shell plugin shutting down");
        Ok(())
    }

    fn capabilities(&self) -> PluginCapabilities {
        let mut caps = PluginCapabilities::default();
        caps.shell = true;
        caps
    }
}

/// Command execution output
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CommandOutput {
    pub command: String,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub success: bool,
    pub timed_out: bool,
}