//! Core types and traits for the Mythos Harness

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// Unique identifier for harness entities
pub type HarnessId = Uuid;

/// Phase in the finish-first loop
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    /// Interrogate - understand the task and context
    Interrogate,
    /// Contract - define the success criteria and plan
    Contract,
    /// Execute - perform the work
    Execute,
    /// Finish - verify completion and deliver results
    Finish,
}

impl Phase {
    /// Get all phases in order
    pub fn all() -> &'static [Phase] {
        &[Phase::Interrogate, Phase::Contract, Phase::Execute, Phase::Finish]
    }

    /// Get the next phase
    pub fn next(&self) -> Option<Phase> {
        match self {
            Phase::Interrogate => Some(Phase::Contract),
            Phase::Contract => Some(Phase::Execute),
            Phase::Execute => Some(Phase::Finish),
            Phase::Finish => None,
        }
    }

    /// Get the previous phase
    pub fn previous(&self) -> Option<Phase> {
        match self {
            Phase::Interrogate => None,
            Phase::Contract => Some(Phase::Interrogate),
            Phase::Execute => Some(Phase::Contract),
            Phase::Finish => Some(Phase::Execute),
        }
    }
}

/// Status of a harness run
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    /// Run is pending
    Pending,
    /// Run is in progress
    Running,
    /// Run completed successfully
    Completed,
    /// Run failed
    Failed,
    /// Run was cancelled
    Cancelled,
}

/// Result of a phase execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhaseResult {
    /// Phase that was executed
    pub phase: Phase,
    /// Whether the phase succeeded
    pub success: bool,
    /// Output from the phase
    pub output: serde_json::Value,
    /// Any errors encountered
    pub errors: Vec<String>,
    /// Start time
    pub started_at: DateTime<Utc>,
    /// End time
    pub completed_at: DateTime<Utc>,
    /// Duration in milliseconds
    pub duration_ms: u64,
}

/// Context passed between phases
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PhaseContext {
    /// Shared data between phases
    pub data: HashMap<String, serde_json::Value>,
    /// Artifacts produced
    pub artifacts: Vec<Artifact>,
    /// Current working directory
    pub working_dir: Option<String>,
    /// Environment variables
    pub env: HashMap<String, String>,
}

/// Artifact produced during execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    /// Unique identifier
    pub id: HarnessId,
    /// Artifact name
    pub name: String,
    /// Artifact type
    pub artifact_type: String,
    /// Content (if small) or path
    pub content: Option<String>,
    /// Path to artifact (if large)
    pub path: Option<String>,
    /// Metadata
    pub metadata: HashMap<String, serde_json::Value>,
    /// Creation timestamp
    pub created_at: DateTime<Utc>,
}

/// Configuration for a harness run
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessConfig {
    /// Harness name
    pub name: String,
    /// Version
    pub version: String,
    /// Tagline
    pub tagline: String,
    /// Description
    pub description: String,
    /// Loop strategy
    pub loop_strategy: LoopStrategy,
    /// Engine name
    pub engine: String,
    /// License
    pub license: String,
    /// Homepage
    pub homepage: Option<String>,
    /// Phases to execute
    pub phases: Vec<Phase>,
    /// Enabled plugins
    pub plugins: Vec<String>,
    /// Maximum iterations per phase
    pub max_iterations: u32,
    /// Timeout per phase (seconds)
    pub phase_timeout_secs: u64,
    /// Global timeout (seconds)
    pub global_timeout_secs: u64,
}

/// Loop execution strategy
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LoopStrategy {
    /// Finish-first: iterate until success criteria met
    FinishFirst,
    /// Single pass through all phases
    SinglePass,
    /// Fixed number of iterations
    FixedIterations(u32),
}

/// Trait for plugins that can participate in phases
#[async_trait]
pub trait Plugin: Send + Sync {
    /// Plugin name
    fn name(&self) -> &str;

    /// Plugin version
    fn version(&self) -> &str;

    /// Plugin description
    fn description(&self) -> &str;

    /// Phases this plugin can participate in
    fn supported_phases(&self) -> Vec<Phase>;

    /// Initialize the plugin
    async fn initialize(&mut self, config: &serde_json::Value) -> anyhow::Result<()>;

    /// Execute during a specific phase
    async fn execute(&self, phase: Phase, context: &mut PhaseContext) -> anyhow::Result<PhaseResult>;

    /// Cleanup resources
    async fn shutdown(&mut self) -> anyhow::Result<()>;

    /// Get plugin capabilities
    fn capabilities(&self) -> PluginCapabilities {
        PluginCapabilities::default()
    }
}

/// Plugin capabilities
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginCapabilities {
    /// Can search the web
    pub web_search: bool,
    /// Can access filesystem
    pub filesystem: bool,
    /// Can execute shell commands
    pub shell: bool,
    /// Can store/retrieve memory
    pub memory: bool,
    /// Can execute code
    pub code_execution: bool,
    /// Custom capabilities
    pub custom: HashMap<String, bool>,
}

/// Trait for agents that drive the harness
#[async_trait]
pub trait Agent: Send + Sync {
    /// Agent name
    fn name(&self) -> &str;

    /// Agent version
    fn version(&self) -> &str;

    /// Run the agent for a single phase
    async fn run_phase(&self, phase: Phase, context: &mut PhaseContext) -> anyhow::Result<PhaseResult>;

    /// Determine if the agent should continue to the next phase
    fn should_continue(&self, result: &PhaseResult) -> bool;

    /// Get the success criteria for finishing
    fn success_criteria(&self) -> SuccessCriteria;
}

/// Success criteria for finish-first loop
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuccessCriteria {
    /// Minimum confidence score (0.0 to 1.0)
    pub min_confidence: f64,
    /// Required artifacts
    pub required_artifacts: Vec<String>,
    /// Custom validation function name
    pub validator: Option<String>,
    /// Maximum retries
    pub max_retries: u32,
}

impl Default for SuccessCriteria {
    fn default() -> Self {
        Self {
            min_confidence: 0.8,
            required_artifacts: Vec::new(),
            validator: None,
            max_retries: 3,
        }
    }
}

/// Event emitted during harness execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessEvent {
    /// Event ID
    pub id: HarnessId,
    /// Event type
    pub event_type: EventType,
    /// Phase when event occurred
    pub phase: Phase,
    /// Timestamp
    pub timestamp: DateTime<Utc>,
    /// Event payload
    pub payload: serde_json::Value,
}

/// Types of harness events
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum EventType {
    /// Phase started
    PhaseStarted,
    /// Phase completed
    PhaseCompleted,
    /// Phase failed
    PhaseFailed,
    /// Plugin invoked
    PluginInvoked { plugin: String },
    /// Artifact created
    ArtifactCreated { artifact_id: HarnessId },
    /// Agent decision
    AgentDecision { decision: String },
    /// Error occurred
    Error { message: String },
    /// Custom event
    Custom { name: String },
}

/// Harness run metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessRun {
    /// Run ID
    pub id: HarnessId,
    /// Configuration used
    pub config: HarnessConfig,
    /// Run status
    pub status: RunStatus,
    /// Current phase
    pub current_phase: Option<Phase>,
    /// Phase results
    pub phase_results: Vec<PhaseResult>,
    /// Events
    pub events: Vec<HarnessEvent>,
    /// Start time
    pub started_at: DateTime<Utc>,
    /// End time (if completed)
    pub completed_at: Option<DateTime<Utc>>,
    /// Final artifacts
    pub artifacts: Vec<Artifact>,
}