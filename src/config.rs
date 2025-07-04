use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use eyre::Result;
use serde::Deserialize;
use serde_with::{DurationSeconds, serde_as};

#[serde_as]
#[derive(Clone, Deserialize)]
pub struct Config {
    pub data_dir: PathBuf,

    #[serde(default)]
    pub cache_entries: u64,

    #[serde_as(as = "DurationSeconds<f64>")]
    #[serde(default)]
    pub cache_duration: Duration,

    #[serde(default)]
    pub crawler_accept_language: String,

    #[serde(default)]
    pub crawler_proxy: String,

    #[serde(default)]
    pub crawler_max_size: usize,

    #[serde_as(as = "DurationSeconds<f64>")]
    #[serde(default)]
    pub crawler_timeout: Duration,

    #[serde(default)]
    pub crawler_user_agent: String,

    #[serde(default)]
    pub rewrite_url: Vec<[String; 2]>,
}

impl Config {
    pub async fn new(path: &Path) -> Result<Arc<Config>> {
        let config_str = tokio::fs::read_to_string(path).await?;
        let mut config: Config = toml::from_str(&config_str)?;
        if config.cache_entries == 0 {
            config.cache_entries = 1024;
        }
        if config.cache_duration.is_zero() {
            config.cache_duration = Duration::from_secs(3600);
        }
        if config.crawler_accept_language.is_empty() {
            config.crawler_accept_language = "en-US,en;q=0.9".to_owned();
        }
        if config.crawler_max_size == 0 {
            config.crawler_max_size = 10 * 1048576;
        }
        if config.crawler_timeout.is_zero() {
            config.crawler_timeout = Duration::from_secs(30);
        }
        if config.crawler_user_agent.is_empty() {
            config.crawler_user_agent =
                "Mozilla/5.0 (compatible; Matrix-URL-Previewer-Bot; +https://github.com/m13253/matrix-url-previewer-bot; like Discordbot, TelegramBot, Twitterbot)".to_owned();
        }
        Ok(Arc::new(config))
    }
}
