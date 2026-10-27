//! Admin slash command `/preview`: replies with example embeds built from the
//! current data, visible only to the admin who ran it.

use crate::{
    Ctx,
    discord::{CS2_ICON, CS2_IMAGE, base_embed, image, truncate},
    news, steam, twitter, updater,
};
use anyhow::Result;
use std::sync::Arc;
use tracing::{info, warn};
use twilight_model::{
    application::{
        command::CommandType,
        interaction::{Interaction, InteractionContextType, InteractionData},
    },
    channel::message::MessageFlags,
    guild::Permissions,
    http::interaction::{InteractionResponse, InteractionResponseType},
    id::{Id, marker::ApplicationMarker},
};
use twilight_util::builder::{
    InteractionResponseDataBuilder,
    command::CommandBuilder,
    embed::{EmbedAuthorBuilder, EmbedBuilder, EmbedFieldBuilder, EmbedFooterBuilder},
};

const PREVIEW: &str = "preview";

/// Registers the global commands. Hidden from non-admins by default; server
/// owners can change that under Server Settings → Integrations.
pub async fn register(ctx: &Ctx, app_id: Id<ApplicationMarker>) -> Result<()> {
    let preview = CommandBuilder::new(
        PREVIEW,
        "Show example embeds with live data (only visible to you)",
        CommandType::ChatInput,
    )
    .default_member_permissions(Permissions::ADMINISTRATOR)
    .contexts([InteractionContextType::Guild])
    .validate()?
    .build();
    ctx.discord
        .http()
        .interaction(app_id)
        .set_global_commands(&[preview])
        .await?;
    info!("Registered slash commands");
    Ok(())
}

pub async fn handle(ctx: Arc<Ctx>, interaction: Interaction) {
    let Some(InteractionData::ApplicationCommand(data)) = &interaction.data else {
        return;
    };
    if data.name != PREVIEW {
        return;
    }
    if let Err(e) = preview(&ctx, &interaction).await {
        warn!("/preview failed: {e:#}");
    }
}

async fn preview(ctx: &Ctx, interaction: &Interaction) -> Result<()> {
    let http = ctx.discord.http().interaction(interaction.application_id);
    let is_admin = interaction
        .member
        .as_ref()
        .and_then(|m| m.permissions)
        .is_some_and(|p| p.contains(Permissions::ADMINISTRATOR));

    if !is_admin {
        let response = InteractionResponse {
            kind: InteractionResponseType::ChannelMessageWithSource,
            data: Some(
                InteractionResponseDataBuilder::new()
                    .content("⛔ This command is for administrators only.")
                    .flags(MessageFlags::EPHEMERAL)
                    .build(),
            ),
        };
        http.create_response(interaction.id, &interaction.token, &response)
            .await?;
        return Ok(());
    }

    // Fetching the latest news can take a moment, so acknowledge first.
    let deferred = InteractionResponse {
        kind: InteractionResponseType::DeferredChannelMessageWithSource,
        data: Some(
            InteractionResponseDataBuilder::new()
                .flags(MessageFlags::EPHEMERAL)
                .build(),
        ),
    };
    http.create_response(interaction.id, &interaction.token, &deferred)
        .await?;

    let embeds = build_previews(ctx).await;
    http.update_response(&interaction.token)
        .content(Some(&format!(
            "👀 **Preview** · {} example embeds from the current data. Changes are simulated, only you can see this.",
            embeds.len()
        )))
        .await?;
    // One message per embed keeps each one under Discord's 6000-char limit.
    for embed in embeds {
        http.create_followup(&interaction.token)
            .embeds(&[embed.build()])
            .flags(MessageFlags::EPHEMERAL)
            .await?;
    }
    Ok(())
}

async fn build_previews(ctx: &Ctx) -> Vec<EmbedBuilder> {
    let (apps, banner) = ctx
        .store
        .read(|s| (s.apps.clone(), s.twitter_banner.clone()))
        .await;

    let mut embeds = vec![status_embed(ctx, &apps, banner.as_deref())];
    embeds.extend(steam::preview_embeds(&apps));
    match news::latest_embed(ctx).await {
        Ok(Some(embed)) => embeds.push(embed),
        Ok(None) => {}
        Err(e) => warn!("Could not fetch news for preview: {e:#}"),
    }
    if let Some(banner) = &banner {
        embeds.push(twitter::banner_embed(
            &ctx.config.twitter_handle,
            Some(banner),
            banner,
            None,
        ));
    }
    embeds
}

fn status_embed(
    ctx: &Ctx,
    apps: &std::collections::BTreeMap<u32, crate::state::AppState>,
    banner: Option<&str>,
) -> EmbedBuilder {
    let config = &ctx.config;
    let tracked = steam::APPS
        .iter()
        .map(|&(appid, name)| match apps.get(&appid) {
            Some(app) => {
                let build = app
                    .info
                    .get("depots/branches/public/buildid")
                    .map(|b| format!(" · build `{b}`"))
                    .unwrap_or_default();
                format!(
                    "[{name}](https://steamdb.info/app/{appid}/) · [#{cl}](https://steamdb.info/changelist/{cl}/){build}",
                    cl = app.change_number
                )
            }
            None => format!("{name} · *not synced yet*"),
        })
        .collect::<Vec<_>>()
        .join("\n");
    let uptime = ctx.started_at.elapsed().as_secs();
    let banner = banner.map_or("*not synced yet*".into(), |b| {
        format!("[current banner]({b}/1500x500)")
    });

    base_embed()
        .author(EmbedAuthorBuilder::new("CS2 Tracker").icon_url(image(CS2_ICON)))
        .title("📊 Bot status")
        .url("https://github.com/wildesPepega/cs2-tracker")
        .thumbnail(image(CS2_IMAGE))
        .field(EmbedFieldBuilder::new(
            "Tracked apps",
            truncate(&tracked, 1024),
        ))
        .field(EmbedFieldBuilder::new("Version", format!("`v{}`", updater::VERSION)).inline())
        .field(
            EmbedFieldBuilder::new(
                "Uptime",
                format!("{}h {}m", uptime / 3600, uptime / 60 % 60),
            )
            .inline(),
        )
        .field(
            EmbedFieldBuilder::new(
                "Auto-update",
                if config.auto_update {
                    format!("every {} min", config.update_interval.as_secs() / 60)
                } else {
                    "off".into()
                },
            )
            .inline(),
        )
        .field(
            EmbedFieldBuilder::new(
                "Polling",
                format!(
                    "PICS {} ms · News {} s · Twitter {} s",
                    config.pics_interval.as_millis(),
                    config.news_interval.as_secs(),
                    config.twitter_interval.as_secs()
                ),
            )
            .inline(),
        )
        .field(EmbedFieldBuilder::new(format!("@{}", config.twitter_handle), banner).inline())
        .footer(EmbedFooterBuilder::new(format!(
            "Presence platform: {:?}",
            config.presence_platform
        )))
}
