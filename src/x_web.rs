//! Free posting to X through the web client's own API, logged in with the
//! account's browser cookies (`auth_token` and `ct0`).
//!
//! This is what x.com itself does when you post, so it needs the same request
//! fingerprint: the public web bearer token, the CSRF cookie as header, the
//! current GraphQL query id and feature flags (both read from x.com's
//! JavaScript), and an `x-client-transaction-id` header derived from values on
//! the home page. Using it breaks X's rules and may get the account locked.

use anyhow::{Context, Result, bail, ensure};
use base64::{Engine, prelude::BASE64_STANDARD};
use regex::Regex;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::{
    sync::LazyLock,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Mutex;
use tracing::info;

/// The bearer token baked into x.com's web client (the same for everyone).
const WEB_BEARER: &str = "AAAAAAAAAAAAAAAAAAAAANRILgAAAAAAnNwIzUejRCOuH5E6I8xnZz4puTs%3D1Zv7ttfk8LF81IUq16cHjhLTvJu4FA33AGWWjCpTnA";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";
/// The legacy web page that still carries everything the transaction id needs.
const HOME_URL: &str = "https://x.com/i/jf/";
const UPLOAD_URL: &str = "https://upload.x.com/i/media/upload.json";
/// Query ids and keys rotate with x.com deploys.
const SESSION_TTL: Duration = Duration::from_secs(3600);

pub struct XWeb {
    http: reqwest::Client,
    auth_token: String,
    /// X sometimes rotates the CSRF cookie in responses.
    ct0: Mutex<String>,
    session: Mutex<Option<(Session, Instant)>>,
}

#[derive(Clone)]
struct Session {
    transaction: Transaction,
    create_tweet_query: String,
    features: Map<String, Value>,
}

impl XWeb {
    pub fn new(http: reqwest::Client, auth_token: String, ct0: String) -> Self {
        Self {
            http,
            auth_token,
            ct0: Mutex::new(ct0),
            session: Mutex::new(None),
        }
    }

    /// Returns the logged-in account's handle.
    pub async fn check(&self) -> Result<String> {
        let path = "/i/api/1.1/account/settings.json";
        let session = self.session().await?;
        let resp = self
            .request(
                reqwest::Method::GET,
                &format!("https://x.com{path}"),
                &session,
                path,
            )
            .await
            .send()
            .await?;
        let settings: Value = self.json(resp).await?;
        settings["screen_name"]
            .as_str()
            .map(str::to_owned)
            .context("not logged in, check X_AUTH_TOKEN and X_CT0")
    }

    pub async fn post(&self, text: &str, image: Option<(Vec<u8>, String)>) -> Result<String> {
        let session = self.session().await?;
        let mut media = Vec::new();
        if let Some((bytes, mime)) = image {
            match self.upload(&session, bytes, &mime).await {
                Ok(id) => media.push(json!({ "media_id": id, "tagged_users": [] })),
                Err(e) => tracing::warn!("Image upload to X failed, posting without it: {e:#}"),
            }
        }
        let path = format!("/i/api/graphql/{}/CreateTweet", session.create_tweet_query);
        let body = json!({
            "variables": {
                "tweet_text": text,
                "dark_request": false,
                "media": { "media_entities": media, "possibly_sensitive": false },
                "semantic_annotation_ids": [],
                "disallowed_reply_options": null,
            },
            "features": session.features,
            "queryId": session.create_tweet_query,
        });
        let resp = self
            .request(
                reqwest::Method::POST,
                &format!("https://x.com{path}"),
                &session,
                &path,
            )
            .await
            .json(&body)
            .send()
            .await;
        let result: Result<Value> = match resp {
            Ok(resp) => self.json(resp).await,
            Err(e) => Err(e.into()),
        };
        let result = match result {
            Ok(r) => r,
            Err(e) => {
                // Likely a rotated query id or key; rebuild before the next post.
                *self.session.lock().await = None;
                return Err(e);
            }
        };
        if let Some(errors) = result["errors"].as_array() {
            let messages: Vec<_> = errors
                .iter()
                .map(|e| format!("{} ({})", e["message"].as_str().unwrap_or("?"), e["code"]))
                .collect();
            bail!("X refused the post: {}", messages.join(", "));
        }
        result["data"]["create_tweet"]["tweet_results"]["result"]["rest_id"]
            .as_str()
            .map(str::to_owned)
            .with_context(|| format!("unexpected CreateTweet response: {result}"))
    }

    async fn upload(&self, session: &Session, bytes: Vec<u8>, mime: &str) -> Result<String> {
        let path = "/i/media/upload.json";
        let init: Value = {
            let url = format!(
                "{UPLOAD_URL}?command=INIT&total_bytes={}&media_type={}&media_category=tweet_image",
                bytes.len(),
                mime.replace('/', "%2F")
            );
            let resp = self
                .request(reqwest::Method::POST, &url, session, path)
                .await
                .send()
                .await?;
            self.json(resp).await?
        };
        let id = init["media_id_string"]
            .as_str()
            .with_context(|| format!("unexpected INIT response: {init}"))?
            .to_owned();

        let part = reqwest::multipart::Part::bytes(bytes)
            .file_name("blob")
            .mime_str("application/octet-stream")?;
        let resp = self
            .request(
                reqwest::Method::POST,
                &format!("{UPLOAD_URL}?command=APPEND&media_id={id}&segment_index=0"),
                session,
                path,
            )
            .await
            .multipart(reqwest::multipart::Form::new().part("media", part))
            .send()
            .await?;
        if !resp.status().is_success() {
            bail!("APPEND returned {}: {}", resp.status(), resp.text().await?);
        }

        let resp = self
            .request(
                reqwest::Method::POST,
                &format!("{UPLOAD_URL}?command=FINALIZE&media_id={id}"),
                session,
                path,
            )
            .await
            .send()
            .await?;
        let _: Value = self.json(resp).await?;
        Ok(id)
    }

    async fn request(
        &self,
        method: reqwest::Method,
        url: &str,
        session: &Session,
        path: &str,
    ) -> reqwest::RequestBuilder {
        let ct0 = self.ct0.lock().await.clone();
        self.http
            .request(method.clone(), url)
            .header("authorization", format!("Bearer {WEB_BEARER}"))
            .header("x-csrf-token", &ct0)
            .header(
                "cookie",
                format!("auth_token={}; ct0={ct0}", self.auth_token),
            )
            .header("x-twitter-auth-type", "OAuth2Session")
            .header("x-twitter-active-user", "yes")
            .header("x-twitter-client-language", "en")
            .header(
                "x-client-transaction-id",
                session.transaction.id(method.as_str(), path),
            )
            .header("user-agent", USER_AGENT)
            .header("origin", "https://x.com")
            .header("referer", "https://x.com/compose/post")
            .header("accept-language", "en-US,en;q=0.9")
    }

    /// Parses a JSON response, keeping X's error text and any new CSRF cookie.
    async fn json(&self, resp: reqwest::Response) -> Result<Value> {
        for cookie in resp.headers().get_all("set-cookie") {
            if let Some(ct0) = cookie
                .to_str()
                .ok()
                .and_then(|c| c.strip_prefix("ct0="))
                .and_then(|c| c.split(';').next())
                .filter(|c| !c.is_empty())
            {
                *self.ct0.lock().await = ct0.to_owned();
            }
        }
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            bail!(
                "x.com returned {status}: {}",
                text.chars().take(500).collect::<String>()
            );
        }
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text).with_context(|| format!("unexpected x.com response: {text}"))
    }

    async fn session(&self) -> Result<Session> {
        let mut cached = self.session.lock().await;
        if let Some((session, at)) = cached.as_ref()
            && at.elapsed() < SESSION_TTL
        {
            return Ok(session.clone());
        }
        let session = self
            .load_session()
            .await
            .context("cannot read x.com's web client")?;
        *cached = Some((session.clone(), Instant::now()));
        Ok(session)
    }

    async fn load_session(&self) -> Result<Session> {
        let ct0 = self.ct0.lock().await.clone();
        let get = |url: String| {
            self.http
                .get(url)
                .header("user-agent", USER_AGENT)
                .header("accept-language", "en-US,en;q=0.9")
                .header(
                    "cookie",
                    format!("auth_token={}; ct0={ct0}", self.auth_token),
                )
        };
        let home = get(HOME_URL.into())
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        let ondemand = get(ondemand_url(&home)?)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        let main_url = MAIN_JS
            .find(&home)
            .context("main.js not found on the x.com page")?
            .as_str()
            .to_owned();
        let main = get(main_url)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;

        let (create_tweet_query, switches) = create_tweet_operation(&main)?;
        let features = switches
            .into_iter()
            .map(|name| {
                let value = feature_value(&home, &name).unwrap_or(false);
                (name, Value::Bool(value))
            })
            .collect();
        info!("X web client loaded (CreateTweet {create_tweet_query})");
        Ok(Session {
            transaction: Transaction::new(&home, &ondemand)?,
            create_tweet_query,
            features,
        })
    }
}

