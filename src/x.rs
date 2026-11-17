//! Optional auto-posting to X/Twitter, either through the paid X API v2
//! (signed with OAuth 1.0a user context) or for free through x.com's web
//! client with the account's cookies (see [`crate::x_web`]). Posts never
//! contain links, because the API charges much more for posts with URLs.

use crate::{
    config::{ApiKeys, Event, XAuth, XConfig},
    x_web::XWeb,
};
use anyhow::{Context, Result, bail};
use base64::{Engine, prelude::BASE64_STANDARD};
use hmac::{Hmac, Mac};
use serde::Deserialize;
use serde_json::json;
use sha1::Sha1;
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};
use tracing::{info, warn};

const API: &str = "https://api.x.com/2";

pub struct X {
    http: reqwest::Client,
    backend: Backend,
    events: Vec<Event>,
}

#[derive(Clone)]
enum Backend {
    Api(Arc<ApiKeys>),
    Web(Arc<XWeb>),
}

#[derive(Deserialize)]
struct Data<T> {
    data: T,
}

#[derive(Deserialize)]
struct Me {
    username: String,
}

#[derive(Deserialize)]
struct Id {
    id: String,
}

impl X {
    pub fn new(http: reqwest::Client, config: XConfig) -> Self {
        let backend = match config.auth {
            XAuth::Api(keys) => Backend::Api(Arc::new(keys)),
            XAuth::Cookies { auth_token, ct0 } => {
                Backend::Web(Arc::new(XWeb::new(http.clone(), auth_token, ct0)))
            }
        };
        Self {
            http,
            backend,
            events: config.events,
        }
    }

    pub fn events(&self) -> &[Event] {
        &self.events
    }

    pub fn mode(&self) -> &'static str {
        match self.backend {
            Backend::Api(_) => "API",
            Backend::Web(_) => "cookies",
        }
    }

    /// Logs which account posts go to, so wrong keys show up at startup.
    pub async fn check(&self) {
        let result = match &self.backend {
            Backend::Api(keys) => {
                let url = format!("{API}/users/me");
                async {
                    let me: Data<Me> = self
                        .http
                        .get(&url)
                        .header("Authorization", keys.authorization("GET", &url))
                        .send()
                        .await?
                        .error_for_status()?
                        .json()
                        .await?;
                    anyhow::Ok(me.data.username)
                }
                .await
            }
            Backend::Web(web) => web.check().await,
        };
        match result {
            Ok(user) => info!(
                "Posting to X as @{user} ({}) on {:?}",
                self.mode(),
                self.events
            ),
            Err(e) => warn!("X login check failed: {e:#}"),
        }
    }

    /// Posts `text` (and the image behind `image_url`) in the background if
    /// `event` is one of `X_EVENTS`. Failures are logged, never retried, so
    /// nothing gets posted twice.
    pub fn post(&self, event: Event, text: String, image_url: Option<String>) {
        if !self.events.contains(&event) {
            return;
        }
        let http = self.http.clone();
        let backend = self.backend.clone();
        tokio::spawn(async move {
            let result = match backend {
                Backend::Api(keys) => post(&http, &keys, &text, image_url.as_deref()).await,
                Backend::Web(web) => {
                    let image = match image_url {
                        Some(url) => match download(&http, &url).await {
                            Ok(image) => Some(image),
                            Err(e) => {
                                warn!("Cannot download image for X, posting without it: {e:#}");
                                None
                            }
                        },
                        None => None,
                    };
                    web.post(&text, image).await
                }
            };
            match result {
                Ok(id) => info!("Posted to X: {id}"),
                Err(e) => warn!("Posting to X failed: {e:#}"),
            }
        });
    }
}

/// Downloads an image and its MIME type.
async fn download(http: &reqwest::Client, url: &str) -> Result<(Vec<u8>, String)> {
    let resp = http.get(url).send().await?.error_for_status()?;
    let mime = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("image/jpeg")
        .to_owned();
    Ok((resp.bytes().await?.to_vec(), mime))
}

async fn post(
    http: &reqwest::Client,
    config: &ApiKeys,
    text: &str,
    image_url: Option<&str>,
) -> Result<String> {
    let media_id = match image_url {
        Some(url) => match upload_image(http, config, url).await {
            Ok(id) => Some(id),
            Err(e) => {
                warn!("Image upload to X failed, posting without it: {e:#}");
                None
            }
        },
        None => None,
    };
    let mut body = json!({ "text": text });
    if let Some(id) = media_id {
        body["media"] = json!({ "media_ids": [id] });
    }
    let url = format!("{API}/tweets");
    let resp = http
        .post(&url)
        .header("Authorization", config.authorization("POST", &url))
        .json(&body)
        .send()
        .await?;
    let created: Data<Id> = ok_json(resp).await?;
    Ok(created.data.id)
}

