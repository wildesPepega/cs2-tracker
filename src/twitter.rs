//! Detects profile banner changes on X/Twitter via the public FxTwitter API.

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
    let Some(banner) = user.banner_url else {
        return Ok(());
    };

    let old = ctx.store.read(|s| s.twitter_banner.clone()).await;
    match old {
        None => info!("Stored Twitter banner as baseline: {banner}"),
        Some(old) if old != banner => {
            info!("New Twitter banner: {banner}");
            let embed = banner_embed(handle, Some(&old), &banner, user.avatar_url.as_deref());
            ctx.discord.send(embed, Event::Banner).await?;
        }
        Some(_) => return Ok(()),
    }
    ctx.store.update(|s| s.twitter_banner = Some(banner)).await;
    Ok(())
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
        embed = embed.thumbnail(image(avatar.replace("_normal.", "_400x400.")));
    }
    // Banner URLs end in the upload's unix timestamp.
    if let Some(t) = banner.rsplit('/').next().and_then(|t| t.parse().ok()) {
        embed = embed.timestamp(timestamp(t));
    }
    embed
}
