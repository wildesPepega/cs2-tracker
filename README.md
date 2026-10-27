# CS2 Tracker

[![CI](https://github.com/wildesPepega/cs2-tracker/actions/workflows/ci.yml/badge.svg)](https://github.com/wildesPepega/cs2-tracker/actions/workflows/ci.yml)

A Discord bot written in Rust that watches Counter-Strike 2 and posts every change as a rich embed.

| Source | What | Interval |
|---|---|---|
| Steam PICS (the same data SteamDB uses) | Changelists for **730** (CS2), **2275500** (DevClient) and **2275530** (DevServer). A new build on the `public` branch gets its own update embed with an optional role ping. | 2 s |
| Steam PICS | Depot changes: new, removed and changed depots, including new manifests per branch and size deltas. | with every changelist |
| Steam News API | Official CS2 patch notes and blog posts | 20 s |
| FxTwitter API | Banner changes of [@CounterStrike](https://x.com/CounterStrike) | 60 s |

It also:

- **shows up as online** with a rotating set of 150+ CS-themed statuses ("Playing de_mirage", "Listening to the bomb beeps", …). By default it uses the VR platform icon (see [Presence](#presence)).
- **updates itself.** It checks GitHub releases, swaps its own binary and restarts in place.

The bot logs in to Steam anonymously, so you don't need a Steam account. For the dev apps, the app info is hidden without an access token, so only their changelist numbers are visible. SteamDB doesn't show more publicly either.

## Setup

1. Create an application with a bot in the [Discord Developer Portal](https://discord.com/developers/applications) and copy the token. No privileged intents are needed.
2. Invite the bot (scope `bot`, permissions *View Channel*, *Send Messages* and *Embed Links*). For role pings, make the role mentionable or grant *Mention Everyone*.
3. Run `cp .env.example .env` and fill in the values.
4. Run it, either with a static binary from the [releases](https://github.com/wildesPepega/cs2-tracker/releases) (Linux x86_64/aarch64):

```sh
curl -fsSL -o cs2-tracker https://github.com/wildesPepega/cs2-tracker/releases/latest/download/cs2-tracker-linux-x86_64
chmod +x cs2-tracker && ./cs2-tracker
```

…or from source:

```sh
cargo run --release
```

On first start, everything that exists right now is stored as a baseline in `state.json` and nothing gets posted.
After that, every change is posted, including changes that happened while the bot was offline.

## Configuration

All settings are environment variables (a `.env` file is loaded automatically).

| Variable | Default | Description |
|---|---|---|
| `DISCORD_TOKEN` | – | Bot token (required) |
| `DISCORD_CHANNEL_ID` | – | Channel to post into (required) |
| `PING_ROLE_ID` | – | Role pinged on new public builds |
| `PICS_INTERVAL_MS` | `2000` | Steam changelist polling interval |
| `NEWS_INTERVAL_SECS` | `20` | Steam News polling interval |
| `TWITTER_INTERVAL_SECS` | `60` | Twitter banner polling interval |
| `TWITTER_HANDLE` | `CounterStrike` | Account whose banner is watched |
| `STATUS_INTERVAL_SECS` | `60` | How often the status rotates (min. 15) |
| `PRESENCE_PLATFORM` | `vr` | Platform icon: `vr`, `mobile` or `desktop` |
| `AUTO_UPDATE` | `true` | Install new releases automatically |
| `UPDATE_INTERVAL_MINS` | `10` | How often to check for updates |
| `STATE_FILE` | `state.json` | Where the state is stored |

## Presence

Discord picks the platform icon next to a user's status from the properties the client sends when it connects.
`PRESENCE_PLATFORM=vr` identifies as the *Discord VR* (Meta Quest) client. That value is undocumented, so Discord may
stop honoring it at any time. Set it to `mobile` or `desktop` if the icon doesn't show up.

## Auto-update

Every `UPDATE_INTERVAL_MINS`, the bot asks GitHub for the latest release. If it's newer than the running version, the bot:

1. downloads the binary for its architecture,
2. runs it with `--version` to make sure it works,
3. replaces its own executable, and
4. re-executes itself in the same process.

Because the PID stays the same, Pterodactyl and systemd don't see a restart. The bot needs write access to
the directory its binary lives in. Set `AUTO_UPDATE=false` to turn this off.

## Pterodactyl

The [`pterodactyl/`](pterodactyl/egg-cs2-tracker.json) folder contains a ready-to-use egg.

1. Go to Panel → *Admin* → *Nests* → *Import Egg* and upload `egg-cs2-tracker.json`.
2. Create a server with the egg. It barely needs anything (about 64 MB RAM, 0.25 CPU, 50 MB disk) and no port.
3. Fill in the bot token and channel ID in the startup variables and start the server.

The install script downloads the matching release binary. After that, the built-in auto-updater keeps it up to date.
*Reinstall* also works and keeps `state.json`.

## Running as a service (systemd)

```ini
# /etc/systemd/system/cs2-tracker.service
[Unit]
Description=CS2 Tracker Discord bot
After=network-online.target
Wants=network-online.target

[Service]
WorkingDirectory=/opt/cs2-tracker
ExecStart=/opt/cs2-tracker/cs2-tracker
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
```

## Development

```sh
cargo test
```

Releases are built locally as static musl binaries with [cargo-zigbuild](https://github.com/rust-cross/cargo-zigbuild)
and published with the GitHub CLI:

```sh
# bump `version` in Cargo.toml, commit, then:
./scripts/release.sh
```
