//! Viewer comments: reading a live chat so the producer can pick comments
//! to show on screen.
//!
//! - Twitch: its chat is IRC; reading it needs no account (anonymous
//!   "justinfan" log-in).
//! - YouTube: the YouTube Data API's live chat messages, with a free API
//!   key, polled at the rate YouTube asks for.

use crate::util::LockExt;
use anyhow::{anyhow, bail, Context, Result};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How many messages are kept.
const KEEP: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Twitch,
    YouTube,
}

impl Platform {
    pub fn name(self) -> &'static str {
        match self {
            Platform::Twitch => "Twitch",
            Platform::YouTube => "YouTube",
        }
    }
    /// The colour for a name that doesn't have its own.
    pub fn color(self) -> &'static str {
        match self {
            Platform::Twitch => "#a970ff",
            Platform::YouTube => "#ff4e45",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChatMessage {
    /// Ours, counting up.
    pub id: u64,
    pub platform: Platform,
    pub author: String,
    pub text: String,
    /// "#RRGGBB" if the chat gave the name a colour.
    pub color: Option<String>,
    pub at: Instant,
}

#[derive(Default)]
struct State {
    messages: VecDeque<ChatMessage>,
    twitch: String,
    youtube: String,
}

pub struct Chat {
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
    #[cfg_attr(not(test), allow(dead_code))]
    next_id: Arc<AtomicU64>,
    /// What it was started with.
    pub made_for: (String, String, String),
}

impl Drop for Chat {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

impl Chat {
    pub fn start(cfg: &crate::config::CommentsConfig, waker: Arc<dyn Fn() + Send + Sync>) -> Chat {
        let state = Arc::new(Mutex::new(State::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let next_id = Arc::new(AtomicU64::new(1));
        let mut threads = Vec::new();
        let channel = cfg.twitch_channel.trim().trim_start_matches('#').to_lowercase();
        if !channel.is_empty() {
            let (st, s2, ids, w) = (state.clone(), stop.clone(), next_id.clone(), waker.clone());
            state.lock_ok().twitch = "connecting…".into();
            if let Ok(t) = std::thread::Builder::new().name("twitch chat".into()).spawn(move || twitch(&channel, st, s2, ids, w)) {
                threads.push(t);
            }
        }
        let video = youtube_video_id(&cfg.youtube);
        let key = cfg.youtube_api_key.trim().to_string();
        if let Some(video) = video.filter(|_| !key.is_empty()) {
            let (st, s2, ids, w) = (state.clone(), stop.clone(), next_id.clone(), waker);
            state.lock_ok().youtube = "connecting…".into();
            if let Ok(t) = std::thread::Builder::new().name("youtube chat".into()).spawn(move || youtube(&video, &key, st, s2, ids, w)) {
                threads.push(t);
            }
        } else if !cfg.youtube.trim().is_empty() {
            state.lock_ok().youtube = if key.is_empty() { "needs a YouTube API key".into() } else { "that isn't a YouTube video address".into() };
        }
        Chat { state, stop, threads, next_id, made_for: (cfg.twitch_channel.clone(), cfg.youtube.clone(), cfg.youtube_api_key.clone()) }
    }

    /// Newest last.
    pub fn messages(&self) -> Vec<ChatMessage> {
        self.state.lock_ok().messages.iter().cloned().collect()
    }

    /// (Twitch, YouTube) connection states ("" when not used).
    pub fn status(&self) -> (String, String) {
        let st = self.state.lock_ok();
        (st.twitch.clone(), st.youtube.clone())
    }

    /// For tests and the UI test: a message as if it came from the chat.
    pub fn inject(&self, platform: Platform, author: &str, text: &str) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        add(&self.state, ChatMessage { id, platform, author: author.into(), text: text.into(), color: None, at: Instant::now() });
    }
}

fn add(state: &Arc<Mutex<State>>, m: ChatMessage) {
    let mut st = state.lock_ok();
    st.messages.push_back(m);
    while st.messages.len() > KEEP {
        st.messages.pop_front();
    }
}

// ---- Twitch ------------------------------------------------------------------

/// A chat line from Twitch IRC: (name, colour, text) for a message.
pub fn parse_twitch(line: &str) -> Option<(String, Option<String>, String)> {
    // @badge-info=;color=#FF4500;display-name=Viewer :viewer!viewer@viewer.tmi.twitch.tv PRIVMSG #chan :hello
    let (tags, rest) = match line.strip_prefix('@') {
        Some(l) => l.split_once(' ')?,
        None => ("", line),
    };
    let rest = rest.trim_start();
    let (prefix, rest) = rest.strip_prefix(':')?.split_once(' ')?;
    let (cmd, rest) = rest.split_once(' ')?;
    if cmd != "PRIVMSG" {
        return None;
    }
    let text = rest.split_once(" :")?.1.trim_end_matches(['\r', '\n']);
    // "/me waves" arrives as an ACTION.
    let text = text.strip_prefix("\u{1}ACTION ").map(|t| t.trim_end_matches('\u{1}')).unwrap_or(text);
    let mut name = prefix.split('!').next().unwrap_or("").to_string();
    let mut color = None;
    for t in tags.split(';') {
        if let Some((k, v)) = t.split_once('=') {
            match k {
                "display-name" if !v.is_empty() => name = v.replace("\\s", " "),
                "color" if v.starts_with('#') && v.len() == 7 => color = Some(v.to_string()),
                _ => {}
            }
        }
    }
    Some((name, color, text.to_string()))
}

fn twitch(channel: &str, state: Arc<Mutex<State>>, stop: Arc<AtomicBool>, ids: Arc<AtomicU64>, waker: Arc<dyn Fn() + Send + Sync>) {
    let set = |s: String| {
        state.lock_ok().twitch = s;
        waker();
    };
    let mut wait = 2;
    while !stop.load(Ordering::Relaxed) {
        match twitch_session(channel, &state, &stop, &ids, &waker, &set) {
            Ok(()) => wait = 2,
            Err(e) => set(format!("{e:#}")),
        }
        for _ in 0..wait * 10 {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        wait = (wait * 2).min(60);
    }
}

fn twitch_session(channel: &str, state: &Arc<Mutex<State>>, stop: &AtomicBool, ids: &AtomicU64, waker: &Arc<dyn Fn() + Send + Sync>, set: &dyn Fn(String)) -> Result<()> {
    let addr = std::net::ToSocketAddrs::to_socket_addrs("irc.chat.twitch.tv:6667").context("couldn't find Twitch's chat server")?.next().ok_or_else(|| anyhow!("no address for Twitch chat"))?;
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(10)).context("couldn't reach Twitch chat")?;
    s.set_read_timeout(Some(Duration::from_millis(500)))?;
    let nick = format!("justinfan{}", 10_000 + std::process::id() % 80_000);
    write!(s, "CAP REQ :twitch.tv/tags\r\nPASS SCHMOOPIIE\r\nNICK {nick}\r\nJOIN #{channel}\r\n")?;
    let mut reader = BufReader::new(s.try_clone()?);
    let mut line = String::new();
    let mut joined = false;
    while !stop.load(Ordering::Relaxed) {
        // A timeout can leave half a line read: keep it for the rest.
        if line.ends_with('\n') {
            line.clear();
        }
        match reader.read_line(&mut line) {
            Ok(0) => bail!("Twitch closed the chat connection"),
            Ok(_) => {}
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => continue,
            Err(e) => return Err(e.into()),
        }
        if !line.ends_with('\n') {
            continue;
        }
        let l = line.trim_end();
        if let Some(arg) = l.strip_prefix("PING") {
            write!(s, "PONG{arg}\r\n")?;
            continue;
        }
        if !joined && (l.contains(" 366 ") || l.contains(" JOIN #")) {
            joined = true;
            set(format!("reading #{channel}"));
        }
        if l.contains(" NOTICE ") && l.to_lowercase().contains("login") {
            bail!("Twitch refused the connection");
        }
        if let Some((author, color, text)) = parse_twitch(l) {
            let id = ids.fetch_add(1, Ordering::Relaxed);
            add(state, ChatMessage { id, platform: Platform::Twitch, author, text, color, at: Instant::now() });
            waker();
        }
    }
    Ok(())
}

// ---- YouTube -----------------------------------------------------------------

/// The video ID in a YouTube address (or the ID itself).
pub fn youtube_video_id(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let valid = |id: &str| id.len() == 11 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if valid(s) {
        return Some(s.to_string());
    }
    let after = |marker: &str| s.split_once(marker).map(|(_, r)| r.split(['&', '?', '/', '#']).next().unwrap_or("").to_string());
    for m in ["v=", "youtu.be/", "/live/", "/shorts/", "/embed/"] {
        if let Some(id) = after(m).filter(|id| valid(id)) {
            return Some(id);
        }
    }
    None
}

fn youtube(video: &str, key: &str, state: Arc<Mutex<State>>, stop: Arc<AtomicBool>, ids: Arc<AtomicU64>, waker: Arc<dyn Fn() + Send + Sync>) {
    let set = |s: String| {
        state.lock_ok().youtube = s;
        waker();
    };
    let agent = crate::util::https_agent(Duration::from_secs(20));
    let sleep = |d: Duration| {
        let until = Instant::now() + d;
        while Instant::now() < until && !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(100));
        }
    };
    let get = |url: &str| -> Result<serde_json::Value> {
        let mut resp = agent.get(url).call().map_err(|e| anyhow!("couldn't reach YouTube ({e})"))?;
        let status = resp.status().as_u16();
        let text = resp.body_mut().read_to_string().map_err(|e| anyhow!("YouTube's answer was cut off ({e})"))?;
        let v: serde_json::Value = serde_json::from_str(&text).map_err(|_| anyhow!("YouTube answered {status}"))?;
        if let Some(msg) = v.pointer("/error/message").and_then(|m| m.as_str()) {
            bail!("YouTube: {msg}");
        }
        Ok(v)
    };
    while !stop.load(Ordering::Relaxed) {
        // The stream's chat (only while it's live).
        let url = format!("https://www.googleapis.com/youtube/v3/videos?part=liveStreamingDetails&id={video}&key={key}");
        let chat_id = match get(&url).map(|v| v.pointer("/items/0/liveStreamingDetails/activeLiveChatId").and_then(|c| c.as_str()).map(str::to_string)) {
            Ok(Some(c)) => c,
            Ok(None) => {
                set("waiting for the stream to go live".into());
                sleep(Duration::from_secs(20));
                continue;
            }
            Err(e) => {
                set(format!("{e:#}"));
                sleep(Duration::from_secs(30));
                continue;
            }
        };
        set("reading the live chat".into());
        let mut page: Option<String> = None;
        let mut first = true;
        while !stop.load(Ordering::Relaxed) {
            let mut url = format!("https://www.googleapis.com/youtube/v3/liveChat/messages?liveChatId={chat_id}&part=snippet,authorDetails&maxResults=200&key={key}");
            if let Some(p) = &page {
                url.push_str(&format!("&pageToken={p}"));
            }
            let v = match get(&url) {
                Ok(v) => v,
                Err(e) => {
                    set(format!("{e:#}"));
                    sleep(Duration::from_secs(15));
                    break;
                }
            };
            page = v.get("nextPageToken").and_then(|p| p.as_str()).map(str::to_string);
            // The first page is the history: start from now.
            if !first {
                for (author, text) in youtube_messages(&v) {
                    let id = ids.fetch_add(1, Ordering::Relaxed);
                    add(&state, ChatMessage { id, platform: Platform::YouTube, author, text, color: None, at: Instant::now() });
                    waker();
                }
            }
            first = false;
            let ms = v.get("pollingIntervalMillis").and_then(|m| m.as_u64()).unwrap_or(5000).clamp(2000, 30_000);
            sleep(Duration::from_millis(ms));
        }
    }
}

/// (author, text) of the chat messages in a liveChatMessages answer.
pub fn youtube_messages(v: &serde_json::Value) -> Vec<(String, String)> {
    let Some(items) = v.get("items").and_then(|i| i.as_array()) else { return Vec::new() };
    items
        .iter()
        .filter(|m| m.pointer("/snippet/type").and_then(|t| t.as_str()).is_none_or(|t| t == "textMessageEvent" || t == "superChatEvent"))
        .filter_map(|m| {
            let author = m.pointer("/authorDetails/displayName")?.as_str()?.to_string();
            let text = m.pointer("/snippet/displayMessage")?.as_str()?.to_string();
            Some((author, text))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_twitch_lines() {
        let l = "@badge-info=;badges=;color=#FF4500;display-name=Cool\\sViewer;emotes= :coolviewer!coolviewer@coolviewer.tmi.twitch.tv PRIVMSG #show :Hello from Perth!";
        assert_eq!(parse_twitch(l), Some(("Cool Viewer".into(), Some("#FF4500".into()), "Hello from Perth!".into())));
        let me = ":someone!someone@someone.tmi.twitch.tv PRIVMSG #show :\u{1}ACTION waves\u{1}";
        assert_eq!(parse_twitch(me), Some(("someone".into(), None, "waves".into())));
        assert_eq!(parse_twitch(":tmi.twitch.tv 001 justinfan123 :Welcome, GLHF!"), None);
        assert_eq!(parse_twitch("PING :tmi.twitch.tv"), None);
    }

    #[test]
    fn finds_youtube_videos_and_messages() {
        assert_eq!(youtube_video_id("https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=3"), Some("dQw4w9WgXcQ".into()));
        assert_eq!(youtube_video_id("https://youtu.be/dQw4w9WgXcQ"), Some("dQw4w9WgXcQ".into()));
        assert_eq!(youtube_video_id("https://www.youtube.com/live/dQw4w9WgXcQ?si=x"), Some("dQw4w9WgXcQ".into()));
        assert_eq!(youtube_video_id("dQw4w9WgXcQ"), Some("dQw4w9WgXcQ".into()));
        assert_eq!(youtube_video_id("not a video"), None);
        let v: serde_json::Value = serde_json::from_str(
            r#"{"items":[{"snippet":{"type":"textMessageEvent","displayMessage":"First!"},"authorDetails":{"displayName":"Ana"}},
                          {"snippet":{"type":"newSponsorEvent","displayMessage":"joined"},"authorDetails":{"displayName":"Bo"}}],
                "pollingIntervalMillis":5000}"#,
        )
        .unwrap();
        assert_eq!(youtube_messages(&v), vec![("Ana".to_string(), "First!".to_string())]);
    }

    #[test]
    fn keeps_recent_messages() {
        let chat = Chat::start(&Default::default(), Arc::new(|| {}));
        assert_eq!(chat.status(), (String::new(), String::new()));
        for i in 0..KEEP + 5 {
            chat.inject(Platform::Twitch, "a", &format!("{i}"));
        }
        let m = chat.messages();
        assert_eq!(m.len(), KEEP);
        assert_eq!(m.last().unwrap().text, format!("{}", KEEP + 4));
        let yt = Chat::start(&crate::config::CommentsConfig { youtube: "https://youtu.be/dQw4w9WgXcQ".into(), ..Default::default() }, Arc::new(|| {}));
        assert_eq!(yt.status().1, "needs a YouTube API key");
    }
}
