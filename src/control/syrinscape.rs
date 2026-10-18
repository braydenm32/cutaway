//! Syrinscape Online control: start and stop moods and sounds, stop
//! everything, set the volume, and browse your soundsets, through
//! Syrinscape's web interface with your auth token. Syrinscape's own
//! player makes the sound; Cutaway hears it through an audio input.
//!
//! Requests run on a background thread, so a slow connection never holds
//! up the show. Syrinscape doesn't say what's playing, so Cutaway keeps
//! track of what it started itself.

use crate::config::{SyrinButton, SyrinKind, SyrinscapeConfig};
use anyhow::{anyhow, bail, Result};
use crossbeam_channel::{unbounded, Sender};
use serde_json::Value;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const API: &str = "https://syrinscape.com/online/frontend-api";

/// A soundset you own.
#[derive(Clone, Debug, PartialEq)]
pub struct Soundset {
    pub uuid: String,
    pub name: String,
}

/// A mood or element in a soundset.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub kind: SyrinKind,
    pub id: u64,
    pub name: String,
    /// "music", "sfx", "oneshot"… when Syrinscape says.
    pub what: String,
}

/// What the panel shows and the Stream Deck reads.
#[derive(Clone, Debug, Default)]
pub struct SyrinState {
    /// The mood Cutaway last started (Syrinscape plays one at a time).
    pub mood: Option<u64>,
    /// Elements Cutaway started and hasn't stopped.
    pub elements: HashSet<u64>,
    pub volume: Option<f64>,
    /// The last thing that went wrong (cleared by the next success).
    pub problem: Option<String>,
    pub busy: bool,
    /// Browsing: soundsets, and the items of the one opened.
    pub soundsets: Option<Result<Vec<Soundset>, String>>,
    pub items: Option<(String, Result<Vec<Item>, String>)>,
}

enum Job {
    Play(SyrinKind, u64),
    Stop(SyrinKind, u64),
    StopAll,
    Volume(f64),
    /// Check the token works (reads the volumes).
    Check,
    Soundsets,
    Items(String),
}

pub struct Syrinscape {
    token: Arc<Mutex<String>>,
    pub state: Arc<Mutex<SyrinState>>,
    jobs: Sender<Job>,
}

impl Syrinscape {
    pub fn new(cfg: &SyrinscapeConfig, waker: Arc<dyn Fn() + Send + Sync>) -> Syrinscape {
        let token = Arc::new(Mutex::new(cfg.token.trim().to_string()));
        let state = Arc::new(Mutex::new(SyrinState::default()));
        let (tx, rx) = unbounded::<Job>();
        let (t2, s2) = (token.clone(), state.clone());
        let _ = std::thread::Builder::new().name("syrinscape".into()).spawn(move || {
            let agent = crate::util::https_agent(Duration::from_secs(15));
            while let Ok(job) = rx.recv() {
                s2.lock().unwrap().busy = true;
                let token = t2.lock().unwrap().clone();
                let r = run(&agent, &token, &job, &s2);
                let mut st = s2.lock().unwrap();
                st.busy = false;
                match r {
                    Ok(()) => st.problem = None,
                    Err(e) => st.problem = Some(format!("{e:#}")),
                }
                drop(st);
                waker();
            }
        });
        Syrinscape { token, state, jobs: tx }
    }

    pub fn set_token(&self, token: &str) {
        *self.token.lock().unwrap() = token.trim().to_string();
    }

    pub fn has_token(&self) -> bool {
        !self.token.lock().unwrap().is_empty()
    }

    fn send(&self, job: Job) -> Result<(), String> {
        if !self.has_token() && !matches!(job, Job::Check) {
            return Err("Syrinscape isn't set up: add your auth token in Settings → Syrinscape".into());
        }
        self.jobs.send(job).map_err(|_| "Syrinscape control stopped".to_string())
    }

    /// Press a button: start it, or stop it if it's an element that's playing.
    pub fn press(&self, b: &SyrinButton) -> Result<String, String> {
        match b.kind {
            SyrinKind::StopAll => {
                self.send(Job::StopAll)?;
                Ok("Syrinscape: stopped everything".into())
            }
            SyrinKind::Element if self.is_playing(b) => {
                self.send(Job::Stop(SyrinKind::Element, b.id))?;
                Ok(format!("Syrinscape: stopped {}", b.name))
            }
            k => {
                self.send(Job::Play(k, b.id))?;
                Ok(format!("Syrinscape: {}", b.name))
            }
        }
    }

