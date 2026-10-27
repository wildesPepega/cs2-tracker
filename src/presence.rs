//! Keeps the bot online via the Discord gateway, rotates CS-themed statuses
//! and dispatches slash commands.

use crate::{Ctx, commands, config::Platform};
use anyhow::Result;
use std::{sync::Arc, time::SystemTime};
use tokio::time::{MissedTickBehavior, interval};
use tracing::{info, warn};
use twilight_gateway::{ConfigBuilder, Event, EventTypeFlags, Intents, Shard, ShardId, StreamExt};
use twilight_model::gateway::{
    payload::outgoing::{
        UpdatePresence, identify::IdentifyProperties, update_presence::UpdatePresencePayload,
    },
    presence::{Activity, ActivityType, MinimalActivity, Status},
};

use ActivityType::{Competing as C, Listening as L, Playing as P, Watching as W};

/// Rendered by Discord as "Playing …", "Watching …", "Listening to …" and
/// "Competing in …".
const STATUSES: &[(ActivityType, &str)] = &[
    (W, "SteamDB for CS2 updates"),
    (W, "for new changelists"),
    (W, "the CS2 depots"),
    (W, "Valve's commit log"),
    (W, "the public branch"),
    (W, "for a 3 AM patch"),
    (W, "the counter-strike.net blog"),
    (W, "@CounterStrike's banner"),
    (W, "the DevClient branch"),
    (W, "the DevServer branch"),
    (W, "PICS like a hawk"),
    (W, "build IDs go up"),
    (W, "for Operation news"),
    (W, "a Major final"),
    (W, "demo reviews"),
    (W, "a 1v5 clutch"),
    (W, "the bomb timer"),
    (W, "an eco round go wrong"),
    (W, "someone bait their team"),
    (W, "the AWPer whiff"),
    (W, "smokes bloom on Mirage"),
    (W, "a ninja defuse"),
    (W, "the scoreboard"),
    (W, "a silver play Nuke"),
    (W, "a pro flick"),
    (W, "the overpass connector"),
    (W, "for jumpthrow binds"),
    (W, "Inferno banana control"),
    (W, "a force buy fail"),
    (W, "the kill feed"),
    (W, "the case opening animation"),
    (W, "a knife round"),
    (W, "Long A on Dust II"),
    (W, "the radar"),
    (W, "for wallbangs"),
    (W, "a molly on the default plant"),
    (W, "the T spawn"),
    (W, "CT side hold the site"),
    (W, "a 16-14 thriller"),
    (W, "the Premier leaderboard"),
    (W, "a Faceit stream"),
    (W, "the HLTV rankings"),
    (W, "the skin market crash"),
    (W, "patch notes drop"),
    (W, "for subtick changes"),
    (W, "the server tick"),
    (W, "the sniper's nest"),
    (W, "the hostages on Office"),
    (W, "Ancient's mid"),
    (W, "a timeout get called"),
    (P, "Counter-Strike 2"),
    (P, "de_dust2"),
    (P, "de_mirage"),
    (P, "de_inferno"),
    (P, "de_nuke"),
    (P, "de_ancient"),
    (P, "de_anubis"),
    (P, "de_overpass"),
    (P, "de_vertigo"),
    (P, "de_train"),
    (P, "cs_office"),
    (P, "cs_italy"),
    (P, "Premier"),
    (P, "Competitive"),
    (P, "Wingman"),
    (P, "Deathmatch"),
    (P, "Arms Race"),
    (P, "Casual"),
    (P, "Retakes"),
    (P, "aim_botz"),
    (P, "with the AWP"),
    (P, "with a Desert Eagle"),
    (P, "AK-47 only"),
    (P, "with a Zeus x27"),
    (P, "the pistol round"),
    (P, "an eco round"),
    (P, "a full buy"),
    (P, "the bomb carrier"),
    (P, "entry fragger"),
    (P, "lurker on B"),
    (P, "the in-game leader"),
    (P, "support, again"),
    (P, "with smokes"),
    (P, "with flashbangs"),
    (P, "the defuse kit lottery"),
    (P, "rush B, no stop"),
    (P, "with 999 ping"),
    (P, "on 128 tick"),
    (P, "the long game"),
    (P, "hide and seek on Office"),
    (P, "the knife round"),
    (P, "surf maps"),
    (P, "bhop maps"),
    (P, "kz_climb"),
    (P, "1v1 on aim_map"),
    (P, "with a P90, sorry"),
    (P, "the Negev spray"),
    (P, "the save round"),
    (P, "for the clutch"),
    (P, "with sticker crafts"),
    (L, "the bomb beeps"),
    (L, "footsteps"),
    (L, "\"Rush B!\""),
    (L, "\"Fire in the hole!\""),
    (L, "\"Terrorists win\""),
    (L, "\"Counter-Terrorists win\""),
    (L, "\"Bomb has been planted\""),
    (L, "\"The bomb has been defused\""),
    (L, "the defuse kit click"),
    (L, "AWP shots"),
    (L, "the MVP anthem"),
    (L, "the CS2 main menu theme"),
    (L, "the Major music kit"),
    (L, "the IGL's calls"),
    (L, "callouts on Mirage"),
    (L, "the caster go wild"),
    (L, "flashbang ringing"),
    (L, "the shop menu"),
    (L, "\"Need backup!\""),
    (L, "\"Go go go!\""),
    (L, "\"Enemy spotted\""),
    (L, "\"Affirmative\""),
    (L, "\"Negative\""),
    (L, "\"Hold this position\""),
    (L, "the reload sound"),
    (L, "Steam's PICS servers"),
    (L, "Valve's servers hum"),
    (L, "the case opening tick"),
    (L, "the knife inspect"),
    (L, "a teammate's mic spam"),
    (L, "the T-side radio"),
    (L, "the last round's replay"),
    (L, "a 1v3 in silence"),
    (L, "the HE grenade bounce"),
    (L, "the timeout tactics"),
    (C, "a CS2 Major"),
    (C, "IEM Cologne"),
    (C, "IEM Katowice"),
    (C, "the BLAST Premier"),
    (C, "the ESL Pro League"),
    (C, "PGL Major"),
    (C, "Premier season"),
    (C, "a Faceit level 10 match"),
    (C, "a Wingman showdown"),
    (C, "an aim duel"),
    (C, "the pistol round"),
    (C, "the overtime"),
    (C, "a 30-round marathon"),
    (C, "a Major qualifier"),
    (C, "the HLTV top 20"),
    (C, "a race to the patch notes"),
    (C, "a race against SteamDB"),
    (C, "a knife round"),
    (C, "the Arms Race"),
    (C, "a deathmatch frenzy"),
];

