//! Google sign-in and uploads (YouTube, Google Drive) with your own OAuth
//! client (a "Desktop app" client from Google Cloud). Cutaway signs in
//! through the browser (a loopback address and PKCE, as Google asks of
//! desktop apps) and keeps the refresh token. Files go up with Google's
//! resumable uploads, so a dropped connection carries on where it stopped.

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Where Google's services are (replaceable for tests).
#[derive(Clone, Debug)]
pub struct Endpoints {
    pub auth: String,
    pub token: String,
    pub userinfo: String,
    pub youtube_upload: String,
    /// YouTube's API (live events).
    pub youtube_api: String,
    pub youtube_thumbnails: String,
    pub drive: String,
    pub drive_upload: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Endpoints {
            auth: "https://accounts.google.com/o/oauth2/v2/auth".into(),
            token: "https://oauth2.googleapis.com/token".into(),
            userinfo: "https://openidconnect.googleapis.com/v1/userinfo".into(),
            youtube_upload: "https://www.googleapis.com/upload/youtube/v3/videos?uploadType=resumable&part=snippet,status".into(),
            youtube_api: "https://www.googleapis.com/youtube/v3".into(),
            youtube_thumbnails: "https://www.googleapis.com/upload/youtube/v3/thumbnails/set".into(),
            drive: "https://www.googleapis.com/drive/v3/files".into(),
            drive_upload: "https://www.googleapis.com/upload/drive/v3/files?uploadType=resumable&fields=id,webViewLink".into(),
        }
    }
}

/// Your OAuth client.
#[derive(Clone, Debug)]
pub struct Client {
    pub id: String,
    pub secret: String,
    pub ep: Endpoints,
}

pub const SCOPE_YOUTUBE: &str = "https://www.googleapis.com/auth/youtube.upload";
/// Managing YouTube: scheduling live streams (and uploading).
pub const SCOPE_YOUTUBE_LIVE: &str = "https://www.googleapis.com/auth/youtube";
/// Only files Cutaway itself puts in Drive.
pub const SCOPE_DRIVE: &str = "https://www.googleapis.com/auth/drive.file";

/// What to ask for: who you are, plus YouTube uploads, Drive and/or
/// scheduling live streams.
pub fn scopes(youtube: bool, drive: bool, live: bool) -> String {
    let mut s = vec!["openid", "email"];
    if youtube {
        s.push(SCOPE_YOUTUBE);
    }
    if drive {
        s.push(SCOPE_DRIVE);
    }
    if live {
        s.push(SCOPE_YOUTUBE_LIVE);
    }
    s.join(" ")
}

/// What a sign-in allowed ("youtube", "drive", "live"), from Google's list.
pub fn granted(scope: &str) -> Vec<String> {
    let has = |s: &str| scope.split_whitespace().any(|x| x == s);
    let mut g = Vec::new();
    // Managing YouTube covers uploading too.
    if has(SCOPE_YOUTUBE) || has(SCOPE_YOUTUBE_LIVE) {
        g.push("youtube".to_string());
    }
    if has(SCOPE_DRIVE) {
        g.push("drive".to_string());
    }
    if has(SCOPE_YOUTUBE_LIVE) {
        g.push("live".to_string());
    }
    g
}

/// For query strings and forms.
pub fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                let hex = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok());
                match hex {
                    Some(v) => {
                        out.push(v);
                        i += 3;
                        continue;
                    }
                    None => out.push(b'%'),
                }
            }
            b'+' => out.push(b' '),
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn b64url(data: &[u8]) -> String {
    crate::control::ws::base64(data).trim_end_matches('=').replace('+', "-").replace('/', "_")
}

fn random_text(n: usize) -> String {
    let mut b = vec![0u8; n];
    let _ = getrandom::fill(&mut b);
    b64url(&b)
}

/// Google's "308 Resume Incomplete" isn't a redirect: don't follow it.
pub(crate) fn agent(timeout: Duration) -> ureq::Agent {
    let tls = ureq::tls::TlsConfig::builder().provider(ureq::tls::TlsProvider::NativeTls).root_certs(ureq::tls::RootCerts::PlatformVerifier).build();
    ureq::Agent::config_builder()
        .tls_config(tls)
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .max_redirects(0)
        .build()
        .into()
}

