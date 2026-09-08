//! Memory plugin for persistent storage

use async_trait::async_trait;
use mythos_harness_core::types::*;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::{sqlite::SqlitePoolOptions, Pool, Sqlite, Row};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Memory plugin for persistent key-value storage with metadata
pub struct MemoryPlugin {
    config: MemoryConfig,
    pool: Arc<RwLock<Option<Pool<Sqlite>>>>,
    working_dir: PathBuf,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct MemoryConfig {
    /// Database file path (relative to working dir)
    pub db_path: String,
    /// Maximum entries per namespace
    pub max_entries_per_namespace: usize,
    /// Maximum value size (bytes)
    pub max_value_size: usize,
    /// Default TTL (seconds, 0 = no expiry)
    pub default_ttl_secs: u64,
    /// Enable vector similarity search
    pub enable_vector_search: bool,
    /// Vector dimension (if enabled)
    pub vector_dimension: usize,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            db_path: ".mythos_memory.db".to_string(),
            max_entries_per_namespace: 10000,
            max_value_size: 1024 * 1024, // 1MB
            default_ttl_secs: 0,
            enable_vector_search: false,
            vector_dimension: 1536, // OpenAI embedding dimension
        }
    }
}

/// Memory entry
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MemoryEntry {
    pub key: String,
    pub value: serde_json::Value,
    pub namespace: String,
    pub tags: Vec<String>,
    pub metadata: HashMap<String, serde_json::Value>,
    pub vector: Option<Vec<f32>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub access_count: u64,
    pub last_accessed: Option<DateTime<Utc>>,
}

impl MemoryPlugin {
    /// Create a new memory plugin
    pub fn new(config: MemoryConfig, working_dir: PathBuf) -> Self {
        Self {
            config,
            pool: Arc::new(RwLock::new(None)),
            working_dir,
        }
    }

    /// Get or create the database pool
    async fn get_pool(&self) -> Result<Pool<Sqlite>> {
        let mut pool_guard = self.pool.write().await;
        
        if let Some(pool) = pool_guard.as_ref() {
            return Ok(pool.clone());
        }
        
        let db_path = self.working_dir.join(&self.config.db_path);
        let db_url = format!("sqlite://{}?mode=rwc", db_path.display());
        
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(&db_url)
            .await
            .context("Failed to connect to memory database")?;
        
        // Run migrations
        self.run_migrations(&pool).await?;
        
        *pool_guard = Some(pool.clone());
        Ok(pool)
    }

