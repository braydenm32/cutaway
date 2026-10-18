//! Network inputs: SRT (listening or calling) and RTMP (Cutaway as the
//! server), e.g. a phone camera app, OBS on another computer, a hardware
//! encoder or a remote guest's encoder.

use super::feed::{AudioHub, Feeder};
use super::sources::{Running, SourceSlot};
use crate::config::SourceConfig;
use crate::media::tsdemux::{is_keyframe, parameter_sets, TsDemuxer, TsUnit};
use crate::media::{h264, VideoCodec};
use crate::net::rtmp_server::{self, Incoming};
use anyhow::{anyhow, bail, Result};
use futures::StreamExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// What a network source's device setting means.
#[derive(Debug, Clone, PartialEq)]
pub enum NetSpec {
    /// "srt-in:9000": wait for an SRT caller on this port.
    SrtListen { port: u16 },
    /// "srt://host:port?streamid=...": call an SRT listener.
    SrtCall { url: String },
    /// "rtmp-in:1935/key": accept an RTMP publisher.
    RtmpListen { port: u16, key: String },
}

pub fn parse_spec(device: &str) -> Option<NetSpec> {
    let d = device.trim();
    let lower = d.to_lowercase();
    if let Some(rest) = lower.strip_prefix("srt-in:") {
        return rest.trim().parse().ok().map(|port| NetSpec::SrtListen { port });
    }
    if lower.starts_with("srt://") {
        let u = crate::net::srt::parse_url(d).ok();
        let host_empty = d[6..].starts_with(':') || u.as_ref().is_some_and(|u| u.host == "0.0.0.0");
        if host_empty || lower.contains("mode=listener") {
            let port = d[6..].split(['?', '/']).next()?.rsplit(':').next()?.parse().ok()?;
            return Some(NetSpec::SrtListen { port });
        }
        return Some(NetSpec::SrtCall { url: d.to_string() });
    }
    if let Some(rest) = d.strip_prefix("rtmp-in:").or_else(|| d.strip_prefix("RTMP-in:")) {
        let (port, key) = rest.split_once('/').unwrap_or((rest, ""));
        return port.trim().parse().ok().map(|port| NetSpec::RtmpListen { port, key: key.trim().to_string() });
    }
    None
}

pub fn is_network(device: &str) -> bool {
    parse_spec(device).is_some()
}

/// This computer's address on the local network (for telling senders
/// where to send).
pub fn local_ip() -> Option<String> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:9").ok()?; // nothing is sent
    s.local_addr().ok().map(|a| a.ip().to_string()).filter(|ip| ip != "0.0.0.0")
}

/// Address to give the sender, for the UI.
pub fn sender_address(spec: &NetSpec) -> String {
    let ip = local_ip().unwrap_or_else(|| "<this computer>".into());
    match spec {
        NetSpec::SrtListen { port } => format!("srt://{ip}:{port}"),
        NetSpec::SrtCall { url } => url.clone(),
        NetSpec::RtmpListen { port, key } => {
            if key.is_empty() {
                format!("rtmp://{ip}:{port}/live  (any stream key)")
            } else {
                format!("rtmp://{ip}:{port}/live  stream key: {key}")
            }
        }
    }
}

struct NetRunning {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for NetRunning {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Live state for the UI.
#[derive(Default, Clone, Debug)]
pub struct NetState {
    pub connected: bool,
    pub detail: String,
}

pub type NetStatus = Arc<Mutex<NetState>>;

/// Feed transport-stream units to the decoder.
fn feed_ts(feeder: &mut Feeder, units: Vec<TsUnit>, params: &mut Option<Vec<Vec<u8>>>) {
    for u in units {
        match u {
            TsUnit::Video { hevc, pts, dts, nals } => {
                let codec = if hevc { VideoCodec::Hevc } else { VideoCodec::H264 };
                let ps = parameter_sets(hevc, &nals);
                if !ps.is_empty() && params.as_ref() != Some(&ps) {
                    *params = Some(ps.clone());
                    feeder.video_config(codec, ps);
                }
                let key = is_keyframe(hevc, &nals);
                let avcc = h264::to_avcc(nals.iter().map(|n| n.as_slice()).filter(|n| {
                    if hevc {
                        let t = (n.first().copied().unwrap_or(0) >> 1) & 0x3f;
                        t != 35 // access unit delimiter
                    } else {
                        h264::h264_nal_type(n) != h264::NAL_AUD
                    }
                }));
                feeder.video(codec, pts, dts, key, &avcc);
            }
            TsUnit::Audio { pts, rate, channels, frame } => {
                feeder.audio_config(&crate::media::h264::aac_asc(rate, channels));
                feeder.audio(pts, &frame);
            }
        }
    }
}

fn srt_loop(spec: NetSpec, latency_ms: u32, mut feeder: Feeder, stop: Arc<AtomicBool>, state: NetStatus) {
    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            state.lock().unwrap().detail = format!("{e}");
            return;
        }
    };
    while !stop.load(Ordering::Relaxed) {
        let r: Result<()> = rt.block_on(async {
            let builder = srt_tokio::SrtSocket::builder().latency(Duration::from_millis(latency_ms.max(20) as u64));
            let connect = async {
                match &spec {
                    NetSpec::SrtListen { port } => builder.listen_on(*port).await.map_err(|e| anyhow!("can't listen on port {port}: {e}")),
                    NetSpec::SrtCall { url } => {
                        let u = crate::net::srt::parse_url(url)?;
                        builder.call(format!("{}:{}", u.host, u.port).as_str(), u.stream_id.as_deref()).await.map_err(|e| anyhow!("{e}"))
                    }
                    _ => bail!("not SRT"),
                }
            };
            // Wait for a connection, checking for stop.
            tokio::pin!(connect);
            let mut socket = loop {
                tokio::select! {
                    s = &mut connect => break s?,
                    _ = tokio::time::sleep(Duration::from_millis(200)) => {
                        if stop.load(Ordering::Relaxed) { return Ok(()); }
                    }
                }
            };
            {
                let mut st = state.lock().unwrap();
                st.connected = true;
                st.detail = "receiving".into();
            }
            let mut demux = TsDemuxer::new();
            let mut params = None;
            loop {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                match tokio::time::timeout(Duration::from_millis(200), socket.next()).await {
                    Ok(Some(Ok((_, data)))) => {
                        feed_ts(&mut feeder, demux.push(&data), &mut params);
                        if let Some(e) = feeder.take_error() {
                            state.lock().unwrap().detail = format!("can't decode the stream: {e}");
                        }
                    }
                    Ok(Some(Err(e))) => bail!("{e}"),
                    Ok(None) => break,
                    Err(_) => {}
                }
            }
            let _ = tokio::time::timeout(Duration::from_millis(300), socket.close_and_finish()).await;
            Ok(())
        });
        feeder.reset();
        {
            let mut st = state.lock().unwrap();
            st.connected = false;
            st.detail = match &r {
                Ok(()) => "waiting for the sender".into(),
                Err(e) => format!("{e:#}"),
            };
        }
        if stop.load(Ordering::Relaxed) {
            break;
        }
        std::thread::sleep(Duration::from_millis(if r.is_err() { 2000 } else { 300 }));
    }
}

