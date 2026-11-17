//! Optional TeamSpeak bot (TS3 and TS6 servers): sits in a channel and plays a
//! sound when a game update is detected.

use crate::{Ctx, config::Event};
use anyhow::{Context, Result, bail, ensure};
use futures::prelude::*;
use std::{collections::VecDeque, path::Path, sync::Arc, time::Duration};
use tokio::{
    sync::mpsc,
    time::{Instant, sleep, sleep_until, timeout},
};
use tracing::{info, warn};
use tsclientlib::{
    ChannelId, Connection, DisconnectOptions, Identity, InMessage, OutCommandExt, StreamItem,
    messages::c2s::{OutClientMoveMessage, OutClientMovePart},
};
use tsproto_packets::packets::{AudioData, CodecType, OutAudio};

/// CS2 update sound bundled with the binary, used unless `TS_SOUND_FILE` is set.
const DEFAULT_SOUND: &[u8] = include_bytes!("../assets/update.opus");
/// Security level most servers require for new clients.
const IDENTITY_LEVEL: u8 = 8;
const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(300);

pub enum Command {
    Play,
    Quit,
}

/// Handle used by the rest of the bot; the connection lives in [`run`].
pub struct TeamSpeak {
    tx: mpsc::Sender<Command>,
    events: Vec<Event>,
}

impl TeamSpeak {
    pub fn new(events: Vec<Event>) -> (Self, mpsc::Receiver<Command>) {
        let (tx, rx) = mpsc::channel(8);
        (Self { tx, events }, rx)
    }

    pub fn events(&self) -> &[Event] {
        &self.events
    }

    /// Plays the sound if `event` is one of `TS_EVENTS`.
    pub fn notify(&self, event: Event) {
        if self.events.contains(&event) {
            self.play();
        }
    }

    pub fn play(&self) -> bool {
        self.tx.try_send(Command::Play).is_ok()
    }

    pub async fn quit(&self) {
        let _ = self.tx.send(Command::Quit).await;
    }
}

/// Loads the configured sound (or the bundled one) as Opus packets.
pub fn load_sound(file: Option<&Path>) -> Result<Vec<Vec<u8>>> {
    match file {
        Some(path) => {
            let bytes = std::fs::read(path)
                .with_context(|| format!("cannot read TS_SOUND_FILE {}", path.display()))?;
            opus_packets(&bytes)
                .with_context(|| format!("{} is not an Ogg Opus file", path.display()))
        }
        None => opus_packets(DEFAULT_SOUND),
    }
}

/// Keeps a connection open, reconnecting with backoff, until told to quit.
pub async fn run(ctx: Arc<Ctx>, mut rx: mpsc::Receiver<Command>, sound: Vec<Vec<u8>>) {
    let mut delay = Duration::from_secs(5);
    loop {
        match session(&ctx, &mut rx, &sound, &mut delay).await {
            Ok(()) => return,
            Err(e) => warn!("TeamSpeak: {e:#}, reconnecting in {}s", delay.as_secs()),
        }
        let quit = tokio::select! {
            _ = sleep(delay) => false,
            cmd = rx.recv() => matches!(cmd, None | Some(Command::Quit)),
        };
        if quit {
            return;
        }
        delay = (delay * 2).min(MAX_RECONNECT_DELAY);
    }
}