    /// Run database migrations
    async fn run_migrations(&self, pool: &Pool<Sqlite>) -> Result<()> {
        // Create main memory table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS memory_entries (
                key TEXT NOT NULL,
                namespace TEXT NOT NULL DEFAULT 'default',
                value TEXT NOT NULL,
                tags TEXT NOT NULL DEFAULT '[]',
                metadata TEXT NOT NULL DEFAULT '{}',
                vector BLOB,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                expires_at TEXT,
                access_count INTEGER NOT NULL DEFAULT 0,
                last_accessed TEXT,
                PRIMARY KEY (key, namespace)
            )
            "#,
        )
        .execute(pool)
        .await?;

        // Create indexes
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_memory_namespace ON memory_entries(namespace)")
            .execute(pool)
            .await?;
        
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_memory_tags ON memory_entries(tags)")
            .execute(pool)
            .await?;
        
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_memory_expires ON memory_entries(expires_at)")
            .execute(pool)
            .await?;

        // Create vector search table if enabled
        if self.config.enable_vector_search {
            sqlx::query(
                r#"
                CREATE VIRTUAL TABLE IF NOT EXISTS memory_vectors 
                USING vec0(vector FLOAT[?])
                "#,
            )
            .bind(self.config.vector_dimension as i64)
            .execute(pool)
            .await?;
        }

        Ok(())
    }

    /// Store a memory entry
    pub async fn store(&self, entry: MemoryEntry) -> Result<()> {
        if entry.key.is_empty() {
            anyhow::bail!("Key cannot be empty");
        }
        
        let value_str = serde_json::to_string(&entry.value)?;
        if value_str.len() > self.config.max_value_size {
            anyhow::bail!("Value too large: {} bytes (max: {})", value_str.len(), self.config.max_value_size);
        }
        
        let pool = self.get_pool().await?;
        
        // Check namespace entry count
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM memory_entries WHERE namespace = ?"
        )
        .bind(&entry.namespace)
        .fetch_one(&pool)
        .await?;
        
        if count >= self.config.max_entries_per_namespace as i64 {
            anyhow::bail!("Namespace '{}' has reached max entries", entry.namespace);
        }
        
        let tags_str = serde_json::to_string(&entry.tags)?;
        let metadata_str = serde_json::to_string(&entry.metadata)?;
        
        let vector_blob = entry.vector.as_ref().map(|v| {
            // Convert f32 vec to bytes
            v.iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>()
        });
        
        sqlx::query(
            r#"
            INSERT OR REPLACE INTO memory_entries 
            (key, namespace, value, tags, metadata, vector, created_at, updated_at, expires_at, access_count, last_accessed)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&entry.key)
        .bind(&entry.namespace)
        .bind(&value_str)
        .bind(&tags_str)
        .bind(&metadata_str)
        .bind(vector_blob)
        .bind(entry.created_at.to_rfc3339())
        .bind(entry.updated_at.to_rfc3339())
        .bind(entry.expires_at.map(|dt| dt.to_rfc3339()))
        .bind(entry.access_count as i64)
        .bind(entry.last_accessed.map(|dt| dt.to_rfc3339()))
        .execute(&pool)
        .await?;
        
        // Update vector index if enabled
        if self.config.enable_vector_search {
            if let Some(vector) = entry.vector {
                // This would require the sqlite-vec extension
                // For now, we'll skip actual vector indexing
            }
        }
        
        Ok(())
    }

    /// Retrieve a memory entry
    pub async fn retrieve(&self, key: &str, namespace: &str) -> Result<Option<MemoryEntry>> {
        let pool = self.get_pool().await?;
        
        let row = sqlx::query(
            "SELECT key, namespace, value, tags, metadata, vector, created_at, updated_at, expires_at, access_count, last_accessed 
             FROM memory_entries WHERE key = ? AND namespace = ?"
        )
        .bind(key)
        .bind(namespace)
        .fetch_optional(&pool)
        .await?;
        
        let Some(row) = row else {
            return Ok(None);
        };
        
        // Check expiry
        let expires_at: Option<String> = row.get("expires_at");
        if let Some(expires_str) = expires_at {
            let expires = DateTime::parse_from_rfc3339(&expires_str)?.with_timezone(&Utc);
            if expires < Utc::now() {
                // Expired - delete and return None
                self.delete(key, namespace).await?;
                return Ok(None);
            }
        }
        
        // Update access count and last accessed
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "UPDATE memory_entries SET access_count = access_count + 1, last_accessed = ? WHERE key = ? AND namespace = ?"
        )
        .bind(&now)
        .bind(key)
        .bind(namespace)
        .execute(&pool)
        .await?;
        
        let entry = self.row_to_entry(row)?;
        Ok(Some(entry))
    }

    /// Delete a memory entry
    pub async fn delete(&self, key: &str, namespace: &str) -> Result<bool> {
        let pool = self.get_pool().await?;
        
        let result = sqlx::query(
            "DELETE FROM memory_entries WHERE key = ? AND namespace = ?"
        )
        .bind(key)
        .bind(namespace)
        .execute(&pool)
        .await?;
        
        Ok(result.rows_affected() > 0)
    }

    /// List keys in a namespace
    pub async fn list_keys(&self, namespace: &str, limit: usize, offset: usize) -> Result<Vec<String>> {
        let pool = self.get_pool().await?;
        
        let rows = sqlx::query(
            "SELECT key FROM memory_entries WHERE namespace = ? ORDER BY updated_at DESC LIMIT ? OFFSET ?"
        )
        .bind(namespace)
        .bind(limit as i64)
        .bind(offset as i64)
        .fetch_all(&pool)
        .await?;
        
        Ok(rows.iter().map(|r| r.get("key")).collect())
    }

    /// Search by tags
    pub async fn search_by_tags(&self, namespace: &str, tags: &[String], limit: usize) -> Result<Vec<MemoryEntry>> {
        let pool = self.get_pool().await?;
        
        // Simple tag search - in production use FTS or proper tag indexing
        let tag_conditions = tags.iter()
            .map(|_| "tags LIKE ?")
            .collect::<Vec<_>>()
            .join(" OR ");
        
        let query = format!(
            "SELECT key, namespace, value, tags, metadata, vector, created_at, updated_at, expires_at, access_count, last_accessed 
             FROM memory_entries WHERE namespace = ? AND ({}) ORDER BY updated_at DESC LIMIT ?",
            tag_conditions
        );
        
        let mut q = sqlx::query(&query).bind(namespace);
        for tag in tags {
            q = q.bind(format!("%{}%", tag));
        }
        q = q.bind(limit as i64);
        
        let rows = q.fetch_all(&pool).await?;
        
        let mut entries = Vec::new();
        for row in rows {
            entries.push(self.row_to_entry(row)?);
        }
        
        Ok(entries)
    }

    /// Clean up expired entries
    pub async fn cleanup_expired(&self) -> Result<u64> {
        let pool = self.get_pool().await?;
        let now = Utc::now().to_rfc3339();
        
        let result = sqlx::query(
            "DELETE FROM memory_entries WHERE expires_at IS NOT NULL AND expires_at < ?"
        )
        .bind(&now)
        .execute(&pool)
        .await?;
        
        Ok(result.rows_affected())
    }

    /// Convert database row to MemoryEntry
    fn row_to_entry(&self, row: sqlx::sqlite::SqliteRow) -> Result<MemoryEntry> {
        let key: String = row.get("key");
        let namespace: String = row.get("namespace");
        let value_str: String = row.get("value");
        let tags_str: String = row.get("tags");
        let metadata_str: String = row.get("metadata");
        let vector_blob: Option<Vec<u8>> = row.get("vector");
        let created_at_str: String = row.get("created_at");
        let updated_at_str: String = row.get("updated_at");
        let expires_at_str: Option<String> = row.get("expires_at");
        let access_count: i64 = row.get("access_count");
        let last_accessed_str: Option<String> = row.get("last_accessed");
        
        let value = serde_json::from_str(&value_str)?;
        let tags = serde_json::from_str(&tags_str)?;
        let metadata = serde_json::from_str(&metadata_str)?;
        
        let vector = vector_blob.map(|bytes| {
            bytes.chunks_exact(4)
                .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                .collect()
        });
        
        Ok(MemoryEntry {
            key,
            namespace,
            value,
            tags,
            metadata,
            vector,
            created_at: DateTime::parse_from_rfc3339(&created_at_str)?.with_timezone(&Utc),
            updated_at: DateTime::parse_from_rfc3339(&updated_at_str)?.with_timezone(&Utc),
            expires_at: expires_at_str.map(|s| DateTime::parse_from_rfc3339(&s).unwrap().with_timezone(&Utc)),
            access_count: access_count as u64,
            last_accessed: last_accessed_str.map(|s| DateTime::parse_from_rfc3339(&s).unwrap().with_timezone(&Utc)),
        })
    }
}