async fn upload_image(http: &reqwest::Client, config: &ApiKeys, image_url: &str) -> Result<String> {
    let bytes = http
        .get(image_url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    let url = format!("{API}/media/upload");
    // JSON bodies are not part of the OAuth 1.0a signature, multipart would
    // be the same, so base64 in JSON is the simplest.
    let resp = http
        .post(&url)
        .header("Authorization", config.authorization("POST", &url))
        .json(&json!({
            "media": BASE64_STANDARD.encode(&bytes),
            "media_category": "tweet_image",
        }))
        .send()
        .await?;
    let media: Data<Id> = ok_json(resp).await?;
    Ok(media.data.id)
}

/// Like `error_for_status`, but keeps X's error message.
async fn ok_json<T: serde::de::DeserializeOwned>(resp: reqwest::Response) -> Result<T> {
    let status = resp.status();
    let text = resp.text().await?;
    if !status.is_success() {
        bail!("X API returned {status}: {text}");
    }
    serde_json::from_str(&text).with_context(|| format!("unexpected X API response: {text}"))
}

impl ApiKeys {
    fn authorization(&self, method: &str, url: &str) -> String {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let nonce = format!(
            "{:x}{:x}",
            now.as_nanos(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        self.sign(method, url, &[], &nonce, now.as_secs())
    }

    /// Builds the OAuth 1.0a `Authorization` header (HMAC-SHA1). `extra` are
    /// query/form parameters that are part of the signature.
    fn sign(
        &self,
        method: &str,
        url: &str,
        extra: &[(&str, &str)],
        nonce: &str,
        timestamp: u64,
    ) -> String {
        let timestamp = timestamp.to_string();
        let oauth = [
            ("oauth_consumer_key", self.api_key.as_str()),
            ("oauth_nonce", nonce),
            ("oauth_signature_method", "HMAC-SHA1"),
            ("oauth_timestamp", &timestamp),
            ("oauth_token", &self.access_token),
            ("oauth_version", "1.0"),
        ];
        let mut params: Vec<(String, String)> = oauth
            .iter()
            .chain(extra)
            .map(|(k, v)| (encode(k), encode(v)))
            .collect();
        params.sort();
        let params = params
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");
        let base = format!("{method}&{}&{}", encode(url), encode(&params));
        let key = format!(
            "{}&{}",
            encode(&self.api_secret),
            encode(&self.access_secret)
        );
        let mut mac = Hmac::<Sha1>::new_from_slice(key.as_bytes()).expect("HMAC takes any key");
        mac.update(base.as_bytes());
        let signature = BASE64_STANDARD.encode(mac.finalize().into_bytes());

        let header = oauth
            .iter()
            .map(|(k, v)| (*k, *v))
            .chain([("oauth_signature", signature.as_str())])
            .map(|(k, v)| format!("{k}=\"{}\"", encode(v)))
            .collect::<Vec<_>>()
            .join(", ");
        format!("OAuth {header}")
    }
}

/// Percent-encoding as OAuth 1.0a wants it (RFC 3986, only unreserved chars
/// stay as they are).
fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The example from X's "Creating a signature" docs.
    #[test]
    fn signs_like_the_docs() {
        let config = ApiKeys {
            api_key: "xvz1evFS4wEEPTGEFPHBog".into(),
            api_secret: "kAcSOqF21Fu85e7zjz7ZN2U4ZRhfV3WpwPAoE3Z7kBw".into(),
            access_token: "370773112-GmHxMAgYyLbNEtIKZeRNFsMKPR9EyMZeS9weJAEb".into(),
            access_secret: "LswwdoUaIvS8ltyTt5jkRh4J50vUPVVHtR2YPi5kE".into(),
        };
        let header = config.sign(
            "POST",
            "https://api.twitter.com/1.1/statuses/update.json",
            &[
                ("include_entities", "true"),
                (
                    "status",
                    "Hello Ladies + Gentlemen, a signed OAuth request!",
                ),
            ],
            "kYjzVBB8Y0ZFabxSWbWovY3uYSQ2pTgmZeNu2VS4cg",
            1318622958,
        );
        assert!(
            header.contains(r#"oauth_signature="hCtSmYh%2BiHYCEqBWrE7C7hYmtUk%3D""#),
            "{header}"
        );
    }

    #[test]
    fn encodes_like_oauth() {
        assert_eq!(encode("Ladies + Gentlemen!"), "Ladies%20%2B%20Gentlemen%21");
        assert_eq!(encode("a-b.c_d~"), "a-b.c_d~");
        assert_eq!(encode("ä"), "%C3%A4");
    }
}
