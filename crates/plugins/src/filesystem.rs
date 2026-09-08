//! Filesystem plugin for reading/writing files

use async_trait::async_trait;
use mythos_harness_core::types::*;
use anyhow::{Context, Result};
use chrono::Utc;
use glob::Pattern;
use std::path::{Path, PathBuf};
use tokio::fs;
use tracing::{debug, info, warn};
use walkdir::WalkDir;

/// Filesystem plugin for file operations
pub struct FilesystemPlugin {
    config: FilesystemConfig,
    working_dir: PathBuf,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct FilesystemConfig {
    /// Base directory for operations (relative to working dir)
    pub base_dir: Option<String>,
    /// Allow reading files
    pub allow_read: bool,
    /// Allow writing files
    pub allow_write: bool,
    /// Allow listing directories
    pub allow_list: bool,
    /// Allow creating directories
    pub allow_mkdir: bool,
    /// Maximum file size to read (bytes)
    pub max_file_size: usize,
    /// Allowed file extensions (empty = all)
    pub allowed_extensions: Vec<String>,
    /// Denied paths (glob patterns)
    pub denied_paths: Vec<String>,
}

impl Default for FilesystemConfig {
    fn default() -> Self {
        Self {
            base_dir: None,
            allow_read: true,
            allow_write: true,
            allow_list: true,
            allow_mkdir: true,
            max_file_size: 10 * 1024 * 1024, // 10MB
            allowed_extensions: Vec::new(),
            denied_paths: vec![
                "**/.git/**".to_string(),
                "**/node_modules/**".to_string(),
                "**/target/**".to_string(),
                "**/*.secret".to_string(),
                "**/*.key".to_string(),
            ],
        }
    }
}

impl FilesystemPlugin {
    /// Create a new filesystem plugin
    pub fn new(config: FilesystemConfig, working_dir: PathBuf) -> Self {
        Self { config, working_dir }
    }

    /// Resolve a path relative to working directory
    fn resolve_path(&self, path: &str) -> Result<PathBuf> {
        let path = Path::new(path);
        let base = self.config.base_dir.as_ref()
            .map(|b| self.working_dir.join(b))
            .unwrap_or_else(|| self.working_dir.clone());
        
        let resolved = if path.is_absolute() {
            path.to_path_buf()
        } else {
            base.join(path)
        };

        // Canonicalize to prevent directory traversal
        let canonical = resolved.canonicalize()
            .context("Failed to canonicalize path")?;
        
        // Check if path is within allowed base
        let base_canonical = base.canonicalize()
            .context("Failed to canonicalize base path")?;
        
        if !canonical.starts_with(&base_canonical) {
            anyhow::bail!("Path traversal attempt detected: {}", path.display());
        }

        // Check denied patterns
        for pattern in &self.config.denied_paths {
            if Self::matches_pattern(&canonical, pattern) {
                anyhow::bail!("Path matches denied pattern: {}", pattern);
            }
        }

        Ok(canonical)
    }

    /// Check if a path matches a glob pattern
    fn matches_pattern(path: &Path, pattern: &str) -> bool {
        // Simple glob matching - in production use a proper glob library
        let path_str = path.to_string_lossy();
        let pattern = pattern.replace("**", "*");
        Pattern::new(&pattern)
            .map(|p| p.matches(&path_str))
            .unwrap_or(false)
    }

    /// Check if file extension is allowed
    fn is_extension_allowed(&self, path: &Path) -> bool {
        if self.config.allowed_extensions.is_empty() {
            return true;
        }
        
        path.extension()
            .and_then(|e| e.to_str())
            .map(|ext| self.config.allowed_extensions.iter().any(|allowed| allowed == ext))
            .unwrap_or(false)
    }
}

#[async_trait]
impl Plugin for FilesystemPlugin {
    fn name(&self) -> &str {
        "filesystem"
    }

    fn version(&self) -> &str {
        "1.0.0"
    }

    fn description(&self) -> &str {
        "File system operations plugin - read, write, list, and manage files"
    }

    fn supported_phases(&self) -> Vec<Phase> {
        vec![Phase::Interrogate, Phase::Contract, Phase::Execute, Phase::Finish]
    }

