# CS2 Tracker

[![CI](https://github.com/wildesPepega/cs2-tracker/actions/workflows/ci.yml/badge.svg)](https://github.com/wildesPepega/cs2-tracker/actions/workflows/ci.yml)

Discord-Bot, der Counter-Strike 2 überwacht und Änderungen als Embeds in einen Channel postet:

| Quelle | Was | Intervall |
|---|---|---|
| Steam PICS (dieselbe Quelle wie SteamDB) | Changelists für **730** (CS2), **2275500** (DevClient), **2275530** (DevServer). Neue Builds auf dem `public`-Branch bekommen ein eigenes Update-Embed mit Rollen-Ping. | 2 s |
| Steam News API | Offizielle CS2-Patchnotes und -Blogposts | 20 s |
| FxTwitter API | Banner-Änderungen von [@CounterStrike](https://x.com/CounterStrike) | 60 s |

Der Bot meldet sich anonym bei Steam an, ein Steam-Account ist nicht nötig. Bei den Dev-Apps
sind die App-Infos ohne Access-Token gesperrt, dort sind nur die Changelist-Nummern sichtbar.
Mehr zeigt auch SteamDB öffentlich nicht an.

## Setup

1. Im [Discord Developer Portal](https://discord.com/developers/applications) eine Application mit Bot anlegen und den Token kopieren.
2. Bot einladen (Scope `bot`, Rechte: *View Channel*, *Send Messages*, *Embed Links*, bei Rollen-Ping zusätzlich *Mention Everyone* oder die Rolle als erwähnbar markieren).
3. `cp .env.example .env` und die Werte eintragen.
4. Starten, entweder mit einem fertigen statischen Binary aus den [Releases](https://github.com/wildesPepega/cs2-tracker/releases) (Linux x86_64/aarch64):

```sh
curl -fsSL -o cs2-tracker https://github.com/wildesPepega/cs2-tracker/releases/latest/download/cs2-tracker-linux-x86_64
chmod +x cs2-tracker && ./cs2-tracker
```

…oder aus dem Quellcode:

```sh
cargo run --release
```

Beim ersten Start werden alle aktuellen Stände nur als Basis gespeichert (`state.json`), ohne etwas zu posten.
Danach wird jede Änderung gepostet, auch Änderungen, die passiert sind, während der Bot offline war.

## Pterodactyl

Im Ordner [`pterodactyl/`](pterodactyl/egg-cs2-tracker.json) liegt ein fertiges Egg.

1. Panel → *Admin* → *Nests* → *Import Egg* → `egg-cs2-tracker.json` hochladen.
2. Einen Server mit dem Egg anlegen. Er braucht kaum Ressourcen (ca. 64 MB RAM, 0,25 CPU, 50 MB Disk) und keinen Port.
3. Bot-Token und Channel-ID in den Startup-Variablen eintragen und den Server starten.

Das Installationsskript lädt das passende Release-Binary herunter. Für ein Update genügt *Reinstall*
(bei `VERSION=latest`), die `state.json` bleibt dabei erhalten.

## Als Dienst (systemd)

```ini
# /etc/systemd/system/cs2-tracker.service
[Unit]
Description=CS2 Tracker Discord Bot
After=network-online.target
Wants=network-online.target

[Service]
WorkingDirectory=/pfad/zu/terroristsnitcher
ExecStart=/pfad/zu/terroristsnitcher/target/release/cs2-tracker
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
```

## Tests

```sh
cargo test
```
