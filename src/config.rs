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
    /// Set when `TS_ADDRESS` is.
    pub teamspeak: Option<TeamSpeakConfig>,
    /// Set when all four X API credentials are.
    pub x: Option<XConfig>,
}

/// X API app keys plus the access token of the account that posts.
pub struct XConfig {
    pub api_key: String,
    pub api_secret: String,
    pub access_token: String,
    pub access_secret: String,
    /// Which events get posted.
    pub events: Vec<Event>,
}

pub struct TeamSpeakConfig {
    pub address: String,
    pub nickname: String,
    /// Channel name or path like `Gaming/CS2`; `None` stays in the default
    /// channel.
    pub channel: Option<String>,
    pub channel_password: Option<String>,
    pub server_password: Option<String>,
    /// Ogg Opus file; `None` plays the bundled sound.
    pub sound_file: Option<PathBuf>,
    /// Which posts play the sound.
    pub events: Vec<Event>,
}

/// Kinds of posts, used to decide which ones ping the role.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Update,
    Changelist,
    /// A changelist of the DevClient or DevServer app.
    Dev,
    Depots,
    News,
    Banner,
    Avatar,
}

const EVENTS: [(&str, Event); 7] = [
    ("update", Event::Update),
    ("changelist", Event::Changelist),
    ("dev", Event::Dev),
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
            ping_events: events("PING_EVENTS", "update,news")?,
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
            teamspeak: non_empty("TS_ADDRESS")
                .map(|address| -> Result<_> {
                    Ok(TeamSpeakConfig {
                        address,
                        nickname: non_empty("TS_NICKNAME").unwrap_or_else(|| "CS2 Tracker".into()),
                        channel: non_empty("TS_CHANNEL"),
                        channel_password: non_empty("TS_CHANNEL_PASSWORD"),
                        server_password: non_empty("TS_SERVER_PASSWORD"),
                        sound_file: non_empty("TS_SOUND_FILE").map(PathBuf::from),
                        events: events("TS_EVENTS", "update")?,
                    })
                })
                .transpose()?,
            x: x()?,
        })
    }
}

fn x() -> Result<Option<XConfig>> {
    let keys = [
        "X_API_KEY",
        "X_API_SECRET",
        "X_ACCESS_TOKEN",
        "X_ACCESS_SECRET",
    ];
    let values = keys.map(non_empty);
    if values.iter().all(Option::is_none) {
        return Ok(None);
    }
    let missing: Vec<_> = keys
        .iter()
        .zip(&values)
        .filter(|(_, v)| v.is_none())
        .map(|(k, _)| *k)
        .collect();
    if !missing.is_empty() {
        bail!(
            "X posting needs all four keys, missing {}",
            missing.join(", ")
        );
    }
    let [api_key, api_secret, access_token, access_secret] = values.map(Option::unwrap_or_default);
    Ok(Some(XConfig {
        api_key,
        api_secret,
        access_token,
        access_secret,
        events: events("X_EVENTS", "update,dev,banner,avatar")?,
    }))
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

/// Reads a comma-separated list like `update,news`, or `all` / `none`.
fn events(key: &str, default: &str) -> Result<Vec<Event>> {
    parse_events(non_empty(key).as_deref().unwrap_or(default))
        .with_context(|| format!("{key} is invalid"))
}

fn parse_events(list: &str) -> Result<Vec<Event>> {
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
                    format!("unknown event {name:?}, use update, changelist, dev, depots, news, banner, avatar, all or none")
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ping_events() {
        assert_eq!(
            parse_events("update,news").unwrap(),
            [Event::Update, Event::News]
        );
        assert_eq!(
            parse_events(" Banner , depots ").unwrap(),
            [Event::Banner, Event::Depots]
        );
        assert_eq!(parse_events("all").unwrap().len(), 7);
        assert!(parse_events("none").unwrap().is_empty());
        assert!(parse_events("updates").is_err());
    }
}
