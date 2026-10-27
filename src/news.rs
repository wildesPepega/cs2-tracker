//! Posts official CS2 announcements (patch notes and blog posts) from the
//! Steam News API.

use crate::{
    discord::{CS2_ICON, CS2_IMAGE, base_embed, timestamp, truncate},
    Ctx,
};
use anyhow::Result;
use regex::Regex;
use serde::Deserialize;
use serenity::all::{CreateEmbedAuthor, CreateEmbedFooter};
use std::sync::{Arc, LazyLock};
use tokio::time::{MissedTickBehavior, interval};
use tracing::{info, warn};

const NEWS_URL: &str = "https://api.steampowered.com/ISteamNews/GetNewsForApp/v2/\
    ?appid=730&count=15&maxlength=0&feeds=steam_community_announcements";
const CLAN_IMAGE_BASE: &str = "https://clan.akamai.steamstatic.com/images/";

#[derive(Deserialize)]
struct Response {
    appnews: AppNews,
}

#[derive(Deserialize)]
struct AppNews {
    newsitems: Vec<NewsItem>,
}

#[derive(Deserialize)]
struct NewsItem {
    gid: String,
    title: String,
    author: String,
    contents: String,
    date: i64,
    #[serde(default)]
    tags: Vec<String>,
}

pub async fn run(ctx: Arc<Ctx>) {
    let mut tick = interval(ctx.config.news_interval);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        if let Err(e) = check(&ctx).await {
            warn!("News-Check fehlgeschlagen: {e:#}");
        }
    }
}

async fn check(ctx: &Ctx) -> Result<()> {
    let resp: Response = ctx
        .http
        .get(NEWS_URL)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let mut items = resp.appnews.newsitems;
    items.sort_by_key(|i| i.date);

    let seen = ctx.store.read(|s| s.seen_news.clone()).await;
    let Some(seen) = seen else {
        info!("News: {} vorhandene Posts als gesehen markiert", items.len());
        ctx.store
            .update(|s| items.iter().for_each(|i| s.mark_news_seen(&i.gid)))
            .await;
        return Ok(());
    };

    for item in items.iter().filter(|i| !seen.contains(&i.gid)) {
        info!("Neuer CS2-Post: {}", item.title);
        ctx.discord.send(news_embed(item), false).await?;
        ctx.store.update(|s| s.mark_news_seen(&item.gid)).await;
    }
    Ok(())
}

fn news_embed(item: &NewsItem) -> serenity::all::CreateEmbed {
    let (text, image) = bbcode_to_markdown(&item.contents);
    let url = format!("https://store.steampowered.com/news/app/730/view/{}", item.gid);
    let kind = if item.tags.iter().any(|t| t == "patchnotes") {
        "📝 Neue Patchnotes"
    } else {
        "📰 Neuer Blogpost"
    };
    let mut footer = "Steam News · Counter-Strike 2".to_owned();
    if !item.author.is_empty() {
        footer = format!("von {} · {footer}", item.author);
    }

    base_embed()
        .author(CreateEmbedAuthor::new(kind).icon_url(CS2_ICON))
        .title(truncate(&item.title, 256))
        .url(&url)
        .description(truncate(&text, 3900))
        .image(image.as_deref().unwrap_or(CS2_IMAGE))
        .footer(CreateEmbedFooter::new(footer))
        .timestamp(timestamp(item.date))
}

static TAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\[(/?)([a-zA-Z0-9*]+)(?:=([^\]]*)|\s([^\]]*))?\]"#).unwrap()
});
static SRC_ATTR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"src="([^"]+)""#).unwrap());
static BLANK_LINES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n{3,}").unwrap());

/// Converts Steam's BBCode to Discord markdown. Also returns the first image.
fn bbcode_to_markdown(input: &str) -> (String, Option<String>) {
    // `\[` is a literal bracket, not a tag
    const LBRACKET: char = '\u{E000}';
    let input = input.replace("\\[", &LBRACKET.to_string());

    let mut out = String::new();
    let mut first_image = None;
    let mut list_depth = 0usize;
    let mut links: Vec<String> = Vec::new();
    // While inside [img]/[video]/..., text content is dropped.
    let mut skip_until: Option<String> = None;
    let mut last = 0;

    for cap in TAG.captures_iter(&input) {
        let whole = cap.get(0).unwrap();
        if skip_until.is_none() {
            out.push_str(&input[last..whole.start()]);
        } else if first_image.is_none() && skip_until.as_deref() == Some("img") {
            let inner = input[last..whole.start()].trim();
            if !inner.is_empty() {
                first_image = Some(resolve_image(inner));
            }
        }
        last = whole.end();

        let closing = !cap[1].is_empty();
        let tag = cap[2].to_ascii_lowercase();
        let arg = cap.get(3).map(|m| m.as_str().trim_matches('"'));
        let attrs = cap.get(4).map_or("", |m| m.as_str());

        if let Some(until) = &skip_until {
            if closing && *until == tag {
                skip_until = None;
            }
            continue;
        }

        match (tag.as_str(), closing) {
            ("p", true) if list_depth == 0 => out.push('\n'),
            ("h1" | "h2" | "h3", false) => out.push_str("\n**"),
            ("h1" | "h2" | "h3", true) => out.push_str("**\n"),
            ("b" | "strong", _) => out.push_str("**"),
            ("i" | "em", _) => out.push('*'),
            ("u", _) => out.push_str("__"),
            ("strike" | "s", _) => out.push_str("~~"),
            ("list" | "olist", false) => list_depth += 1,
            ("list" | "olist", true) => {
                list_depth = list_depth.saturating_sub(1);
                if list_depth == 0 {
                    out.push('\n');
                }
            }
            ("*", false) => {
                out.push('\n');
                out.push_str(&"  ".repeat(list_depth.saturating_sub(1)));
                out.push_str("• ");
            }
            ("hr", false) | ("br", false) => out.push('\n'),
            ("url", false) => {
                links.push(arg.unwrap_or_default().to_owned());
                out.push('[');
            }
            ("url", true) => {
                let href = links.pop().unwrap_or_default();
                out.push_str(&format!("]({href})"));
            }
            ("img", false) => {
                if first_image.is_none() {
                    first_image = SRC_ATTR
                        .captures(attrs)
                        .map(|c| resolve_image(&c[1]))
                        .or_else(|| arg.map(resolve_image));
                }
                skip_until = Some("img".into());
            }
            ("previewyoutube" | "video" | "carousel", false) => skip_until = Some(tag),
            _ => {}
        }
    }
    if skip_until.is_none() {
        out.push_str(&input[last..]);
    }

    let out = out.replace(LBRACKET, "[").replace("&amp;", "&");
    let out = BLANK_LINES.replace_all(out.trim(), "\n\n").into_owned();
    (out, first_image)
}

fn resolve_image(src: &str) -> String {
    src.replace("{STEAM_CLAN_IMAGE}/", CLAN_IMAGE_BASE)
        .replace("{STEAM_CLAN_IMAGE}", CLAN_IMAGE_BASE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_patchnotes() {
        let input = r"[p]\[ UI ][/p][list][*][p]Added a bar[/p][/*][*][p]Nested[/p][list][*][p]Deep[/p][/*][/list][/*][/list][p][/p][p]\[ MISC ][/p]";
        let (md, img) = bbcode_to_markdown(input);
        assert_eq!(md, "[ UI ]\n\n• Added a bar\n• Nested\n  • Deep\n\n[ MISC ]");
        assert_eq!(img, None);
    }

    #[test]
    fn extracts_image_and_links() {
        let input = r#"[img src="{STEAM_CLAN_IMAGE}/123/abc.png"][/img][p]See [url="https://x.y"]here[/url][/p]"#;
        let (md, img) = bbcode_to_markdown(input);
        assert_eq!(md, "See [here](https://x.y)");
        assert_eq!(img.as_deref(), Some("https://clan.akamai.steamstatic.com/images/123/abc.png"));

        let (_, img) = bbcode_to_markdown("[img]{STEAM_CLAN_IMAGE}/1/2.jpg[/img]");
        assert_eq!(img.as_deref(), Some("https://clan.akamai.steamstatic.com/images/1/2.jpg"));
    }
}
