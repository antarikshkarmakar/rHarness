//! Plugins crate public API

pub mod filesystem;
pub mod shell;
pub mod memory;
pub mod web_search;
pub mod code_executor;

pub use filesystem::{FilesystemPlugin, FilesystemConfig, read_file, write_file, list_dir, create_dir, FileEntry};
pub use shell::{ShellPlugin, ShellConfig, CommandOutput};
pub use memory::{MemoryPlugin, MemoryConfig, MemoryEntry};
pub use web_search::{WebSearchPlugin, WebSearchConfig, SearchProvider, SearchResult, SearchResponse};
pub use code_executor::{CodeExecutorPlugin, CodeExecutorConfig, SandboxType, ResourceLimits, CodeExecutionRequest, CodeExecutionResult};

use mythos_harness_core::types::Plugin;

/// Create all built-in plugins with default configurations
pub fn create_builtin_plugins(working_dir: std::path::PathBuf) -> Vec<Box<dyn Plugin>> {
    vec![
        Box::new(FilesystemPlugin::new(FilesystemConfig::default(), working_dir.clone())),
        Box::new(ShellPlugin::new(ShellConfig::default(), working_dir.clone())),
        Box::new(MemoryPlugin::new(MemoryConfig::default(), working_dir.clone())),
        Box::new(WebSearchPlugin::new(WebSearchConfig::default()).expect("Failed to create web search plugin")),
        Box::new(CodeExecutorPlugin::new(CodeExecutorConfig::default(), working_dir)),
    ]
}