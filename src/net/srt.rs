//! SRT caller: sends an MPEG-TS stream to a listener.

use anyhow::{anyhow, bail, Context, Result};
use bytes::Bytes;
use futures::SinkExt;
use srt_tokio::SrtSocket;
use std::net::ToSocketAddrs;
use std::time::{Duration, Instant};

#[derive(Debug, PartialEq)]
pub struct SrtUrl {
    pub host: String,
    pub port: u16,
    pub stream_id: Option<String>,
    pub latency_ms: u64,
}

/// srt://host:port[?streamid=...&latency=ms]
pub fn parse_url(url: &str) -> Result<SrtUrl> {
    let rest = url.trim().strip_prefix("srt://").ok_or_else(|| anyhow!("the address must start with srt://"))?;
    let (hostport, query) = rest.split_once('?').unwrap_or((rest, ""));
    let hostport = hostport.trim_end_matches('/');
    let (host, port) = hostport.rsplit_once(':').ok_or_else(|| anyhow!("the address needs a port (srt://host:port)"))?;
    // An IPv6 address is written in brackets: srt://[::1]:9000
    let host = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
    let port: u16 = port.parse().map_err(|_| anyhow!("bad port \"{port}\""))?;
    let mut stream_id = None;
    let mut latency_ms = 200;
    for kv in query.split('&').filter(|s| !s.is_empty()) {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        match k {
            "streamid" => stream_id = Some(crate::util::percent_decode(v, false)),
            "latency" => latency_ms = v.parse().unwrap_or(200),
            _ => {}
        }
    }
    if host.is_empty() {
        bail!("the address has no host");
    }
    Ok(SrtUrl { host: host.to_string(), port, stream_id, latency_ms })
}

pub struct SrtSender {
    rt: tokio::runtime::Runtime,
    socket: SrtSocket,
    sent: u64,
}

impl SrtSender {
    pub fn connect(url: &str, timeout: Duration) -> Result<SrtSender> {
        let u = parse_url(url)?;
        let addr = (u.host.as_str(), u.port)
            .to_socket_addrs()
            .with_context(|| format!("can't find {}", u.host))?
            .min_by_key(|a| a.is_ipv6())
            .ok_or_else(|| anyhow!("can't find {}", u.host))?;
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build()?;
        let socket = rt.block_on(async {
            tokio::time::timeout(
                timeout,
                SrtSocket::builder().latency(Duration::from_millis(u.latency_ms)).call(addr.to_string().as_str(), u.stream_id.as_deref()),
            )
            .await
        });
        let socket = match socket {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                let detail = e.to_string();
                if detail.trim().is_empty() {
                    match e.kind() {
                        std::io::ErrorKind::ConnectionRefused => bail!("{}:{} refused the SRT connection (is the listener running?)", u.host, u.port),
                        kind => bail!("SRT connection to {}:{} failed ({kind:?})", u.host, u.port),
                    }
                }
                bail!("SRT connection to {}:{} failed: {detail}", u.host, u.port)
            }
            Err(_) => bail!("no SRT listener answered at {}:{}", u.host, u.port),
        };
        Ok(SrtSender { rt, socket, sent: 0 })
    }

    /// Send transport stream data (multiple of 188 bytes), 7 packets per
    /// SRT datagram.
    pub fn send(&mut self, ts: &[u8]) -> Result<()> {
        let socket = &mut self.socket;
        self.rt.block_on(async {
            for chunk in ts.chunks(188 * 7) {
                tokio::time::timeout(Duration::from_secs(5), socket.send((Instant::now(), Bytes::copy_from_slice(chunk))))
                    .await
                    .map_err(|_| anyhow!("the SRT connection stalled"))?
                    .map_err(|e| anyhow!("SRT connection lost: {e}"))?;
            }
            Ok::<_, anyhow::Error>(())
        })?;
        self.sent += ts.len() as u64;
        Ok(())
    }

    pub fn bytes_sent(&self) -> u64 {
        self.sent
    }

    pub fn close(mut self) {
        let socket = &mut self.socket;
        let _ = self.rt.block_on(async { tokio::time::timeout(Duration::from_secs(2), socket.close_and_finish()).await });
        self.rt.shutdown_timeout(Duration::from_millis(200));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        let u = parse_url("srt://10.0.0.5:9000?streamid=show&latency=500").unwrap();
        assert_eq!(u, SrtUrl { host: "10.0.0.5".into(), port: 9000, stream_id: Some("show".into()), latency_ms: 500 });
        assert!(parse_url("srt://host").is_err());
        assert_eq!(parse_url("srt://h:1").unwrap().latency_ms, 200);
        let u = parse_url("srt://[::1]:9000?streamid=%23%21%3A%3Ar%3Dlive").unwrap();
        assert_eq!((u.host.as_str(), u.stream_id.as_deref()), ("::1", Some("#!::r=live")));
    }
}