fn rtmp_loop(port: u16, key: String, mut feeder: Feeder, stop: Arc<AtomicBool>, state: NetStatus) {
    while !stop.load(Ordering::Relaxed) {
        let st2 = state.clone();
        let r = rtmp_server::serve(port, &key, &stop, &mut |i| match i {
            Incoming::Started(k) => {
                feeder.reset();
                let mut st = st2.lock().unwrap();
                st.connected = true;
                st.detail = format!("receiving \"{k}\"");
            }
            Incoming::VideoConfig { codec, parameter_sets } => feeder.video_config(codec, parameter_sets),
            Incoming::Video { pts, dts, keyframe, data } => {
                let codec = feeder_codec(&feeder);
                feeder.video(codec, pts, dts, keyframe, &data);
                if let Some(e) = feeder.take_error() {
                    st2.lock().unwrap().detail = format!("can't decode the stream: {e}");
                }
            }
            Incoming::AudioConfig { asc } => feeder.audio_config(&asc),
            Incoming::Audio { pts, data } => feeder.audio(pts, &data),
            Incoming::Ended(why) => {
                feeder.reset();
                let mut st = st2.lock().unwrap();
                st.connected = false;
                st.detail = if why.contains(' ') { why } else { "waiting for the sender".into() };
            }
        });
        if let Err(e) = r {
            state.lock().unwrap().detail = format!("{e:#}");
            std::thread::sleep(Duration::from_secs(2));
        }
    }
}

/// The codec the feeder was configured with (RTMP sends it once).
fn feeder_codec(f: &Feeder) -> VideoCodec {
    f.codec().unwrap_or(VideoCodec::H264)
}

pub fn start(cfg: &SourceConfig, slot: Arc<SourceSlot>, hub: Arc<AudioHub>, state: NetStatus) -> Result<(Box<dyn Running>, String)> {
    let spec = parse_spec(&cfg.device).ok_or_else(|| anyhow!("\"{}\" isn't a network input", cfg.device))?;
    let stop = Arc::new(AtomicBool::new(false));
    let feeder = Feeder::new(&cfg.name, slot, hub, (cfg.width, cfg.height), cfg.network_latency_ms, stop.clone());
    let desc = match &spec {
        NetSpec::SrtListen { port } => format!("SRT input on port {port}"),
        NetSpec::SrtCall { url } => format!("SRT from {url}"),
        NetSpec::RtmpListen { port, .. } => format!("RTMP input on port {port}"),
    };
    *state.lock().unwrap() = NetState { connected: false, detail: "waiting for the sender".into() };
    let s2 = stop.clone();
    let latency = cfg.network_latency_ms;
    let thread = std::thread::Builder::new().name(format!("net {}", cfg.name)).spawn(move || match spec {
        NetSpec::RtmpListen { port, key } => rtmp_loop(port, key, feeder, s2, state),
        other => srt_loop(other, latency, feeder, s2, state),
    })?;
    Ok((Box::new(NetRunning { stop, thread: Some(thread) }), desc))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs() {
        assert_eq!(parse_spec("srt-in:9000"), Some(NetSpec::SrtListen { port: 9000 }));
        assert_eq!(parse_spec("srt://:9001"), Some(NetSpec::SrtListen { port: 9001 }));
        assert_eq!(parse_spec("srt://0.0.0.0:9002"), Some(NetSpec::SrtListen { port: 9002 }));
        assert_eq!(parse_spec("srt://10.0.0.4:9000?streamid=cam"), Some(NetSpec::SrtCall { url: "srt://10.0.0.4:9000?streamid=cam".into() }));
        assert_eq!(parse_spec("rtmp-in:1935/abc"), Some(NetSpec::RtmpListen { port: 1935, key: "abc".into() }));
        assert_eq!(parse_spec("rtmp-in:1936"), Some(NetSpec::RtmpListen { port: 1936, key: String::new() }));
        assert_eq!(parse_spec("FaceTime HD Camera"), None);
    }
}
