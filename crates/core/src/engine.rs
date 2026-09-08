//! Core harness engine implementation

use crate::types::*;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::time::{timeout, Duration};
use tracing::{debug, error, info, instrument, warn};
use uuid::Uuid;

/// Main harness engine
pub struct HarnessEngine {
    config: HarnessConfig,
    plugins: HashMap<String, Box<dyn Plugin>>,
    agent: Box<dyn Agent>,
    run: Arc<RwLock<HarnessRun>>,
    phase_context: Arc<RwLock<PhaseContext>>,
}

impl HarnessEngine {
    /// Create a new harness engine
    pub fn new(config: HarnessConfig, agent: Box<dyn Agent>) -> Self {
        let run_id = Uuid::new_v4();
        let run = HarnessRun {
            id: run_id,
            config: config.clone(),
            status: RunStatus::Pending,
            current_phase: None,
            phase_results: Vec::new(),
            events: Vec::new(),
            started_at: Utc::now(),
            completed_at: None,
            artifacts: Vec::new(),
        };

        Self {
            config,
            plugins: HashMap::new(),
            agent,
            run: Arc::new(RwLock::new(run)),
            phase_context: Arc::new(RwLock::new(PhaseContext::default())),
        }
    }

    /// Register a plugin
    pub fn register_plugin(&mut self, plugin: Box<dyn Plugin>) {
        let name = plugin.name().to_string();
        self.plugins.insert(name, plugin);
    }

    /// Get the current run state
    pub fn run_state(&self) -> Arc<RwLock<HarnessRun>> {
        self.run.clone()
    }

    /// Get the phase context
    pub fn phase_context(&self) -> Arc<RwLock<PhaseContext>> {
        self.phase_context.clone()
    }

    /// Emit an event
    async fn emit_event(&self, event_type: EventType, phase: Phase, payload: serde_json::Value) {
        let event = HarnessEvent {
            id: Uuid::new_v4(),
            event_type,
            phase,
            timestamp: Utc::now(),
            payload,
        };

        let mut run = self.run.write();
        run.events.push(event);
    }

    /// Execute a single phase
    #[instrument(skip(self))]
    async fn execute_phase(&self, phase: Phase) -> Result<PhaseResult> {
        info!("Starting phase: {:?}", phase);
        
        // Update run state
        {
            let mut run = self.run.write();
            run.current_phase = Some(phase);
            run.status = RunStatus::Running;
        }

        self.emit_event(EventType::PhaseStarted, phase, serde_json::json!({})).await;

        let start_time = Instant::now();
        let started_at = Utc::now();

        // Get phase timeout
        let phase_timeout = Duration::from_secs(self.config.phase_timeout_secs);
        
        // Execute with timeout
        let result = timeout(phase_timeout, async {
            // Run agent for this phase
            let mut context = self.phase_context.write();
            let agent_result = self.agent.run_phase(phase, &mut context).await;
            
            // Also run relevant plugins
            let mut plugin_results = Vec::new();
            for plugin in self.plugins.values() {
                if plugin.supported_phases().contains(&phase) {
                    self.emit_event(
                        EventType::PluginInvoked { plugin: plugin.name().to_string() },
                        phase,
                        serde_json::json!({})
                    ).await;
                    
                    match plugin.execute(phase, &mut context).await {
                        Ok(result) => plugin_results.push(result),
                        Err(e) => {
                            warn!("Plugin {} failed in phase {:?}: {}", plugin.name(), phase, e);
                        }
                    }
                }
            }
            
            // Combine results
            let mut combined_output = serde_json::json!({
                "agent": agent_result.as_ref().map(|r| &r.output).unwrap_or(&serde_json::Value::Null),
                "plugins": plugin_results.iter().map(|r| &r.output).collect::<Vec<_>>(),
            });
            
            let success = agent_result.as_ref().map(|r| r.success).unwrap_or(false) 
                && plugin_results.iter().all(|r| r.success);
            
            let errors = agent_result.as_ref().map(|r| r.errors.clone()).unwrap_or_default()
                .into_iter()
                .chain(plugin_results.iter().flat_map(|r| r.errors.clone()))
                .collect();
            
            Ok::<_, anyhow::Error>(PhaseResult {
                phase,
                success,
                output: combined_output,
                errors,
                started_at,
                completed_at: Utc::now(),
                duration_ms: start_time.elapsed().as_millis() as u64,
            })
        }).await;

        let phase_result = match result {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                error!("Phase {:?} failed: {}", phase, e);
                PhaseResult {
                    phase,
                    success: false,
                    output: serde_json::Value::Null,
                    errors: vec![e.to_string()],
                    started_at,
                    completed_at: Utc::now(),
                    duration_ms: start_time.elapsed().as_millis() as u64,
                }
            }
            Err(_) => {
                error!("Phase {:?} timed out", phase);
                PhaseResult {
                    phase,
                    success: false,
                    output: serde_json::Value::Null,
                    errors: vec!["Phase timed out".to_string()],
                    started_at,
                    completed_at: Utc::now(),
                    duration_ms: start_time.elapsed().as_millis() as u64,
                }
            }
        };

