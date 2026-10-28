//! Watches CS2 apps via Steam PICS (the same data SteamDB uses) for new
//! changelists and builds.

use crate::{
    Ctx,
    config::Event,
    depots::{depot_changes, depot_embed, is_depot_key},
    discord::{CS2_ICON, CS2_IMAGE, base_embed, image, timestamp, truncate},
    state::AppState,
    vdf::{self, Node},
};
use anyhow::{Context, Result, bail};
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
    time::Duration,
};
use steam_vent::{Connection, ConnectionTrait, ServerList};
use steam_vent_proto::steammessages_clientserver_appinfo::{
    CMsgClientPICSChangesSinceRequest, CMsgClientPICSChangesSinceResponse,
    CMsgClientPICSProductInfoRequest, CMsgClientPICSProductInfoResponse,
    cmsg_client_picsproduct_info_request::AppInfo,
};
use tokio::time::{MissedTickBehavior, interval, sleep, timeout};
use tracing::{info, warn};
use twilight_util::builder::embed::{
    EmbedAuthorBuilder, EmbedBuilder, EmbedFieldBuilder, EmbedFooterBuilder,
};

pub const APPS: &[(u32, &str)] = &[
    (730, "Counter-Strike 2"),
    (2275500, "CS2 DevClient"),
    (2275530, "CS2 DevServer"),
];

const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_DIFF_LINES: usize = 25;

pub fn app_name(appid: u32) -> &'static str {
    APPS.iter()
        .find(|(id, _)| *id == appid)
        .map_or("Unknown app", |(_, name)| name)
}

pub async fn run(ctx: Arc<Ctx>) {
    loop {
        if let Err(e) = session(&ctx).await {
            warn!("Lost Steam connection: {e:#}");
        }
        sleep(Duration::from_secs(5)).await;
    }
}

/// One connection lifetime: connect anonymously, then poll for changes.
async fn session(ctx: &Ctx) -> Result<()> {
    let servers = ServerList::discover()
        .await
        .context("server list discovery")?;
    let conn = timeout(REQUEST_TIMEOUT, Connection::anonymous(&servers))
        .await
        .context("login timed out")??;
    info!("Connected to Steam (anonymous)");

    // Catch up on anything that changed while we were offline.
    let all: HashMap<u32, u32> = APPS.iter().map(|&(id, _)| (id, 0)).collect();
    let mut pending = check_apps(ctx, &conn, &all).await?;
    let mut since = changes_since(&conn, 0).await?.current_change_number();
    info!("Polling PICS from changelist {since}");

    let mut tick = interval(ctx.config.pics_interval);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        let resp = changes_since(&conn, since).await?;
        let current = resp.current_change_number();

        if resp.force_full_update() || resp.force_full_app_update() {
            // Too far behind for a delta; just recheck every app.
            pending.extend(APPS.iter().map(|&(id, _)| (id, 0)));
        }
        for change in &resp.app_changes {
            if all.contains_key(&change.appid()) {
                let expected = pending.entry(change.appid()).or_default();
                *expected = (*expected).max(change.change_number());
            }
        }
        if !pending.is_empty() {
            pending = check_apps(ctx, &conn, &pending).await?;
        }
        since = since.max(current);
    }
}

async fn changes_since(
    conn: &Connection,
    since: u32,
) -> Result<CMsgClientPICSChangesSinceResponse> {
    let req = CMsgClientPICSChangesSinceRequest {
        since_change_number: Some(since),
        send_app_info_changes: Some(true),
        send_package_info_changes: Some(false),
        ..Default::default()
    };
    Ok(timeout(REQUEST_TIMEOUT, conn.job(req))
        .await
        .context("ChangesSince timed out")??)
}

