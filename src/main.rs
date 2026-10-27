mod config;
mod depots;
mod discord;
mod news;
mod state;
mod steam;
mod twitter;
mod vdf;

use anyhow::Result;
use config::Config;
use discord::Discord;
use state::Store;
use std::{sync::Arc, time::Duration};
use tracing::info;

pub struct Ctx {
    pub config: Config,
    pub discord: Discord,
    pub store: Store,
    pub http: reqwest::Client,
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,steam_vent=warn,serenity=warn".into()),
        )
        .init();

    let config = Config::from_env()?;
    let discord = Discord::new(
        &config.discord_token,
        config.channel_id,
        config.ping_role_id,
    );
    let channel = discord.check().await?;
    info!("Poste in {channel}");

    let ctx = Arc::new(Ctx {
        store: Store::load(&config.state_file)?,
        http: reqwest::Client::builder()
            .user_agent("cs2-tracker-bot/0.1")
            .timeout(Duration::from_secs(15))
            .build()?,
        discord,
        config,
    });

    tokio::select! {
        _ = steam::run(ctx.clone()) => {}
        _ = news::run(ctx.clone()) => {}
        _ = twitter::run(ctx.clone()) => {}
        _ = tokio::signal::ctrl_c() => info!("Beende…"),
    }
    Ok(())
}