        // Emit completion event
        if phase_result.success {
            self.emit_event(EventType::PhaseCompleted, phase, serde_json::to_value(&phase_result)?).await;
        } else {
            self.emit_event(EventType::PhaseFailed, phase, serde_json::to_value(&phase_result)?).await;
        }

        // Store result
        {
            let mut run = self.run.write();
            run.phase_results.push(phase_result.clone());
        }

        info!("Completed phase: {:?} (success: {})", phase, phase_result.success);
        Ok(phase_result)
    }

    /// Check if we should continue to the next phase
    fn should_continue(&self, phase: Phase, result: &PhaseResult) -> bool {
        if !result.success {
            return false;
        }

        // Check agent's decision
        if !self.agent.should_continue(result) {
            return false;
        }

        // Check success criteria for finish-first loop
        if self.config.loop_strategy == LoopStrategy::FinishFirst {
            let criteria = self.agent.success_criteria();
            let context = self.phase_context.read();
            
            // Check confidence (would need to be in output)
            let confidence = result.output.get("confidence")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            
            if confidence < criteria.min_confidence {
                debug!("Confidence {} below threshold {}", confidence, criteria.min_confidence);
                return true; // Continue to improve
            }

            // Check required artifacts
            for artifact_name in &criteria.required_artifacts {
                let has_artifact = context.artifacts.iter().any(|a| a.name == *artifact_name);
                if !has_artifact {
                    debug!("Required artifact {} not found", artifact_name);
                    return true; // Continue to produce it
                }
            }
        }

        true
    }

    /// Run the harness
    #[instrument(skip(self))]
    pub async fn run(&mut self) -> Result<HarnessRun> {
        info!("Starting harness run: {}", self.run.read().id);
        
        let global_timeout = Duration::from_secs(self.config.global_timeout_secs);
        let start_time = Instant::now();

        // Initialize plugins
        for plugin in self.plugins.values_mut() {
            plugin.initialize(&serde_json::json!({})).await
                .context("Failed to initialize plugin")?;
        }

        // Run phases according to strategy
        let mut current_phase_idx = 0;
        let phases = &self.config.phases;
        let mut iteration = 0;
        let max_iterations = match self.config.loop_strategy {
            LoopStrategy::FixedIterations(n) => n,
            _ => self.config.max_iterations,
        };

        loop {
            // Check global timeout
            if start_time.elapsed() >= global_timeout {
                error!("Global timeout reached");
                break;
            }

            // Check max iterations
            if iteration >= max_iterations {
                warn!("Max iterations ({}) reached", max_iterations);
                break;
            }

            if current_phase_idx >= phases.len() {
                // All phases completed
                if self.config.loop_strategy == LoopStrategy::FinishFirst {
                    // Check if we should loop back
                    let last_result = self.run.read().phase_results.last().cloned();
                    if let Some(result) = last_result {
                        let last_phase = phases.last().copied().unwrap_or(Phase::Finish);
                        if self.should_continue(last_phase, &result) {
                            // Loop back to first phase or interrogate
                            current_phase_idx = 0;
                            iteration += 1;
                            info!("Looping back for iteration {}", iteration + 1);
                            continue;
                        }
                    }
                }
                break;
            }

            let phase = phases[current_phase_idx];
            let result = self.execute_phase(phase).await?;

            if result.success {
                current_phase_idx += 1;
            } else {
                // Phase failed - decide what to do
                match self.config.loop_strategy {
                    LoopStrategy::FinishFirst => {
                        // In finish-first, we might retry the same phase
                        warn!("Phase {:?} failed, will retry", phase);
                        // Could add retry logic here
                    }
                    _ => {
                        // In other strategies, move to next phase or stop
                        current_phase_idx += 1;
                    }
                }
            }
        }

        // Shutdown plugins
        for plugin in self.plugins.values_mut() {
            if let Err(e) = plugin.shutdown().await {
                warn!("Plugin shutdown failed: {}", e);
            }
        }

        // Finalize run
        let mut run = self.run.write();
        run.status = if run.phase_results.iter().all(|r| r.success) {
            RunStatus::Completed
        } else {
            RunStatus::Failed
        };
        run.completed_at = Some(Utc::now());
        run.artifacts = self.phase_context.read().artifacts.clone();

        info!("Harness run completed with status: {:?}", run.status);
        Ok(run.clone())
    }
}

