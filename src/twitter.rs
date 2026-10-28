//! Detects profile banner and profile picture changes on X/Twitter via the
//! public FxTwitter API.

use crate::{
    Ctx,
    config::Event,
    discord::{CS2_ICON, base_embed, image, timestamp},
};
use anyhow::Result;
use serde::Deserialize;
use std::sync::Arc;
use tokio::time::{MissedTickBehavior, interval};
use tracing::{debug, info, warn};
use twilight_util::builder::embed::{EmbedAuthorBuilder, EmbedBuilder, EmbedFooterBuilder};

#[derive(Deserialize)]
struct Response {
    code: u16,
    user: Option<User>,
}

#[derive(Deserialize)]
struct User {
    banner_url: Option<String>,
    avatar_url: Option<String>,
}

pub async fn run(ctx: Arc<Ctx>) {
    let mut tick = interval(ctx.config.twitter_interval);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        if let Err(e) = check(&ctx).await {
            warn!("Twitter check failed: {e:#}");
        }
    }
}

async fn check(ctx: &Ctx) -> Result<()> {
    let handle = &ctx.config.twitter_handle;
    let resp: Response = ctx
        .http
        .get(format!("https://api.fxtwitter.com/{handle}"))
        .send()
        .await?
        .json()
        .await?;
    // FxTwitter sporadically answers 404 for existing users; just retry next tick.
    let Some(user) = resp.user.filter(|_| resp.code == 200) else {
        debug!("FxTwitter returned code {}", resp.code);
        return Ok(());
    };
    let avatar = user.avatar_url.as_deref().map(full_size_avatar);
    if let Some(banner) = user.banner_url {
        check_banner(ctx, banner, avatar.as_deref()).await?;
    }
    if let Some(avatar) = avatar {
        check_avatar(ctx, avatar).await?;
    }
    Ok(())
}

async fn check_banner(ctx: &Ctx, banner: String, avatar: Option<&str>) -> Result<()> {
    let handle = &ctx.config.twitter_handle;
    let old = ctx.store.read(|s| s.twitter_banner.clone()).await;
    match old {
        None => info!("Stored Twitter banner as baseline: {banner}"),
        Some(old) if old != banner => {
            info!("New Twitter banner: {banner}");
            let embed = banner_embed(handle, Some(&old), &banner, avatar);
            ctx.discord.send(embed, Event::Banner).await?;
        }
        Some(_) => return Ok(()),
    }
    ctx.store.update(|s| s.twitter_banner = Some(banner)).await;
    Ok(())
}

async fn check_avatar(ctx: &Ctx, avatar: String) -> Result<()> {
    let handle = &ctx.config.twitter_handle;
    let old = ctx.store.read(|s| s.twitter_avatar.clone()).await;
    match old {
        None => info!("Stored Twitter profile picture as baseline: {avatar}"),
        Some(old) if old != avatar => {
            info!("New Twitter profile picture: {avatar}");
            let embed = avatar_embed(handle, Some(&old), &avatar);
            ctx.discord.send(embed, Event::Avatar).await?;
        }
        Some(_) => return Ok(()),
    }
    ctx.store.update(|s| s.twitter_avatar = Some(avatar)).await;
    Ok(())
}

/// Avatar URLs come in the 48px `_normal` size; the image id only changes on
/// a new upload, so the full-size variant is stable to compare against.
fn full_size_avatar(url: &str) -> String {
    url.replace("_normal.", "_400x400.")
}

pub fn banner_embed(
    handle: &str,
    old: Option<&str>,
    banner: &str,
    avatar: Option<&str>,
) -> EmbedBuilder {
    let mut description =
        format!("[@{handle}](https://x.com/{handle}) changed their profile banner.");
    if let Some(old) = old {
        description.push_str(&format!("\n[Previous banner]({old}/1500x500)"));
    }
    let mut embed = base_embed()
        .author(EmbedAuthorBuilder::new(format!("@{handle} on X")).icon_url(image(CS2_ICON)))
        .title("🖼️ New Twitter banner!")
        .url(format!("https://x.com/{handle}"))
        .description(description)
        .image(image(format!("{banner}/1500x500")))
        .footer(EmbedFooterBuilder::new("X / Twitter"));
    if let Some(avatar) = avatar {
        embed = embed.thumbnail(image(avatar));
    }
    // Banner URLs end in the upload's unix timestamp.
    if let Some(t) = banner.rsplit('/').next().and_then(|t| t.parse().ok()) {
        embed = embed.timestamp(timestamp(t));
    }
    embed
}

pub fn avatar_embed(handle: &str, old: Option<&str>, avatar: &str) -> EmbedBuilder {
    let mut description =
        format!("[@{handle}](https://x.com/{handle}) changed their profile picture.");
    if let Some(old) = old {
        description.push_str(&format!("\n[Previous profile picture]({old})"));
    }
    let mut embed = base_embed()
        .author(EmbedAuthorBuilder::new(format!("@{handle} on X")).icon_url(image(CS2_ICON)))
        .title("👤 New Twitter profile picture!")
        .url(format!("https://x.com/{handle}"))
        .description(description)
        .image(image(avatar))
        .footer(EmbedFooterBuilder::new("X / Twitter"));
    if let Some(old) = old {
        embed = embed.thumbnail(image(old));
    }
    embed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn avatar_url_is_full_size() {
        assert_eq!(
            full_size_avatar("https://pbs.twimg.com/profile_images/1/abc_normal.jpg"),
            "https://pbs.twimg.com/profile_images/1/abc_400x400.jpg"
        );
    }

    #[test]
    fn avatar_embed_passes_discord_validation() {
        let url = "https://pbs.twimg.com/profile_images/1/abc_400x400.jpg";
        let _ = avatar_embed("CounterStrike", Some(url), url)
            .validate()
            .expect("embed within Discord limits");
    }
}
