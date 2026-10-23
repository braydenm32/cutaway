//! Scheduling a live stream on YouTube ahead of the show: the event's page
//! (title, description, thumbnail, start time, who can watch) exists before
//! you go live, so it can be shared, and it goes live by itself when
//! Cutaway's stream arrives.

use super::google::{agent, read_json, reason, Auth};
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// What to put on YouTube.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub title: String,
    pub description: String,
    /// When it starts (UTC, RFC 3339).
    pub start: String,
    /// "private", "unlisted" or "public".
    pub privacy: String,
    pub thumbnail: Option<PathBuf>,
    /// The stream key Cutaway already sends to, to tie the event to (so the
    /// key doesn't change). None, or not found: a new key is made.
    pub stream_key: Option<String>,
    /// Change this event instead of making a new one.
    pub existing: Option<String>,
}

/// The event on YouTube.
#[derive(Clone, Debug, PartialEq)]
pub struct Scheduled {
    pub id: String,
    /// The watch page, to share.
    pub url: String,
    /// Where to send the stream: server and key (None when updating an
    /// event that already has its stream).
    pub ingest: Option<(String, String)>,
    /// Something that didn't work but didn't stop it (the thumbnail).
    pub note: Option<String>,
}

pub fn watch_url(id: &str) -> String {
    format!("https://www.youtube.com/watch?v={id}")
}

/// One call to YouTube's API, with a fresh token (and once more if the
/// token had just expired).
fn call(auth: &mut Auth, method: &str, url: &str, body: Option<&Value>) -> Result<Value> {
    let http = agent(Duration::from_secs(60));
    for attempt in 0..2 {
        let token = auth.token()?;
        let bearer = format!("Bearer {token}");
        let resp = match (method, body) {
            ("POST", Some(b)) => http.post(url).header("Authorization", &bearer).header("Content-Type", "application/json; charset=UTF-8").send(b.to_string().as_bytes()),
            ("POST", None) => http.post(url).header("Authorization", &bearer).send_empty(),
            ("PUT", Some(b)) => http.put(url).header("Authorization", &bearer).header("Content-Type", "application/json; charset=UTF-8").send(b.to_string().as_bytes()),
            ("DELETE", _) => http.delete(url).header("Authorization", &bearer).call(),
            _ => http.get(url).header("Authorization", &bearer).call(),
        };
        let mut r = resp.map_err(|e| anyhow!("couldn't reach YouTube ({e})"))?;
        let code = r.status().as_u16();
        if code == 401 && attempt == 0 {
            auth.expired();
            continue;
        }
        if code == 204 {
            return Ok(Value::Null);
        }
        let v = read_json(&mut r).unwrap_or_default();
        if !(200..300).contains(&code) {
            let why = reason(&v);
            if v["error"]["errors"][0]["reason"].as_str() == Some("liveStreamingNotEnabled") || why.contains("not enabled for live streaming") {
                bail!("live streaming isn't turned on for this YouTube channel yet: turn it on at youtube.com/features (it can take a day the first time)");
            }
            if code == 403 && why.to_lowercase().contains("insufficient") {
                bail!("sign in again and allow Cutaway to manage your YouTube live streams");
            }
            bail!("YouTube said no ({code}): {why}");
        }
        return Ok(v);
    }
    bail!("YouTube wouldn't take the sign-in; sign in again")
}

fn snippet(p: &Plan) -> Value {
    json!({
        "title": p.title.chars().take(100).collect::<String>(),
        "description": p.description.chars().take(5000).collect::<String>(),
        "scheduledStartTime": p.start,
    })
}

