//! CLI entry point for Mythos Harness

use clap::{Parser, Subcommand};
use mythos_harness_core::types::*;
use mythos_harness_core::{HarnessBuilder, HarnessEngine};
use mythos_harness_plugins::create_builtin_plugins;
use mythos_harness_agents::FinishFirstAgent;
use anyhow::{Context, Result};
use config::{Config, File, FileFormat};
use directories::ProjectDirs;
use std::path::{Path, PathBuf};
use std::time::Instant;
use tracing::{error, info, Level};
use tracing_subscriber::FmtSubscriber;

#[derive(Parser)]
#[command(name = "mythos")]
#[command(about = "rHarness — a finish-first autonomous agent loop")]
#[command(version)]
#[command(long_about = "Mythos Harness is a finish-first autonomous agent loop and an open, plugin-based AI agent harness with a local web UI.")]
struct Cli {
    /// Configuration file path
    #[arg(short, long, global = true)]
    config: Option<PathBuf>,

    /// Working directory
    #[arg(short, long, global = true)]
    dir: Option<PathBuf>,

    /// Verbose output
    #[arg(short, long, global = true)]
    verbose: bool,

    /// Quiet output
    #[arg(short, long, global = true)]
    quiet: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run a harness task
    Run {
        /// Task description or path to task file
        #[arg(short, long)]
        task: Option<String>,
        
        /// Task file path
        #[arg(short, long)]
        file: Option<PathBuf>,
        
        /// Loop strategy
        #[arg(long, value_enum, default_value = "finish-first")]
        strategy: LoopStrategyArg,
        
        /// Maximum iterations
        #[arg(long, default_value = "10")]
        max_iterations: u32,
        
        /// Phase timeout (seconds)
        #[arg(long, default_value = "300")]
        phase_timeout: u64,
        
        /// Global timeout (seconds)
        #[arg(long, default_value = "3600")]
        global_timeout: u64,
        
        /// Output format
        #[arg(long, value_enum, default_value = "json")]
        output: OutputFormat,
    },
    
    /// Initialize a new harness project
    Init {
        /// Project name
        #[arg(short, long)]
        name: Option<String>,
        
        /// Project directory
        #[arg(short, long)]
        dir: Option<PathBuf>,
    },
    
    /// List available plugins
    Plugins,
    
    /// Show harness configuration
    Config {
        /// Show effective configuration
        #[arg(long)]
        effective: bool,
    },
    
    /// Run web UI server
    Serve {
        /// Port to listen on
        #[arg(short, long, default_value = "8080")]
        port: u16,
        
        /// Host to bind to
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
    },
}

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
enum LoopStrategyArg {
    #[value(name = "finish-first")]
    FinishFirst,
    #[value(name = "single-pass")]
    SinglePass,
    #[value(name = "fixed")]
    Fixed,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
enum OutputFormat {
    Json,
    Yaml,
    Pretty,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    
    // Setup logging
    let level = if cli.verbose {
        Level::DEBUG
    } else if cli.quiet {
        Level::WARN
    } else {
        Level::INFO
    };
    
    let subscriber = FmtSubscriber::builder()
        .with_max_level(level)
        .finish();
    tracing::subscriber::set_global_default(subscriber)
        .context("Failed to set tracing subscriber")?;
    
    // Determine working directory
    let working_dir = cli.dir.unwrap_or_else(|| std::env::current_dir().unwrap());
    
    // Load configuration
    let harness_config = load_config(&working_dir, cli.config.as_deref())?;
    
