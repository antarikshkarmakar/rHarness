//! Web UI server for Mythos Harness

use axum::{
    extract::{ws::WebSocketUpgrade, State, Path, Query},
    http::StatusCode,
    response::{Html, IntoResponse, Json},
    routing::{get, post},
    Router,
};
use chrono::Utc;
use mythos_harness_core::types::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{RwLock, broadcast};
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;
use tracing::{info, error};
use uuid::Uuid;

/// Web UI state
#[derive(Clone)]
pub struct WebUiState {
    /// Active runs
    runs: Arc<RwLock<HashMap<HarnessId, HarnessRun>>>,
    /// Event broadcaster for real-time updates
    event_tx: broadcast::Sender<HarnessEvent>,
}

impl WebUiState {
    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(100);
        Self {
            runs: Arc::new(RwLock::new(HashMap::new())),
            event_tx: tx,
        }
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<HarnessEvent> {
        self.event_tx.subscribe()
    }

    pub async fn add_run(&self, run: HarnessRun) {
        let mut runs = self.runs.write().await;
        runs.insert(run.id, run);
    }

    pub async fn update_run(&self, run: HarnessRun) {
        let mut runs = self.runs.write().await;
        runs.insert(run.id, run);
    }

    pub async fn get_run(&self, id: HarnessId) -> Option<HarnessRun> {
        let runs = self.runs.read().await;
        runs.get(&id).cloned()
    }

    pub async fn list_runs(&self) -> Vec<HarnessRun> {
        let runs = self.runs.read().await;
        runs.values().cloned().collect()
    }

    pub fn broadcast_event(&self, event: HarnessEvent) {
        let _ = self.event_tx.send(event);
    }
}

