//! Code executor plugin for running code in sandboxes

use async_trait::async_trait;
use mythos_harness_core::types::*;
use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Code executor plugin for running code in various languages
pub struct CodeExecutorPlugin {
    config: CodeExecutorConfig,
    working_dir: PathBuf,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct CodeExecutorConfig {
    /// Allow code execution
    pub allow_execute: bool,
    /// Default timeout (seconds)
    pub default_timeout_secs: u64,
    /// Maximum timeout (seconds)
    pub max_timeout_secs: u64,
    /// Working directory for code execution
    pub working_dir: Option<String>,
    /// Enable sandboxing (if available)
    pub enable_sandbox: bool,
    /// Sandbox type (none, docker, gvisor, firejail, nsjail)
    pub sandbox_type: SandboxType,
    /// Allowed languages
    pub allowed_languages: Vec<String>,
    /// Resource limits
    pub resource_limits: ResourceLimits,
    /// Pre-installed packages per language
    pub packages: HashMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SandboxType {
    None,
    Docker,
    Gvisor,
    Firejail,
    Nsjail,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct ResourceLimits {
    /// Max memory (MB)
    pub max_memory_mb: u64,
    /// Max CPU time (seconds)
    pub max_cpu_time_secs: u64,
    /// Max processes
    pub max_processes: u32,
    /// Max file size (MB)
    pub max_file_size_mb: u64,
    /// Network access
    pub allow_network: bool,
}

impl Default for CodeExecutorConfig {
    fn default() -> Self {
        let mut packages = HashMap::new();
        packages.insert("python".to_string(), vec!["requests".to_string(), "numpy".to_string()]);
        packages.insert("node".to_string(), vec!["typescript".to_string()]);
        packages.insert("rust".to_string(), vec!["serde".to_string(), "tokio".to_string()]);
        
        Self {
            allow_execute: true,
            default_timeout_secs: 30,
            max_timeout_secs: 300,
            working_dir: Some(".code_exec".to_string()),
            enable_sandbox: false,
            sandbox_type: SandboxType::None,
            allowed_languages: vec![
                "python".to_string(),
                "node".to_string(),
                "rust".to_string(),
                "bash".to_string(),
                "go".to_string(),
            ],
            resource_limits: ResourceLimits {
                max_memory_mb: 512,
                max_cpu_time_secs: 60,
                max_processes: 50,
                max_file_size_mb: 100,
                allow_network: false,
            },
            packages,
        }
    }
}

/// Code execution request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeExecutionRequest {
    pub language: String,
    pub code: String,
    pub files: HashMap<String, String>, // filename -> content
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub timeout_secs: Option<u64>,
}

/// Code execution result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeExecutionResult {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub execution_time_ms: u64,
    pub memory_used_mb: Option<u64>,
    pub error: Option<String>,
}

impl CodeExecutorPlugin {
    /// Create a new code executor plugin
    pub fn new(config: CodeExecutorConfig, working_dir: PathBuf) -> Self {
        Self { config, working_dir }
    }

    /// Check if a language is allowed
    fn is_language_allowed(&self, language: &str) -> bool {
        self.config.allowed_languages.iter().any(|l| l.eq_ignore_ascii_case(language))
    }

