//! Detects profile banner changes on X/Twitter via the public FxTwitter API.

use crate::{
    Ctx,
    discord::{CS2_ICON, base_embed, timestamp},
};
use anyhow::Result;
use serde::Deserialize;
use serenity::all::{CreateEmbedAuthor, CreateEmbedFooter};
use std::sync::Arc;
use tokio::time::{MissedTickBehavior, interval};
use tracing::{debug, info, warn};

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
            warn!("Twitter-Check fehlgeschlagen: {e:#}");
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
        debug!("FxTwitter lieferte Code {}", resp.code);
        return Ok(());
    };
    let Some(banner) = user.banner_url else {
        return Ok(());
    };

    let old = ctx.store.read(|s| s.twitter_banner.clone()).await;
    match old {
        None => info!("Twitter-Banner als Basis gespeichert: {banner}"),
        Some(old) if old != banner => {
            info!("Neuer Twitter-Banner: {banner}");
            let mut embed = base_embed()
                .author(CreateEmbedAuthor::new(format!("@{handle} auf X")).icon_url(CS2_ICON))
                .title("🖼️ Neuer Twitter-Banner!")
                .url(format!("https://x.com/{handle}"))
                .description(format!(
                    "[@{handle}](https://x.com/{handle}) hat den Profil-Banner geändert.\n[Alter Banner]({old}/1500x500)"
                ))
                .image(format!("{banner}/1500x500"))
                .footer(CreateEmbedFooter::new("X / Twitter"));
            if let Some(avatar) = user.avatar_url {
                embed = embed.thumbnail(avatar.replace("_normal.", "_400x400."));
            }
            // Banner URLs end in the upload's unix timestamp.
            if let Some(t) = banner.rsplit('/').next().and_then(|t| t.parse().ok()) {
                embed = embed.timestamp(timestamp(t));
            }
            ctx.discord.send(embed, false).await?;
        }
        Some(_) => return Ok(()),
    }
    ctx.store.update(|s| s.twitter_banner = Some(banner)).await;
    Ok(())
}