/// Create the web UI router
pub fn create_router(state: WebUiState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/api/runs", get(list_runs).post(create_run))
        .route("/api/runs/:id", get(get_run))
        .route("/api/runs/:id/events", get(stream_events))
        .route("/api/config", get(get_config).post(update_config))
        .route("/api/plugins", get(list_plugins))
        .nest_service("/assets", ServeDir::new("assets"))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

/// Serve the main index page
async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

/// List all runs
async fn list_runs(State(state): State<WebUiState>) -> Json<Vec<HarnessRun>> {
    let runs = state.list_runs().await;
    Json(runs)
}

/// Create a new run
async fn create_run(
    State(state): State<WebUiState>,
    Json(request): Json<CreateRunRequest>,
) -> Result<Json<HarnessRun>, StatusCode> {
    // This would create and start a new harness run
    // For now, return a placeholder
    let run = HarnessRun {
        id: Uuid::new_v4(),
        config: HarnessConfig::default(),
        status: RunStatus::Pending,
        current_phase: None,
        phase_results: Vec::new(),
        events: Vec::new(),
        started_at: Utc::now(),
        completed_at: None,
        artifacts: Vec::new(),
    };
    
    state.add_run(run.clone()).await;
    Ok(Json(run))
}

/// Get a specific run
async fn get_run(
    State(state): State<WebUiState>,
    Path(id): Path<HarnessId>,
) -> Result<Json<HarnessRun>, StatusCode> {
    state.get_run(id).await
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

/// Stream events for a run (WebSocket)
async fn stream_events(
    ws: WebSocketUpgrade,
    State(state): State<WebUiState>,
    Path(id): Path<HarnessId>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_websocket(socket, state, id))
}

async fn handle_websocket(
    mut socket: axum::extract::ws::WebSocket,
    state: WebUiState,
    run_id: HarnessId,
) {
    let mut rx = state.subscribe_events();
    
    // Send initial run state
    if let Some(run) = state.get_run(run_id).await {
        let msg = serde_json::to_string(&WsMessage::RunState(run)).unwrap();
        let _ = socket.send(axum::extract::ws::Message::Text(msg)).await;
    }
    
    // Forward events (in a real implementation, we'd filter by run_id)
    while let Ok(event) = rx.recv().await {
        let msg = serde_json::to_string(&WsMessage::Event(event)).unwrap();
        if socket.send(axum::extract::ws::Message::Text(msg)).await.is_err() {
            break;
        }
    }
}

/// Get configuration
async fn get_config() -> Json<HarnessConfig> {
    Json(HarnessConfig::default())
}

/// Update configuration
async fn update_config(Json(config): Json<HarnessConfig>) -> Json<HarnessConfig> {
    Json(config)
}

/// List available plugins
async fn list_plugins() -> Json<Vec<PluginInfo>> {
    Json(vec![
        PluginInfo {
            name: "filesystem".to_string(),
            version: "1.0.0".to_string(),
            description: "File system operations".to_string(),
            phases: vec![Phase::Interrogate, Phase::Contract, Phase::Execute, Phase::Finish],
            capabilities: PluginCapabilities {
                filesystem: true,
                ..Default::default()
            },
        },
        PluginInfo {
            name: "shell".to_string(),
            version: "1.0.0".to_string(),
            description: "Shell command execution".to_string(),
            phases: vec![Phase::Interrogate, Phase::Contract, Phase::Execute, Phase::Finish],
            capabilities: PluginCapabilities {
                shell: true,
                ..Default::default()
            },
        },
        PluginInfo {
            name: "memory".to_string(),
            version: "1.0.0".to_string(),
            description: "Persistent memory storage".to_string(),
            phases: vec![Phase::Interrogate, Phase::Contract, Phase::Execute, Phase::Finish],
            capabilities: PluginCapabilities {
                memory: true,
                ..Default::default()
            },
        },
        PluginInfo {
            name: "web-search".to_string(),
            version: "1.0.0".to_string(),
            description: "Web search capabilities".to_string(),
            phases: vec![Phase::Interrogate, Phase::Contract, Phase::Execute],
            capabilities: PluginCapabilities {
                web_search: true,
                ..Default::default()
            },
        },
        PluginInfo {
            name: "code-executor".to_string(),
            version: "1.0.0".to_string(),
            description: "Code execution in multiple languages".to_string(),
            phases: vec![Phase::Execute, Phase::Finish],
            capabilities: PluginCapabilities {
                code_execution: true,
                ..Default::default()
            },
        },
    ])
}

/// Request to create a new run
#[derive(Deserialize)]
pub struct CreateRunRequest {
    pub task: String,
    pub config: Option<HarnessConfig>,
}

/// Plugin information
#[derive(Serialize)]
pub struct PluginInfo {
    pub name: String,
    pub version: String,
    pub description: String,
    pub phases: Vec<Phase>,
    pub capabilities: PluginCapabilities,
}

/// WebSocket messages
#[derive(Serialize)]
#[serde(tag = "type")]
pub enum WsMessage {
    #[serde(rename = "run_state")]
    RunState(HarnessRun),
    #[serde(rename = "event")]
    Event(HarnessEvent),
    #[serde(rename = "error")]
    Error(String),
}

/// Start the web UI server
pub async fn serve(state: WebUiState, addr: SocketAddr) -> anyhow::Result<()> {
    let app = create_router(state);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("Web UI server listening on {}", addr);
    axum::serve(listener, app).await?;
    Ok(())
}

const INDEX_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>rHarness</title>
    <style>
        * { box-sizing: border-box; margin: 0; padding: 0; }
        body { font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif; background: #f5f5f5; min-height: 100vh; }
        header { background: #1a1a2e; color: white; padding: 1rem 2rem; display: flex; justify-content: space-between; align-items: center; }
        h1 { font-size: 1.5rem; font-weight: 600; }
        .tagline { color: #e94560; font-size: 0.875rem; }
        main { max-width: 1200px; margin: 0 auto; padding: 2rem; }
        .card { background: white; border-radius: 8px; box-shadow: 0 2px 8px rgba(0,0,0,0.1); margin-bottom: 1.5rem; overflow: hidden; }
        .card-header { padding: 1rem 1.5rem; border-bottom: 1px solid #eee; display: flex; justify-content: space-between; align-items: center; }
        .card-body { padding: 1.5rem; }
        .btn { background: #1a1a2e; color: white; border: none; padding: 0.5rem 1rem; border-radius: 4px; cursor: pointer; font-size: 0.875rem; }
        .btn:hover { background: #16213e; }
        .btn-primary { background: #e94560; }
        .btn-primary:hover { background: #d63954; }
        .form-group { margin-bottom: 1rem; }
        label { display: block; margin-bottom: 0.5rem; font-weight: 500; }
        textarea, input, select { width: 100%; padding: 0.75rem; border: 1px solid #ddd; border-radius: 4px; font-size: 1rem; }
        textarea { min-height: 150px; font-family: monospace; }
        .run-list { display: grid; gap: 1rem; }
        .run-item { display: flex; justify-content: space-between; align-items: center; padding: 1rem; background: #fafafa; border-radius: 6px; }
        .run-info { flex: 1; }
        .run-id { font-family: monospace; font-size: 0.875rem; color: #666; }
        .run-task { font-weight: 500; margin-top: 0.25rem; }
        .run-status { padding: 0.25rem 0.75rem; border-radius: 9999px; font-size: 0.75rem; font-weight: 600; text-transform: uppercase; }
        .status-pending { background: #fff3cd; color: #856404; }
        .status-running { background: #cce5ff; color: #004085; }
        .status-completed { background: #d4edda; color: #155724; }
        .status-failed { background: #f8d7da; color: #721c24; }
        .status-cancelled { background: #e2e3e5; color: #383d41; }
        .phase-list { display: flex; gap: 0.5rem; margin-top: 1rem; flex-wrap: wrap; }
        .phase-badge { padding: 0.25rem 0.75rem; background: #e9ecef; border-radius: 4px; font-size: 0.75rem; }
        .phase-badge.completed { background: #d4edda; color: #155724; }
        .phase-badge.failed { background: #f8d7da; color: #721c24; }
        .phase-badge.current { background: #cce5ff; color: #004085; font-weight: 600; }
        .empty-state { text-align: center; padding: 3rem; color: #666; }
        .empty-state svg { width: 64px; height: 64px; margin-bottom: 1rem; opacity: 0.5; }
    </style>
</head>
<body>
    <header>
        <div>
            <h1>rHarness</h1>
            <div class="tagline">Finish First.</div>
        </div>
        <div style="display: flex; gap: 0.5rem;">
            <button class="btn" onclick="showNewRunModal()">New Run</button>
        </div>
    </header>
    
    <main>
        <div class="card">
            <div class="card-header">
                <h2>Recent Runs</h2>
                <button class="btn" onclick="loadRuns()">Refresh</button>
            </div>
            <div class="card-body">
                <div id="runs-container" class="run-list">
                    <div class="empty-state">
                        <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M9 5H7a2 2 0 00-2 2v12a2 2 0 002 2h10a2 2 0 002-2V7a2 2 0 00-2-2h-2M9 5a2 2 0 002 2h2a2 2 0 002-2M9 5a2 2 0 012-2h2a2 2 0 012 2m-6 9l2 2 4-4" />
                        </svg>
                        <p>No runs yet. Click "New Run" to start.</p>
                    </div>
                </div>
            </div>
        </div>
    </main>

    <!-- New Run Modal -->
    <div id="new-run-modal" class="modal" style="display: none;">
        <div class="modal-overlay" onclick="hideNewRunModal()"></div>
        <div class="modal-content card" style="max-width: 600px; margin: 2rem auto;">
            <div class="card-header">
                <h2>New Harness Run</h2>
                <button class="btn" onclick="hideNewRunModal()" style="background: none; color: #666;">✕</button>
            </div>
            <div class="card-body">
                <form id="new-run-form">
                    <div class="form-group">
                        <label for="task-input">Task Description</label>
                        <textarea id="task-input" name="task" placeholder="Describe the task you want the agent to accomplish..." required></textarea>
                    </div>
                    <div class="form-group">
                        <label for="strategy-select">Loop Strategy</label>
                        <select id="strategy-select" name="strategy">
                            <option value="finish-first">Finish First (iterate until done)</option>
                            <option value="single-pass">Single Pass</option>
                            <option value="fixed">Fixed Iterations</option>
                        </select>
                    </div>
                    <div style="display: flex; gap: 0.5rem; justify-content: flex-end;">
                        <button type="button" class="btn" onclick="hideNewRunModal()">Cancel</button>
                        <button type="submit" class="btn btn-primary">Start Run</button>
                    </div>
                </form>
            </div>
        </div>
    </div>

    <script>
        // Load runs on page load
        document.addEventListener('DOMContentLoaded', loadRuns);
        
        async function loadRuns() {
            try {
                const response = await fetch('/api/runs');
                const runs = await response.json();
                renderRuns(runs);
            } catch (error) {
                console.error('Failed to load runs:', error);
            }
        }
        
        function renderRuns(runs) {
            const container = document.getElementById('runs-container');
            
            if (runs.length === 0) {
                container.innerHTML = `
                    <div class="empty-state">
                        <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M9 5H7a2 2 0 00-2 2v12a2 2 0 002 2h10a2 2 0 002-2V7a2 2 0 00-2-2h-2M9 5a2 2 0 002 2h2a2 2 0 002-2M9 5a2 2 0 012-2h2a2 2 0 012 2m-6 9l2 2 4-4" />
                        </svg>
                        <p>No runs yet. Click "New Run" to start.</p>
                    </div>
                `;
                return;
            }
            
            container.innerHTML = runs.map(run => `
                <div class="run-item" onclick="showRunDetails('${run.id}')">
                    <div class="run-info">
                        <div class="run-id">${run.id}</div>
                        <div class="run-task">${run.config.name} - ${run.phase_results.length} phases</div>
                    </div>
                    <span class="run-status status-${run.status.toLowerCase()}">${run.status}</span>
                </div>
            `).join('');
        }
        
        function showNewRunModal() {
            document.getElementById('new-run-modal').style.display = 'block';
        }
        
        function hideNewRunModal() {
            document.getElementById('new-run-modal').style.display = 'none';
        }
        
        document.getElementById('new-run-form').addEventListener('submit', async (e) => {
            e.preventDefault();
            const formData = new FormData(e.target);
            const task = formData.get('task');
            const strategy = formData.get('strategy');
            
            try {
                const response = await fetch('/api/runs', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({ task, config: { loop_strategy: strategy } })
                });
                
                if (response.ok) {
                    hideNewRunModal();
                    loadRuns();
                } else {
                    alert('Failed to create run');
                }
            } catch (error) {
                console.error('Failed to create run:', error);
                alert('Failed to create run');
            }
        });
        
        function showRunDetails(runId) {
            window.location.href = `/run/${runId}`;
        }
    </script>
</body>
</html>
"#;