/// Make (or change) the event, tie it to a stream, and set its thumbnail.
pub fn schedule(auth: &mut Auth, p: &Plan) -> Result<Scheduled> {
    if p.title.trim().is_empty() {
        bail!("give the stream a title first");
    }
    let api = auth.client.ep.youtube_api.clone();
    let status = json!({ "privacyStatus": p.privacy, "selfDeclaredMadeForKids": false });
    let (id, ingest) = match &p.existing {
        Some(id) => {
            let body = json!({ "id": id, "snippet": snippet(p), "status": status });
            call(auth, "PUT", &format!("{api}/liveBroadcasts?part=snippet,status"), Some(&body))?;
            (id.clone(), None)
        }
        None => {
            let body = json!({
                "snippet": snippet(p),
                "status": status,
                // Live as soon as the stream arrives, finished when it stops.
                "contentDetails": { "enableAutoStart": true, "enableAutoStop": true, "enableDvr": true, "latencyPreference": "normal" },
            });
            let v = call(auth, "POST", &format!("{api}/liveBroadcasts?part=snippet,status,contentDetails"), Some(&body))?;
            let id = v["id"].as_str().ok_or_else(|| anyhow!("YouTube didn't say which event it made"))?.to_string();
            let tied = find_or_make_stream(auth, &api, p.stream_key.as_deref()).and_then(|stream| {
                call(auth, "POST", &format!("{api}/liveBroadcasts/bind?id={id}&part=id,contentDetails&streamId={}", stream.0), None).context("tying the event to the stream")?;
                Ok(stream)
            });
            match tied {
                Ok(stream) => (id, Some(stream.1)),
                Err(e) => {
                    // Don't leave a half-made event on the channel (trying
                    // again would make another).
                    let _ = cancel(auth, &id);
                    return Err(e);
                }
            }
        }
    };
    let note = match &p.thumbnail {
        Some(path) => set_thumbnail(auth, &id, path).err().map(|e| format!("the thumbnail wasn't set: {e:#}")),
        None => None,
    };
    Ok(Scheduled { url: watch_url(&id), id, ingest, note })
}

/// The stream with this key (so Cutaway's key stays), or a new one.
/// Returns its id and (server, key).
fn find_or_make_stream(auth: &mut Auth, api: &str, key: Option<&str>) -> Result<(String, (String, String))> {
    let ingest = |s: &Value| {
        let i = &s["cdn"]["ingestionInfo"];
        (i["ingestionAddress"].as_str().unwrap_or("rtmp://a.rtmp.youtube.com/live2").to_string(), i["streamName"].as_str().unwrap_or("").to_string())
    };
    if let Some(key) = key.map(str::trim).filter(|k| !k.is_empty()) {
        let mut page = String::new();
        for _ in 0..10 {
            let v = call(auth, "GET", &format!("{api}/liveStreams?part=id,cdn&mine=true&maxResults=50{page}"), None)?;
            if let Some(s) = v["items"].as_array().into_iter().flatten().find(|s| s["cdn"]["ingestionInfo"]["streamName"].as_str() == Some(key)) {
                return Ok((s["id"].as_str().unwrap_or("").to_string(), ingest(s)));
            }
            match v["nextPageToken"].as_str() {
                Some(t) => page = format!("&pageToken={t}"),
                None => break,
            }
        }
    }
    let body = json!({
        "snippet": { "title": "Cutaway" },
        "cdn": { "ingestionType": "rtmp", "resolution": "variable", "frameRate": "variable" },
        "contentDetails": { "isReusable": true },
    });
    let s = call(auth, "POST", &format!("{api}/liveStreams?part=snippet,cdn,contentDetails"), Some(&body))?;
    let id = s["id"].as_str().ok_or_else(|| anyhow!("YouTube didn't make a stream key"))?.to_string();
    Ok((id, ingest(&s)))
}

/// A picture for the event's page (JPEG or PNG, under 2 MB).
fn set_thumbnail(auth: &mut Auth, id: &str, path: &Path) -> Result<()> {
    let data = std::fs::read(path).with_context(|| format!("can't read {}", path.display()))?;
    if data.len() > 2 * 1024 * 1024 {
        bail!("YouTube takes thumbnails up to 2 MB, and this one is {:.1} MB", data.len() as f64 / 1048576.0);
    }
    let mime = if data.starts_with(&[0x89, b'P', b'N', b'G']) { "image/png" } else { "image/jpeg" };
    let url = format!("{}?videoId={id}", auth.client.ep.youtube_thumbnails);
    for attempt in 0..2 {
        let token = auth.token()?;
        let mut r = agent(Duration::from_secs(120))
            .post(&url)
            .header("Authorization", &format!("Bearer {token}"))
            .header("Content-Type", mime)
            .send(&data[..])
            .map_err(|e| anyhow!("couldn't reach YouTube ({e})"))?;
        match r.status().as_u16() {
            401 if attempt == 0 => auth.expired(),
            200..=299 => return Ok(()),
            c => {
                let v = read_json(&mut r).unwrap_or_default();
                bail!("YouTube said no ({c}): {}", reason(&v));
            }
        }
    }
    bail!("YouTube wouldn't take the sign-in")
}