    pub fn play(&self, kind: SyrinKind, id: u64) -> Result<(), String> {
        self.send(Job::Play(kind, id))
    }

    pub fn stop(&self, kind: SyrinKind, id: u64) -> Result<(), String> {
        self.send(Job::Stop(kind, id))
    }

    pub fn stop_all(&self) -> Result<(), String> {
        self.send(Job::StopAll)
    }

    /// Overall volume, 0 to 1.5 (1 = normal).
    pub fn set_volume(&self, v: f64) -> Result<(), String> {
        self.send(Job::Volume(v.clamp(0.0, 1.5)))
    }

    pub fn check(&self) {
        let _ = self.send(Job::Check);
    }

    pub fn load_soundsets(&self) {
        self.state.lock().unwrap().soundsets = None;
        let _ = self.send(Job::Soundsets);
    }

    pub fn load_items(&self, soundset: &str) {
        self.state.lock().unwrap().items = None;
        let _ = self.send(Job::Items(soundset.to_string()));
    }

    pub fn is_playing(&self, b: &SyrinButton) -> bool {
        let st = self.state.lock().unwrap();
        match b.kind {
            SyrinKind::Mood => st.mood == Some(b.id),
            SyrinKind::Element => st.elements.contains(&b.id),
            SyrinKind::StopAll => false,
        }
    }
}

fn post(agent: &ureq::Agent, token: &str, path: &str) -> Result<()> {
    let resp = agent
        .post(format!("{API}/{path}"))
        .header("Authorization", format!("token {token}"))
        .header("Accept", "application/json")
        .send_empty()
        .map_err(|e| anyhow!("couldn't reach Syrinscape ({e})"))?;
    check_status(resp.status().as_u16())
}

fn get(agent: &ureq::Agent, token: &str, path: &str) -> Result<Value> {
    let mut resp = agent
        .get(format!("{API}/{path}"))
        .header("Authorization", format!("token {token}"))
        .header("Accept", "application/json")
        .call()
        .map_err(|e| anyhow!("couldn't reach Syrinscape ({e})"))?;
    check_status(resp.status().as_u16())?;
    let text = resp.body_mut().read_to_string().map_err(|e| anyhow!("Syrinscape's answer was cut off ({e})"))?;
    serde_json::from_str(&text).map_err(|_| anyhow!("Syrinscape answered with something unexpected"))
}

fn check_status(code: u16) -> Result<()> {
    match code {
        200..=299 => Ok(()),
        401 | 403 => bail!("Syrinscape didn't accept the auth token (check it in Settings → Syrinscape)"),
        404 => bail!("Syrinscape doesn't know that mood or sound (was it removed?)"),
        429 => bail!("Syrinscape asked Cutaway to slow down; try again in a moment"),
        c => bail!("Syrinscape answered with error {c}"),
    }
}

fn kind_path(kind: SyrinKind) -> &'static str {
    match kind {
        SyrinKind::Element => "elements",
        _ => "moods",
    }
}

fn run(agent: &ureq::Agent, token: &str, job: &Job, state: &Mutex<SyrinState>) -> Result<()> {
    if token.is_empty() {
        bail!("there's no auth token yet (Settings → Syrinscape)");
    }
    match job {
        Job::Play(kind, id) => {
            post(agent, token, &format!("{}/{id}/play/", kind_path(*kind)))?;
            let mut st = state.lock().unwrap();
            match kind {
                SyrinKind::Mood => st.mood = Some(*id),
                _ => {
                    st.elements.insert(*id);
                }
            }
        }
        Job::Stop(kind, id) => {
            post(agent, token, &format!("{}/{id}/stop/", kind_path(*kind)))?;
            let mut st = state.lock().unwrap();
            match kind {
                SyrinKind::Mood if st.mood == Some(*id) => st.mood = None,
                _ => {
                    st.elements.remove(id);
                }
            }
        }
        Job::StopAll => {
            post(agent, token, "stop-all/")?;
            let mut st = state.lock().unwrap();
            st.mood = None;
            st.elements.clear();
        }
        Job::Volume(v) => {
            post(agent, token, &format!("global-volume/{v:.2}/"))?;
            state.lock().unwrap().volume = Some(*v);
        }
        Job::Check => {
            let v = get(agent, token, "state/")?;
            state.lock().unwrap().volume = find_number(&v, &["global_volume", "volume", "globalVolume"]);
        }
        Job::Soundsets => {
            let r = get(agent, token, "soundsets/").map(|v| parse_soundsets(&v));
            state.lock().unwrap().soundsets = Some(r.as_ref().map(|s| s.clone()).map_err(|e| format!("{e:#}")));
            r?;
        }
        Job::Items(uuid) => {
            let r = (|| -> Result<Vec<Item>> {
                let mut items = parse_items(&get(agent, token, &format!("moods/?soundset__uuid={uuid}"))?, SyrinKind::Mood);
                items.extend(parse_items(&get(agent, token, &format!("elements/?soundset__uuid={uuid}"))?, SyrinKind::Element));
                Ok(items)
            })();
            state.lock().unwrap().items = Some((uuid.clone(), r.as_ref().map(|s| s.clone()).map_err(|e| format!("{e:#}"))));
            r?;
        }
    }
    Ok(())
}

