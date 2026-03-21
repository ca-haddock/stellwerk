use anyhow::{Context, Result};
use serde_json::json;
use tracing::debug;

use crate::config::HomeAssistantConfig;

#[derive(Clone)]
pub struct HomeAssistantClient {
    http: reqwest::Client,
    base_url: String,
    token: String,
    starlink_entity: String,
}

impl HomeAssistantClient {
    pub fn new(cfg: &HomeAssistantConfig) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("Failed to build HTTP client");

        Self {
            http,
            base_url: cfg.url.trim_end_matches('/').to_string(),
            token: cfg.token.clone(),
            starlink_entity: cfg.starlink_entity.clone(),
        }
    }

    pub async fn turn_on_starlink(&self) -> Result<()> {
        self.call_service("switch", "turn_on", &self.starlink_entity.clone()).await
    }

    pub async fn turn_off_starlink(&self) -> Result<()> {
        self.call_service("switch", "turn_off", &self.starlink_entity.clone()).await
    }

    pub async fn get_state(&self, entity_id: &str) -> Result<String> {
        let url = format!("{}/api/states/{}", self.base_url, entity_id);
        let resp = self.http
            .get(&url)
            .bearer_auth(&self.token)
            .header("Content-Type", "application/json")
            .send()
            .await
            .context("HA API request failed")?;

        let json: serde_json::Value = resp.json().await?;
        Ok(json["state"].as_str().unwrap_or("unknown").to_string())
    }

    async fn call_service(&self, domain: &str, service: &str, entity_id: &str) -> Result<()> {
        let url = format!("{}/api/services/{}/{}", self.base_url, domain, service);
        debug!("HA call: {} {}", url, entity_id);

        let resp = self.http
            .post(&url)
            .bearer_auth(&self.token)
            .header("Content-Type", "application/json")
            .json(&json!({ "entity_id": entity_id }))
            .send()
            .await
            .context("HA API request failed")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!("HA API error {}: {}", status, body));
        }
        Ok(())
    }
}