pub async fn run(ctx: Arc<Ctx>) {
    loop {
        if let Err(e) = session(&ctx).await {
            warn!("Gateway error: {e:#}");
        }
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
    }
}

async fn session(ctx: &Arc<Ctx>) -> Result<()> {
    // Start at a random spot so restarts don't always show the same status.
    let mut index = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos() as usize)
        % STATUSES.len();

    let config = ConfigBuilder::new(ctx.config.discord_token.clone(), Intents::empty())
        .identify_properties(identify_properties(ctx.config.presence_platform))
        .presence(presence(index)?)
        .build();
    let mut shard = Shard::with_config(ShardId::ONE, config);
    let sender = shard.sender();

    let mut tick = interval(ctx.config.status_interval);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    tick.tick().await;

    loop {
        tokio::select! {
            event = shard.next_event(EventTypeFlags::READY | EventTypeFlags::INTERACTION_CREATE) => match event {
                Some(Ok(Event::Ready(ready))) => {
                    info!("Discord gateway ready as {}", ready.user.name);
                    if let Err(e) = commands::register(ctx, ready.application.id).await {
                        warn!("Failed to register slash commands: {e:#}");
                    }
                }
                Some(Ok(Event::InteractionCreate(interaction))) => {
                    tokio::spawn(commands::handle(ctx.clone(), interaction.0));
                }
                Some(Ok(_)) => {}
                // The shard reconnects on its own after non-fatal errors.
                Some(Err(e)) => warn!("Gateway receive error: {e}"),
                None => anyhow::bail!("gateway closed"),
            },
            _ = tick.tick() => {
                index = (index + 1) % STATUSES.len();
                let update = UpdatePresence::new(vec![activity(index)], false, None, Status::Online)?;
                if let Err(e) = sender.command(&update) {
                    warn!("Failed to update status: {e}");
                }
            }
        }
    }
}

fn activity(index: usize) -> Activity {
    let (kind, name) = STATUSES[index];
    MinimalActivity {
        kind,
        name: name.to_owned(),
        url: None,
    }
    .into()
}

fn presence(index: usize) -> Result<UpdatePresencePayload> {
    Ok(UpdatePresencePayload::new(
        vec![activity(index)],
        false,
        None,
        Status::Online,
    )?)
}

/// Discord picks the platform icon from the identify properties. "Discord VR"
/// is the browser value of the Meta Quest client (undocumented, may change).
fn identify_properties(platform: Platform) -> IdentifyProperties {
    match platform {
        Platform::Vr => IdentifyProperties::new("Discord VR", "Discord VR", "android"),
        Platform::Mobile => {
            IdentifyProperties::new("Discord Android", "Discord Android", "android")
        }
        Platform::Desktop => IdentifyProperties::new("Discord Client", "Discord Client", "linux"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_are_valid_and_unique() {
        assert!(STATUSES.len() >= 150, "only {} statuses", STATUSES.len());
        let mut seen = std::collections::HashSet::new();
        for (i, (kind, name)) in STATUSES.iter().enumerate() {
            assert!(name.len() <= 128, "{name:?} too long");
            assert!(
                seen.insert(format!("{kind:?} {name}")),
                "duplicate status {name:?}"
            );
            presence(i).unwrap();
        }
    }
}