/// A list, whether plain or paged ({"results": [...]}).
fn list(v: &Value) -> Vec<&Value> {
    match v {
        Value::Array(a) => a.iter().collect(),
        Value::Object(o) => ["results", "items", "data", "soundsets", "moods", "elements"]
            .iter()
            .find_map(|k| o.get(*k).and_then(|x| x.as_array()))
            .map(|a| a.iter().collect())
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn find_str(v: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| v.get(*k).and_then(|x| x.as_str()).map(|s| s.trim().to_string())).filter(|s| !s.is_empty())
}

fn find_number(v: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|k| v.get(*k).and_then(|x| x.as_f64().or_else(|| x.as_str().and_then(|s| s.parse().ok()))))
}

fn find_id(v: &Value) -> Option<u64> {
    ["id", "pk"].iter().find_map(|k| v.get(*k).and_then(|x| x.as_u64().or_else(|| x.as_str().and_then(|s| s.parse().ok()))))
}

fn parse_soundsets(v: &Value) -> Vec<Soundset> {
    let mut out: Vec<Soundset> = list(v)
        .into_iter()
        .filter_map(|s| {
            let uuid = find_str(s, &["uuid", "soundset_uuid", "id"])?;
            let name = find_str(s, &["name", "title", "full_name"]).unwrap_or_else(|| uuid.clone());
            Some(Soundset { uuid, name })
        })
        .collect();
    out.sort_by_key(|s| s.name.to_lowercase());
    out
}

fn parse_items(v: &Value, kind: SyrinKind) -> Vec<Item> {
    list(v)
        .into_iter()
        .filter_map(|x| {
            let id = find_id(x)?;
            let name = find_str(x, &["name", "title"]).unwrap_or_else(|| format!("#{id}"));
            let what = find_str(x, &["element_type", "type", "kind", "elementType"]).unwrap_or_default();
            Some(Item { kind, id, name, what: what.replace("_element", "").replace('_', " ") })
        })
        .collect()
}

/// A Syrinscape remote-control link ("…/moods/284437/play/…" or
/// "…/elements/1541818/play/") as what it starts.
pub fn parse_link(link: &str) -> Option<(SyrinKind, u64)> {
    let l = link.trim().to_lowercase();
    for (word, kind) in [("/moods/", SyrinKind::Mood), ("/elements/", SyrinKind::Element)] {
        if let Some(i) = l.find(word) {
            let rest = &l[i + word.len()..];
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(id) = digits.parse() {
                return Some((kind, id));
            }
        }
    }
    if l.contains("stop-all") {
        return Some((SyrinKind::StopAll, 0));
    }
    None
}

/// "syrinscape:…" commands: a button by number or name, or
/// mood:ID / element:ID / stop / stop:BUTTON / volume:0.8.
#[derive(Clone, Debug, PartialEq)]
pub enum SyrinCmd {
    Button(crate::control::Target),
    StopButton(crate::control::Target),
    Play(SyrinKind, u64),
    Stop(SyrinKind, u64),
    StopAll,
    Volume(f64),
}

