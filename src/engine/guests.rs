//! Remote guests by link: a guest opens a web page, allows their camera and
//! microphone, and joins. Their picture becomes a video source and their
//! voice an audio input; they hear the show without their own voice
//! (mix-minus, with talkback) and can see the program.
//!
//! The page is served on its own port (guest pages only) and reached
//! through a Cloudflare quick tunnel, so no router setup is needed; the
//! media goes directly between the guest's browser and Cutaway (WebRTC:
//! H.264 and Opus), helped by STUN — or a TURN server, if one is set, for
//! guests behind strict firewalls.
//!
//! The hub lives for the whole app, so links and connected guests survive
//! the engine restarting; the engine lends it its mixes and program while
//! it runs.

use super::audio::{MixKind, MixTaps};
use super::feed::{AudioHub, Feeder};
use super::sources::{Running, SourceSlot};
use super::{LogLevel, Status, VideoTaps, Waker};
use crate::config::{Config, GuestAccess, GuestsConfig, SourceConfig};
use crate::media::{clock, h264, AudioBlock, Ns, VideoCodec, VideoFrame, NS_PER_SEC};
use crate::net::tunnel::{Tunnel, TunnelState};
use anyhow::{anyhow, bail, Context, Result};
use crossbeam_channel::{bounded, RecvTimeoutError};
use rtc::media::io::sample_builder::SampleBuilder;
use rtc::media::Sample;
use rtc::media_stream::MediaStreamTrack;
use rtc::peer_connection::configuration::interceptor_registry::register_default_interceptors;
use rtc::peer_connection::configuration::media_engine::{MediaEngine, MIME_TYPE_H264, MIME_TYPE_OPUS};
use rtc::peer_connection::configuration::RTCConfigurationBuilder;
use rtc::peer_connection::sdp::RTCSessionDescription;
use rtc::peer_connection::transport::RTCIceServer;
use rtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication;
use rtc::rtp::codec::h264::H264Packet;
use rtc::rtp_transceiver::rtp_sender::{RTCPFeedback, RTCRtpCodec, RTCRtpCodecParameters, RTCRtpCodingParameters, RTCRtpEncodingParameters, RtpCodecKind};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};
use webrtc::media_stream::track_local::static_sample::TrackLocalStaticSample;
use webrtc::media_stream::track_local::TrackLocal;
use webrtc::media_stream::Track;
use webrtc::media_stream::track_remote::{TrackRemote, TrackRemoteEvent};
use webrtc::peer_connection::{PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCIceGatheringState, RTCPeerConnectionState};
use webrtc::rtp_transceiver::RtpSender;

const PAGE: &str = include_str!("guest.html");
const STUN: [&str; 2] = ["stun:stun.cloudflare.com:3478", "stun:stun.l.google.com:19302"];
/// The program as guests see it.
const RETURN_SIZE: (u32, u32) = (640, 360);
const RETURN_FPS: u32 = 15;

/// What the window shows about a guest.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GuestInfo {
    /// The link's id.
    pub id: String,
    /// The video source (and audio input) the guest appears as.
    pub source: String,
    pub connected: bool,
    /// Connecting or reconnecting.
    pub connecting: bool,
    /// The name the guest typed.
    pub guest_name: String,
    pub since: Option<Instant>,
}

/// What the engine lends the hub while it runs.
pub struct EngineLink {
    pub mix_taps: MixTaps,
    pub video_taps: Arc<Mutex<VideoTaps>>,
    /// Audio inputs: (device, number).
    pub inputs: Vec<(String, usize)>,
    pub status: Arc<Mutex<Status>>,
    pub waker: Waker,
}

/// A guest source while the engine runs.
struct Slot {
    source: String,
    feeder: Arc<Mutex<Feeder>>,
    /// Ask the browser for a fresh keyframe (a new decoder needs one).
    want_key: Arc<AtomicBool>,
    last_video: Arc<Mutex<Option<Instant>>>,
}

struct Conn {
    gen: u64,
    pc: Arc<dyn PeerConnection>,
    guest_name: String,
    connected: bool,
    since: Option<Instant>,
    video_out: Option<tokio::sync::mpsc::Sender<bytes::Bytes>>,
    /// Stops the return-audio thread.
    stop: Arc<AtomicBool>,
}

struct Server {
    port: u16,
    public: bool,
    stop: Arc<AtomicBool>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[derive(Default)]
struct Inner {
    cfg: GuestsConfig,
    show: String,
    talkback: bool,
    slots: HashMap<String, Slot>,
    conns: HashMap<String, Conn>,
    engine: Option<EngineLink>,
    server: Option<Server>,
    server_problem: Option<String>,
    tunnel: Option<(u16, Tunnel)>,
    program_feed: Option<Arc<AtomicBool>>,
}

pub struct GuestHub {
    rt: tokio::runtime::Runtime,
    inner: Mutex<Inner>,
    waker: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    gens: AtomicU64,
    tap_ids: AtomicU64,
    keyframe: AtomicBool,
    me: Mutex<Weak<GuestHub>>,
}

/// The app's guest hub.
pub fn hub() -> Arc<GuestHub> {
    static HUB: OnceLock<Arc<GuestHub>> = OnceLock::new();
    HUB.get_or_init(|| {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).thread_name("guests").enable_all().build().expect("tokio runtime");
        let h = Arc::new(GuestHub {
            rt,
            inner: Mutex::new(Inner::default()),
            waker: Mutex::new(None),
            gens: AtomicU64::new(1),
            tap_ids: AtomicU64::new(1 << 48),
            keyframe: AtomicBool::new(false),
            me: Mutex::new(Weak::new()),
        });
        *h.me.lock().unwrap() = Arc::downgrade(&h);
        h
    })
    .clone()
}

/// A new link id: 12 random letters and digits.
pub fn new_id() -> String {
    let mut b = [0u8; 12];
    let _ = getrandom::fill(&mut b);
    const A: &[u8] = b"abcdefghijkmnpqrstuvwxyz23456789";
    b.iter().map(|x| A[(*x as usize) % A.len()] as char).collect()
}

fn rand_u32() -> u32 {
    let mut b = [0u8; 4];
    let _ = getrandom::fill(&mut b);
    u32::from_le_bytes(b)
}

impl GuestHub {
    fn arc(&self) -> Arc<GuestHub> {
        self.me.lock().unwrap().upgrade().expect("hub")
    }