pub(crate) fn read_json(resp: &mut ureq::http::Response<ureq::Body>) -> Result<Value> {
    let text = resp.body_mut().read_to_string().map_err(|e| anyhow!("Google's answer was cut off ({e})"))?;
    serde_json::from_str(&text).map_err(|_| anyhow!("Google answered with something unexpected: {}", text.chars().take(200).collect::<String>()))
}

/// Google's reason, from an error answer.
pub(crate) fn reason(v: &Value) -> String {
    v["error_description"]
        .as_str()
        .or_else(|| v["error"]["message"].as_str())
        .or_else(|| v["error"].as_str())
        .unwrap_or("no reason given")
        .to_string()
}

// ---- signing in ---------------------------------------------------------------------

/// Tokens from signing in.
#[derive(Clone, Debug)]
pub struct Tokens {
    pub access: String,
    pub refresh: String,
    /// What was granted.
    pub scope: String,
}

/// A sign-in under way: open `url` in the browser, then `finish`.
pub struct SignIn {
    pub url: String,
    listener: TcpListener,
    redirect: String,
    verifier: String,
    state: String,
}

impl SignIn {
    pub fn begin(client: &Client, scope: &str) -> Result<SignIn> {
        if client.id.trim().is_empty() || client.secret.trim().is_empty() {
            bail!("enter your OAuth client's ID and secret first");
        }
        let listener = TcpListener::bind("127.0.0.1:0").context("listening for Google's answer")?;
        let redirect = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
        let verifier = random_text(48);
        let challenge = {
            use sha2::Digest;
            b64url(&sha2::Sha256::digest(verifier.as_bytes()))
        };
        let state = random_text(16);
        let url = format!(
            "{}?client_id={}&redirect_uri={}&response_type=code&scope={}&code_challenge={challenge}&code_challenge_method=S256&access_type=offline&prompt=consent&state={state}",
            client.ep.auth,
            encode(client.id.trim()),
            encode(&redirect),
            encode(scope)
        );
        Ok(SignIn { url, listener, redirect, verifier, state })
    }

