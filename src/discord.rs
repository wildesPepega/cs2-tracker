use anyhow::Result;
use serenity::{
    all::{ChannelId, CreateAllowedMentions, CreateEmbed, CreateMessage, Http, RoleId},
    model::Timestamp,
};

pub const CS2_COLOR: u32 = 0xDE9B35;
pub const CS2_IMAGE: &str = "https://cdn.cloudflare.steamstatic.com/steam/apps/730/header.jpg";
pub const CS2_ICON: &str =
    "https://cdn.cloudflare.steamstatic.com/steam/apps/730/capsule_231x87.jpg";

pub struct Discord {
    http: Http,
    channel: ChannelId,
    ping_role: Option<RoleId>,
}

impl Discord {
    pub fn new(token: &str, channel_id: u64, ping_role_id: Option<u64>) -> Self {
        Self {
            http: Http::new(token),
            channel: ChannelId::new(channel_id),
            ping_role: ping_role_id.map(RoleId::new),
        }
    }

    /// Fails fast at startup if the token or channel is wrong.
    pub async fn check(&self) -> Result<String> {
        let channel = self.channel.to_channel(&self.http).await?;
        Ok(channel
            .guild()
            .map(|c| format!("#{}", c.name))
            .unwrap_or_else(|| self.channel.to_string()))
    }

    pub async fn send(&self, embed: CreateEmbed, ping: bool) -> Result<()> {
        let mut msg = CreateMessage::new().embed(embed);
        match self.ping_role.filter(|_| ping) {
            Some(role) => {
                msg = msg
                    .content(format!("<@&{role}>"))
                    .allowed_mentions(CreateAllowedMentions::new().roles([role]));
            }
            None => msg = msg.allowed_mentions(CreateAllowedMentions::new()),
        }
        self.channel.send_message(&self.http, msg).await?;
        Ok(())
    }
}

/// Base embed with the CS2 look shared by all posts.
pub fn base_embed() -> CreateEmbed {
    CreateEmbed::new()
        .colour(CS2_COLOR)
        .timestamp(Timestamp::now())
}

pub fn timestamp(unix: i64) -> Timestamp {
    Timestamp::from_unix_timestamp(unix).unwrap_or_else(|_| Timestamp::now())
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