static MAIN_JS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"https://abs\.twimg\.com/responsive-web/client-web/main\.[0-9a-f]+a?\.js").unwrap()
});

fn ondemand_url(home: &str) -> Result<String> {
    let index = Regex::new(r#",(\d+):["']ondemand\.s["']"#)
        .unwrap()
        .captures(home)
        .context("ondemand.s not found on the x.com page")?[1]
        .to_owned();
    let hash = Regex::new(&format!(r#",{index}:"([0-9a-f]+)""#))
        .unwrap()
        .captures(home)
        .context("ondemand.s hash not found on the x.com page")?[1]
        .to_owned();
    Ok(format!(
        "https://abs.twimg.com/responsive-web/client-web/ondemand.s.{hash}a.js"
    ))
}

/// The CreateTweet query id and its feature switch names from main.js.
fn create_tweet_operation(main_js: &str) -> Result<(String, Vec<String>)> {
    let caps = Regex::new(
        r#"queryId:"([^"]+)",operationName:"CreateTweet",operationType:"mutation",metadata:\{featureSwitches:\[([^\]]*)\]"#,
    )
    .unwrap()
    .captures(main_js)
    .context("CreateTweet not found in main.js")?;
    let switches = caps[2]
        .split(',')
        .map(|s| s.trim().trim_matches('"').to_owned())
        .filter(|s| !s.is_empty())
        .collect();
    Ok((caps[1].to_owned(), switches))
}

fn feature_value(home: &str, name: &str) -> Option<bool> {
    let needle = format!("\"{name}\":{{\"value\":");
    let rest = &home[home.find(&needle)? + needle.len()..];
    Some(rest.starts_with("true"))
}

/// Generates `x-client-transaction-id` values: a XOR-masked blob of the
/// page's verification key, the time and a hash over the request and an
/// "animation key" computed from the home page's loading animation.
#[derive(Clone)]
struct Transaction {
    key_bytes: Vec<u8>,
    animation_key: String,
}

const KEYWORD: &str = "obfiowerehiring";
const EXTRA_BYTE: u8 = 3;
/// 2023-05-01, the epoch of the time field.
const EPOCH_SECS: u64 = 1_682_924_400;

impl Transaction {
    fn new(home: &str, ondemand_js: &str) -> Result<Self> {
        let indices: Vec<usize> = Regex::new(r"\(\w\[(\d{1,2})\],\s*16\)")
            .unwrap()
            .captures_iter(ondemand_js)
            .map(|c| c[1].parse().unwrap())
            .collect();
        ensure!(
            indices.len() >= 2,
            "key byte indices not found in ondemand.s"
        );

        let key = Regex::new(r#"name="twitter-site-verification"\s+content="([^"]+)""#)
            .unwrap()
            .captures(home)
            .context("twitter-site-verification not found on the x.com page")?[1]
            .to_owned();
        let key_bytes = BASE64_STANDARD.decode(&key)?;
        let byte = |i: usize| -> Result<f64> {
            Ok(f64::from(*key_bytes.get(i).context("key too short")? % 16))
        };

        let frames = animation_frames(
            home,
            usize::from(*key_bytes.get(5).context("key too short")?) % 4,
        )?;
        let row = frames
            .get(byte(indices[0])? as usize)
            .context("animation row missing")?;
        let mut frame_time = 1.0;
        for &i in &indices[1..] {
            frame_time *= byte(i)?;
        }
        let frame_time = js_round(frame_time / 10.0) * 10.0;
        let animation_key = animate(row, frame_time / 4096.0)?;
        Ok(Self {
            key_bytes,
            animation_key,
        })
    }

    fn id(&self, method: &str, path: &str) -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let time = now.saturating_sub(EPOCH_SECS) as u32;
        let random = (SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos()
            % 256) as u8;
        self.id_at(method, path, time, random)
    }

    fn id_at(&self, method: &str, path: &str, time: u32, random: u8) -> String {
        let hash = Sha256::digest(format!(
            "{method}!{path}!{time}{KEYWORD}{}",
            self.animation_key
        ));
        let mut out = vec![random];
        out.extend(
            self.key_bytes
                .iter()
                .chain(&time.to_le_bytes())
                .chain(&hash[..16])
                .chain(&[EXTRA_BYTE])
                .map(|b| b ^ random),
        );
        BASE64_STANDARD.encode(out).trim_end_matches('=').to_owned()
    }
}

/// The numbers of each cubic segment ("C ...") of the second path in
/// `#loading-x-anim-{index}`.
fn animation_frames(home: &str, index: usize) -> Result<Vec<Vec<f64>>> {
    let svg = &home[home
        .find(&format!("id=\"loading-x-anim-{index}\""))
        .context("loading animation not found on the x.com page")?..];
    let second_path = svg
        .match_indices("<path")
        .nth(1)
        .context("animation path missing")?
        .0;
    let d = &svg[second_path..];
    let start = d.find("d=\"").context("animation path has no d")? + 3;
    let d = &d[start..];
    let d = &d[..d.find('"').context("unterminated d")?];
    let digits = Regex::new(r"[^\d]+").unwrap();
    Ok(d.get(9..)
        .context("animation path too short")?
        .split('C')
        .map(|segment| {
            digits
                .replace_all(segment, " ")
                .split_whitespace()
                .filter_map(|n| n.parse().ok())
                .collect()
        })
        .collect())
}

fn animate(frames: &[f64], target_time: f64) -> Result<String> {
    ensure!(frames.len() >= 11, "animation row too short");
    let solve = |value: f64, min: f64, max: f64, rounding: bool| {
        let result = value * (max - min) / 255.0 + min;
        if rounding {
            result.floor()
        } else {
            round2(result)
        }
    };
    let to_rotation = solve(frames[6], 60.0, 360.0, true);
    let curves: Vec<f64> = frames[7..]
        .iter()
        .enumerate()
        .map(|(i, &v)| solve(v, if i % 2 == 1 { -1.0 } else { 0.0 }, 1.0, false))
        .collect();
    let value = cubic(&curves, target_time);

    let mut parts: Vec<String> = (0..3)
        .map(|i| {
            let color = (frames[i] * (1.0 - value) + frames[i + 3] * value).clamp(0.0, 255.0);
            format!("{:x}", color.round_ties_even() as i64)
        })
        .collect();
    let rad = (to_rotation * value).to_radians();
    for v in [rad.cos(), -rad.sin(), rad.sin(), rad.cos()] {
        let hex = float_to_hex(round2(v).abs());
        parts.push(if hex.starts_with('.') {
            format!("0{hex}").to_lowercase()
        } else if hex.is_empty() {
            "0".into()
        } else {
            hex
        });
    }
    parts.extend(["0".into(), "0".into()]);
    Ok(parts.concat().replace(['.', '-'], ""))
}

/// Cubic bezier easing with control points `curves` (x1, y1, x2, y2).
fn cubic(curves: &[f64], time: f64) -> f64 {
    let calc = |a: f64, b: f64, m: f64| {
        3.0 * a * (1.0 - m) * (1.0 - m) * m + 3.0 * b * (1.0 - m) * m * m + m * m * m
    };
    if time <= 0.0 {
        let gradient = if curves[0] > 0.0 {
            curves[1] / curves[0]
        } else if curves[1] == 0.0 && curves[2] > 0.0 {
            curves[3] / curves[2]
        } else {
            0.0
        };
        return gradient * time;
    }
    if time >= 1.0 {
        let gradient = if curves[2] < 1.0 {
            (curves[3] - 1.0) / (curves[2] - 1.0)
        } else if curves[2] == 1.0 && curves[0] < 1.0 {
            (curves[1] - 1.0) / (curves[0] - 1.0)
        } else {
            0.0
        };
        return 1.0 + gradient * (time - 1.0);
    }
    let (mut start, mut end, mut mid) = (0.0, 1.0, 0.0);
    while start < end {
        mid = (start + end) / 2.0;
        let x = calc(curves[0], curves[2], mid);
        if (time - x).abs() < 0.00001 {
            return calc(curves[1], curves[3], mid);
        }
        if x < time {
            start = mid;
        } else {
            end = mid;
        }
    }
    calc(curves[1], curves[3], mid)
}

/// Hex digits of a float, uppercase like the original implementation.
fn float_to_hex(mut x: f64) -> String {
    let digit = |d: u32| char::from_digit(d, 16).unwrap().to_ascii_uppercase();
    let mut int_digits = Vec::new();
    let mut quotient = x.trunc();
    let mut fraction = x - quotient;
    while quotient > 0.0 {
        quotient = (x / 16.0).trunc();
        let remainder = (x - quotient * 16.0).trunc() as u32;
        int_digits.push(digit(remainder));
        x = quotient;
    }
    let mut out: String = int_digits.iter().rev().collect();
    if fraction == 0.0 {
        return out;
    }
    out.push('.');
    while fraction > 0.0 {
        fraction *= 16.0;
        let integer = fraction.trunc();
        fraction -= integer;
        out.push(digit(integer as u32));
    }
    out
}

/// Python's `round(x, 2)`.
fn round2(x: f64) -> f64 {
    (x * 100.0).round_ties_even() / 100.0
}

/// JavaScript's `Math.round` as the original implementation emulates it.
fn js_round(x: f64) -> f64 {
    let floor = x.floor();
    let rounded = if x - floor >= 0.5 { x.ceil() } else { floor };
    rounded.copysign(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = include_str!("../tests/fixtures/x-home.html");
    const ONDEMAND: &str = "(n[9],16),u[1](n[33],16),u[2](n[31],16),u[3](n[17],16)";

    /// Expected values come from the Python reference implementation
    /// (iSarabjitDhiman/XClientTransaction) on the same page.
    #[test]
    fn matches_reference_transaction_id() {
        let t = Transaction::new(HOME, ONDEMAND).unwrap();
        assert_eq!(
            t.animation_key,
            "23dc3604f5c28f5c28f5c0f3333333333330f33333333333304f5c28f5c28f5c00"
        );
        assert_eq!(
            t.id_at(
                "POST",
                "/i/api/graphql/l4hk8uVGvR-a8kzQ4nOhPA/CreateTweet",
                123456789,
                7
            ),
            "BzNgSiVsHrc8CyHcLYKplbS7MEdBygeZRKFB1YnO9cG/y6yvIDCXGsxRdnQYn9D1bxLKXAAl35ZwMoFx0L6y5u9QcerIBA"
        );
    }

    #[test]
    fn reads_create_tweet_operation() {
        let js = r#"e.exports={queryId:"abc-123",operationName:"CreateTweet",operationType:"mutation",metadata:{featureSwitches:["a_enabled","b_enabled"],fieldToggles:[]}}"#;
        let (id, switches) = create_tweet_operation(js).unwrap();
        assert_eq!(id, "abc-123");
        assert_eq!(switches, ["a_enabled", "b_enabled"]);
        let page = r#"{"a_enabled":{"value":true},"b_enabled":{"value":false}}"#;
        assert_eq!(feature_value(page, "a_enabled"), Some(true));
        assert_eq!(feature_value(page, "b_enabled"), Some(false));
        assert_eq!(feature_value(page, "c_enabled"), None);
    }

    #[test]
    fn converts_floats_to_hex() {
        assert_eq!(float_to_hex(0.0), "");
        assert_eq!(float_to_hex(0.5), ".8");
        assert_eq!(float_to_hex(255.0), "FF");
        assert_eq!(float_to_hex(1.25), "1.4");
    }
}

#[cfg(test)]
mod live {
    use super::*;

    /// Loads x.com's current web client; run with `cargo test -- --ignored`.
    #[tokio::test]
    #[ignore = "needs network"]
    async fn loads_web_client() {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let web = XWeb::new(reqwest::Client::new(), "x".into(), "x".into());
        let session = web.load_session().await.unwrap();
        println!(
            "CreateTweet {} with {} features, id {}",
            session.create_tweet_query,
            session.features.len(),
            session.transaction.id("POST", "/x")
        );
        assert!(session.features.len() > 10);
    }
}
