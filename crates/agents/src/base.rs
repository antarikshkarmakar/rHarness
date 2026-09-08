//! Base agent implementation

use async_trait::async_trait;
use mythos_harness_core::types::*;
use anyhow::Result;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};
use chrono::Utc;

/// Base agent that implements the finish-first loop logic
pub struct FinishFirstAgent {
    name: String,
    version: String,
    success_criteria: SuccessCriteria,
    plugins: Arc<RwLock<Vec<Box<dyn Plugin>>>>,
    phase_handlers: Arc<
        RwLock<
            HashMap<
                Phase,
                Box<dyn Fn(Phase, &mut PhaseContext) -> Result<PhaseResult> + Send + Sync>,
            >,
        >,
    >,
}

impl FinishFirstAgent {
    /// Create a new finish-first agent
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            success_criteria: SuccessCriteria::default(),
            plugins: Arc::new(RwLock::new(Vec::new())),
            phase_handlers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Set success criteria
    pub fn with_success_criteria(mut self, criteria: SuccessCriteria) -> Self {
        self.success_criteria = criteria;
        self
    }

    /// Add a plugin
    pub async fn add_plugin(&self, plugin: Box<dyn Plugin>) {
        let mut plugins = self.plugins.write().await;
        plugins.push(plugin);
    }

    /// Register a phase handler
    pub async fn register_handler<F>(&self, phase: Phase, handler: F)
    where
        F: Fn(Phase, &mut PhaseContext) -> Result<PhaseResult> + Send + Sync + 'static,
    {
        let mut handlers = self.phase_handlers.write().await;
        handlers.insert(phase, Box::new(handler));
    }

    /// Get plugins
    pub async fn plugins(&self) -> Vec<String> {
        let plugins = self.plugins.read().await;
        plugins.iter().map(|p| p.name().to_string()).collect()
    }
}

#[async_trait]
impl Agent for FinishFirstAgent {
    fn name(&self) -> &str {
        &self.name
    }

    fn version(&self) -> &str {
        &self.version
    }

    async fn run_phase(&self, phase: Phase, context: &mut PhaseContext) -> Result<PhaseResult> {
        info!("Agent {} running phase: {:?}", self.name, phase);
        
        let started_at = Utc::now();
        let start_time = std::time::Instant::now();
        
        // Check for custom handler
        let handlers = self.phase_handlers.read().await;
        if let Some(handler) = handlers.get(&phase) {
            let result = handler(phase, context)?;
            return Ok(result);
        }
        
        // Default phase behavior
        let result = match phase {
            Phase::Interrogate => self.interrogate_phase(context).await?,
            Phase::Contract => self.contract_phase(context).await?,
            Phase::Execute => self.execute_phase(context).await?,
            Phase::Finish => self.finish_phase(context).await?,
        };
        
        let mut phase_result = PhaseResult {
            phase,
            success: true,
            output: serde_json::to_value(&result)?,
            errors: Vec::new(),
            started_at,
            completed_at: Utc::now(),
            duration_ms: start_time.elapsed().as_millis() as u64,
        };
        
        // Run plugins for this phase
        let plugins = self.plugins.read().await;
        for plugin in plugins.iter() {
            if plugin.supported_phases().contains(&phase) {
                match plugin.execute(phase, context).await {
                    Ok(plugin_result) => {
                        if !plugin_result.success {
                            phase_result.success = false;
                            phase_result.errors.extend(plugin_result.errors);
                        }
                        // Merge plugin output
                        if let serde_json::Value::Object(ref mut obj) = phase_result.output {
                            obj.insert(
                                format!("plugin_{}", plugin.name()),
                                plugin_result.output,
                            );
                        }
                    }
                    Err(e) => {
                        phase_result.success = false;
                        phase_result.errors.push(format!("Plugin {} failed: {}", plugin.name(), e));
                    }
                }
            }
        }
        
        Ok(phase_result)
    }

    fn should_continue(&self, result: &PhaseResult) -> bool {
        // Continue if phase failed or if we're in finish-first mode and criteria not met
        if !result.success {
            return true;
        }
        
        // Check if this was the finish phase
        if result.phase == Phase::Finish {
            // Check success criteria
            let confidence = result.output.get("confidence")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            
            if confidence < self.success_criteria.min_confidence {
                debug!("Confidence {} below threshold {}", confidence, self.success_criteria.min_confidence);
                return true;
            }
            
            // Check required artifacts (would need access to context)
            // For now, assume success if confidence is high enough
            return false;
        }
        
        // Continue to next phase
        true
    }

    fn success_criteria(&self) -> SuccessCriteria {
        self.success_criteria.clone()
    }
}

impl FinishFirstAgent {
    /// Default interrogate phase - understand the task
    async fn interrogate_phase(&self, context: &mut PhaseContext) -> Result<serde_json::Value> {
        info!("Interrogate phase: Understanding task and context");
        
        // In a real implementation, this would:
        // 1. Analyze the task description
        // 2. Gather context from memory/plugins
        // 3. Identify requirements and constraints
        // 4. Formulate questions if needed
        
        Ok(serde_json::json!({
            "task_understood": true,
            "requirements_identified": [],
            "constraints": [],
            "questions": [],
            "confidence": 0.7,
        }))
    }

    /// Default contract phase - define success criteria and plan
    async fn contract_phase(&self, context: &mut PhaseContext) -> Result<serde_json::Value> {
        info!("Contract phase: Defining success criteria and plan");
        
        // In a real implementation, this would:
        // 1. Define clear success criteria
        // 2. Create execution plan
        // 3. Identify required resources/tools
        // 4. Set milestones
        
        Ok(serde_json::json!({
            "success_criteria_defined": true,
            "plan": [],
            "required_resources": [],
            "milestones": [],
            "confidence": 0.8,
        }))
    }

    /// Default execute phase - perform the work
    async fn execute_phase(&self, context: &mut PhaseContext) -> Result<serde_json::Value> {
        info!("Execute phase: Performing work");
        
        // In a real implementation, this would:
        // 1. Execute the plan steps
        // 2. Use plugins to perform actions
        // 3. Track progress and artifacts
        // 4. Handle errors and retries
        
        Ok(serde_json::json!({
            "work_completed": true,
            "artifacts_created": [],
            "steps_executed": [],
            "errors": [],
            "confidence": 0.85,
        }))
    }

    /// Default finish phase - verify and deliver
    async fn finish_phase(&self, context: &mut PhaseContext) -> Result<serde_json::Value> {
        info!("Finish phase: Verifying completion and delivering results");
        
        // In a real implementation, this would:
        // 1. Verify all success criteria met
        // 2. Validate artifacts
        // 3. Run final checks/tests
        // 4. Package and deliver results
        
        Ok(serde_json::json!({
            "verified": true,
            "all_criteria_met": true,
            "deliverables": [],
            "validation_results": [],
            "confidence": 0.9,
        }))
    }
}

use std::collections::HashMap;