    fn wake(&self) {
        if let Some(w) = self.waker.lock().unwrap().as_ref() {
            w();
        }
        let e = self.inner.lock().unwrap();
        if let Some(l) = &e.engine {
            (l.waker)();
        }
    }

    fn log(&self, level: LogLevel, text: String) {
        let e = self.inner.lock().unwrap();
        if let Some(l) = &e.engine {
            l.status.lock().unwrap().push(level, text);
        } else {
            eprintln!("{text}");
        }
    }

    /// New settings: start, move or stop the guest page and the tunnel.
    pub fn apply(&self, cfg: &Config, waker: Arc<dyn Fn() + Send + Sync>) {
        *self.waker.lock().unwrap() = Some(waker.clone());
        let g = cfg.guests.clone();
        let mut inner = self.inner.lock().unwrap();
        inner.show = [cfg.podcast.show.trim(), cfg.ui.profile.trim()].into_iter().find(|s| !s.is_empty()).unwrap_or("the show").to_string();
        inner.talkback = cfg.mix.talkback_to_guests;
        let public = g.access == GuestAccess::Custom;
        let want_server = g.enabled;
        let same = inner.server.as_ref().is_some_and(|s| s.port == g.port && s.public == public);
        if !want_server {
            inner.server = None;
            inner.server_problem = None;
        } else if !same {
            inner.server = None;
            match self.start_server(g.port, public) {
                Ok(s) => {
                    inner.server = Some(s);
                    inner.server_problem = None;
                }
                Err(e) => inner.server_problem = Some(format!("{e:#}")),
            }
        }
        let want_tunnel = g.enabled && g.access == GuestAccess::Tunnel && inner.server.is_some();
        if !want_tunnel {
            inner.tunnel = None;
        } else if inner.tunnel.as_ref().is_none_or(|(p, _)| *p != g.port) {
            inner.tunnel = Some((g.port, Tunnel::start(g.port, waker)));
        }
        if !g.enabled {
            // Everyone leaves.
            let conns: Vec<Conn> = inner.conns.drain().map(|(_, c)| c).collect();
            drop(inner);
            for c in conns {
                self.close_conn(c);
            }
            self.inner.lock().unwrap().cfg = g;
            return;
        }
        inner.cfg = g;
    }

    /// The guest page's public address, once there is one.
    pub fn base_url(&self) -> Option<String> {
        let inner = self.inner.lock().unwrap();
        inner.server.as_ref()?;
        match inner.cfg.access {
            GuestAccess::Tunnel => inner.tunnel.as_ref().and_then(|(_, t)| t.address()),
            GuestAccess::Custom => Some(inner.cfg.public_url.trim().trim_end_matches('/').to_string()).filter(|u| u.starts_with("https://") || u.starts_with("http://")),
        }
    }

    pub fn link(&self, id: &str) -> Option<String> {
        self.base_url().map(|b| format!("{b}/g/{id}"))
    }

    /// The tunnel (or why guests can't reach the page).
    pub fn reach(&self) -> TunnelState {
        let inner = self.inner.lock().unwrap();
        if !inner.cfg.enabled {
            return TunnelState::Off;
        }
        if let Some(p) = &inner.server_problem {
            return TunnelState::Failed(format!("the guest page couldn't start: {p}"));
        }
        match inner.cfg.access {
            GuestAccess::Tunnel => inner.tunnel.as_ref().map(|(_, t)| t.state.lock().unwrap().clone()).unwrap_or_default(),
            GuestAccess::Custom => {
                if inner.cfg.public_url.trim().starts_with("https://") {
                    TunnelState::Up(inner.cfg.public_url.trim().trim_end_matches('/').to_string())
                } else {
                    TunnelState::Failed("enter your https:// address".into())
                }
            }
        }
    }

    /// Every guest source, connected or not.
    pub fn guests(&self) -> Vec<GuestInfo> {
        let inner = self.inner.lock().unwrap();
        let mut v: Vec<GuestInfo> = inner
            .slots
            .iter()
            .map(|(id, s)| {
                let c = inner.conns.get(id);
                GuestInfo {
                    id: id.clone(),
                    source: s.source.clone(),
                    connected: c.is_some_and(|c| c.connected),
                    connecting: c.is_some_and(|c| !c.connected),
                    guest_name: c.map(|c| c.guest_name.clone()).unwrap_or_default(),
                    since: c.and_then(|c| c.since),
                }
            })
            .collect();
        v.sort_by(|a, b| a.source.cmp(&b.source));
        v
    }

    /// Disconnect a guest (or they left).
    pub fn kick(&self, id: &str) {
        let mut inner = self.inner.lock().unwrap();
        let source = inner.slots.get(id).map(|s| s.source.clone()).unwrap_or_default();
        let c = inner.conns.remove(id);
        drop(inner);
        if let Some(c) = c {
            let who = if c.guest_name.is_empty() { "The guest".to_string() } else { c.guest_name.clone() };
            let was_on = c.connected;
            self.close_conn(c);
            if was_on {
                self.log(LogLevel::Info, format!("{who} left ({source})"));
            }
            self.wake();
        }
    }

    fn close_conn(&self, c: Conn) {
        c.stop.store(true, Ordering::Relaxed);
        let pc = c.pc.clone();
        self.rt.spawn(async move {
            let _ = pc.close().await;
        });
    }

    pub fn attach_engine(&self, link: EngineLink) {
        let mut inner = self.inner.lock().unwrap();
        inner.engine = Some(link);
        let feeding = inner.conns.values().any(|c| c.video_out.is_some());
        drop(inner);
        if feeding {
            self.ensure_program_feed();
        }
    }

    pub fn detach_engine(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.engine = None;
        if let Some(stop) = inner.program_feed.take() {
            stop.store(true, Ordering::Relaxed);
        }
    }

    // ---- the guest page ----------------------------------------------------------------

