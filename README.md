# CS2 Tracker

[![CI](https://github.com/wildesPepega/cs2-tracker/actions/workflows/ci.yml/badge.svg)](https://github.com/wildesPepega/cs2-tracker/actions/workflows/ci.yml)

A Discord bot written in Rust that watches Counter-Strike 2 and posts every change as a rich embed.

| Source | What | Interval |
|---|---|---|
| Steam PICS (the same data SteamDB uses) | Changelists for **730** (CS2), **2275500** (DevClient) and **2275530** (DevServer). A new build on the `public` branch gets its own update embed. | 2 s |
| Steam PICS | Depot changes: new, removed and changed depots, including new manifests per branch and size deltas. | with every changelist |
| Steam News API | Official CS2 patch notes and blog posts | 20 s |
| FxTwitter API | Banner and profile picture changes of [@CounterStrike](https://x.com/CounterStrike) | 60 s |

It also:

- **shows up as online** with a rotating set of 150+ CS-themed statuses ("Playing de_mirage", "Listening to the bomb beeps", …). By default it uses the VR platform icon (see [Presence](#presence)).
- **updates itself.** It checks GitHub releases, swaps its own binary and restarts in place.
- can **post to X/Twitter** on game updates, DevClient/DevServer updates and @CounterStrike banner and profile picture changes (see [Posting to X](#posting-to-x)).
- can **play a sound in a TeamSpeak channel** (TS3 and TS6 servers) when a game update is detected (see [TeamSpeak](#teamspeak)).
- has an admin-only **`/preview`** command that shows example embeds built from the live data (see [Commands](#commands)).

The bot logs in to Steam anonymously, so you don't need a Steam account. For the dev apps, the app info is hidden without an access token, so only their changelist numbers are visible. SteamDB doesn't show more publicly either.

## Setup

1. Create an application with a bot in the [Discord Developer Portal](https://discord.com/developers/applications) and copy the token. No privileged intents are needed.
2. Invite the bot (scopes `bot` and `applications.commands`, permissions *View Channel*, *Send Messages* and *Embed Links*). For role pings, make the role mentionable or grant *Mention Everyone*. On startup the bot logs which role it pings and warns if it isn't mentionable.
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
| `PING_ROLE_ID` | – | Role to ping (the role's ID, not its @mention) |
| `PING_EVENTS` | `update,news` | Posts that ping the role: `update` (new public build), `changelist` (CS2), `dev` (DevClient/DevServer changelist), `depots`, `news`, `banner`, `avatar` (profile picture), or `all` / `none` |
| `PICS_INTERVAL_MS` | `2000` | Steam changelist polling interval |
| `NEWS_INTERVAL_SECS` | `20` | Steam News polling interval |
| `TWITTER_INTERVAL_SECS` | `60` | Twitter banner / profile picture polling interval |
| `TWITTER_HANDLE` | `CounterStrike` | Account whose banner and profile picture are watched |
| `STATUS_INTERVAL_SECS` | `60` | How often the status rotates (min. 15) |
| `PRESENCE_PLATFORM` | `vr` | Platform icon: `vr`, `mobile` or `desktop` |
| `AUTO_UPDATE` | `true` | Install new releases automatically |
| `UPDATE_INTERVAL_MINS` | `10` | How often to check for updates |
| `STATE_FILE` | `state.json` | Where the state is stored |
| `TS_ADDRESS` | – | TeamSpeak server (`host` or `host:port`). Enables the TeamSpeak bot. |
| `TS_CHANNEL` | default channel | Channel to sit in: its name, or the full path (`Parent/Child`) if the name isn't unique |
| `TS_CHANNEL_PASSWORD` | – | Password of that channel |
| `TS_SERVER_PASSWORD` | – | Server password |
| `TS_NICKNAME` | `CS2 Tracker` | Nickname in TeamSpeak |
| `TS_EVENTS` | `update` | Posts that play the sound, same names as `PING_EVENTS` |
| `X_API_KEY`, `X_API_SECRET`, `X_ACCESS_TOKEN`, `X_ACCESS_SECRET` | – | X API credentials. Enable posting to X (see [Posting to X](#posting-to-x)). |
| `X_EVENTS` | `update,dev,banner,avatar` | Events that get posted to X, same names as `PING_EVENTS` |
| `TS_SOUND_FILE` | bundled CS2 update sound | Your own sound as an Ogg Opus file (see [TeamSpeak](#teamspeak)) |

## Commands

| Command | Who | What |
|---|---|---|
| `/soundtest` | Administrators | Plays the sound in TeamSpeak. Only registered when `TS_ADDRESS` is set. |
| `/preview` | Administrators | Replies with a bot status overview and example embeds built from the current data: a CS2 update, changelists for all three apps, depot changes, the latest blog post and the current Twitter banner and profile picture. Changes are simulated. The replies are ephemeral, so only you can see them. |

The command is registered globally when the bot connects. New global commands can take a few minutes to show up.
By default only administrators can see it. Server owners can change that under *Server Settings → Integrations*,
but the bot still refuses anyone without the *Administrator* permission.

## TeamSpeak

Set `TS_ADDRESS` and the bot also joins a TeamSpeak server. It sits in `TS_CHANNEL` and plays the CS2 update sound whenever a
new public CS2 build is detected (`TS_EVENTS=update`). Use `/soundtest` to hear it. It works with TS3 and TS6 servers
and reconnects on its own if the connection drops.

- On first start the bot creates a TeamSpeak identity (security level 8) and stores it in `state.json`, so it keeps its
  server groups across restarts.
- The bot needs enough talk power for the channel. It logs a warning if it isn't allowed to talk.
- `TS_SOUND_FILE` takes an Ogg Opus file. Convert any sound with ffmpeg:

  ```sh
  ffmpeg -i sound.mp3 -c:a libopus -b:a 96k -ar 48000 -frame_duration 20 sound.opus
  ```

  The bot plays the file as it is, so set the volume in the file (for example with `-af volume=0.5`).

## Posting to X

With X API credentials the bot also posts to an X account:

| Event | Post |
|---|---|
| `update` | 🚨 Counter-Strike 2 has been updated! with the old and new build ID and the changelist |
| `dev` | 🛠️ CS2 DevClient / CS2 DevServer has been updated with the changelist |
| `banner` | 🖼️ @CounterStrike has a new banner! with the banner attached |
| `avatar` | 👤 @CounterStrike has a new profile picture! with the picture attached |

Posts never contain links, because X charges a lot more for those. Failed posts are logged and not retried, so
nothing is posted twice.

Setup:

1. Create a project and app in the [X Developer Console](https://console.x.com) and add credits. Posting is paid
   (pay-per-use, roughly $0.015 per post as of 2026). DevClient/DevServer changelists can come in several times a day,
   so leave `dev` out of `X_EVENTS` if that gets too expensive.
2. Under *User authentication settings*, set the app permissions to **Read and write**.
3. Copy the *API Key* and *API Key Secret*. Then generate the *Access Token* and *Access Token Secret* for the
   account that should post. Generate them after setting *Read and write*, or they stay read-only.
4. Set `X_API_KEY`, `X_API_SECRET`, `X_ACCESS_TOKEN` and `X_ACCESS_SECRET`. On startup the bot logs
   `Posting to X as @<account>`.

## Presence

Discord picks the platform icon next to a user's status from the properties the client sends when it connects.
`PRESENCE_PLATFORM=vr` identifies as the *Discord VR* (Meta Quest) client. That value is undocumented, so Discord may
stop honoring it at any time. Set it to `mobile` or `desktop` if the icon doesn't show up.

## Auto-update

The bot checks GitHub for a newer release at three points:

- **On startup**, before it connects to anything. If there's an update, it installs it and restarts right away.
- **Every `UPDATE_INTERVAL_MINS`** while running. If there's an update, it installs it and restarts in place.
- **On shutdown** (Ctrl+C / SIGINT, which is what Pterodactyl sends, or SIGTERM). If there's an update, it installs it
  so the next start already runs the new version. This waits at most 20 seconds.

Installing an update means:

1. downloading the binary for its architecture,
2. running it with `--version` to make sure it works, and
3. replacing its own executable.

Restarts re-execute the binary in the same process. Because the PID stays the same, Pterodactyl and systemd don't see
a restart. The bot needs write access to the directory its binary lives in. Set `AUTO_UPDATE=false` to turn all of
this off.

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
