mod commands;
mod config;
mod depots;
mod discord;
mod news;
mod presence;
mod state;
mod steam;
mod teamspeak;
mod twitter;
mod updater;
mod vdf;

use anyhow::{Context, Result};
use config::{Config, Event};
use discord::Discord;
use state::Store;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use teamspeak::TeamSpeak;
use tracing::{info, warn};
use twilight_util::builder::embed::EmbedBuilder;

pub struct Ctx {
    pub config: Config,
    pub discord: Discord,
    pub store: Store,
    pub http: reqwest::Client,
    pub started_at: Instant,
    pub teamspeak: Option<TeamSpeak>,
}

impl Ctx {
    /// Posts to Discord and plays the TeamSpeak sound if `event` asks for it.
    pub async fn post(&self, embed: EmbedBuilder, event: Event) -> Result<()> {
        self.discord.send(embed, event).await?;
        if let Some(ts) = &self.teamspeak {
            ts.notify(event);
        }
        Ok(())
    }
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
    let http = reqwest::Client::builder()
        .user_agent(concat!("cs2-tracker/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(15))
        .build()?;
    if config.auto_update {
        updater::on_startup(&http, &exe).await;
    }

    let discord = Discord::new(
        &config.discord_token,
        config.channel_id,
        config.ping_role_id,
        config.ping_events.clone(),
    );
    let channel = discord.check().await?;
    info!("Posting to {channel}");

    let teamspeak = match &config.teamspeak {
        Some(ts) => {
            let sound = teamspeak::load_sound(ts.sound_file.as_deref())?;
            let (handle, rx) = TeamSpeak::new(ts.events.clone());
            Some((handle, rx, sound))
        }
        None => None,
    };
    let (teamspeak, ts_task) = match teamspeak {
        Some((handle, rx, sound)) => (Some(handle), Some((rx, sound))),
        None => (None, None),
    };
    let ctx = Arc::new(Ctx {
        store: Store::load(&config.state_file)?,
        http,
        discord,
        config,
        started_at: Instant::now(),
        teamspeak,
    });
    // Spawned so it can still disconnect cleanly after the select below.
    let ts_task = ts_task.map(|(rx, sound)| tokio::spawn(teamspeak::run(ctx.clone(), rx, sound)));

    tokio::select! {
        _ = steam::run(ctx.clone()) => {}
        _ = news::run(ctx.clone()) => {}
        _ = twitter::run(ctx.clone()) => {}
        _ = presence::run(ctx.clone()) => {}
        _ = updater::run(ctx.clone(), exe.clone()) => {}
        _ = shutdown_signal() => info!("Shutting down"),
    }
    if let (Some(ts), Some(task)) = (&ctx.teamspeak, ts_task) {
        ts.quit().await;
        if tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .is_err()
        {
            warn!("TeamSpeak did not disconnect in time");
        }
    }
    if ctx.config.auto_update {
        updater::on_shutdown(&ctx.http, &exe).await;
    }
    Ok(())
}

/// Ctrl+C / SIGINT (Pterodactyl's `^C`) or SIGTERM (systemd, Docker).
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = signal(SignalKind::terminate()).expect("install SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}