/// Fetches appinfo for `apps` (appid -> expected minimum change number),
/// posts anything new and returns the apps whose info hasn't caught up yet.
async fn check_apps(
    ctx: &Ctx,
    conn: &Connection,
    apps: &HashMap<u32, u32>,
) -> Result<HashMap<u32, u32>> {
    let req = CMsgClientPICSProductInfoRequest {
        apps: apps
            .keys()
            .map(|&appid| AppInfo {
                appid: Some(appid),
                ..Default::default()
            })
            .collect(),
        meta_data_only: Some(false),
        single_response: Some(true),
        ..Default::default()
    };
    let resp: CMsgClientPICSProductInfoResponse = timeout(REQUEST_TIMEOUT, conn.job(req))
        .await
        .context("ProductInfo timed out")??;
    if resp.apps.is_empty() && !apps.is_empty() {
        bail!("ProductInfo returned no apps");
    }

    let mut still_pending = HashMap::new();
    for app in &resp.apps {
        let appid = app.appid();
        let change_number = app.change_number();
        let expected = apps.get(&appid).copied().unwrap_or(0);
        if change_number < expected {
            // PICS announced a newer changelist than productinfo serves yet.
            still_pending.insert(appid, expected);
        }

        let text = String::from_utf8_lossy(app.buffer());
        let info = vdf::parse(&text).map(|n| flatten(&n)).unwrap_or_default();
        let new = AppState {
            change_number,
            info,
        };

        let old = ctx.store.read(|s| s.apps.get(&appid).cloned()).await;
        match old {
            None => info!(
                "{} ({appid}): baseline changelist {change_number}",
                app_name(appid)
            ),
            Some(old) if change_number > old.change_number => {
                info!(
                    "{} ({appid}): new changelist {change_number}",
                    app_name(appid)
                );
                if let Err(e) = announce(ctx, appid, &old, &new).await {
                    // Keep the old state so the next poll retries the post.
                    warn!("Discord post failed: {e:#}");
                    still_pending.insert(appid, change_number);
                    continue;
                }
            }
            Some(_) => continue,
        }
        ctx.store.update(|s| s.apps.insert(appid, new)).await;
    }
    Ok(still_pending)
}

async fn announce(ctx: &Ctx, appid: u32, old: &AppState, new: &AppState) -> Result<()> {
    let diff = diff(&old.info, &new.info);
    let builds = build_changes(&old.info, &new.info);
    let depots = depot_changes(&old.info, &new.info);

    if let Some(public) = builds.iter().find(|b| b.branch == "public") {
        ctx.discord
            .send(
                update_embed(appid, public, new.change_number),
                Event::Update,
            )
            .await?;
    }
    let embed = changelist_embed(
        appid,
        old.change_number,
        new.change_number,
        &builds,
        &diff,
        !depots.is_empty(),
    );
    ctx.discord.send(embed, Event::Changelist).await?;
    if !depots.is_empty() {
        let embed = depot_embed(
            app_name(appid),
            appid,
            new.change_number,
            &depots,
            &new.info,
        );
        ctx.discord.send(embed, Event::Depots).await?;
    }
    Ok(())
}

pub struct BuildChange {
    branch: String,
    old: Option<String>,
    new: String,
    time_updated: Option<i64>,
}

pub fn build_changes(
    old: &BTreeMap<String, String>,
    new: &BTreeMap<String, String>,
) -> Vec<BuildChange> {
    let mut out: Vec<BuildChange> = new
        .iter()
        .filter_map(|(path, build)| {
            let branch = path
                .strip_prefix("depots/branches/")?
                .strip_suffix("/buildid")?;
            let old_build = old.get(path);
            (old_build != Some(build)).then(|| BuildChange {
                branch: branch.to_owned(),
                old: old_build.cloned(),
                new: build.clone(),
                time_updated: new
                    .get(&format!("depots/branches/{branch}/timeupdated"))
                    .and_then(|t| t.parse().ok()),
            })
        })
        .collect();
    // public first, it's the one people care about
    out.sort_by_key(|b| (b.branch != "public", b.branch.clone()));
    out
}

