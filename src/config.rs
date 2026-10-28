use anyhow::{Context, Result, bail};
use std::{env, path::PathBuf, time::Duration};

pub struct Config {
    pub discord_token: String,
    pub channel_id: u64,
    pub ping_role_id: Option<u64>,
    /// Which posts ping the role.
    pub ping_events: Vec<Event>,
    pub state_file: PathBuf,
    pub pics_interval: Duration,
    pub news_interval: Duration,
    pub twitter_interval: Duration,
    pub twitter_handle: String,
    pub status_interval: Duration,
    pub presence_platform: Platform,
    pub auto_update: bool,
    pub update_interval: Duration,
}

/// Kinds of posts, used to decide which ones ping the role.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Update,
    Changelist,
    Depots,
    News,
    Banner,
    Avatar,
}

const EVENTS: [(&str, Event); 6] = [
    ("update", Event::Update),
    ("changelist", Event::Changelist),
    ("depots", Event::Depots),
    ("news", Event::News),
    ("banner", Event::Banner),
    ("avatar", Event::Avatar),
];

/// Which client the bot pretends to be, deciding the platform icon next to
/// its online status.
#[derive(Clone, Copy, Debug)]
pub enum Platform {
    Vr,
    Mobile,
    Desktop,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            discord_token: env::var("DISCORD_TOKEN").context("DISCORD_TOKEN is not set")?,
            channel_id: env::var("DISCORD_CHANNEL_ID")
                .context("DISCORD_CHANNEL_ID is not set")?
                .parse()
                .context("DISCORD_CHANNEL_ID is not a valid ID")?,
            ping_role_id: number("PING_ROLE_ID")?,
            ping_events: events(non_empty("PING_EVENTS").as_deref().unwrap_or("update,news"))?,
            state_file: env::var("STATE_FILE")
                .unwrap_or_else(|_| "state.json".into())
                .into(),
            pics_interval: Duration::from_millis(number("PICS_INTERVAL_MS")?.unwrap_or(2000)),
            news_interval: Duration::from_secs(number("NEWS_INTERVAL_SECS")?.unwrap_or(20)),
            twitter_interval: Duration::from_secs(number("TWITTER_INTERVAL_SECS")?.unwrap_or(60)),
            twitter_handle: env::var("TWITTER_HANDLE").unwrap_or_else(|_| "CounterStrike".into()),
            status_interval: Duration::from_secs(
                number("STATUS_INTERVAL_SECS")?.unwrap_or(60).max(15),
            ),
            presence_platform: platform()?,
            auto_update: flag("AUTO_UPDATE")?.unwrap_or(true),
            update_interval: Duration::from_secs(
                number("UPDATE_INTERVAL_MINS")?.unwrap_or(10).max(1) * 60,
            ),
        })
    }
}

fn non_empty(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
}

fn number(key: &str) -> Result<Option<u64>> {
    non_empty(key)
        .map(|v| v.parse().with_context(|| format!("{key} is not a number")))
        .transpose()
}

fn flag(key: &str) -> Result<Option<bool>> {
    non_empty(key)
        .map(|v| match v.to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Ok(true),
            "0" | "false" | "no" | "off" => Ok(false),
            _ => bail!("{key} must be true or false"),
        })
        .transpose()
}

fn platform() -> Result<Platform> {
    Ok(
        match non_empty("PRESENCE_PLATFORM")
            .unwrap_or_else(|| "vr".into())
            .to_ascii_lowercase()
            .as_str()
        {
            "vr" => Platform::Vr,
            "mobile" => Platform::Mobile,
            "desktop" => Platform::Desktop,
            other => bail!("PRESENCE_PLATFORM must be vr, mobile or desktop, not {other:?}"),
        },
    )
}

/// Parses a comma-separated list like `update,news`, or `all` / `none`.
fn events(list: &str) -> Result<Vec<Event>> {
    match list.trim().to_ascii_lowercase().as_str() {
        "all" => return Ok(EVENTS.iter().map(|&(_, e)| e).collect()),
        "none" => return Ok(Vec::new()),
        _ => {}
    }
    list.split(',')
        .map(|name| name.trim().to_ascii_lowercase())
        .filter(|name| !name.is_empty())
        .map(|name| {
            EVENTS
                .iter()
                .find(|(n, _)| *n == name)
                .map(|&(_, e)| e)
                .with_context(|| {
                    format!("PING_EVENTS: unknown event {name:?}, use update, changelist, depots, news, banner, avatar, all or none")
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ping_events() {
        assert_eq!(events("update,news").unwrap(), [Event::Update, Event::News]);
        assert_eq!(
            events(" Banner , depots ").unwrap(),
            [Event::Banner, Event::Depots]
        );
        assert_eq!(events("all").unwrap().len(), 6);
        assert!(events("none").unwrap().is_empty());
        assert!(events("updates").is_err());
    }
}