    /// Wait for the browser to come back (up to `timeout`), and swap the
    /// code for tokens.
    pub fn finish(self, client: &Client, timeout: Duration, cancel: &AtomicBool) -> Result<Tokens> {
        self.listener.set_nonblocking(true)?;
        let until = Instant::now() + timeout;
        let code = loop {
            if cancel.load(Ordering::Relaxed) {
                bail!("signing in was cancelled");
            }
            if Instant::now() > until {
                bail!("Google didn't come back in time; try signing in again");
            }
            match self.listener.accept() {
                Ok((conn, _)) => {
                    if let Some(r) = self.answer(conn)? {
                        break r?;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(100)),
                Err(e) => return Err(e.into()),
            }
        };
        let body = format!(
            "grant_type=authorization_code&code={}&client_id={}&client_secret={}&redirect_uri={}&code_verifier={}",
            encode(&code),
            encode(client.id.trim()),
            encode(client.secret.trim()),
            encode(&self.redirect),
            encode(&self.verifier)
        );
        let mut resp = agent(Duration::from_secs(30))
            .post(&client.ep.token)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .send(body.as_bytes())
            .map_err(|e| anyhow!("couldn't reach Google ({e})"))?;
        let ok = resp.status().is_success();
        let v = read_json(&mut resp)?;
        if !ok {
            bail!("Google didn't accept the sign-in: {}", reason(&v));
        }
        let refresh = v["refresh_token"].as_str().ok_or_else(|| anyhow!("Google didn't give a refresh token"))?.to_string();
        Ok(Tokens {
            access: v["access_token"].as_str().unwrap_or("").to_string(),
            refresh,
            scope: v["scope"].as_str().unwrap_or("").to_string(),
        })
    }

    /// The browser's request: Some(code) or Some(Err) once it's Google's
    /// answer; None for anything else (a favicon).
    fn answer(&self, mut conn: TcpStream) -> Result<Option<Result<String>>> {
        conn.set_nonblocking(false)?;
        conn.set_read_timeout(Some(Duration::from_secs(5)))?;
        let mut buf = [0u8; 8192];
        let n = conn.read(&mut buf).unwrap_or(0);
        let head = String::from_utf8_lossy(&buf[..n]).to_string();
        let target = head.split_whitespace().nth(1).unwrap_or("/").to_string();
        let query = target.split_once('?').map(|q| q.1).unwrap_or("");
        let get = |k: &str| query.split('&').filter_map(|kv| kv.split_once('=')).find(|(a, _)| *a == k).map(|(_, v)| decode(v));
        let page = |title: &str, text: &str| {
            format!(
                "<!doctype html><meta charset=utf-8><title>Cutaway</title><body style=\"font:16px -apple-system,Segoe UI,sans-serif;background:#0e1014;color:#e8eaf0;display:grid;place-items:center;height:90vh\"><div><h2>{title}</h2><p>{text}</p></div>"
            )
        };
        let reply = |conn: &mut TcpStream, body: String| {
            let _ = write!(conn, "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        };
        if get("state").is_none() && get("code").is_none() && get("error").is_none() {
            let _ = write!(conn, "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            return Ok(None);
        }
        if get("state").as_deref() != Some(self.state.as_str()) {
            reply(&mut conn, page("That didn't work", "This answer wasn't for this sign-in. Try again from Cutaway."));
            return Ok(Some(Err(anyhow!("the answer didn't match this sign-in"))));
        }
        if let Some(e) = get("error") {
            reply(&mut conn, page("Not signed in", "You can close this tab and try again from Cutaway."));
            return Ok(Some(Err(anyhow!(if e == "access_denied" { "you didn't allow Cutaway in".to_string() } else { format!("Google said: {e}") }))));
        }
        let code = get("code").unwrap_or_default();
        reply(&mut conn, page("Signed in", "You can close this tab and go back to Cutaway."));
        Ok(Some(Ok(code)))
    }
}

/// Access with a refresh token: a fresh access token, refreshed as needed.
pub struct Auth {
    pub client: Client,
    pub refresh: String,
    access: Option<(String, Instant)>,
}

impl Auth {
    pub fn new(client: Client, refresh: String) -> Auth {
        Auth { client, refresh, access: None }
    }

    pub fn token(&mut self) -> Result<String> {
        if let Some((t, until)) = &self.access {
            if Instant::now() + Duration::from_secs(60) < *until {
                return Ok(t.clone());
            }
        }
        let body = format!(
            "grant_type=refresh_token&refresh_token={}&client_id={}&client_secret={}",
            encode(&self.refresh),
            encode(self.client.id.trim()),
            encode(self.client.secret.trim())
        );
        let mut resp = agent(Duration::from_secs(30))
            .post(&self.client.ep.token)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .send(body.as_bytes())
            .map_err(|e| anyhow!("couldn't reach Google ({e})"))?;
        let ok = resp.status().is_success();
        let v = read_json(&mut resp)?;
        if !ok {
            let why = reason(&v);
            if v["error"].as_str() == Some("invalid_grant") {
                bail!("Google signed Cutaway out ({why}); sign in again in Settings → Upload");
            }
            bail!("Google didn't give access: {why}");
        }
        let t = v["access_token"].as_str().ok_or_else(|| anyhow!("Google gave no access token"))?.to_string();
        self.access = Some((t.clone(), Instant::now() + Duration::from_secs(v["expires_in"].as_u64().unwrap_or(3600))));
        Ok(t)
    }

    /// The access token stopped working: get another next time.
    pub fn expired(&mut self) {
        self.access = None;
    }
}

/// The signed-in account's email.
pub fn account(ep: &Endpoints, access: &str) -> Result<String> {
    let mut resp = agent(Duration::from_secs(20)).get(&ep.userinfo).header("Authorization", &format!("Bearer {access}")).call().map_err(|e| anyhow!("couldn't reach Google ({e})"))?;
    let v = read_json(&mut resp)?;
    Ok(v["email"].as_str().or_else(|| v["name"].as_str()).unwrap_or("your Google account").to_string())
}

// ---- uploads --------------------------------------------------------------------------

/// Upload pieces (a multiple of 256 KiB, as Google asks).
pub const CHUNK: usize = 8 * 1024 * 1024;

/// Send a file with a resumable upload: start a session at `init_url` with
/// `metadata`, then send it in pieces, picking up after errors. Returns
/// Google's answer for the finished file.
#[allow(clippy::too_many_arguments)]
pub fn resumable(auth: &mut Auth, init_url: &str, metadata: &Value, path: &Path, mime: &str, chunk: usize, progress: &mut dyn FnMut(u64, u64), cancel: &AtomicBool) -> Result<Value> {
    let total = std::fs::metadata(path).with_context(|| format!("reading {}", path.display()))?.len();
    let mut file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let http = agent(Duration::from_secs(300));
    let meta = metadata.to_string();
    // Start the session.
    let mut session = None;
    for attempt in 0..4 {
        let token = auth.token()?;
        let mut resp = http
            .post(init_url)
            .header("Authorization", &format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=UTF-8")
            .header("X-Upload-Content-Type", mime)
            .header("X-Upload-Content-Length", &total.to_string())
            .send(meta.as_bytes())
            .map_err(|e| anyhow!("couldn't reach Google ({e})"));
        match &mut resp {
            Ok(r) if r.status().is_success() => {
                session = r.headers().get("location").and_then(|l| l.to_str().ok()).map(str::to_string);
                break;
            }
            Ok(r) if r.status().as_u16() == 401 => auth.expired(),
            Ok(r) if r.status().is_server_error() || r.status().as_u16() == 429 => {}
            Ok(r) => {
                let code = r.status().as_u16();
                let v = read_json(r).unwrap_or_default();
                bail!("Google refused the upload ({code}): {}", reason(&v));
            }
            Err(e) if attempt == 3 => bail!("{e:#}"),
            Err(_) => {}
        }
        std::thread::sleep(Duration::from_secs(1 << attempt));
    }
    let session = session.ok_or_else(|| anyhow!("Google didn't start the upload"))?;

    let mut sent: u64 = 0;
    let mut buf = vec![0u8; chunk.max(256 * 1024)];
    let mut failures = 0u32;
    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = http.delete(&session).call();
            bail!("cancelled");
        }
        progress(sent, total);
        let n = if total == 0 {
            0
        } else {
            file.seek(SeekFrom::Start(sent))?;
            let want = ((total - sent) as usize).min(buf.len());
            file.read_exact(&mut buf[..want])?;
            want
        };
        let range = if n == 0 { format!("bytes */{total}") } else { format!("bytes {}-{}/{total}", sent, sent + n as u64 - 1) };
        let token = auth.token()?;
        let resp = http.put(&session).header("Authorization", &format!("Bearer {token}")).header("Content-Range", &range).send(&buf[..n]);
        // Where Google has got to (after a 308, or after asking).
        let received = |r: &ureq::http::Response<ureq::Body>| -> u64 {
            r.headers().get("range").and_then(|v| v.to_str().ok()).and_then(|v| v.rsplit('-').next()).and_then(|e| e.trim().parse::<u64>().ok()).map(|e| e + 1).unwrap_or(0)
        };
        match resp {
            Ok(mut r) => match r.status().as_u16() {
                200 | 201 => {
                    progress(total, total);
                    return read_json(&mut r);
                }
                308 => {
                    sent = received(&r);
                    failures = 0;
                }
                401 => auth.expired(),
                404 | 410 => bail!("the upload session expired; start the upload again"),
                c if c >= 500 || c == 429 || c == 408 => {
                    failures += 1;
                    sent = ask(&http, &session, auth, total).unwrap_or(sent);
                }
                c => {
                    let v = read_json(&mut r).unwrap_or_default();
                    bail!("Google stopped the upload ({c}): {}", reason(&v));
                }
            },
            Err(_) => {
                failures += 1;
                sent = ask(&http, &session, auth, total).unwrap_or(sent);
            }
        }
        if failures > 0 {
            if failures > 12 {
                bail!("the connection to Google keeps failing");
            }
            std::thread::sleep(Duration::from_secs((1u64 << failures.min(6)).min(60)));
        }
    }
}

/// How much of an interrupted upload Google has.
fn ask(http: &ureq::Agent, session: &str, auth: &mut Auth, total: u64) -> Result<u64> {
    let token = auth.token()?;
    let r = http.put(session).header("Authorization", &format!("Bearer {token}")).header("Content-Range", &format!("bytes */{total}")).send_empty().map_err(|e| anyhow!("{e}"))?;
    match r.status().as_u16() {
        308 => Ok(r.headers().get("range").and_then(|v| v.to_str().ok()).and_then(|v| v.rsplit('-').next()).and_then(|e| e.trim().parse::<u64>().ok()).map(|e| e + 1).unwrap_or(0)),
        200 | 201 => Ok(total),
        c => bail!("Google answered {c}"),
    }
}

/// YouTube's privacy setting.
pub fn youtube_metadata(title: &str, description: &str, privacy: &str) -> Value {
    json!({
        "snippet": { "title": title.chars().take(100).collect::<String>(), "description": description.chars().take(5000).collect::<String>(), "categoryId": "22" },
        "status": { "privacyStatus": privacy, "selfDeclaredMadeForKids": false }
    })
}

/// A Drive folder Cutaway made (found again by name), made if needed.
pub fn drive_folder(auth: &mut Auth, name: &str, parent: Option<&str>) -> Result<String> {
    let http = agent(Duration::from_secs(30));
    let q = format!(
        "name = '{}' and mimeType = 'application/vnd.google-apps.folder' and trashed = false{}",
        name.replace('\\', "\\\\").replace('\'', "\\'"),
        parent.map(|p| format!(" and '{p}' in parents")).unwrap_or_default()
    );
    let token = auth.token()?;
    let mut r = http
        .get(&format!("{}?q={}&fields=files(id)&spaces=drive", auth.client.ep.drive, encode(&q)))
        .header("Authorization", &format!("Bearer {token}"))
        .call()
        .map_err(|e| anyhow!("couldn't reach Google Drive ({e})"))?;
    let ok = r.status().is_success();
    let v = read_json(&mut r)?;
    if !ok {
        bail!("Google Drive: {}", reason(&v));
    }
    if let Some(id) = v["files"][0]["id"].as_str() {
        return Ok(id.to_string());
    }
    let mut meta = json!({ "name": name, "mimeType": "application/vnd.google-apps.folder" });
    if let Some(p) = parent {
        meta["parents"] = json!([p]);
    }
    let token = auth.token()?;
    let mut r = http
        .post(&format!("{}?fields=id", auth.client.ep.drive))
        .header("Authorization", &format!("Bearer {token}"))
        .header("Content-Type", "application/json; charset=UTF-8")
        .send(meta.to_string().as_bytes())
        .map_err(|e| anyhow!("couldn't reach Google Drive ({e})"))?;
    let ok = r.status().is_success();
    let v = read_json(&mut r)?;
    if !ok {
        bail!("Google Drive: {}", reason(&v));
    }
    v["id"].as_str().map(str::to_string).ok_or_else(|| anyhow!("Google Drive didn't make the folder"))
}

/// The kind of file, for Google.
pub fn mime(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(|e| e.to_lowercase()).as_deref() {
        Some("mp4") => "video/mp4",
        Some("mov") => "video/quicktime",
        Some("wav") => "audio/wav",
        Some("m4a") => "audio/mp4",
        Some("txt") | Some("csv") | Some("srt") | Some("vtt") => "text/plain",
        Some("xml") | Some("fcpxml") => "application/xml",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::util::LockExt;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    #[test]
    fn encodes_like_a_browser() {
        assert_eq!(encode("a b&c=d/é"), "a%20b%26c%3Dd%2F%C3%A9");
        assert_eq!(decode("4%2F0Ab-x_y+z"), "4/0Ab-x_y z");
        assert_eq!(b64url(&[0xfb, 0xff]), "-_8");
        assert_eq!(scopes(true, false, false), format!("openid email {SCOPE_YOUTUBE}"));
        assert_eq!(granted(&format!("openid {SCOPE_YOUTUBE} {SCOPE_DRIVE}")), ["youtube", "drive"]);
        assert_eq!(granted(&format!("email {SCOPE_YOUTUBE_LIVE}")), ["youtube", "live"]);
    }

    /// A stand-in for Google: tokens, and resumable uploads that sometimes
    /// fail part-way.
    pub struct Fake {
        pub port: u16,
        pub log: Arc<Mutex<Vec<String>>>,
        /// Uploads by session, and their metadata.
        pub files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
        pub meta: Arc<Mutex<HashMap<String, String>>>,
    }

    pub fn fake_google(fail_every: usize) -> Fake {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let log = Arc::new(Mutex::new(Vec::new()));
        let files: Arc<Mutex<HashMap<String, Vec<u8>>>> = Arc::new(Mutex::new(HashMap::new()));
        let meta: Arc<Mutex<HashMap<String, String>>> = Arc::new(Mutex::new(HashMap::new()));
        let (l2, f2, m2) = (log.clone(), files.clone(), meta.clone());
        let mut folders = 0usize;
        std::thread::spawn(move || {
            let mut puts = 0usize;
            let mut tokens = 0usize;
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
                let header = |k: &str| head.lines().find_map(|l| l.split_once(':').filter(|(a, _)| a.trim().eq_ignore_ascii_case(k)).map(|(_, v)| v.trim().to_string()));
                let len: usize = header("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
                while data.len() < head_end + len {
                    let n = c.read(&mut buf).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    data.extend_from_slice(&buf[..n]);
                }
                let body = data[head_end..].to_vec();
                let mut first = head.lines().next().unwrap_or("").split_whitespace();
                let (method, target) = (first.next().unwrap_or("").to_string(), first.next().unwrap_or("").to_string());
                l2.lock_ok().push(format!("{method} {}", target.split('?').next().unwrap_or("")));
                let send = |c: &mut TcpStream, status: &str, extra: &str, body: &str| {
                    let _ = write!(c, "HTTP/1.1 {status}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                };
                let auth = header("authorization").unwrap_or_default();
                match (method.as_str(), target.split('?').next().unwrap_or("")) {
                    ("POST", "/token") => {
                        tokens += 1;
                        let b = String::from_utf8_lossy(&body).to_string();
                        if b.contains("grant_type=authorization_code") {
                            assert!(b.contains("code_verifier=") && b.contains("code=thecode"), "{b}");
                            send(&mut c, "200 OK", "Content-Type: application/json\r\n", r#"{"access_token":"a1","refresh_token":"r1","expires_in":3600,"scope":"openid email"}"#);
                        } else if b.contains("refresh_token=bad") {
                            send(&mut c, "400 Bad Request", "", r#"{"error":"invalid_grant","error_description":"Token has been expired or revoked."}"#);
                        } else {
                            send(&mut c, "200 OK", "", &format!(r#"{{"access_token":"a{tokens}","expires_in":3600}}"#));
                        }
                    }
                    _ if !auth.starts_with("Bearer a") => send(&mut c, "401 Unauthorized", "", "{}"),
                    ("GET", "/drive") => send(&mut c, "200 OK", "", r#"{"files":[]}"#),
                    ("POST", "/drive") => {
                        folders += 1;
                        let v: Value = serde_json::from_slice(&body).unwrap();
                        assert_eq!(v["mimeType"], "application/vnd.google-apps.folder");
                        m2.lock_ok().insert(format!("folder{folders}"), v.to_string());
                        send(&mut c, "200 OK", "", &format!(r#"{{"id":"folder{folders}"}}"#));
                    }
                    ("POST", "/upload") => {
                        let id = format!("s{}", f2.lock_ok().len() + 1);
                        f2.lock_ok().insert(id.clone(), Vec::new());
                        m2.lock_ok().insert(id.clone(), String::from_utf8_lossy(&body).to_string());
                        send(&mut c, "200 OK", &format!("Location: http://127.0.0.1:{port}/session/{id}\r\n"), "");
                    }
                    ("PUT", t) if t.starts_with("/session/") => {
                        let id = t.trim_start_matches("/session/").to_string();
                        let range = header("content-range").unwrap_or_default();
                        let total: usize = range.rsplit('/').next().unwrap().parse().unwrap();
                        let mut files = f2.lock_ok();
                        let have = files.get_mut(&id).unwrap();
                        if !range.contains('*') {
                            puts += 1;
                            // Now and then: take half the piece, then fail.
                            if fail_every > 0 && puts % fail_every == 0 {
                                let start: usize = range.trim_start_matches("bytes ").split('-').next().unwrap().parse().unwrap();
                                assert_eq!(start, have.len());
                                have.extend_from_slice(&body[..body.len() / 2]);
                                drop(files);
                                send(&mut c, "503 Service Unavailable", "", "");
                                continue;
                            }
                            let start: usize = range.trim_start_matches("bytes ").split('-').next().unwrap().parse().unwrap();
                            assert_eq!(start, have.len(), "pieces in order, no gaps");
                            have.extend_from_slice(&body);
                        }
                        if have.len() >= total {
                            send(&mut c, "200 OK", "", &format!(r#"{{"id":"vid-{id}","webViewLink":"https://drive/{id}"}}"#));
                        } else {
                            let r = if have.is_empty() { String::new() } else { format!("Range: bytes=0-{}\r\n", have.len() - 1) };
                            send(&mut c, "308 Resume Incomplete", &r, "");
                        }
                    }
                    _ => send(&mut c, "404 Not Found", "", "{}"),
                }
            }
        });
        Fake { port, log, files, meta }
    }

    pub fn client(port: u16) -> Client {
        let base = format!("http://127.0.0.1:{port}");
        Client {
            id: "client".into(),
            secret: "secret".into(),
            ep: Endpoints {
                auth: format!("{base}/auth"),
                token: format!("{base}/token"),
                userinfo: format!("{base}/userinfo"),
                youtube_upload: format!("{base}/upload?uploadType=resumable"),
                youtube_api: format!("{base}/yt"),
                youtube_thumbnails: format!("{base}/thumbs"),
                drive: format!("{base}/drive"),
                drive_upload: format!("{base}/upload?uploadType=resumable"),
            },
        }
    }

    #[test]
    fn signs_in_through_the_browser() {
        let g = fake_google(0);
        let c = client(g.port);
        let s = SignIn::begin(&c, &scopes(true, true, false)).unwrap();
        assert!(s.url.contains("code_challenge_method=S256") && s.url.contains("access_type=offline") && s.url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A"));
        let state = s.url.split("state=").nth(1).unwrap().to_string();
        let redirect = s.redirect.clone();
        // The browser: a favicon first, then Google's answer.
        let browser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            let get = |path: &str| {
                let addr = redirect.trim_start_matches("http://").to_string();
                let mut conn = TcpStream::connect(addr).unwrap();
                write!(conn, "GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
                let mut s = String::new();
                let _ = conn.read_to_string(&mut s);
                s
            };
            get("/favicon.ico");
            get(&format!("/?state={state}&code=thecode&scope=email"))
        });
        let t = s.finish(&c, Duration::from_secs(10), &AtomicBool::new(false)).unwrap();
        assert!(browser.join().unwrap().contains("Signed in"));
        assert_eq!((t.access.as_str(), t.refresh.as_str()), ("a1", "r1"));
        // A stranger's answer is refused.
        let s = SignIn::begin(&c, "email").unwrap();
        let redirect = s.redirect.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            let mut conn = TcpStream::connect(redirect.trim_start_matches("http://")).unwrap();
            write!(conn, "GET /?state=wrong&code=x HTTP/1.1\r\n\r\n").unwrap();
        });
        assert!(s.finish(&c, Duration::from_secs(10), &AtomicBool::new(false)).is_err());
        // A revoked sign-in says so.
        let mut a = Auth::new(c, "bad".into());
        assert!(format!("{:#}", a.token().unwrap_err()).contains("sign in again"));
    }

    #[test]
    fn uploads_resume_after_failures() {
        let g = fake_google(3);
        let c = client(g.port);
        let path = std::env::temp_dir().join(format!("cutaway-upload-{}.bin", std::process::id()));
        let data: Vec<u8> = (0..1_300_000u32).map(|i| (i * 7 % 251) as u8).collect();
        std::fs::write(&path, &data).unwrap();
        let mut auth = Auth::new(c.clone(), "r1".into());
        let mut seen = Vec::new();
        let meta = youtube_metadata("Show – 20 October 2026", "0:00 Start", "private");
        let v = resumable(&mut auth, &c.ep.youtube_upload, &meta, &path, "video/mp4", 256 * 1024, &mut |s, t| seen.push((s, t)), &AtomicBool::new(false)).unwrap();
        assert_eq!(v["id"], "vid-s1");
        assert_eq!(g.files.lock_ok()["s1"], data, "every byte, once, in order");
        assert_eq!(seen.last(), Some(&(data.len() as u64, data.len() as u64)));
        let log = g.log.lock_ok().clone();
        assert!(log.iter().filter(|l| l.starts_with("PUT")).count() > 6, "pieces, and some asked again: {log:?}");
        // Cancelled: stops.
        let cancel = AtomicBool::new(true);
        assert!(resumable(&mut auth, &c.ep.youtube_upload, &meta, &path, "video/mp4", 256 * 1024, &mut |_, _| {}, &cancel).is_err());
        let _ = std::fs::remove_file(&path);
    }
}