pub fn update_embed(appid: u32, build: &BuildChange, change_number: u32) -> EmbedBuilder {
    let when = build
        .time_updated
        .map_or("just now".into(), |t| format!("<t:{t}:R>"));
    let mut embed = base_embed()
        .author(EmbedAuthorBuilder::new("Steam update detected").icon_url(image(CS2_ICON)))
        .title(format!("🚨 {} has been updated!", app_name(appid)))
        .url(format!("https://steamdb.info/patchnotes/{}/", build.new))
        .description("A new build is live on the **public** branch.")
        .field(EmbedFieldBuilder::new("New build", format!("`{}`", build.new)).inline())
        .field(
            EmbedFieldBuilder::new(
                "Previous build",
                format!("`{}`", build.old.as_deref().unwrap_or("–")),
            )
            .inline(),
        )
        .field(
            EmbedFieldBuilder::new(
                "Changelist",
                format!("[#{change_number}](https://steamdb.info/changelist/{change_number}/)"),
            )
            .inline(),
        )
        .field(EmbedFieldBuilder::new("Live since", when).inline())
        .field(EmbedFieldBuilder::new(
            "Links",
            format!(
                "[Patch notes](https://steamdb.info/patchnotes/{}/) · [Depots](https://steamdb.info/app/{appid}/depots/) · [Official updates](https://www.counter-strike.net/news/updates)",
                build.new
            ),
        ))
        .image(image(CS2_IMAGE))
        .footer(EmbedFooterBuilder::new("SteamDB · PICS"));
    if let Some(t) = build.time_updated {
        embed = embed.timestamp(timestamp(t));
    }
    embed
}

