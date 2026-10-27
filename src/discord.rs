use anyhow::Result;
use twilight_http::Client;
use twilight_model::{
    channel::message::AllowedMentions,
    id::{
        Id,
        marker::{ChannelMarker, RoleMarker},
    },
    util::Timestamp,
};
use twilight_util::builder::embed::{EmbedBuilder, ImageSource};

pub const CS2_COLOR: u32 = 0xDE9B35;
pub const CS2_IMAGE: &str = "https://cdn.cloudflare.steamstatic.com/steam/apps/730/header.jpg";
pub const CS2_ICON: &str =
    "https://cdn.cloudflare.steamstatic.com/steam/apps/730/capsule_231x87.jpg";

pub struct Discord {
    http: Client,
    channel: Id<ChannelMarker>,
    ping_role: Option<Id<RoleMarker>>,
}

impl Discord {
    pub fn new(token: &str, channel_id: u64, ping_role_id: Option<u64>) -> Self {
        Self {
            http: Client::new(token.to_owned()),
            channel: Id::new(channel_id),
            ping_role: ping_role_id.map(Id::new),
        }
    }

    pub fn http(&self) -> &Client {
        &self.http
    }

    /// Fails fast at startup if the token or channel is wrong.
    pub async fn check(&self) -> Result<String> {
        let channel = self.http.channel(self.channel).await?.model().await?;
        Ok(channel
            .name
            .map(|n| format!("#{n}"))
            .unwrap_or_else(|| self.channel.to_string()))
    }

    pub async fn send(&self, embed: EmbedBuilder, ping: bool) -> Result<()> {
        let embeds = [embed.build()];
        let role = self.ping_role.filter(|_| ping);
        let mentions = AllowedMentions {
            roles: role.into_iter().collect(),
            ..Default::default()
        };
        let content = role.map(|r| format!("<@&{r}>")).unwrap_or_default();

        let mut msg = self
            .http
            .create_message(self.channel)
            .embeds(&embeds)
            .allowed_mentions(Some(&mentions));
        if !content.is_empty() {
            msg = msg.content(&content);
        }
        msg.await?;
        Ok(())
    }
}

/// Base embed with the CS2 look shared by all posts.
pub fn base_embed() -> EmbedBuilder {
    EmbedBuilder::new().color(CS2_COLOR).timestamp(now())
}

pub fn image(url: impl Into<String>) -> ImageSource {
    ImageSource::url(url).unwrap_or_else(|_| ImageSource::url(CS2_IMAGE).unwrap())
}

pub fn timestamp(unix: i64) -> Timestamp {
    Timestamp::from_secs(unix).unwrap_or_else(|_| now())
}

fn now() -> Timestamp {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    Timestamp::from_secs(secs).expect("current time is a valid timestamp")
}

/// Cuts `text` to at most `max` chars, preferring a line break.
pub fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let cut: String = text.chars().take(max.saturating_sub(1)).collect();
    let cut = match cut.rfind('\n') {
        Some(i) if i > max / 2 => &cut[..i],
        _ => &cut,
    };
    format!("{}…", cut.trim_end())
}
