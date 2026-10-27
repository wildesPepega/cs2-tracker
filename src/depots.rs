//! Depot tracking: added, removed and changed depots (new manifests per branch).

use crate::discord::{CS2_ICON, CS2_IMAGE, base_embed, image, truncate};
use std::collections::{BTreeMap, BTreeSet};
use twilight_util::builder::embed::{EmbedAuthorBuilder, EmbedBuilder, EmbedFooterBuilder};

type Info = BTreeMap<String, String>;
/// depot id -> (key below `depots/<id>/` -> value)
type Depots<'a> = BTreeMap<&'a str, BTreeMap<&'a str, &'a str>>;

#[derive(Debug, Default, PartialEq)]
pub struct DepotChanges {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub changed: Vec<DepotChange>,
}

#[derive(Debug, PartialEq)]
pub struct DepotChange {
    pub id: String,
    pub manifests: Vec<ManifestChange>,
    /// Number of changed keys outside of `manifests/` (config, flags, ...).
    pub other_keys: usize,
}

#[derive(Debug, PartialEq)]
pub struct ManifestChange {
    pub branch: String,
    pub old_gid: Option<String>,
    pub new_gid: Option<String>,
    pub old_size: Option<u64>,
    pub new_size: Option<u64>,
}

impl DepotChanges {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

/// True for keys like `depots/2347770/...` (as opposed to `depots/branches/...`).
pub fn is_depot_key(key: &str) -> bool {
    depot_id(key).is_some()
}

fn depot_id(key: &str) -> Option<(&str, &str)> {
    let (id, rest) = key.strip_prefix("depots/")?.split_once('/')?;
    id.bytes().all(|b| b.is_ascii_digit()).then_some((id, rest))
}

fn group(info: &Info) -> Depots<'_> {
    let mut out = Depots::new();
    for (k, v) in info {
        if let Some((id, rest)) = depot_id(k) {
            out.entry(id).or_default().insert(rest, v);
        }
    }
    out
}

pub fn depot_changes(old: &Info, new: &Info) -> DepotChanges {
    let (old, new) = (group(old), group(new));
    let mut changes = DepotChanges {
        added: new
            .keys()
            .filter(|id| !old.contains_key(*id))
            .map(|id| id.to_string())
            .collect(),
        removed: old
            .keys()
            .filter(|id| !new.contains_key(*id))
            .map(|id| id.to_string())
            .collect(),
        changed: Vec::new(),
    };

    for (id, new_keys) in &new {
        let Some(old_keys) = old.get(id) else {
            continue;
        };
        if old_keys == new_keys {
            continue;
        }
        let branches: BTreeSet<&str> = old_keys
            .keys()
            .chain(new_keys.keys())
            .filter_map(|k| {
                k.strip_prefix("manifests/")?
                    .split_once('/')
                    .map(|(b, _)| b)
            })
            .collect();
        let get = |keys: &BTreeMap<&str, &str>, branch: &str, field: &str| {
            keys.get(format!("manifests/{branch}/{field}").as_str())
                .map(|v| v.to_string())
        };
        let mut manifests: Vec<ManifestChange> = branches
            .into_iter()
            .filter_map(|branch| {
                let change = ManifestChange {
                    branch: branch.to_owned(),
                    old_gid: get(old_keys, branch, "gid"),
                    new_gid: get(new_keys, branch, "gid"),
                    old_size: get(old_keys, branch, "size").and_then(|s| s.parse().ok()),
                    new_size: get(new_keys, branch, "size").and_then(|s| s.parse().ok()),
                };
                (change.old_gid != change.new_gid).then_some(change)
            })
            .collect();
        manifests.sort_by_key(|m| (m.branch != "public", m.branch.clone()));

        let other_keys = old_keys
            .keys()
            .chain(new_keys.keys())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter(|k| !k.starts_with("manifests/") && old_keys.get(*k) != new_keys.get(*k))
            .count();

        if !manifests.is_empty() || other_keys > 0 {
            changes.changed.push(DepotChange {
                id: id.to_string(),
                manifests,
                other_keys,
            });
        }
    }
    changes
}

pub fn depot_embed(
    app_name: &str,
    appid: u32,
    change_number: u32,
    changes: &DepotChanges,
    new: &Info,
) -> EmbedBuilder {
    let mut sections = Vec::new();

    if !changes.added.is_empty() {
        let lines: Vec<String> = changes
            .added
            .iter()
            .map(|id| format!("{} {}", depot_link(id), describe(id, new)))
            .collect();
        sections.push(format!(
            "**🆕 New depots ({})**\n{}",
            changes.added.len(),
            lines.join("\n")
        ));
    }
    if !changes.removed.is_empty() {
        let lines: Vec<String> = changes.removed.iter().map(|id| depot_link(id)).collect();
        sections.push(format!(
            "**🗑️ Removed depots ({})**\n{}",
            changes.removed.len(),
            lines.join("\n")
        ));
    }
    if !changes.changed.is_empty() {
        let mut lines = Vec::new();
        for depot in &changes.changed {
            lines.push(depot_link(&depot.id));
            for m in &depot.manifests {
                lines.push(format!("└ {}", manifest_line(&depot.id, m)));
            }
            if depot.other_keys > 0 {
                lines.push(format!("└ {} config value(s) changed", depot.other_keys));
            }
        }
        sections.push(format!(
            "**🔄 Changed depots ({})**\n{}",
            changes.changed.len(),
            lines.join("\n")
        ));
    }

    base_embed()
        .author(EmbedAuthorBuilder::new(format!("{app_name} · {appid}")).icon_url(image(CS2_ICON)))
        .title(format!("📦 Depot changes · Changelist #{change_number}"))
        .url(format!("https://steamdb.info/app/{appid}/depots/"))
        .description(truncate(&sections.join("\n\n"), 4000))
        .thumbnail(image(CS2_IMAGE))
        .footer(EmbedFooterBuilder::new("SteamDB · PICS"))
}