async fn session(
    ctx: &Ctx,
    rx: &mut mpsc::Receiver<Command>,
    sound: &[Vec<u8>],
    delay: &mut Duration,
) -> Result<()> {
    let config = ctx
        .config
        .teamspeak
        .as_ref()
        .context("TeamSpeak is not configured")?;
    let mut options = Connection::build(config.address.clone())
        .name(config.nickname.clone())
        .identity(identity(ctx).await?);
    if let Some(password) = &config.server_password {
        options = options.password(password.clone());
    }
    let mut con = options.connect()?;

    // The channel list is complete once the server says so.
    loop {
        let item = con.events().next().await;
        match item {
            Some(Ok(StreamItem::MessageEvent(InMessage::ChannelListFinished(_)))) => break,
            Some(Ok(StreamItem::IdentityLevelIncreasing(level))) => {
                info!("TeamSpeak: server wants identity level {level}, computing it");
            }
            Some(Ok(StreamItem::IdentityLevelIncreased)) => {
                if let Some(identity) = con.get_options().get_identity().cloned() {
                    ctx.store.update(|s| s.ts_identity = Some(identity)).await;
                }
            }
            Some(Ok(_)) => {}
            Some(Err(e)) => return Err(e).context("cannot connect"),
            None => bail!("connection closed while connecting"),
        }
    }
    *delay = Duration::from_secs(5);
    if let Some(channel) = &config.channel
        && let Err(e) = join_channel(&mut con, channel, config.channel_password.as_deref()).await
    {
        warn!("TeamSpeak: {e:#}, staying in the default channel");
    }
    log_connected(&con);

    let mut queue: VecDeque<&[u8]> = VecDeque::new();
    let mut next_packet = Instant::now();
    let mut packet_id: u16 = 0;
    loop {
        let events = con.events().try_for_each(|_| future::ready(Ok(())));
        let packet_due = async {
            if queue.is_empty() {
                future::pending::<()>().await;
            }
            sleep_until(next_packet).await;
        };
        tokio::select! {
            r = events => {
                r?;
                bail!("disconnected");
            }
            cmd = rx.recv() => match cmd {
                Some(Command::Play) => {
                    if queue.is_empty() {
                        if !con.can_send_audio() {
                            warn!("TeamSpeak: not allowed to talk in this channel (talk power?), trying anyway");
                        }
                        info!("TeamSpeak: playing sound");
                        queue = sound.iter().map(Vec::as_slice).collect();
                        next_packet = Instant::now();
                    }
                }
                None | Some(Command::Quit) => {
                    con.disconnect(DisconnectOptions::new())?;
                    let drain = con.events().for_each(|_| future::ready(()));
                    let _ = timeout(Duration::from_secs(3), drain).await;
                    info!("TeamSpeak: disconnected");
                    return Ok(());
                }
            },
            _ = packet_due => {
                let packet = queue.pop_front().unwrap_or_default();
                send(&mut con, &mut packet_id, packet)?;
                next_packet += packet_duration(packet);
                if queue.is_empty() {
                    // An empty packet tells clients the sound is over.
                    send(&mut con, &mut packet_id, &[])?;
                }
            }
        }
    }
}

fn send(con: &mut Connection, id: &mut u16, data: &[u8]) -> Result<()> {
    let packet = OutAudio::new(&AudioData::C2S {
        id: *id,
        codec: CodecType::OpusMusic,
        data,
    });
    *id = id.wrapping_add(1);
    con.send_audio(packet)?;
    Ok(())
}

/// Moves the bot into `wanted`, given as a full path (`Parent/Child`) or just
/// the channel's name.
async fn join_channel(con: &mut Connection, wanted: &str, password: Option<&str>) -> Result<()> {
    let state = con.get_state()?;
    let channel = find_channel(state, wanted)?;
    let own = state.own_client;
    if state
        .clients
        .get(&own)
        .is_some_and(|c| c.channel == channel)
    {
        return Ok(());
    }
    let handle = OutClientMoveMessage::new(&mut std::iter::once(OutClientMovePart {
        client_id: own,
        channel_id: channel,
        channel_password: password.map(Into::into),
    }))
    .send_with_result(con)?;

    let result = timeout(Duration::from_secs(10), async {
        loop {
            match con.events().next().await {
                Some(Ok(StreamItem::MessageResult(h, result))) if h == handle => return Ok(result),
                Some(Ok(_)) => {}
                Some(Err(e)) => return Err(anyhow::Error::from(e)),
                None => bail!("disconnected"),
            }
        }
    })
    .await
    .context("no answer to the channel move")??;
    result.map_err(|e| anyhow::anyhow!("cannot join channel {wanted:?}: {e}"))
}

