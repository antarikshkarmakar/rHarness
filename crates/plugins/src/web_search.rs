//! Web search plugin

use async_trait::async_trait;
use mythos_harness_core::types::*;
use anyhow::{Context, Result};
use chrono::Utc;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;
use tracing::{debug, info, warn};

/// Web search plugin
pub struct WebSearchPlugin {
    config: WebSearchConfig,
    client: Client,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct WebSearchConfig {
    /// Search provider (google, bing, duckduckgo, brave, custom)
    pub provider: SearchProvider,
    /// API key (for providers that require it)
    pub api_key: Option<String>,
    /// Custom search endpoint
    pub custom_endpoint: Option<String>,
    /// Maximum results per query
    pub max_results: usize,
    /// Request timeout (seconds)
    pub timeout_secs: u64,
    /// User agent
    pub user_agent: String,
    /// Safe search
    pub safe_search: bool,
    /// Region/locale
    pub region: String,
    /// Rate limit (requests per minute)
    pub rate_limit_rpm: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchProvider {
    Google,
    Bing,
    DuckDuckGo,
    Brave,
    Custom,
}

impl Default for WebSearchConfig {
    fn default() -> Self {
        Self {
            provider: SearchProvider::DuckDuckGo,
            api_key: None,
            custom_endpoint: None,
            max_results: 10,
            timeout_secs: 30,
            user_agent: "MythosHarness/1.0".to_string(),
            safe_search: true,
            region: "us-en".to_string(),
            rate_limit_rpm: 60,
        }
    }
}

/// Search result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub source: String,
    pub rank: usize,
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Search response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResponse {
    pub query: String,
    pub results: Vec<SearchResult>,
    pub total_results: Option<usize>,
    pub search_time_ms: u64,
    pub provider: String,
}

impl WebSearchPlugin {
    /// Create a new web search plugin
    pub fn new(config: WebSearchConfig) -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .user_agent(&config.user_agent)
            .build()
            .context("Failed to create HTTP client")?;
        
        Ok(Self { config, client })
    }

    /// Perform a web search
    pub async fn search(&self, query: &str, max_results: Option<usize>) -> Result<SearchResponse> {
        let start = std::time::Instant::now();
        let max_results = max_results.unwrap_or(self.config.max_results).min(50);
        
        let results = match self.config.provider {
            SearchProvider::DuckDuckGo => self.search_duckduckgo(query, max_results).await?,
            SearchProvider::Google => self.search_google(query, max_results).await?,
            SearchProvider::Bing => self.search_bing(query, max_results).await?,
            SearchProvider::Brave => self.search_brave(query, max_results).await?,
            SearchProvider::Custom => self.search_custom(query, max_results).await?,
        };
        
        let search_time_ms = start.elapsed().as_millis() as u64;
        
        Ok(SearchResponse {
            query: query.to_string(),
            results,
            total_results: None,
            search_time_ms,
            provider: format!("{:?}", self.config.provider),
        })
    }

    /// Search using DuckDuckGo HTML scraping
    async fn search_duckduckgo(&self, query: &str, max_results: usize) -> Result<Vec<SearchResult>> {
        let url = "https://html.duckduckgo.com/html/";
        let params = [("q", query), ("kl", &self.config.region)];
        
        let response = self.client.post(url)
            .form(&params)
            .send()
            .await
            .context("Failed to send search request")?;
        
        let html = response.text().await
            .context("Failed to read response")?;
        
        // Parse HTML results (simplified)
        let results = self.parse_duckduckgo_html(&html, max_results)?;
        Ok(results)
    }

    /// Parse DuckDuckGo HTML results
    fn parse_duckduckgo_html(&self, html: &str, max_results: usize) -> Result<Vec<SearchResult>> {
        use scraper::{Html, Selector};
        
        let document = Html::parse_document(html);
        let fallback_selector =
            |name: &str| Selector::parse(name).unwrap_or_else(|_| Selector::parse("html").unwrap());
        let result_selector = fallback_selector(".result__snippet");
        let title_selector = fallback_selector(".result__title");
        let url_selector = fallback_selector(".result__url");

        let mut results = Vec::new();
        let mut rank = 0;

        // This is a simplified parser - in production use a more robust approach
        for element in document.select(&result_selector) {
            if rank >= max_results {
                break;
            }
            
            let snippet = element.text().collect::<String>().trim().to_string();
            
            // Try to find associated title and URL
            let title = element
                .select(&title_selector)
                .next()
                .map(|e| e.text().collect::<String>().trim().to_string())
                .unwrap_or_else(|| format!("Result {}", rank + 1));
            
            let url = element
                .select(&url_selector)
                .next()
                .map(|e| e.text().collect::<String>().trim().to_string())
                .unwrap_or_default();
            
            if !snippet.is_empty() {
                results.push(SearchResult {
                    title,
                    url,
                    snippet,
                    source: "duckduckgo".to_string(),
                    rank,
                    metadata: HashMap::new(),
                });
                rank += 1;
            }
        }
        
        Ok(results)
    }

    /// Search using Google Custom Search API
    async fn search_google(&self, query: &str, max_results: usize) -> Result<Vec<SearchResult>> {
        let api_key = self.config.api_key.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Google API key required"))?;
        
        let cx = std::env::var("GOOGLE_CSE_ID")
            .context("Google Custom Search Engine ID required (GOOGLE_CSE_ID env var)")?;
        
        let url = "https://www.googleapis.com/customsearch/v1";
        let params = [
            ("key", api_key.as_str()),
            ("cx", cx.as_str()),
            ("q", query),
            ("num", &max_results.to_string()),
            ("safe", if self.config.safe_search { "active" } else { "off" }),
        ];
        
        let response = self.client.get(url)
            .query(&params)
            .send()
            .await
            .context("Failed to send Google search request")?;
        
        let json: serde_json::Value = response.json().await
            .context("Failed to parse Google response")?;
        
        let mut results = Vec::new();
        if let Some(items) = json.get("items").and_then(|i| i.as_array()) {
            for (rank, item) in items.iter().enumerate() {
                if rank >= max_results {
                    break;
                }
                
                results.push(SearchResult {
                    title: item.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    url: item.get("link").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    snippet: item.get("snippet").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    source: "google".to_string(),
                    rank,
                    metadata: HashMap::new(),
                });
            }
        }
        
        Ok(results)
    }

    /// Search using Bing Web Search API
    async fn search_bing(&self, query: &str, max_results: usize) -> Result<Vec<SearchResult>> {
        let api_key = self.config.api_key.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Bing API key required"))?;
        
        let url = "https://api.bing.microsoft.com/v7.0/search";
        let params = [
            ("q", query),
            ("count", &max_results.to_string()),
            ("safeSearch", if self.config.safe_search { "Strict" } else { "Off" }),
            ("mkt", &self.config.region),
        ];
        
        let response = self.client.get(url)
            .query(&params)
            .header("Ocp-Apim-Subscription-Key", api_key)
            .send()
            .await
            .context("Failed to send Bing search request")?;
        
        let json: serde_json::Value = response.json().await
            .context("Failed to parse Bing response")?;
        
        let mut results = Vec::new();
        if let Some(web_pages) = json.get("webPages").and_then(|w| w.get("value")).and_then(|v| v.as_array()) {
            for (rank, item) in web_pages.iter().enumerate() {
                if rank >= max_results {
                    break;
                }
                
                results.push(SearchResult {
                    title: item.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    url: item.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    snippet: item.get("snippet").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    source: "bing".to_string(),
                    rank,
                    metadata: HashMap::new(),
                });
            }
        }
        
        Ok(results)
    }

    /// Search using Brave Search API
    async fn search_brave(&self, query: &str, max_results: usize) -> Result<Vec<SearchResult>> {
        let api_key = self.config.api_key.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Brave API key required"))?;
        
        let url = "https://api.search.brave.com/res/v1/web/search";
        let params = [
            ("q", query),
            ("count", &max_results.to_string()),
            ("safesearch", if self.config.safe_search { "strict" } else { "off" }),
        ];
        
        let response = self.client.get(url)
            .query(&params)
            .header("Accept", "application/json")
            .header("X-Subscription-Token", api_key)
            .send()
            .await
            .context("Failed to send Brave search request")?;
        
        let json: serde_json::Value = response.json().await
            .context("Failed to parse Brave response")?;
        
        let mut results = Vec::new();
        if let Some(web) = json.get("web").and_then(|w| w.get("results")).and_then(|v| v.as_array()) {
            for (rank, item) in web.iter().enumerate() {
                if rank >= max_results {
                    break;
                }
                
                results.push(SearchResult {
                    title: item.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    url: item.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    snippet: item.get("description").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    source: "brave".to_string(),
                    rank,
                    metadata: HashMap::new(),
                });
            }
        }
        
        Ok(results)
    }

    /// Search using custom endpoint
    async fn search_custom(&self, query: &str, max_results: usize) -> Result<Vec<SearchResult>> {
        let endpoint = self.config.custom_endpoint.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Custom search endpoint required"))?;
        
        let response = self.client.post(endpoint)
            .json(&serde_json::json!({
                "query": query,
                "max_results": max_results,
            }))
            .send()
            .await
            .context("Failed to send custom search request")?;
        
        let json: serde_json::Value = response.json().await
            .context("Failed to parse custom response")?;
        
        // Expect custom format - adapt as needed
        let mut results = Vec::new();
        if let Some(items) = json.get("results").and_then(|v| v.as_array()) {
            for (rank, item) in items.iter().enumerate() {
                if rank >= max_results {
                    break;
                }
                
                results.push(SearchResult {
                    title: item.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    url: item.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    snippet: item.get("snippet").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    source: "custom".to_string(),
                    rank,
                    metadata: HashMap::new(),
                });
            }
        }
        
        Ok(results)
    }
}

#[async_trait]
impl Plugin for WebSearchPlugin {
    fn name(&self) -> &str {
        "web-search"
    }

    fn version(&self) -> &str {
        "1.0.0"
    }

    fn description(&self) -> &str {
        "Web search plugin supporting multiple providers (DuckDuckGo, Google, Bing, Brave)"
    }

    fn supported_phases(&self) -> Vec<Phase> {
        vec![Phase::Interrogate, Phase::Contract, Phase::Execute]
    }

    async fn initialize(&mut self, config: &serde_json::Value) -> Result<()> {
        if let Ok(cfg) = serde_json::from_value::<WebSearchConfig>(config.clone()) {
            self.config = cfg;
        }
        info!("Web search plugin initialized with provider: {:?}", self.config.provider);
        Ok(())
    }

    async fn execute(&self, phase: Phase, context: &mut PhaseContext) -> Result<PhaseResult> {
        let started_at = Utc::now();
        let start_time = std::time::Instant::now();
        
        // Demonstrate capabilities
        let mut output = serde_json::json!({
            "capabilities": {
                "search": true,
                "provider": format!("{:?}", self.config.provider),
                "max_results": self.config.max_results,
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
        info!("Web search plugin shutting down");
        Ok(())
    }

    fn capabilities(&self) -> PluginCapabilities {
        let mut caps = PluginCapabilities::default();
        caps.web_search = true;
        caps
    }
}