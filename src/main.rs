mod config;
mod depots;
mod discord;
mod news;
mod presence;
mod state;
mod steam;
mod twitter;
mod updater;
mod vdf;

use anyhow::{Context, Result};
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
    if std::env::args().any(|a| a == "--version" || a == "-V") {
        println!("cs2-tracker {}", updater::VERSION);
        return Ok(());
    }
    // Resolve before an update renames the file under us.
    let exe = std::env::current_exe().context("cannot locate own executable")?;

    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,steam_vent=warn,twilight_gateway=warn".into()),
        )
        .init();
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    info!("cs2-tracker v{}", updater::VERSION);

    let config = Config::from_env()?;
    let discord = Discord::new(
        &config.discord_token,
        config.channel_id,
        config.ping_role_id,
    );
    let channel = discord.check().await?;
    info!("Posting to {channel}");

    let ctx = Arc::new(Ctx {
        store: Store::load(&config.state_file)?,
        http: reqwest::Client::builder()
            .user_agent(concat!("cs2-tracker/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(15))
            .build()?,
        discord,
        config,
    });

    tokio::select! {
        _ = steam::run(ctx.clone()) => {}
        _ = news::run(ctx.clone()) => {}
        _ = twitter::run(ctx.clone()) => {}
        _ = presence::run(ctx.clone()) => {}
        _ = updater::run(ctx.clone(), exe) => {}
        _ = tokio::signal::ctrl_c() => info!("Shutting down"),
    }
    Ok(())
}