impl SyrinCmd {
    pub fn parse(rest: &str) -> Result<SyrinCmd, String> {
        let r = rest.trim();
        let (head, tail) = r.split_once(':').map(|(a, b)| (a.trim().to_lowercase(), Some(b.trim()))).unwrap_or((r.to_lowercase(), None));
        let id = |t: Option<&str>| t.and_then(|x| x.parse::<u64>().ok()).ok_or_else(|| "needs a number, e.g. syrinscape:mood:284437".to_string());
        Ok(match (head.as_str(), tail) {
            ("mood", t) => SyrinCmd::Play(SyrinKind::Mood, id(t)?),
            ("element" | "sound", t) => SyrinCmd::Play(SyrinKind::Element, id(t)?),
            ("stop" | "stopall" | "stop-all", None) => SyrinCmd::StopAll,
            ("stop", Some(t)) => match t.split_once(':') {
                Some((k, n)) if k.trim().eq_ignore_ascii_case("mood") => SyrinCmd::Stop(SyrinKind::Mood, id(Some(n.trim()))?),
                Some((k, n)) if k.trim().eq_ignore_ascii_case("element") => SyrinCmd::Stop(SyrinKind::Element, id(Some(n.trim()))?),
                _ => SyrinCmd::StopButton(crate::control::Target::parse(t)),
            },
            ("volume" | "vol", Some(t)) => SyrinCmd::Volume(t.trim_end_matches('%').trim().parse::<f64>().map(|v| if v > 1.5 { v / 100.0 } else { v }).map_err(|_| format!("\"{t}\" isn't a volume (0 to 1.5)"))?),
            _ if r.is_empty() => return Err("syrinscape needs a button, e.g. syrinscape:1".into()),
            _ => SyrinCmd::Button(crate::control::Target::parse(r)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::Target;

    #[test]
    fn reads_links_and_commands() {
        assert_eq!(parse_link("https://syrinscape.com/online/frontend-api/moods/284437/play/?auth_token=X"), Some((SyrinKind::Mood, 284437)));
        assert_eq!(parse_link("https://www.syrinscape.com/online/frontend-api/elements/1541818/play/"), Some((SyrinKind::Element, 1541818)));
        assert_eq!(parse_link("…/frontend-api/stop-all/"), Some((SyrinKind::StopAll, 0)));
        assert_eq!(parse_link("hello"), None);
        assert_eq!(SyrinCmd::parse("2"), Ok(SyrinCmd::Button(Target::Index(1))));
        assert_eq!(SyrinCmd::parse("Tavern"), Ok(SyrinCmd::Button(Target::Name("Tavern".into()))));
        assert_eq!(SyrinCmd::parse("mood:19"), Ok(SyrinCmd::Play(SyrinKind::Mood, 19)));
        assert_eq!(SyrinCmd::parse("element:1541818"), Ok(SyrinCmd::Play(SyrinKind::Element, 1541818)));
        assert_eq!(SyrinCmd::parse("stop"), Ok(SyrinCmd::StopAll));
        assert_eq!(SyrinCmd::parse("stop:mood:19"), Ok(SyrinCmd::Stop(SyrinKind::Mood, 19)));
        assert_eq!(SyrinCmd::parse("stop:Rain"), Ok(SyrinCmd::StopButton(Target::Name("Rain".into()))));
        assert_eq!(SyrinCmd::parse("volume:0.8"), Ok(SyrinCmd::Volume(0.8)));
        assert_eq!(SyrinCmd::parse("volume:80%"), Ok(SyrinCmd::Volume(0.8)));
        assert!(SyrinCmd::parse("mood:abc").is_err());
    }

    #[test]
    fn reads_lists_however_they_come() {
        let plain: Value = serde_json::json!([{"uuid": "b-2", "name": "Tavern"}, {"uuid": "a-1", "name": "Forest"}]);
        assert_eq!(parse_soundsets(&plain).iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), vec!["Forest", "Tavern"]);
        let paged: Value = serde_json::json!({"count": 2, "next": null, "results": [{"pk": "19", "title": "Battle"}, {"id": 7, "name": "Rain", "element_type": "sfx_element"}]});
        let items = parse_items(&paged, SyrinKind::Element);
        assert_eq!(items[0], Item { kind: SyrinKind::Element, id: 19, name: "Battle".into(), what: String::new() });
        assert_eq!(items[1].what, "sfx");
        assert!(parse_items(&serde_json::json!({"oops": true}), SyrinKind::Mood).is_empty());
    }
}