fn depot_link(id: &str) -> String {
    format!("[`{id}`](https://steamdb.info/depot/{id}/)")
}

/// Short summary of a depot: OS and public manifest size, if known.
fn describe(id: &str, info: &Info) -> String {
    let get = |k: &str| info.get(&format!("depots/{id}/{k}"));
    let mut parts = Vec::new();
    if let Some(name) = get("name") {
        parts.push(name.clone());
    }
    if let Some(os) = get("config/oslist") {
        parts.push(os.clone());
    }
    if let Some(size) = get("manifests/public/size").and_then(|s| s.parse().ok()) {
        parts.push(format_size(size));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("· {}", parts.join(" · "))
    }
}

fn manifest_line(depot: &str, m: &ManifestChange) -> String {
    let gid = |g: &Option<String>| match g {
        Some(g) => format!("[`{g}`](https://steamdb.info/depot/{depot}/history/?changeid=M:{g})"),
        None => "–".into(),
    };
    let size = match (m.old_size, m.new_size) {
        (Some(old), Some(new)) => format!(
            " · {} ({})",
            format_size(new),
            format_delta(new as i64 - old as i64)
        ),
        (None, Some(new)) => format!(" · {}", format_size(new)),
        _ => String::new(),
    };
    match (&m.old_gid, &m.new_gid) {
        (None, Some(_)) => format!("**{}**: new manifest {}{size}", m.branch, gid(&m.new_gid)),
        (Some(_), None) => format!("**{}**: manifest removed", m.branch),
        // Only the new manifest is linked to stay within Discord's embed limit.
        _ => format!(
            "**{}**: `{}` → {}{size}",
            m.branch,
            m.old_gid.as_deref().unwrap_or("–"),
            gid(&m.new_gid)
        ),
    }
}

fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

fn format_delta(delta: i64) -> String {
    let sign = if delta < 0 { "-" } else { "+" };
    format!("{sign}{}", format_size(delta.unsigned_abs()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> Info {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn detects_added_removed_and_changed_depots() {
        let old = map(&[
            ("depots/branches/public/buildid", "1"),
            ("depots/100/manifests/public/gid", "aaa"),
            ("depots/100/manifests/public/size", "1000"),
            ("depots/100/manifests/old/gid", "zzz"),
            ("depots/200/config/oslist", "windows"),
            ("depots/300/manifests/public/gid", "same"),
        ]);
        let new = map(&[
            ("depots/branches/public/buildid", "2"),
            ("depots/100/manifests/public/gid", "bbb"),
            ("depots/100/manifests/public/size", "1500"),
            ("depots/100/manifests/beta/gid", "ccc"),
            ("depots/300/manifests/public/gid", "same"),
            ("depots/400/config/oslist", "linux"),
        ]);
        let changes = depot_changes(&old, &new);
        assert_eq!(changes.added, ["400"]);
        assert_eq!(changes.removed, ["200"]);
        assert_eq!(changes.changed.len(), 1);

        let depot = &changes.changed[0];
        assert_eq!(depot.id, "100");
        assert_eq!(depot.other_keys, 0);
        let branches: Vec<_> = depot.manifests.iter().map(|m| m.branch.as_str()).collect();
        assert_eq!(branches, ["public", "beta", "old"]);
        assert_eq!(depot.manifests[0].old_size, Some(1000));
        assert_eq!(depot.manifests[0].new_size, Some(1500));
        assert_eq!(depot.manifests[1].old_gid, None);
        assert_eq!(depot.manifests[2].new_gid, None);
    }

    #[test]
    fn config_only_change_counts_other_keys() {
        let old = map(&[("depots/100/config/oslist", "windows")]);
        let new = map(&[("depots/100/config/oslist", "windows,linux")]);
        let changes = depot_changes(&old, &new);
        assert_eq!(changes.changed[0].other_keys, 1);
        assert!(changes.changed[0].manifests.is_empty());
    }

    #[test]
    fn depot_keys_exclude_branches() {
        assert!(is_depot_key("depots/2347770/manifests/public/gid"));
        assert!(!is_depot_key("depots/branches/public/buildid"));
        assert!(!is_depot_key("depots/baselanguages"));
    }

    #[test]
    fn formats_sizes() {
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(65_027_793_761), "65.03 GB");
        assert_eq!(format_delta(-1_500_000), "-1.50 MB");
    }
}
