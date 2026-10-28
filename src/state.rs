//! Persisted bot state, so restarts neither re-post nor miss anything.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    path::{Path, PathBuf},
};
use tokio::sync::Mutex;

const MAX_SEEN_NEWS: usize = 200;

#[derive(Default, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub apps: BTreeMap<u32, AppState>,
    /// `None` until the first successful fetch, which only seeds without posting.
    #[serde(default)]
    pub seen_news: Option<VecDeque<String>>,
    #[serde(default)]
    pub twitter_banner: Option<String>,
    #[serde(default)]
    pub twitter_avatar: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct AppState {
    pub change_number: u32,
    /// Flattened appinfo (`path -> value`) used to diff the next changelist.
    pub info: BTreeMap<String, String>,
}

impl State {
    pub fn mark_news_seen(&mut self, gid: &str) {
        let seen = self.seen_news.get_or_insert_default();
        seen.push_back(gid.to_owned());
        while seen.len() > MAX_SEEN_NEWS {
            seen.pop_front();
        }
    }
}

pub struct Store {
    path: PathBuf,
    state: Mutex<State>,
}

impl Store {
    pub fn load(path: &Path) -> Result<Self> {
        let state = match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => State::default(),
            Err(e) => return Err(e.into()),
        };
        Ok(Self {
            path: path.to_owned(),
            state: Mutex::new(state),
        })
    }

    /// Runs `f` on the state and writes it to disk afterwards.
    pub async fn update<T>(&self, f: impl FnOnce(&mut State) -> T) -> T {
        let mut state = self.state.lock().await;
        let out = f(&mut state);
        if let Err(e) = self.save(&state) {
            tracing::error!("Failed to save state: {e:#}");
        }
        out
    }

    pub async fn read<T>(&self, f: impl FnOnce(&State) -> T) -> T {
        f(&*self.state.lock().await)
    }

    fn save(&self, state: &State) -> Result<()> {
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(state)?)?;
        std::fs::rename(tmp, &self.path)?;
        Ok(())
    }
}