/// Builder for creating harness engines
pub struct HarnessBuilder {
    config: HarnessConfig,
    agent: Option<Box<dyn Agent>>,
    plugins: Vec<Box<dyn Plugin>>,
}

impl HarnessBuilder {
    /// Create a new builder with default config
    pub fn new() -> Self {
        Self {
            config: HarnessConfig::default(),
            agent: None,
            plugins: Vec::new(),
        }
    }

    /// Set the harness name
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.config.name = name.into();
        self
    }

    /// Set the version
    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.config.version = version.into();
        self
    }

    /// Replace the whole harness config
    pub fn config(mut self, config: HarnessConfig) -> Self {
        self.config = config;
        self
    }

    /// Set the loop strategy
    pub fn loop_strategy(mut self, strategy: LoopStrategy) -> Self {
        self.config.loop_strategy = strategy;
        self
    }

    /// Set the agent
    pub fn agent(mut self, agent: Box<dyn Agent>) -> Self {
        self.agent = Some(agent);
        self
    }

    /// Add a plugin
    pub fn plugin(mut self, plugin: Box<dyn Plugin>) -> Self {
        self.plugins.push(plugin);
        self
    }

    /// Set phases
    pub fn phases(mut self, phases: Vec<Phase>) -> Self {
        self.config.phases = phases;
        self
    }

    /// Set max iterations
    pub fn max_iterations(mut self, max: u32) -> Self {
        self.config.max_iterations = max;
        self
    }

    /// Set phase timeout
    pub fn phase_timeout(mut self, secs: u64) -> Self {
        self.config.phase_timeout_secs = secs;
        self
    }

    /// Set global timeout
    pub fn global_timeout(mut self, secs: u64) -> Self {
        self.config.global_timeout_secs = secs;
        self
    }

    /// Build the harness engine
    pub fn build(self) -> Result<HarnessEngine> {
        let agent = self.agent.ok_or_else(|| anyhow::anyhow!("Agent is required"))?;
        
        let mut engine = HarnessEngine::new(self.config, agent);
        for plugin in self.plugins {
            engine.register_plugin(plugin);
        }
        Ok(engine)
    }
}

impl Default for HarnessConfig {
    fn default() -> Self {
        Self {
            name: "rHarness".to_string(),
            version: "1.0.0".to_string(),
            tagline: "Finish First.".to_string(),
            description: "A finish-first autonomous agent loop and an open, plugin-based agent harness.".to_string(),
            loop_strategy: LoopStrategy::FinishFirst,
            engine: "harness-core".to_string(),
            license: "MIT".to_string(),
            homepage: None,
            phases: Phase::all().to_vec(),
            plugins: vec![
                "web-search".to_string(),
                "filesystem".to_string(),
                "shell".to_string(),
                "memory".to_string(),
                "code-executor".to_string(),
            ],
            max_iterations: 10,
            phase_timeout_secs: 300,
            global_timeout_secs: 3600,
        }
    }
}

impl Default for HarnessBuilder {
    fn default() -> Self {
        Self::new()
    }
}