    fn start_server(&self, port: u16, public: bool) -> Result<Server> {
        let addr = if public { format!("0.0.0.0:{port}") } else { format!("127.0.0.1:{port}") };
        let listener = TcpListener::bind(&addr).with_context(|| format!("port {port} is in use (change it in Settings → Remote guests)"))?;
        listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let s2 = stop.clone();
        let hub = self.arc();
        std::thread::Builder::new().name("guest page".into()).spawn(move || {
            while !s2.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((conn, _)) => {
                        let hub = hub.clone();
                        let _ = std::thread::Builder::new().name("guest request".into()).spawn(move || {
                            let _ = conn.set_nonblocking(false);
                            if let Err(e) = handle(conn, &hub) {
                                eprintln!("guest page: {e}");
                            }
                        });
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(100)),
                    Err(_) => std::thread::sleep(Duration::from_millis(500)),
                }
            }
        })?;
        Ok(Server { port, public, stop })
    }

    fn ice_servers(&self) -> Vec<RTCIceServer> {
        let cfg = self.inner.lock().unwrap().cfg.clone();
        let mut v = vec![RTCIceServer { urls: STUN.iter().map(|s| s.to_string()).collect(), ..Default::default() }];
        if !cfg.turn_url.trim().is_empty() {
            v.push(RTCIceServer { urls: cfg.turn_url.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(), username: cfg.turn_username.clone(), credential: cfg.turn_password.clone() });
        }
        v
    }

    fn info_json(&self, id: &str) -> Option<String> {
        let inner = self.inner.lock().unwrap();
        if !inner.cfg.enabled {
            return None;
        }
        let slot = inner.slots.get(id)?;
        let (show, rv) = (inner.show.clone(), inner.cfg.return_video);
        let source = slot.source.clone();
        drop(inner);
        let ice: Vec<serde_json::Value> = self
            .ice_servers()
            .into_iter()
            .map(|s| if s.username.is_empty() { serde_json::json!({ "urls": s.urls }) } else { serde_json::json!({ "urls": s.urls, "username": s.username, "credential": s.credential }) })
            .collect();
        Some(serde_json::json!({ "show": show, "slot": source, "returnVideo": rv, "iceServers": ice }).to_string())
    }

    // ---- connecting --------------------------------------------------------------------

    /// A guest's offer in, Cutaway's answer out.
    async fn negotiate(self: Arc<Self>, id: String, offer: String, guest_name: String) -> Result<String> {
        let return_video = {
            let inner = self.inner.lock().unwrap();
            if !inner.cfg.enabled || !inner.slots.contains_key(&id) {
                bail!("this link isn't active");
            }
            inner.cfg.return_video
        };
        // A guest joining again (reloading the page) replaces their old connection.
        let old = self.inner.lock().unwrap().conns.remove(&id);
        if let Some(c) = old {
            self.close_conn(c);
        }
        let gen = self.gens.fetch_add(1, Ordering::Relaxed);

        let mut me = MediaEngine::default();
        let opus = RTCRtpCodec { mime_type: MIME_TYPE_OPUS.to_owned(), clock_rate: 48_000, channels: 2, sdp_fmtp_line: "minptime=10;useinbandfec=1".into(), rtcp_feedback: vec![] };
        me.register_codec(RTCRtpCodecParameters { rtp_codec: opus.clone(), payload_type: 111 }, RtpCodecKind::Audio)?;
        let fb = || vec![RTCPFeedback { typ: "nack".into(), parameter: String::new() }, RTCPFeedback { typ: "nack".into(), parameter: "pli".into() }, RTCPFeedback { typ: "ccm".into(), parameter: "fir".into() }];
        let h264 = |profile: &str| RTCRtpCodec {
            mime_type: MIME_TYPE_H264.to_owned(),
            clock_rate: 90_000,
            channels: 0,
            sdp_fmtp_line: format!("level-asymmetry-allowed=1;packetization-mode=1;profile-level-id={profile}"),
            rtcp_feedback: fb(),
        };
        for (pt, profile) in [(102u8, "42e01f"), (104, "42001f"), (106, "4d001f"), (108, "640c1f"), (112, "640032")] {
            me.register_codec(RTCRtpCodecParameters { rtp_codec: h264(profile), payload_type: pt }, RtpCodecKind::Video)?;
        }
        let registry = register_default_interceptors(rtc::interceptor::Registry::new(), &mut me)?;
        let config = RTCConfigurationBuilder::new().with_ice_servers(self.ice_servers()).build();
        let (gathered_tx, mut gathered_rx) = webrtc::runtime::channel::<()>(1);
        let handler = Arc::new(Handler { hub: Arc::downgrade(&self), id: id.clone(), gen, gathered: gathered_tx });
        let runtime = webrtc::runtime::default_runtime().ok_or_else(|| anyhow!("no async runtime"))?;
        let pc: Arc<dyn PeerConnection> = Arc::new(
            PeerConnectionBuilder::new()
                .with_configuration(config)
                .with_media_engine(me)
                .with_interceptor_registry(registry)
                .with_handler(handler)
                .with_runtime(runtime)
                .with_udp_addrs(vec!["0.0.0.0:0"])
                .build()
                .await?,
        );
        pc.set_remote_description(RTCSessionDescription::offer(offer)?).await.map_err(|e| anyhow!("the browser's offer wasn't usable: {e}"))?;

        // What the guest hears and sees.
        let track = |kind: RtpCodecKind, codec: RTCRtpCodec, name: &str| -> Result<Arc<TrackLocalStaticSample>> {
            Ok(Arc::new(TrackLocalStaticSample::new(
                Instant::now(),
                MediaStreamTrack::new(
                    "cutaway".to_owned(),
                    format!("cutaway-{name}"),
                    format!("Cutaway {name}"),
                    kind,
                    vec![RTCRtpEncodingParameters { rtp_coding_parameters: RTCRtpCodingParameters { ssrc: Some(rand_u32()), ..Default::default() }, codec, ..Default::default() }],
                ),
            )?))
        };
        let audio = track(RtpCodecKind::Audio, opus, "audio")?;
        let audio_sender = pc.add_track(audio.clone() as Arc<dyn TrackLocal>).await?;
        // (A browser without H.264 gets no picture back, but can still join.)
        let video = if return_video {
            let v = track(RtpCodecKind::Video, h264("42e01f"), "program")?;
            match pc.add_track(v.clone() as Arc<dyn TrackLocal>).await {
                Ok(s) => Some((v, s)),
                Err(e) => {
                    eprintln!("guest {id}: no program picture for them ({e})");
                    None
                }
            }
        } else {
            None
        };

        let answer = pc.create_answer(None).await?;
        pc.set_local_description(answer).await?;
        // Every way to reach Cutaway, in the one answer.
        let _ = tokio::time::timeout(Duration::from_secs(4), gathered_rx.recv()).await;
        let sdp = pc.local_description().await.ok_or_else(|| anyhow!("no answer was made"))?.sdp;

        // Senders for the return feeds.
        let (atx, arx) = tokio::sync::mpsc::channel::<bytes::Bytes>(100);
        self.rt.spawn(write_samples(audio, audio_sender, arx, Duration::from_millis(20)));
        let video_out = video.map(|(track, sender)| {
            let (vtx, vrx) = tokio::sync::mpsc::channel::<bytes::Bytes>(30);
            self.rt.spawn(write_samples(track, sender, vrx, Duration::from_nanos((NS_PER_SEC / RETURN_FPS as i64) as u64)));
            vtx
        });
        let stop = Arc::new(AtomicBool::new(false));
        {
            let hub = self.clone();
            let (id, stop) = (id.clone(), stop.clone());
            std::thread::Builder::new().name("guest return".into()).spawn(move || return_audio(hub, id, atx, stop))?;
        }
        let name = guest_name.trim().chars().take(40).collect::<String>();
        self.inner.lock().unwrap().conns.insert(id, Conn { gen, pc, guest_name: name, connected: false, since: None, video_out, stop });
        if return_video {
            self.ensure_program_feed();
        }
        self.wake();
        Ok(sdp)
    }

    /// The program, encoded once for every guest who sees it.
    fn ensure_program_feed(&self) {
        let mut inner = self.inner.lock().unwrap();
        if inner.program_feed.is_some() || inner.engine.is_none() {
            return;
        }
        let stop = Arc::new(AtomicBool::new(false));
        inner.program_feed = Some(stop.clone());
        drop(inner);
        let hub = self.arc();
        let _ = std::thread::Builder::new().name("guest program".into()).spawn(move || {
            if let Err(e) = program_feed(&hub, &stop) {
                hub.log(LogLevel::Warning, format!("Guests can't see the program: {e:#}"));
            }
            let mut inner = hub.inner.lock().unwrap();
            if inner.program_feed.as_ref().is_some_and(|s| Arc::ptr_eq(s, &stop)) {
                inner.program_feed = None;
            }
        });
    }

    fn connection_changed(&self, id: &str, gen: u64, state: RTCPeerConnectionState) {
        let mut inner = self.inner.lock().unwrap();
        let source = inner.slots.get(id).map(|s| s.source.clone()).unwrap_or_default();
        let Some(c) = inner.conns.get_mut(id).filter(|c| c.gen == gen) else { return };
        match state {
            RTCPeerConnectionState::Connected => {
                let first = !c.connected;
                c.connected = true;
                c.since.get_or_insert_with(Instant::now);
                let who = if c.guest_name.is_empty() { "A guest".to_string() } else { c.guest_name.clone() };
                if let Some(s) = inner.slots.get(id) {
                    s.want_key.store(true, Ordering::Relaxed);
                }
                drop(inner);
                self.keyframe.store(true, Ordering::Relaxed);
                if first {
                    self.log(LogLevel::Info, format!("{who} joined as {source}"));
                }
            }
            RTCPeerConnectionState::Disconnected => {
                c.connected = false;
            }
            RTCPeerConnectionState::Failed | RTCPeerConnectionState::Closed => {
                let c = inner.conns.remove(id).unwrap();
                let was = c.connected || c.since.is_some();
                let who = if c.guest_name.is_empty() { "The guest".to_string() } else { c.guest_name.clone() };
                drop(inner);
                self.close_conn(c);
                if was {
                    self.log(LogLevel::Info, format!("{who} left ({source})"));
                } else if state == RTCPeerConnectionState::Failed {
                    self.log(LogLevel::Warning, format!("{who} couldn't connect to {source}: their network blocks direct connections. A TURN server (Settings → Remote guests) gets round that."));
                }
            }
            _ => {}
        }
        self.wake();
    }

    /// Run `f` on the guest source's decoder, if the engine has it.
    fn with_slot<T>(&self, id: &str, f: impl FnOnce(&Slot) -> T) -> Option<T> {
        let inner = self.inner.lock().unwrap();
        inner.slots.get(id).map(f)
    }

    /// Tap the engine's mix for a guest: everything but their own voice.
    fn feed_tap(&self, id: &str) -> Option<(u64, crossbeam_channel::Receiver<AudioBlock>, MixTaps)> {
        let inner = self.inner.lock().unwrap();
        let engine = inner.engine.as_ref()?;
        let source = inner.slots.get(id)?.source.to_lowercase();
        let own = engine.inputs.iter().find(|(d, _)| d.trim().to_lowercase().strip_prefix("source:").is_some_and(|n| n.trim() == source)).map(|(_, i)| *i);
        let minus = own.filter(|i| *i < 64).map(|i| 1u64 << i).unwrap_or(0);
        let (tx, rx) = bounded(200);
        let tap = self.tap_ids.fetch_add(1, Ordering::Relaxed);
        engine.mix_taps.lock().unwrap().push((tap, MixKind::Feed { stream: false, minus, talkback: inner.talkback }, tx));
        Some((tap, rx, engine.mix_taps.clone()))
    }
}