    async fn initialize(&mut self, config: &serde_json::Value) -> Result<()> {
        if let Ok(cfg) = serde_json::from_value::<FilesystemConfig>(config.clone()) {
            self.config = cfg;
        }
        info!("Filesystem plugin initialized with base dir: {:?}", self.working_dir);
        Ok(())
    }

    async fn execute(&self, phase: Phase, context: &mut PhaseContext) -> Result<PhaseResult> {
        let started_at = Utc::now();
        let start_time = std::time::Instant::now();
        
        // In a real implementation, this would be driven by the agent's requests
        // For now, we'll demonstrate the plugin capabilities
        
        let mut output = serde_json::json!({
            "capabilities": {
                "read": self.config.allow_read,
                "write": self.config.allow_write,
                "list": self.config.allow_list,
                "mkdir": self.config.allow_mkdir,
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
        info!("Filesystem plugin shutting down");
        Ok(())
    }

    fn capabilities(&self) -> PluginCapabilities {
        let mut caps = PluginCapabilities::default();
        caps.filesystem = true;
        caps
    }
}

/// Read a file
pub async fn read_file(plugin: &FilesystemPlugin, path: &str) -> Result<String> {
    if !plugin.config.allow_read {
        anyhow::bail!("File reading not allowed");
    }
    
    let resolved = plugin.resolve_path(path)?;
    
    if !resolved.exists() {
        anyhow::bail!("File not found: {}", path);
    }
    
    if !resolved.is_file() {
        anyhow::bail!("Not a file: {}", path);
    }
    
    let metadata = fs::metadata(&resolved).await?;
    if metadata.len() > plugin.config.max_file_size as u64 {
        anyhow::bail!("File too large: {} bytes (max: {})", metadata.len(), plugin.config.max_file_size);
    }
    
    if !plugin.is_extension_allowed(&resolved) {
        anyhow::bail!("File extension not allowed: {}", path);
    }
    
    let content = fs::read_to_string(&resolved).await
        .context("Failed to read file")?;
    
    Ok(content)
}

/// Write a file
pub async fn write_file(plugin: &FilesystemPlugin, path: &str, content: &str) -> Result<()> {
    if !plugin.config.allow_write {
        anyhow::bail!("File writing not allowed");
    }
    
    let resolved = plugin.resolve_path(path)?;
    
    if !plugin.is_extension_allowed(&resolved) {
        anyhow::bail!("File extension not allowed: {}", path);
    }
    
    // Create parent directories if needed
    if let Some(parent) = resolved.parent() {
        fs::create_dir_all(parent).await
            .context("Failed to create parent directories")?;
    }
    
    fs::write(&resolved, content).await
        .context("Failed to write file")?;
    
    Ok(())
}

/// List directory contents
pub async fn list_dir(plugin: &FilesystemPlugin, path: &str) -> Result<Vec<FileEntry>> {
    if !plugin.config.allow_list {
        anyhow::bail!("Directory listing not allowed");
    }
    
    let resolved = plugin.resolve_path(path)?;
    
    if !resolved.exists() {
        anyhow::bail!("Directory not found: {}", path);
    }
    
    if !resolved.is_dir() {
        anyhow::bail!("Not a directory: {}", path);
    }
    
    let mut entries = Vec::new();
    let mut dir = fs::read_dir(&resolved).await?;
    
    while let Some(entry) = dir.next_entry().await? {
        let path = entry.path();
        let metadata = entry.metadata().await?;
        
        entries.push(FileEntry {
            name: entry.file_name().to_string_lossy().to_string(),
            path: path.to_string_lossy().to_string(),
            is_dir: metadata.is_dir(),
            is_file: metadata.is_file(),
            size: metadata.len(),
            modified: metadata.modified().ok(),
        });
    }
    
    entries.sort_by(|a, b| {
        // Directories first, then alphabetical
        match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.cmp(&b.name),
        }
    });
    
    Ok(entries)
}

/// Create directory
pub async fn create_dir(plugin: &FilesystemPlugin, path: &str) -> Result<()> {
    if !plugin.config.allow_mkdir {
        anyhow::bail!("Directory creation not allowed");
    }
    
    let resolved = plugin.resolve_path(path)?;
    fs::create_dir_all(&resolved).await
        .context("Failed to create directory")?;
    
    Ok(())
}

/// File entry for directory listings
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub is_file: bool,
    pub size: u64,
    pub modified: Option<std::time::SystemTime>,
}