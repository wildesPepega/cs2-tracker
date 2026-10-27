use anyhow::{Context, Result};
use std::{env, path::PathBuf, time::Duration};

pub struct Config {
    pub discord_token: String,
    pub channel_id: u64,
    /// Role pinged when a new public CS2 build goes live.
    pub ping_role_id: Option<u64>,
    pub state_file: PathBuf,
    pub pics_interval: Duration,
    pub news_interval: Duration,
    pub twitter_interval: Duration,
    pub twitter_handle: String,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            discord_token: env::var("DISCORD_TOKEN").context("DISCORD_TOKEN fehlt")?,
            channel_id: env::var("DISCORD_CHANNEL_ID")
                .context("DISCORD_CHANNEL_ID fehlt")?
                .parse()
                .context("DISCORD_CHANNEL_ID ist keine gültige ID")?,
            ping_role_id: optional("PING_ROLE_ID")?,
            state_file: env::var("STATE_FILE")
                .unwrap_or_else(|_| "state.json".into())
                .into(),
            pics_interval: Duration::from_millis(optional("PICS_INTERVAL_MS")?.unwrap_or(2000)),
            news_interval: Duration::from_secs(optional("NEWS_INTERVAL_SECS")?.unwrap_or(20)),
            twitter_interval: Duration::from_secs(optional("TWITTER_INTERVAL_SECS")?.unwrap_or(60)),
            twitter_handle: env::var("TWITTER_HANDLE").unwrap_or_else(|_| "CounterStrike".into()),
        })
    }
}

fn optional(key: &str) -> Result<Option<u64>> {
    match env::var(key) {
        Ok(v) if !v.trim().is_empty() => Ok(Some(
            v.trim().parse().with_context(|| format!("{key} ist keine Zahl"))?,
        )),
        _ => Ok(None),
    }
}