    // Execute command
    match cli.command {
        Commands::Run { task, file, strategy, max_iterations, phase_timeout, global_timeout, output } => {
            run_command(working_dir, harness_config, task, file, strategy, max_iterations, phase_timeout, global_timeout, output).await
        }
        Commands::Init { name, dir } => {
            init_command(dir.unwrap_or(working_dir), name).await
        }
        Commands::Plugins => {
            plugins_command().await
        }
        Commands::Config { effective } => {
            config_command(harness_config, effective).await
        }
        Commands::Serve { port, host } => {
            serve_command(working_dir, harness_config, host, port).await
        }
    }
}

fn load_config(working_dir: &Path, config_file: Option<&Path>) -> Result<HarnessConfig> {
    let mut builder = Config::builder();
    
    // Default config
    builder = builder.add_source(config::Config::try_from(&HarnessConfig::default())?);
    
    // Project config file
    let project_config = working_dir.join("rharness.toml");
    if project_config.exists() {
        builder = builder.add_source(File::from(project_config).format(FileFormat::Toml));
    }
    
    // User config file
    if let Some(proj_dirs) = ProjectDirs::from("com", "mythos", "harness") {
        let user_config = proj_dirs.config_dir().join("config.toml");
        if user_config.exists() {
            builder = builder.add_source(File::from(user_config).format(FileFormat::Toml));
        }
    }
    
    // Explicit config file
    if let Some(path) = config_file {
        builder = builder.add_source(File::from(path).format(FileFormat::Toml));
    }
    
    // Environment variables
    builder = builder.add_source(config::Environment::with_prefix("MYTHOS"));
    
    let config = builder.build()?;
    let harness_config: HarnessConfig = config.try_deserialize()?;
    
    Ok(harness_config)
}

async fn run_command(
    working_dir: PathBuf,
    mut config: HarnessConfig,
    task: Option<String>,
    file: Option<PathBuf>,
    strategy: LoopStrategyArg,
    max_iterations: u32,
    phase_timeout: u64,
    global_timeout: u64,
    output: OutputFormat,
) -> Result<()> {
    info!("Starting Mythos Harness run");
    let start_time = Instant::now();
    
    // Override config from CLI args
    config.loop_strategy = match strategy {
        LoopStrategyArg::FinishFirst => LoopStrategy::FinishFirst,
        LoopStrategyArg::SinglePass => LoopStrategy::SinglePass,
        LoopStrategyArg::Fixed => LoopStrategy::FixedIterations(max_iterations),
    };
    config.max_iterations = max_iterations;
    config.phase_timeout_secs = phase_timeout;
    config.global_timeout_secs = global_timeout;
    
    // Load task
    let task_description = if let Some(file) = file {
        tokio::fs::read_to_string(&file).await
            .context("Failed to read task file")?
    } else if let Some(task) = task {
        task
    } else {
        anyhow::bail!("Task description required (use --task or --file)");
    };
    
    info!("Task: {}", task_description);
    
    // Create agent
    let agent = Box::new(FinishFirstAgent::new("mythos-agent", "1.0.0")
        .with_success_criteria(SuccessCriteria {
            min_confidence: 0.8,
            required_artifacts: vec![],
            validator: None,
            max_retries: 3,
        }));
    
    // Build harness
    let mut engine = HarnessBuilder::new()
        .config(config)
        .agent(agent)
        .build()?;
    
    // Register built-in plugins
    for plugin in create_builtin_plugins(working_dir.clone()) {
        engine.register_plugin(plugin);
    }
    
    // Add task to context
    {
        let context_lock = engine.phase_context();
        let mut context = context_lock.write();
        context.data.insert("task".to_string(), serde_json::json!(task_description));
    }
    
    // Run harness
    let run = engine.run().await?;
    
    // Output results
    let output_value = serde_json::to_value(&run)?;
    match output {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&output_value)?);
        }
        OutputFormat::Yaml => {
            println!("{}", serde_yaml::to_string(&output_value)?);
        }
        OutputFormat::Pretty => {
            print_run_summary(&run);
        }
    }
    
    info!("Harness run completed in {:.2}s", start_time.elapsed().as_secs_f64());
    
    if run.status != RunStatus::Completed {
        std::process::exit(1);
    }
    
    Ok(())
}