/// Write samples to a track in order, as they come.
async fn write_samples(track: Arc<TrackLocalStaticSample>, sender: Arc<dyn RtpSender>, mut rx: tokio::sync::mpsc::Receiver<bytes::Bytes>, duration: Duration) {
    let Some(&ssrc) = track.ssrcs().await.first() else { return };
    let mut pt = None;
    while let Some(data) = rx.recv().await {
        if pt.is_none() {
            pt = sender.get_parameters().await.ok().and_then(|p| p.rtp_parameters.codecs.first().map(|c| c.payload_type));
        }
        let Some(pt) = pt else { continue };
        let sample = Sample { data, duration, ..Sample::new(Instant::now()) };
        if track.sample_writer(ssrc, pt).write_sample(&sample).await.is_err() && rx.is_closed() {
            break;
        }
    }
}

/// The guest's ears: the mix without them, as Opus.
fn return_audio(hub: Arc<GuestHub>, id: String, tx: tokio::sync::mpsc::Sender<bytes::Bytes>, stop: Arc<AtomicBool>) {
    let Ok(mut enc) = super::opus::Encoder::new(64_000) else { return };
    while !stop.load(Ordering::Relaxed) {
        // (Again after the engine restarts.)
        let Some((tap, rx, taps)) = hub.feed_tap(&id) else {
            std::thread::sleep(Duration::from_millis(300));
            continue;
        };
        loop {
            match rx.recv_timeout(Duration::from_millis(300)) {
                Ok(block) => {
                    for p in enc.push(&block.samples) {
                        let _ = tx.try_send(bytes::Bytes::from(p));
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            if stop.load(Ordering::Relaxed) || tx.is_closed() {
                break;
            }
        }
        taps.lock().unwrap().retain(|t| t.0 != tap);
        if tx.is_closed() {
            break;
        }
    }
}

/// An encoded picture as H.264 Annex B, with its parameter sets on keyframes.
fn annexb(p: &crate::media::VideoPacket) -> Vec<u8> {
    let mut nals: Vec<&[u8]> = Vec::new();
    if p.keyframe {
        nals.extend(p.config.parameter_sets.iter().map(|x| x.as_slice()));
    }
    nals.extend(h264::split_avcc(&p.data));
    h264::to_annexb(nals)
}

/// Encode the program small for guests to watch.
fn program_feed(hub: &Arc<GuestHub>, stop: &AtomicBool) -> Result<()> {
    let backend = super::encode::resolve_backend(VideoCodec::H264, "auto")?;
    let settings = super::encode::VideoSettings {
        codec: VideoCodec::H264,
        width: RETURN_SIZE.0,
        height: RETURN_SIZE.1,
        fps: RETURN_FPS,
        bitrate_kbps: 800,
        purpose: super::encode::Purpose::Stream,
        backend: backend.into(),
    };
    let (ptx, prx) = crossbeam_channel::unbounded();
    let mut enc = super::encode::create_video(&settings, Arc::new(move |p| {
        let _ = ptx.send(p);
    }))?;
    let (ftx, frx) = bounded::<VideoFrame>(2);
    let tap = hub.tap_ids.fetch_add(1, Ordering::Relaxed);
    let taps = {
        let inner = hub.inner.lock().unwrap();
        let Some(e) = inner.engine.as_ref() else { return Ok(()) };
        e.video_taps.lock().unwrap().listeners.push((tap, ftx));
        e.video_taps.clone()
    };
    let step = NS_PER_SEC / RETURN_FPS as Ns;
    let mut next: Option<Ns> = None;
    let mut t0 = None;
    let mut idle_since = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        let watching = hub.inner.lock().unwrap().conns.values().any(|c| c.video_out.is_some());
        if watching {
            idle_since = Instant::now();
        } else if idle_since.elapsed() > Duration::from_secs(5) {
            break;
        }
        let f = match frx.recv_timeout(Duration::from_millis(300)) {
            Ok(f) => f,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        if next.is_some_and(|n| f.pts < n) || !watching {
            continue;
        }
        // Every other frame or so; catch up rather than burst after a gap.
        next = Some(match next {
            Some(n) if f.pts - n < step => n + step,
            _ => f.pts + step,
        });
        let start = *t0.get_or_insert(f.pts);
        let small = f.scaled(RETURN_SIZE.0, RETURN_SIZE.1);
        let key = hub.keyframe.swap(false, Ordering::Relaxed);
        enc.encode(&small, f.pts - start, key)?;
        while let Ok(p) = prx.try_recv() {
            let data = bytes::Bytes::from(annexb(&p));
            let inner = hub.inner.lock().unwrap();
            for c in inner.conns.values().filter(|c| c.connected) {
                if let Some(v) = &c.video_out {
                    let _ = v.try_send(data.clone());
                }
            }
        }
    }
    taps.lock().unwrap().listeners.retain(|l| l.0 != tap);
    Ok(())
}

// ---- the browser's media coming in ----------------------------------------------------

struct Handler {
    hub: Weak<GuestHub>,
    id: String,
    gen: u64,
    gathered: webrtc::runtime::Sender<()>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for Handler {
    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if state == RTCIceGatheringState::Complete {
            let _ = self.gathered.try_send(());
        }
    }

    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        if let Some(h) = self.hub.upgrade() {
            h.connection_changed(&self.id, self.gen, state);
        }
    }

    async fn on_track(&self, track: Arc<dyn TrackRemote>) {
        let Some(hub) = self.hub.upgrade() else { return };
        let id = self.id.clone();
        match track.kind().await {
            RtpCodecKind::Video => {
                hub.rt.spawn(receive_video(hub.clone(), id, track));
            }
            _ => {
                hub.rt.spawn(receive_audio(hub.clone(), id, track));
            }
        }
    }
}

/// RTP timestamps on the host clock: anchored to the first packet's
/// arrival, unwrapped as they go round.
struct RtpClock {
    rate: i64,
    first: Option<(u32, Ns)>,
    last: u32,
    turns: i64,
}

impl RtpClock {
    fn new(rate: u32) -> Self {
        RtpClock { rate: rate as i64, first: None, last: 0, turns: 0 }
    }

    fn host(&mut self, ts: u32) -> Ns {
        let (t0, at) = *self.first.get_or_insert((ts, clock::now()));
        if self.last > 0xC000_0000 && ts < 0x4000_0000 {
            self.turns += 1;
        } else if ts > 0xC000_0000 && self.last < 0x4000_0000 && self.turns > 0 {
            // A late packet from before the wrap.
            return at + ((((self.turns - 1) << 32) + ts as i64 - t0 as i64) * NS_PER_SEC / self.rate);
        }
        self.last = ts;
        at + (((self.turns << 32) + ts as i64 - t0 as i64) * NS_PER_SEC / self.rate)
    }
}

async fn receive_video(hub: Arc<GuestHub>, id: String, track: Arc<dyn TrackRemote>) {
    let ssrc = track.ssrcs().await.first().copied().unwrap_or(0);
    let mut depacketizer = H264Packet::default();
    depacketizer.is_avc = true;
    let mut builder = SampleBuilder::new(512, depacketizer, 90_000).with_max_time_delay(Duration::from_millis(500));
    let mut clock = RtpClock::new(90_000);
    let mut got_key = false;
    let mut last_pli: Option<Instant> = None;
    while let Some(ev) = track.poll().await {
        let TrackRemoteEvent::OnRtpPacket(packet) = ev else { continue };
        // Ask for a keyframe at the start, and when a new decoder needs one.
        let want = hub.with_slot(&id, |s| s.want_key.swap(false, Ordering::Relaxed)).unwrap_or(false);
        if want {
            got_key = false;
        }
        if want || (!got_key && last_pli.is_none_or(|t| t.elapsed() > Duration::from_millis(800))) {
            last_pli = Some(Instant::now());
            let _ = track.write_rtcp(vec![Box::new(PictureLossIndication { sender_ssrc: 0, media_ssrc: ssrc })]).await;
        }
        builder.push(Instant::now(), packet);
        while let Some(sample) = builder.pop(Instant::now()) {
            let pts = clock.host(sample.packet_timestamp);
            let nals = h264::split_avcc(&sample.data);
            let key = nals.iter().any(|n| h264::h264_nal_type(n) == 5);
            let ps: Vec<Vec<u8>> = nals.iter().filter(|n| matches!(h264::h264_nal_type(n), 7 | 8)).map(|n| n.to_vec()).collect();
            let data = h264::to_avcc(nals.iter().copied().filter(|n| h264::h264_nal_type(n) != h264::NAL_AUD));
            got_key |= key;
            hub.with_slot(&id, |s| {
                let mut f = s.feeder.lock().unwrap();
                // (A no-op unless they changed or the decoder is new.)
                if ps.len() >= 2 {
                    f.video_config(VideoCodec::H264, ps.clone());
                }
                f.video(VideoCodec::H264, pts, pts, key, &data);
                *s.last_video.lock().unwrap() = Some(Instant::now());
            });
        }
    }
}

async fn receive_audio(hub: Arc<GuestHub>, id: String, track: Arc<dyn TrackRemote>) {
    let Ok(mut dec) = super::opus::Decoder::new() else { return };
    let mut clock = RtpClock::new(48_000);
    let mut mix = None;
    let mut last_seq: Option<u16> = None;
    while let Some(ev) = track.poll().await {
        let TrackRemoteEvent::OnRtpPacket(packet) = ev else { continue };
        let seq = packet.header.sequence_number;
        // A gap: let the decoder fill it in.
        if let Some(prev) = last_seq {
            let missing = seq.wrapping_sub(prev).wrapping_sub(1);
            if missing > 0 && missing < 5 {
                for _ in 0..missing {
                    let _ = dec.decode(None);
                }
            } else if missing >= 0x8000 {
                continue; // old, already played
            }
        }
        last_seq = Some(seq);
        let pts = clock.host(packet.header.timestamp);
        let pcm = dec.decode(Some(&packet.payload));
        if pcm.is_empty() {
            continue;
        }
        hub.with_slot(&id, |s| s.feeder.lock().unwrap().pcm(pts, 48_000, 2, &pcm, &mut mix));
    }
}

// ---- the guest page's requests ------------------------------------------------------------

fn respond(conn: &mut TcpStream, status: &str, ctype: &str, body: &[u8]) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\n\r\n",
        body.len()
    );
    conn.write_all(head.as_bytes())?;
    conn.write_all(body)
}

