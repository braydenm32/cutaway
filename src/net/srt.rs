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
    /// Encrypts the stream (AES): both ends need the same one.
    pub passphrase: Option<String>,
}

/// SRT's own limits on a passphrase.
pub fn check_passphrase(p: &str) -> Result<()> {
    let n = p.chars().count();
    if !(10..=79).contains(&n) {
        bail!("an SRT passphrase has to be 10 to 79 characters (this one is {n})");
    }
    Ok(())
}

/// The passphrase in a `?passphrase=…` (decoded), if there is one.
pub fn passphrase_in(query: &str) -> Option<String> {
    query.split('&').filter_map(|kv| kv.split_once('=')).find(|(k, _)| *k == "passphrase").map(|(_, v)| crate::util::percent_decode(v, false)).filter(|p| !p.is_empty())
}

/// An SRT socket builder with the passphrase on (when there is one).
pub fn with_passphrase(b: srt_tokio::SrtSocketBuilder, passphrase: Option<&str>) -> Result<srt_tokio::SrtSocketBuilder> {
    Ok(match passphrase {
        Some(p) => {
            check_passphrase(p)?;
            b.encryption(16, p)
        }
        None => b,
    })
}

/// srt://host:port[?streamid=...&latency=ms&passphrase=...]
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
    let passphrase = passphrase_in(query);
    if let Some(p) = &passphrase {
        check_passphrase(p)?;
    }
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
    Ok(SrtUrl { host: host.to_string(), port, stream_id, latency_ms, passphrase })
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
        let builder = with_passphrase(SrtSocket::builder().latency(Duration::from_millis(u.latency_ms)), u.passphrase.as_deref())?;
        let socket = rt.block_on(async {
            tokio::time::timeout(
                timeout,
                builder.call(addr.to_string().as_str(), u.stream_id.as_deref()),
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
        assert_eq!(u, SrtUrl { host: "10.0.0.5".into(), port: 9000, stream_id: Some("show".into()), latency_ms: 500, passphrase: None });
        assert_eq!(parse_url("srt://h:1?passphrase=a%20long%20secret").unwrap().passphrase.as_deref(), Some("a long secret"));
        assert!(parse_url("srt://h:1?passphrase=short").is_err(), "SRT needs 10 to 79 characters");
        assert!(parse_url("srt://host").is_err());
        assert_eq!(parse_url("srt://h:1").unwrap().latency_ms, 200);
        let u = parse_url("srt://[::1]:9000?streamid=%23%21%3A%3Ar%3Dlive").unwrap();
        assert_eq!((u.host.as_str(), u.stream_id.as_deref()), ("::1", Some("#!::r=live")));
    }

    /// With a passphrase, the sender with the same one gets in; one with
    /// another (or none) doesn't.
    #[test]
    fn passphrases_must_match() {
        let port = crate::util::free_udp_port();
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
        let listener = rt.spawn(async move {
            let mut got = 0;
            for _ in 0..1 {
                let b = with_passphrase(SrtSocket::builder().latency(Duration::from_millis(50)), Some("the right passphrase")).unwrap();
                if let Ok(Ok(mut s)) = tokio::time::timeout(Duration::from_secs(10), b.listen_on(port)).await {
                    use futures::StreamExt;
                    if let Ok(Some(Ok(_))) = tokio::time::timeout(Duration::from_secs(5), s.next()).await {
                        got += 1;
                    }
                }
            }
            got
        });
        std::thread::sleep(Duration::from_millis(300));
        let wrong = SrtSender::connect(&format!("srt://127.0.0.1:{port}?passphrase=the%20wrong%20passphrase"), Duration::from_secs(2));
        assert!(wrong.is_err(), "the wrong passphrase is refused");
        let mut right = SrtSender::connect(&format!("srt://127.0.0.1:{port}?passphrase=the%20right%20passphrase"), Duration::from_secs(5)).expect("the right one gets in");
        right.send(&[0x47; 188]).unwrap();
        assert_eq!(rt.block_on(listener).unwrap(), 1);
        right.close();
    }
}