/// Take the event off YouTube.
pub fn cancel(auth: &mut Auth, id: &str) -> Result<()> {
    let api = auth.client.ep.youtube_api.clone();
    call(auth, "DELETE", &format!("{api}/liveBroadcasts?id={id}"), None).map(|_| ())
}

/// "2026-10-25 19:30" (this computer's time) as UTC for YouTube, if it's a
/// time.
pub fn to_utc(local: &str) -> Option<String> {
    let t = parse_local(local)?;
    Some(t.with_timezone(&chrono::Utc).format("%Y-%m-%dT%H:%M:%SZ").to_string())
}

/// "2026-10-25 19:30" (or with seconds, or a T) in this computer's time.
pub fn parse_local(s: &str) -> Option<chrono::DateTime<chrono::Local>> {
    use chrono::TimeZone;
    let s = s.trim().replace('T', " ");
    let naive = ["%Y-%m-%d %H:%M", "%Y-%m-%d %H:%M:%S"].iter().find_map(|f| chrono::NaiveDateTime::parse_from_str(&s, f).ok())?;
    chrono::Local.from_local_datetime(&naive).earliest()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::google::{Client, Endpoints};
    use crate::util::LockExt;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    /// A stand-in for YouTube: (method, path and query, body) of each
    /// request, and answers by path.
    fn fake(streams: Value) -> (u16, Arc<Mutex<Vec<(String, String, String)>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let log = Arc::new(Mutex::new(Vec::new()));
        let l2 = log.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut c) = conn else { continue };
                let mut data = Vec::new();
                let mut buf = [0u8; 65536];
                let head_end = loop {
                    let n = c.read(&mut buf).unwrap_or(0);
                    if n == 0 {
                        break None;
                    }
                    data.extend_from_slice(&buf[..n]);
                    if let Some(i) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                        break Some(i + 4);
                    }
                };
                let Some(head_end) = head_end else { continue };
                let head = String::from_utf8_lossy(&data[..head_end]).to_string();
                let len: usize = head.lines().find_map(|l| l.split_once(':').filter(|(a, _)| a.trim().eq_ignore_ascii_case("content-length")).map(|(_, v)| v.trim().parse().unwrap_or(0))).unwrap_or(0);
                while data.len() < head_end + len {
                    let n = c.read(&mut buf).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    data.extend_from_slice(&buf[..n]);
                }
                let body = String::from_utf8_lossy(&data[head_end..]).to_string();
                let mut first = head.lines().next().unwrap_or("").split_whitespace();
                let (method, target) = (first.next().unwrap_or("").to_string(), first.next().unwrap_or("").to_string());
                l2.lock_ok().push((method.clone(), target.clone(), if target.starts_with("/thumbs") { format!("{} bytes", body.len()) } else { body.clone() }));
                let path = target.split('?').next().unwrap_or("").to_string();
                let answer = match (method.as_str(), path.as_str()) {
                    ("POST", "/token") => r#"{"access_token":"a1","expires_in":3600}"#.to_string(),
                    ("POST", "/yt/liveBroadcasts") => r#"{"id":"ev1"}"#.to_string(),
                    ("PUT", "/yt/liveBroadcasts") => r#"{"id":"ev1"}"#.to_string(),
                    ("GET", "/yt/liveStreams") => streams.to_string(),
                    ("POST", "/yt/liveStreams") => r#"{"id":"st-new","cdn":{"ingestionInfo":{"ingestionAddress":"rtmp://a.rtmp.youtube.com/live2","streamName":"new-key"}}}"#.to_string(),
                    ("POST", "/yt/liveBroadcasts/bind") => r#"{"id":"ev1"}"#.to_string(),
                    ("POST", "/thumbs") => "{}".to_string(),
                    _ => r#"{"error":{"message":"not here"}}"#.to_string(),
                };
                let status = if answer.contains("not here") { "404 Not Found" } else { "200 OK" };
                let _ = write!(c, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}", answer.len());
            }
        });
        (port, log)
    }

    fn auth(port: u16) -> Auth {
        let base = format!("http://127.0.0.1:{port}");
        let ep = Endpoints {
            token: format!("{base}/token"),
            youtube_api: format!("{base}/yt"),
            youtube_thumbnails: format!("{base}/thumbs"),
            ..Default::default()
        };
        Auth::new(Client { id: "c".into(), secret: "s".into(), ep }, "r1".into())
    }

    fn plan() -> Plan {
        Plan {
            title: "Friday show".into(),
            description: "With guests".into(),
            start: "2026-10-30T18:30:00Z".into(),
            privacy: "unlisted".into(),
            thumbnail: None,
            stream_key: Some("my-key".into()),
            existing: None,
        }
    }

    #[test]
    fn schedules_on_the_key_already_in_use() {
        let (port, log) = fake(json!({"items":[{"id":"st-other","cdn":{"ingestionInfo":{"streamName":"other"}}},{"id":"st-mine","cdn":{"ingestionInfo":{"ingestionAddress":"rtmp://a.rtmp.youtube.com/live2","streamName":"my-key"}}}]}));
        let thumb = std::env::temp_dir().join(format!("cutaway-thumb-{}.png", std::process::id()));
        image::RgbaImage::from_pixel(16, 9, image::Rgba([200, 30, 30, 255])).save(&thumb).unwrap();
        let s = schedule(&mut auth(port), &Plan { thumbnail: Some(thumb.clone()), ..plan() }).unwrap();
        let _ = std::fs::remove_file(&thumb);
        assert_eq!(s.id, "ev1");
        assert_eq!(s.url, "https://www.youtube.com/watch?v=ev1");
        assert_eq!(s.ingest, Some(("rtmp://a.rtmp.youtube.com/live2".into(), "my-key".into())), "the key stays the same");
        assert_eq!(s.note, None);
        let log = log.lock_ok().clone();
        let made: Value = serde_json::from_str(&log.iter().find(|r| r.0 == "POST" && r.1.starts_with("/yt/liveBroadcasts?")).unwrap().2).unwrap();
        assert_eq!(made["snippet"]["title"], "Friday show");
        assert_eq!(made["snippet"]["scheduledStartTime"], "2026-10-30T18:30:00Z");
        assert_eq!(made["status"]["privacyStatus"], "unlisted");
        assert_eq!(made["contentDetails"]["enableAutoStart"], true);
        assert!(log.iter().any(|r| r.1.contains("/yt/liveBroadcasts/bind?id=ev1") && r.1.contains("streamId=st-mine")), "{log:?}");
        assert!(!log.iter().any(|r| r.0 == "POST" && r.1.starts_with("/yt/liveStreams")), "no new key");
        assert!(log.iter().any(|r| r.1 == "/thumbs?videoId=ev1"), "{log:?}");
    }

    #[test]
    fn makes_a_key_when_needed_and_changes_an_event() {
        let (port, log) = fake(json!({"items":[]}));
        let s = schedule(&mut auth(port), &plan()).unwrap();
        assert_eq!(s.ingest, Some(("rtmp://a.rtmp.youtube.com/live2".into(), "new-key".into())));
        assert!(log.lock_ok().iter().any(|r| r.1.contains("streamId=st-new")));
        // Changing it later: the same event, no new stream.
        log.lock_ok().clear();
        let s = schedule(&mut auth(port), &Plan { title: "Friday show (moved)".into(), existing: Some("ev1".into()), ..plan() }).unwrap();
        assert_eq!((s.id.as_str(), s.ingest), ("ev1", None));
        let log = log.lock_ok().clone();
        assert!(log.iter().any(|r| r.0 == "PUT" && r.2.contains("(moved)") && r.2.contains("\"id\":\"ev1\"")), "{log:?}");
        assert!(!log.iter().any(|r| r.1.contains("bind")));
        assert!(schedule(&mut auth(port), &Plan { title: " ".into(), ..plan() }).is_err());
    }

    #[test]
    fn times() {
        let utc = to_utc("2026-10-30 18:30").unwrap();
        let back = chrono::DateTime::parse_from_rfc3339(&utc).unwrap();
        assert_eq!(back.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string(), "2026-10-30 18:30");
        assert!(parse_local("2026-10-30T18:30").is_some());
        assert!(parse_local("tomorrow").is_none());
    }
}