fn handle(mut conn: TcpStream, hub: &Arc<GuestHub>) -> std::io::Result<()> {
    conn.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut buf = Vec::with_capacity(8192);
    let mut chunk = [0u8; 8192];
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        if buf.len() > 32 * 1024 {
            return respond(&mut conn, "431 Too Large", "text/plain", b"too large");
        }
        let n = conn.read(&mut chunk)?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.lines();
    let mut first = lines.next().unwrap_or("").split_whitespace();
    let method = first.next().unwrap_or("GET").to_string();
    let target = first.next().unwrap_or("/").to_string();
    let path = target.split('?').next().unwrap_or("/");
    let length: usize = lines.find_map(|l| l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap_or(0))).unwrap_or(0);
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    if parts.first() != Some(&"g") || parts.len() < 2 || !parts[1].chars().all(|c| c.is_ascii_alphanumeric()) {
        return respond(&mut conn, "404 Not Found", "text/plain", b"not found");
    }
    let id = parts[1].to_string();
    match (method.as_str(), parts.get(2).copied()) {
        ("GET", None) => respond(&mut conn, "200 OK", "text/html; charset=utf-8", PAGE.as_bytes()),
        ("GET", Some("info")) => match hub.info_json(&id) {
            Some(j) => respond(&mut conn, "200 OK", "application/json", j.as_bytes()),
            None => respond(&mut conn, "404 Not Found", "text/plain", b"this link isn't active"),
        },
        ("POST", Some("offer")) => {
            if length > 256 * 1024 {
                return respond(&mut conn, "413 Too Large", "text/plain", b"too large");
            }
            let mut body = buf[head_end..].to_vec();
            while body.len() < length {
                let n = conn.read(&mut chunk)?;
                if n == 0 {
                    break;
                }
                body.extend_from_slice(&chunk[..n]);
            }
            let v: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
            let sdp = v["sdp"].as_str().unwrap_or("").to_string();
            let name = v["name"].as_str().unwrap_or("").to_string();
            if sdp.is_empty() {
                return respond(&mut conn, "400 Bad Request", "application/json", br#"{"error":"no offer"}"#);
            }
            let h = hub.clone();
            let r = hub.rt.block_on(async move { tokio::time::timeout(Duration::from_secs(15), h.negotiate(id, sdp, name)).await });
            match r {
                Ok(Ok(answer)) => respond(&mut conn, "200 OK", "application/json", serde_json::json!({ "sdp": answer }).to_string().as_bytes()),
                Ok(Err(e)) => respond(&mut conn, "409 Conflict", "application/json", serde_json::json!({ "error": format!("{e:#}") }).to_string().as_bytes()),
                Err(_) => respond(&mut conn, "504 Timeout", "application/json", br#"{"error":"the show took too long to answer"}"#),
            }
        }
        ("POST", Some("leave")) => {
            hub.kick(&id);
            respond(&mut conn, "204 No Content", "text/plain", b"")
        }
        _ => respond(&mut conn, "404 Not Found", "text/plain", b"not found"),
    }
}

// ---- the guest as a video source ------------------------------------------------------------

struct GuestRunning {
    id: String,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for GuestRunning {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        let h = hub();
        h.inner.lock().unwrap().slots.remove(&self.id);
        h.wake();
    }
}

/// The picture while nobody's there.
fn slate(cfg: &SourceConfig, line: &str) -> VideoFrame {
    use super::text::{draw_text, fonts, text_width};
    let (w, h) = (cfg.width.max(160), cfg.height.max(90));
    let mut img = image::RgbaImage::from_pixel(w, h, image::Rgba([18, 22, 30, 255]));
    let (bold, regular) = fonts(std::path::Path::new(""));
    let big = h as f32 * 0.09;
    let small = h as f32 * 0.05;
    let tw = text_width(&bold, big, &cfg.name);
    draw_text(&mut img, &bold, big, (w as f32 - tw) / 2.0, h as f32 * 0.47, &cfg.name, [235, 238, 245]);
    let lw = text_width(&regular, small, line);
    draw_text(&mut img, &regular, small, (w as f32 - lw) / 2.0, h as f32 * 0.60, line, [140, 147, 160]);
    super::player::image_frame(&img, (w, h), clock::now())
}

/// Start a guest source ("guest:ID").
pub fn start_source(cfg: &SourceConfig, slot: Arc<SourceSlot>, audio: Arc<AudioHub>) -> Result<Box<dyn Running>> {
    let id = cfg.guest_id().ok_or_else(|| anyhow!("{}: the guest link id is missing", cfg.name))?.to_string();
    let stop = Arc::new(AtomicBool::new(false));
    let latency = if cfg.network_latency_ms == 0 { 250 } else { cfg.network_latency_ms };
    let feeder = Feeder::new(&cfg.name, slot.clone(), audio, (cfg.width, cfg.height), latency, stop.clone());
    let want_key = Arc::new(AtomicBool::new(true));
    let last_video = Arc::new(Mutex::new(None));
    let h = hub();
    h.inner.lock().unwrap().slots.insert(id.clone(), Slot { source: cfg.name.clone(), feeder: Arc::new(Mutex::new(feeder)), want_key, last_video: last_video.clone() });
    h.wake();
    // The slate, until (and whenever) no pictures come.
    let (c2, s2, id2) = (cfg.clone(), stop.clone(), id.clone());
    let thread = std::thread::Builder::new().name(format!("guest {}", cfg.name)).spawn(move || {
        let mut shown: Option<(String, VideoFrame)> = None;
        while !s2.load(Ordering::Relaxed) {
            let fresh = last_video.lock().unwrap().is_some_and(|t| t.elapsed() < Duration::from_secs(2));
            if !fresh {
                let h = hub();
                let state = h.guests().into_iter().find(|g| g.id == id2);
                let line = match state {
                    Some(g) if g.connected => format!("{} — waiting for their camera", if g.guest_name.is_empty() { "Connected" } else { &g.guest_name }),
                    Some(g) if g.connecting => "Joining…".to_string(),
                    _ => "Waiting for the guest to join".to_string(),
                };
                if shown.as_ref().is_none_or(|(l, _)| *l != line) {
                    shown = Some((line.clone(), slate(&c2, &line)));
                }
                let mut f = shown.as_ref().unwrap().1.detached();
                f.pts = clock::now();
                slot.push(f);
            }
            std::thread::sleep(Duration::from_millis(400));
        }
    })?;
    Ok(Box::new(GuestRunning { id, stop, thread: Some(thread) }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn ids_and_clocks() {
        let a = new_id();
        assert_eq!(a.len(), 12);
        assert!(a.chars().all(|c| c.is_ascii_alphanumeric()));
        assert_ne!(a, new_id());
        let mut c = RtpClock::new(90_000);
        let t0 = c.host(0xFFFF_0000);
        let t1 = c.host(0xFFFF_0000u32.wrapping_add(90_000));
        assert_eq!(t1 - t0, NS_PER_SEC, "across the wrap");
        assert_eq!(c.host(0xFFFF_0000u32.wrapping_add(45_000)) - t0, NS_PER_SEC / 2, "a late packet");
    }

    /// A guest from a WebRTC client (standing in for a browser) sending
    /// H.264 and Opus: the picture reaches the source, the voice the mixer.
    #[test]
    fn a_guest_joins_with_picture_and_sound() {
        use crate::media::testsrc::TestPattern;
        let port = 18_791;
        let id = "e2etestguest";
        // The guest spot, as the engine would start it.
        let slot = SourceSlot::new(0, 0, 0);
        let audio = AudioHub::new();
        let (atx, arx) = bounded::<AudioBlock>(1000);
        audio.register("Guest E2E", atx);
        let src = SourceConfig { name: "Guest E2E".into(), device: format!("guest:{id}"), width: 320, height: 180, fps: 30, network_latency_ms: 100, ..Default::default() };
        let running = start_source(&src, slot.clone(), audio.clone()).unwrap();
        let mut cfg = Config::default();
        cfg.guests = GuestsConfig { enabled: true, port, access: GuestAccess::Custom, public_url: format!("http://127.0.0.1:{port}"), return_video: false, ..Default::default() };
        let h = hub();
        h.apply(&cfg, Arc::new(|| {}));
        assert_eq!(h.link(id).as_deref(), Some(format!("http://127.0.0.1:{port}/g/{id}").as_str()));
        // The page and its settings.
        let get = |path: &str| -> String {
            let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
            write!(c, "GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
            let mut s = String::new();
            c.read_to_string(&mut s).unwrap();
            s
        };
        assert!(get(&format!("/g/{id}")).contains("Join the show"));
        assert!(get(&format!("/g/{id}/info")).contains("\"slot\":\"Guest E2E\""));
        assert!(get("/g/nosuchguest/info").starts_with("HTTP/1.1 404"));
        assert!(get("/").starts_with("HTTP/1.1 404"), "only guest pages are served");

        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        let frames_before = slot.frames();
        let result: Result<()> = rt.block_on(async {
            let mut me = MediaEngine::default();
            let opus = RTCRtpCodec { mime_type: MIME_TYPE_OPUS.to_owned(), clock_rate: 48_000, channels: 2, sdp_fmtp_line: "minptime=10;useinbandfec=1".into(), rtcp_feedback: vec![] };
            let h264 = RTCRtpCodec { mime_type: MIME_TYPE_H264.to_owned(), clock_rate: 90_000, channels: 0, sdp_fmtp_line: "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f".into(), rtcp_feedback: vec![] };
            me.register_codec(RTCRtpCodecParameters { rtp_codec: opus.clone(), payload_type: 111 }, RtpCodecKind::Audio)?;
            me.register_codec(RTCRtpCodecParameters { rtp_codec: h264.clone(), payload_type: 102 }, RtpCodecKind::Video)?;
            let registry = register_default_interceptors(rtc::interceptor::Registry::new(), &mut me)?;
            struct Client {
                gathered: webrtc::runtime::Sender<()>,
                connected: webrtc::runtime::Sender<()>,
            }
            #[async_trait::async_trait]
            impl PeerConnectionEventHandler for Client {
                async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
                    if state == RTCIceGatheringState::Complete {
                        let _ = self.gathered.try_send(());
                    }
                }
                async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
                    if state == RTCPeerConnectionState::Connected {
                        let _ = self.connected.try_send(());
                    }
                }
            }
            let (gtx, mut grx) = webrtc::runtime::channel::<()>(1);
            let (ctx, mut crx) = webrtc::runtime::channel::<()>(1);
            let pc = PeerConnectionBuilder::new()
                .with_configuration(RTCConfigurationBuilder::new().build())
                .with_media_engine(me)
                .with_interceptor_registry(registry)
                .with_handler(Arc::new(Client { gathered: gtx, connected: ctx }))
                .with_runtime(webrtc::runtime::default_runtime().unwrap())
                .with_udp_addrs(vec!["0.0.0.0:0"])
                .build()
                .await?;
            let mk = |kind, codec: RTCRtpCodec| {
                Arc::new(
                    TrackLocalStaticSample::new(
                        Instant::now(),
                        MediaStreamTrack::new("guest".into(), format!("guest-{kind:?}"), "guest".into(), kind, vec![RTCRtpEncodingParameters { rtp_coding_parameters: RTCRtpCodingParameters { ssrc: Some(rand_u32()), ..Default::default() }, codec, ..Default::default() }]),
                    )
                    .unwrap(),
                )
            };
            let vtrack = mk(RtpCodecKind::Video, h264);
            let atrack = mk(RtpCodecKind::Audio, opus);
            let vs = pc.add_track(vtrack.clone() as Arc<dyn TrackLocal>).await?;
            let asnd = pc.add_track(atrack.clone() as Arc<dyn TrackLocal>).await?;
            let offer = pc.create_offer(None).await?;
            pc.set_local_description(offer).await?;
            let _ = tokio::time::timeout(Duration::from_secs(4), grx.recv()).await;
            let sdp = pc.local_description().await.unwrap().sdp;
            // Join, as the page does.
            let body = serde_json::json!({ "sdp": sdp, "name": "Robo Guest" }).to_string();
            let answer = tokio::task::spawn_blocking(move || {
                let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
                write!(c, "POST /g/{id}/offer HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
                let mut s = String::new();
                c.read_to_string(&mut s).unwrap();
                s
            })
            .await
            .unwrap();
            let json = answer.split("\r\n\r\n").nth(1).unwrap_or("");
            let v: serde_json::Value = serde_json::from_str(json).map_err(|e| anyhow!("{e}: {answer}"))?;
            let sdp = v["sdp"].as_str().ok_or_else(|| anyhow!("no answer: {answer}"))?.to_string();
            pc.set_remote_description(RTCSessionDescription::answer(sdp)?).await?;
            tokio::time::timeout(Duration::from_secs(10), crx.recv()).await.map_err(|_| anyhow!("never connected"))?;

            // Three seconds of picture and sound.
            let vpt = vs.get_parameters().await?.rtp_parameters.codecs[0].payload_type;
            let apt = asnd.get_parameters().await?.rtp_parameters.codecs[0].payload_type;
            let vssrc = *vtrack.ssrcs().await.first().unwrap();
            let assrc = *atrack.ssrcs().await.first().unwrap();
            let (ptx, prx) = crossbeam_channel::unbounded();
            let settings = super::super::encode::VideoSettings { codec: VideoCodec::H264, width: 320, height: 180, fps: 30, bitrate_kbps: 600, purpose: super::super::encode::Purpose::Stream, backend: "auto".into() };
            let mut enc = super::super::encode::create_video(&settings, Arc::new(move |p| {
                let _ = ptx.send(p);
            }))?;
            let mut tp = TestPattern::new("colors", 320, 180);
            let mut opus_enc = super::super::opus::Encoder::new(64_000)?;
            for i in 0..90i64 {
                let pts = i * NS_PER_SEC / 30;
                let img = tp.render(pts, 30);
                enc.encode(&VideoFrame::new_nv12(320, 180, pts, img.y, img.uv, i as u64), pts, i == 0)?;
                while let Ok(p) = prx.try_recv() {
                    let sample = Sample { data: bytes::Bytes::from(annexb(&p)), duration: Duration::from_millis(33), ..Sample::new(Instant::now()) };
                    vtrack.sample_writer(vssrc, vpt).write_sample(&sample).await?;
                }
                // 33 ms of a 440 Hz tone.
                let tone: Vec<f32> = (0..1600).flat_map(|k| {
                    let t = (i * 1600 + k) as f32 / 48_000.0;
                    let v = (t * 440.0 * std::f32::consts::TAU).sin() * 0.5;
                    [v, v]
                }).collect();
                for p in opus_enc.push(&tone) {
                    let sample = Sample { data: bytes::Bytes::from(p), duration: Duration::from_millis(20), ..Sample::new(Instant::now()) };
                    atrack.sample_writer(assrc, apt).write_sample(&sample).await?;
                }
                tokio::time::sleep(Duration::from_millis(33)).await;
            }
            tokio::time::sleep(Duration::from_millis(800)).await;
            let _ = pc.close().await;
            Ok(())
        });
        result.unwrap();
        let g = h.guests().into_iter().find(|g| g.id == id);
        // The picture: far more frames than the slate's few, and not the slate.
        let frames = slot.frames() - frames_before;
        assert!(frames > 40, "only {frames} pictures arrived");
        let last = slot.pick(clock::now() + NS_PER_SEC).unwrap();
        assert_eq!((last.width, last.height), (320, 180));
        let distinct = last.pixels.with_planes(|p| {
            let mut seen = std::collections::HashSet::new();
            for x in (0..320).step_by(16) {
                seen.insert(p.y[90 * p.y_stride + x] / 16);
            }
            seen.len()
        });
        assert!(distinct >= 4, "the test pattern, not the slate ({distinct} levels)");
        // The voice, in the mixer's input.
        let blocks: Vec<AudioBlock> = arx.try_iter().collect();
        let loud = blocks.iter().filter(|b| (b.samples.iter().map(|v| v * v).sum::<f32>() / b.samples.len().max(1) as f32).sqrt() > 0.15).count();
        assert!(loud > 50, "{} audio blocks, {loud} with the tone", blocks.len());
        println!("guest: {g:?}; {frames} pictures; {} audio blocks", blocks.len());
        cfg.guests.enabled = false;
        h.apply(&cfg, Arc::new(|| {}));
        drop(running);
    }
}