fn find_channel(state: &tsclientlib::data::Connection, wanted: &str) -> Result<ChannelId> {
    let path = |mut id: ChannelId| {
        let mut names = Vec::new();
        while let Some(channel) = state.channels.get(&id) {
            names.push(channel.name.as_str());
            id = channel.parent;
        }
        names.reverse();
        names.join("/")
    };
    let wanted = wanted.trim();
    let mut by_path = state.channels.keys().filter(|&&id| path(id) == wanted);
    if let Some(&id) = by_path.next() {
        return Ok(id);
    }
    let by_name: Vec<_> = state
        .channels
        .values()
        .filter(|c| c.name == wanted)
        .collect();
    match by_name[..] {
        [channel] => Ok(channel.id),
        [] => bail!("channel {wanted:?} not found"),
        _ => bail!(
            "{} channels are named {wanted:?}, use the full path (Parent/Child)",
            by_name.len()
        ),
    }
}

fn log_connected(con: &Connection) {
    let Ok(state) = con.get_state() else { return };
    let channel = state
        .clients
        .get(&state.own_client)
        .and_then(|c| state.channels.get(&c.channel))
        .map_or("?", |c| c.name.as_str());
    info!(
        "Connected to TeamSpeak server {:?}, channel {channel:?}",
        state.server.name
    );
}

/// The stored identity, or a new one (saved so the bot keeps its permissions
/// across restarts).
async fn identity(ctx: &Ctx) -> Result<Identity> {
    if let Some(identity) = ctx.store.read(|s| s.ts_identity.clone()).await {
        return Ok(identity);
    }
    info!("TeamSpeak: creating identity (level {IDENTITY_LEVEL})");
    let identity = tokio::task::spawn_blocking(|| {
        let mut identity = Identity::create();
        identity.upgrade_level(IDENTITY_LEVEL);
        identity
    })
    .await?;
    ctx.store
        .update(|s| s.ts_identity = Some(identity.clone()))
        .await;
    Ok(identity)
}

/// Splits an Ogg Opus file into its Opus packets, skipping the two header
/// packets.
fn opus_packets(bytes: &[u8]) -> Result<Vec<Vec<u8>>> {
    let mut reader = ogg::PacketReader::new(std::io::Cursor::new(bytes));
    let mut packets = Vec::new();
    while let Some(packet) = reader.read_packet()? {
        packets.push(packet.data);
    }
    ensure!(
        packets.first().is_some_and(|p| p.starts_with(b"OpusHead")),
        "missing OpusHead"
    );
    ensure!(packets.len() > 2, "no audio");
    Ok(packets.split_off(2))
}

/// Playback length of an Opus packet, read from its TOC byte (RFC 6716 3.1).
fn packet_duration(packet: &[u8]) -> Duration {
    let Some(&toc) = packet.first() else {
        return Duration::from_millis(20);
    };
    let config = toc >> 3;
    let frame_us: u64 = match config {
        0..=11 => [10_000, 20_000, 40_000, 60_000][usize::from(config % 4)],
        12..=15 => [10_000, 20_000][usize::from(config % 2)],
        _ => [2_500, 5_000, 10_000, 20_000][usize::from(config % 4)],
    };
    let frames = match toc & 3 {
        0 => 1,
        1 | 2 => 2,
        _ => packet.get(1).map_or(1, |b| u64::from(b & 0x3f)),
    };
    Duration::from_micros(frame_us * frames)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_sound_is_valid_opus() {
        let packets = load_sound(None).unwrap();
        let total: Duration = packets.iter().map(|p| packet_duration(p)).sum();
        assert!(total > Duration::from_secs(8) && total <= Duration::from_secs(10));
        assert!(
            packets.iter().all(|p| p.len() < 1200),
            "packets fit a UDP datagram"
        );
    }

    #[test]
    fn reads_packet_durations() {
        // CELT 20 ms, one frame
        assert_eq!(packet_duration(&[31 << 3]), Duration::from_millis(20));
        // SILK 60 ms, two frames
        assert_eq!(packet_duration(&[(3 << 3) | 1]), Duration::from_millis(120));
        // CELT 10 ms, code 3 with 3 frames
        assert_eq!(
            packet_duration(&[(30 << 3) | 3, 3]),
            Duration::from_millis(30)
        );
    }

    #[test]
    fn rejects_non_opus() {
        assert!(opus_packets(b"not an ogg file").is_err());
    }
}