pub fn changelist_embed(
    appid: u32,
    old_cl: u32,
    new_cl: u32,
    builds: &[BuildChange],
    diff: &[String],
    has_depot_changes: bool,
) -> EmbedBuilder {
    let mut description = String::new();
    if diff.is_empty() && has_depot_changes {
        description.push_str("*Depot changes only, see the next embed.*");
    } else if diff.is_empty() {
        description.push_str("*No visible changes. This app's info requires an access token.*");
    } else {
        let shown = diff
            .iter()
            .take(MAX_DIFF_LINES)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        let more = diff.len().saturating_sub(MAX_DIFF_LINES);
        description = format!("```diff\n{}\n```", truncate(&shown, 3800));
        if more > 0 {
            description.push_str(&format!("…and {more} more changes"));
        }
    }

    let mut embed = base_embed()
        .author(
            EmbedAuthorBuilder::new(format!("{} · {appid}", app_name(appid)))
                .icon_url(image(CS2_ICON)),
        )
        .title(format!("Changelist #{new_cl}"))
        .url(format!("https://steamdb.info/changelist/{new_cl}/"))
        .description(description)
        .thumbnail(image(CS2_IMAGE))
        .field(
            EmbedFieldBuilder::new(
                "App",
                format!(
                    "[{}](https://steamdb.info/app/{appid}/history/)",
                    app_name(appid)
                ),
            )
            .inline(),
        )
        .field(EmbedFieldBuilder::new("Previous changelist", format!("#{old_cl}")).inline())
        .footer(EmbedFooterBuilder::new("SteamDB · PICS"));

    if !builds.is_empty() {
        let lines = builds
            .iter()
            .map(|b| {
                format!(
                    "**{}**: `{}` → `{}`",
                    b.branch,
                    b.old.as_deref().unwrap_or("new"),
                    b.new
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        embed = embed.field(EmbedFieldBuilder::new(
            "Build changes",
            truncate(&lines, 1024),
        ));
    }
    embed
}

fn flatten(node: &Node) -> BTreeMap<String, String> {
    fn walk(node: &Node, prefix: &str, out: &mut BTreeMap<String, String>) {
        match node {
            Node::Str(s) => {
                out.insert(prefix.to_owned(), s.clone());
            }
            Node::Obj(map) => {
                for (k, v) in map {
                    let path = if prefix.is_empty() {
                        k.clone()
                    } else {
                        format!("{prefix}/{k}")
                    };
                    walk(v, &path, out);
                }
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(node, "", &mut out);
    out
}

/// SteamDB-style key diff, formatted for a ```diff block. Depot keys are
/// left out; they get their own embed.
pub fn diff(old: &BTreeMap<String, String>, new: &BTreeMap<String, String>) -> Vec<String> {
    let clean = |s: &str| truncate(&s.replace("```", "'''").replace('\n', " "), 80);
    let mut lines = Vec::new();
    for (k, v) in new.iter().filter(|(k, _)| !is_depot_key(k)) {
        match old.get(k) {
            None => lines.push(format!("+ {k}: {}", clean(v))),
            Some(o) if o != v => {
                lines.push(format!("- {k}: {}", clean(o)));
                lines.push(format!("+ {k}: {}", clean(v)));
            }
            Some(_) => {}
        }
    }
    for (k, v) in old.iter().filter(|(k, _)| !is_depot_key(k)) {
        if !new.contains_key(k) {
            lines.push(format!("- {k}: {}", clean(v)));
        }
    }
    lines
}

/// Example embeds built from the current state with a simulated changelist,
/// for the admin `/preview` command.
pub fn preview_embeds(apps: &BTreeMap<u32, AppState>) -> Vec<EmbedBuilder> {
    let mut embeds = Vec::new();
    for (&appid, new) in apps {
        let mut old = new.clone();
        old.change_number = new.change_number.saturating_sub(1);
        // Simulate a new public build plus one depot manifest update.
        let build_key = "depots/branches/public/buildid";
        if let Some(build) = new.info.get(build_key).and_then(|b| b.parse::<u64>().ok()) {
            old.info.insert(build_key.into(), (build - 1).to_string());
        }
        if let Some(key) = new
            .info
            .keys()
            .find(|k| is_depot_key(k) && k.ends_with("/manifests/public/gid"))
        {
            old.info.insert(key.clone(), "1234567890123456789".into());
        }

        let builds = build_changes(&old.info, &new.info);
        let depots = depot_changes(&old.info, &new.info);
        if let Some(public) = builds.iter().find(|b| b.branch == "public") {
            embeds.push(update_embed(appid, public, new.change_number));
        }
        embeds.push(changelist_embed(
            appid,
            old.change_number,
            new.change_number,
            &builds,
            &diff(&old.info, &new.info),
            !depots.is_empty(),
        ));
        if !depots.is_empty() {
            embeds.push(depot_embed(
                app_name(appid),
                appid,
                new.change_number,
                &depots,
                &new.info,
            ));
        }
    }
    embeds
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn detects_build_changes_public_first() {
        let old = map(&[
            ("depots/branches/public/buildid", "1"),
            ("depots/branches/beta/buildid", "5"),
        ]);
        let new = map(&[
            ("depots/branches/public/buildid", "2"),
            ("depots/branches/public/timeupdated", "1700000000"),
            ("depots/branches/beta/buildid", "6"),
            ("depots/branches/aaa/buildid", "9"),
        ]);
        let builds = build_changes(&old, &new);
        let names: Vec<_> = builds.iter().map(|b| b.branch.as_str()).collect();
        assert_eq!(names, ["public", "aaa", "beta"]);
        assert_eq!(builds[0].old.as_deref(), Some("1"));
        assert_eq!(builds[0].time_updated, Some(1_700_000_000));
        assert_eq!(builds[1].old, None);
    }

    #[test]
    fn preview_embeds_pass_discord_validation() {
        let mut apps = BTreeMap::new();
        apps.insert(
            730,
            AppState {
                change_number: 100,
                info: map(&[
                    ("common/name", "Counter-Strike 2"),
                    ("depots/branches/public/buildid", "26144732"),
                    ("depots/branches/public/timeupdated", "1792709693"),
                    ("depots/2347770/manifests/public/gid", "67499503218971311"),
                    ("depots/2347770/manifests/public/size", "65027793761"),
                ]),
            },
        );
        apps.insert(
            2275500,
            AppState {
                change_number: 50,
                info: BTreeMap::new(),
            },
        );

        let embeds = preview_embeds(&apps);
        // update + changelist + depots for 730, changelist for the dev app
        assert_eq!(embeds.len(), 4);
        for embed in embeds {
            let _ = embed.validate().expect("embed within Discord limits");
        }
    }

    #[test]
    fn diff_lists_added_changed_removed() {
        let old = map(&[("a", "1"), ("b", "2"), ("c", "3")]);
        let new = map(&[("a", "1"), ("b", "9"), ("d", "4")]);
        assert_eq!(diff(&old, &new), ["- b: 2", "+ b: 9", "+ d: 4", "- c: 3"]);
    }
}