#[async_trait]
impl Plugin for MemoryPlugin {
    fn name(&self) -> &str {
        "memory"
    }

    fn version(&self) -> &str {
        "1.0.0"
    }

    fn description(&self) -> &str {
        "Persistent memory storage plugin with key-value, tags, and optional vector search"
    }

    fn supported_phases(&self) -> Vec<Phase> {
        vec![Phase::Interrogate, Phase::Contract, Phase::Execute, Phase::Finish]
    }

    async fn initialize(&mut self, config: &serde_json::Value) -> Result<()> {
        if let Ok(cfg) = serde_json::from_value::<MemoryConfig>(config.clone()) {
            self.config = cfg;
        }
        
        // Initialize pool
        self.get_pool().await?;
        
        info!("Memory plugin initialized with database: {}", self.config.db_path);
        Ok(())
    }

    async fn execute(&self, phase: Phase, context: &mut PhaseContext) -> Result<PhaseResult> {
        let started_at = Utc::now();
        let start_time = std::time::Instant::now();
        
        // Demonstrate capabilities
        let mut output = serde_json::json!({
            "capabilities": {
                "key_value": true,
                "tags": true,
                "metadata": true,
                "vector_search": self.config.enable_vector_search,
                "ttl": self.config.default_ttl_secs > 0,
            },
            "db_path": self.config.db_path,
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
        info!("Memory plugin shutting down");
        let mut pool_guard = self.pool.write().await;
        if let Some(pool) = pool_guard.take() {
            pool.close().await;
        }
        Ok(())
    }

    fn capabilities(&self) -> PluginCapabilities {
        let mut caps = PluginCapabilities::default();
        caps.memory = true;
        caps
    }
}