async fn init_command(dir: PathBuf, name: Option<String>) -> Result<()> {
    info!("Initializing Mythos Harness project in {:?}", dir);
    
    tokio::fs::create_dir_all(&dir).await?;
    
    let project_name = name.unwrap_or_else(|| {
        dir.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("mythos-project")
            .to_string()
    });
    
    // Create default config
    let config = HarnessConfig {
        name: project_name.clone(),
        ..Default::default()
    };
    
    let config_toml = toml::to_string_pretty(&config)?;
    tokio::fs::write(dir.join("rharness.toml"), config_toml).await?;
    
    // Create example task file
    let task_content = r#"# Mythos Harness Task
# Describe the task you want the agent to accomplish

task: |
  Your task description here.
  
  Be specific about:
  - What you want to achieve
  - Any constraints or requirements
  - Expected deliverables
  - Success criteria

# Example:
# task: |
#   Create a REST API for a todo list application with the following endpoints:
#   - GET /todos - List all todos
#   - POST /todos - Create a new todo
#   - GET /todos/:id - Get a specific todo
#   - PUT /todos/:id - Update a todo
#   - DELETE /todos/:id - Delete a todo
#   
#   Requirements:
#   - Use Rust with Axum framework
#   - Use SQLite for persistence
#   - Include input validation
#   - Write unit tests
#   - Document the API with OpenAPI
"#;
    
    tokio::fs::write(dir.join("task.md"), task_content).await?;
    
    println!("Initialized Mythos Harness project: {}", project_name);
    println!("Config written to: {}", dir.join("rharness.toml").display());
    println!("Example task written to: {}", dir.join("task.md").display());
    
    Ok(())
}

async fn plugins_command() -> Result<()> {
    println!("Available built-in plugins:");
    println!("  filesystem  - File system operations (read, write, list, mkdir)");
    println!("  shell       - Shell command execution");
    println!("  memory      - Persistent key-value storage with tags");
    println!("  web-search  - Web search (DuckDuckGo, Google, Bing, Brave)");
    println!("  code-executor - Code execution in multiple languages");
    Ok(())
}

async fn config_command(config: HarnessConfig, effective: bool) -> Result<()> {
    if effective {
        println!("{}", toml::to_string_pretty(&config)?);
    } else {
        println!("Configuration sources (in order of precedence):");
        println!("  1. Default values");
        println!("  2. ./rharness.toml (project config)");
        println!("  3. ~/.config/mythos-harness/config.toml (user config)");
        println!("  4. --config flag (explicit config file)");
        println!("  5. MYTHOS_* environment variables");
        println!();
        println!("Run with --effective to see the merged configuration.");
    }
    Ok(())
}

async fn serve_command(
    working_dir: PathBuf,
    config: HarnessConfig,
    host: String,
    port: u16,
) -> Result<()> {
    info!("Starting Mythos Harness web UI on {}:{}", host, port);
    
    // This would start the web server
    // For now, just print a message
    println!("Web UI server would start on http://{}:{}", host, port);
    println!("(Web UI not yet implemented)");
    
    Ok(())
}

fn print_run_summary(run: &HarnessRun) {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║                    Mythos Harness Run                        ║");
    println!("╠══════════════════════════════════════════════════════════════╣");
    println!("║  Run ID:     {}", run.id);
    println!("║  Status:     {:?}", run.status);
    println!("║  Started:    {}", run.started_at.format("%Y-%m-%d %H:%M:%S UTC"));
    if let Some(completed) = run.completed_at {
        println!("║  Completed:  {}", completed.format("%Y-%m-%d %H:%M:%S UTC"));
        println!("║  Duration:   {:.2}s", (completed - run.started_at).num_seconds());
    }
    println!("╠══════════════════════════════════════════════════════════════╣");
    println!("║  Phases Executed: {}", run.phase_results.len());
    println!("╠══════════════════════════════════════════════════════════════╣");
    
    for result in &run.phase_results {
        let status = if result.success { "✓" } else { "✗" };
        println!("║  {} {:?} ({}ms)", status, result.phase, result.duration_ms);
        if !result.errors.is_empty() {
            for error in &result.errors {
                println!("║    Error: {}", error);
            }
        }
    }
    
    println!("╠══════════════════════════════════════════════════════════════╣");
    println!("║  Artifacts: {}", run.artifacts.len());
    for artifact in &run.artifacts {
        println!("║    - {} ({})", artifact.name, artifact.artifact_type);
    }
    
    println!("╠══════════════════════════════════════════════════════════════╣");
    println!("║  Events: {}", run.events.len());
    println!("╚══════════════════════════════════════════════════════════════╝");
}

use serde_yaml;