    /// Execute code
    pub async fn execute_code(&self, request: CodeExecutionRequest) -> Result<CodeExecutionResult> {
        if !self.config.allow_execute {
            anyhow::bail!("Code execution not allowed");
        }
        
        if !self.is_language_allowed(&request.language) {
            anyhow::bail!("Language not allowed: {}", request.language);
        }
        
        let start_time = std::time::Instant::now();
        let timeout_secs = request.timeout_secs
            .unwrap_or(self.config.default_timeout_secs)
            .min(self.config.max_timeout_secs);
        
        // Prepare execution environment
        let exec_dir = self.config.working_dir.as_ref()
            .map(|d| self.working_dir.join(d))
            .unwrap_or_else(|| self.working_dir.join(".code_exec"));
        
        tokio::fs::create_dir_all(&exec_dir).await
            .context("Failed to create execution directory")?;
        
        // Write code files
        for (filename, content) in &request.files {
            let filepath = exec_dir.join(filename);
            tokio::fs::write(&filepath, content).await
                .context("Failed to write code file")?;
        }
        
        // Write main code file if not provided
        let main_file = match request.language.as_str() {
            "python" => "main.py",
            "node" | "javascript" | "typescript" => "main.js",
            "rust" => "main.rs",
            "go" => "main.go",
            "bash" | "sh" => "main.sh",
            _ => "main.txt",
        };
        
        if !request.files.contains_key(main_file) {
            let filepath = exec_dir.join(main_file);
            tokio::fs::write(&filepath, &request.code).await
                .context("Failed to write main code file")?;
        }
        
        // Build command based on language
        let (cmd, args) = self.build_command(&request.language, main_file, &request.args)?;
        
        info!("Executing {} code with command: {} {}", request.language, cmd, args.join(" "));
        
        // Execute with timeout
        let result = timeout(Duration::from_secs(timeout_secs), async {
            let mut command = Command::new(&cmd);
            command.args(&args);
            command.current_dir(&exec_dir);
            
            // Set environment
            for (k, v) in &request.env {
                command.env(k, v);
            }
            
            // Apply resource limits if sandboxing enabled
            if self.config.enable_sandbox {
                self.apply_sandbox_limits(&mut command).await?;
            }
            
            let output = command.output().await?;
            Ok::<_, anyhow::Error>(output)
        }).await;
        
        let execution_time_ms = start_time.elapsed().as_millis() as u64;
        
        match result {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                let stderr_for_error = stderr.clone();
                
                Ok(CodeExecutionResult {
                    success: output.status.success(),
                    stdout,
                    stderr,
                    exit_code: output.status.code(),
                    execution_time_ms,
                    memory_used_mb: None, // Would need platform-specific code
                    error: if output.status.success() { None } else { Some(stderr_for_error) },
                })
            }
            Ok(Err(e)) => {
                Ok(CodeExecutionResult {
                    success: false,
                    stdout: String::new(),
                    stderr: String::new(),
                    exit_code: None,
                    execution_time_ms,
                    memory_used_mb: None,
                    error: Some(e.to_string()),
                })
            }
            Err(_) => {
                Ok(CodeExecutionResult {
                    success: false,
                    stdout: String::new(),
                    stderr: String::new(),
                    exit_code: None,
                    execution_time_ms,
                    memory_used_mb: None,
                    error: Some(format!("Execution timed out after {} seconds", timeout_secs)),
                })
            }
        }
    }

    /// Build command for a language
    fn build_command(&self, language: &str, main_file: &str, args: &[String]) -> Result<(String, Vec<String>)> {
        match language.to_lowercase().as_str() {
            "python" => Ok(("python3".to_string(), vec![main_file.to_string()])),
            "python3" => Ok(("python3".to_string(), vec![main_file.to_string()])),
            "node" | "javascript" => Ok(("node".to_string(), vec![main_file.to_string()])),
            "typescript" => Ok(("ts-node".to_string(), vec![main_file.to_string()])),
            "rust" => {
                // For Rust, we need to compile first
                // In a real implementation, this would be more sophisticated
                Ok(("rustc".to_string(), vec![main_file.to_string(), "-o".to_string(), "main".to_string()]))
            }
            "go" => Ok(("go".to_string(), vec!["run".to_string(), main_file.to_string()])),
            "bash" | "sh" => Ok(("bash".to_string(), vec![main_file.to_string()])),
            _ => anyhow::bail!("Unsupported language: {}", language),
        }
    }

    /// Apply sandbox limits to command
    async fn apply_sandbox_limits(&self, command: &mut Command) -> Result<()> {
        match self.config.sandbox_type {
            SandboxType::None => {}
            SandboxType::Docker => {
                // Would wrap command in docker run with limits
                // This is a placeholder
            }
            SandboxType::Firejail => {
                // Would wrap with firejail
            }
            SandboxType::Nsjail => {
                // Would wrap with nsjail
            }
            SandboxType::Gvisor => {
                // Would wrap with gvisor
            }
        }
        Ok(())
    }
}

#[async_trait]
impl Plugin for CodeExecutorPlugin {
    fn name(&self) -> &str {
        "code-executor"
    }

    fn version(&self) -> &str {
        "1.0.0"
    }

    fn description(&self) -> &str {
        "Code execution plugin supporting multiple languages with optional sandboxing"
    }

    fn supported_phases(&self) -> Vec<Phase> {
        vec![Phase::Execute, Phase::Finish]
    }

    async fn initialize(&mut self, config: &serde_json::Value) -> Result<()> {
        if let Ok(cfg) = serde_json::from_value::<CodeExecutorConfig>(config.clone()) {
            self.config = cfg;
        }
        info!("Code executor plugin initialized with languages: {:?}", self.config.allowed_languages);
        Ok(())
    }

    async fn execute(&self, phase: Phase, context: &mut PhaseContext) -> Result<PhaseResult> {
        let started_at = Utc::now();
        let start_time = std::time::Instant::now();
        
        let mut output = serde_json::json!({
            "capabilities": {
                "execute": self.config.allow_execute,
                "languages": self.config.allowed_languages,
                "sandbox": format!("{:?}", self.config.sandbox_type),
                "default_timeout": self.config.default_timeout_secs,
            },
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
        info!("Code executor plugin shutting down");
        Ok(())
    }

    fn capabilities(&self) -> PluginCapabilities {
        let mut caps = PluginCapabilities::default();
        caps.code_execution = true;
        caps
    }
}