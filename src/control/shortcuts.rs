//! Keyboard shortcuts: the built-in ones (each can be moved to another key
//! or turned off) and your own, which run any command.

use super::Action;
use crate::config::KeysConfig;
use eframe::egui::{Key, Modifiers};

/// A key with the modifiers held for it. `command` is ⌘ on a Mac and Ctrl
/// elsewhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct KeyCombo {
    pub key: Key,
    pub command: bool,
    pub shift: bool,
    pub alt: bool,
}

impl KeyCombo {
    pub const fn plain(key: Key) -> KeyCombo {
        KeyCombo { key, command: false, shift: false, alt: false }
    }

    pub fn new(key: Key, m: Modifiers) -> KeyCombo {
        KeyCombo { key, command: m.command, shift: m.shift, alt: m.alt }
    }

    /// "Cmd+Shift+G", "F5", "Space" (Cmd, Ctrl and ⌘ all mean ⌘ on a Mac
    /// and Ctrl elsewhere; Alt and Option are the same).
    pub fn parse(s: &str) -> Option<KeyCombo> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }
        let (mut command, mut shift, mut alt) = (false, false, false);
        let mut key = None;
        // "+" on its own is the plus key ("Cmd++").
        let parts: Vec<&str> = if let Some(head) = s.strip_suffix("++") { head.split('+').chain(["+"]).collect() } else { s.split('+').collect() };
        for p in parts {
            let p = p.trim();
            match p.to_lowercase().as_str() {
                "cmd" | "command" | "ctrl" | "control" | "⌘" | "^" => command = true,
                "shift" | "⇧" => shift = true,
                "alt" | "option" | "opt" | "⌥" => alt = true,
                "" => {}
                _ => {
                    if key.is_some() {
                        return None;
                    }
                    key = Key::from_name(p).or_else(|| Key::from_name(&p.to_uppercase())).or_else(|| Key::from_name(&capitalized(p)));
                    key?;
                }
            }
        }
        let key = key?;
        if is_modifier(key) {
            return None;
        }
        Some(KeyCombo { key, command, shift, alt })
    }

    /// As stored: "Cmd+Shift+G".
    pub fn code(&self) -> String {
        let mut s = String::new();
        if self.command {
            s.push_str("Cmd+");
        }
        if self.alt {
            s.push_str("Alt+");
        }
        if self.shift {
            s.push_str("Shift+");
        }
        s.push_str(self.key.name());
        s
    }

    /// As people on this computer write it: "⌘⇧G" on a Mac, "Ctrl+Shift+G"
    /// on Windows.
    pub fn label(&self) -> String {
        let key = match self.key {
            Key::Comma => ",",
            Key::Period => ".",
            Key::Plus => "+",
            Key::Minus => "-",
            k => k.name(),
        };
        if cfg!(target_os = "macos") {
            let mut s = String::new();
            if self.alt {
                s.push('⌥');
            }
            if self.shift {
                s.push('⇧');
            }
            if self.command {
                s.push('⌘');
            }
            s.push_str(key);
            s
        } else {
            let mut s = String::new();
            if self.command {
                s.push_str("Ctrl+");
            }
            if self.alt {
                s.push_str("Alt+");
            }
            if self.shift {
                s.push_str("Shift+");
            }
            s.push_str(key);
            s
        }
    }

    /// Keys taken by the numbered shortcuts (scenes, graphics, pads), which
    /// stay where they are.
    pub fn is_numbered(&self) -> bool {
        let digit = DIGITS.contains(&self.key);
        let f = PAD_KEYS.contains(&self.key);
        (digit && !self.command && (!self.shift || !self.alt)) || (f && !self.command && !self.alt && !self.shift)
    }
}

/// Shift, Ctrl, Alt and ⌘ on their own: never a shortcut's key.
pub fn is_modifier(key: Key) -> bool {
    matches!(key, Key::ShiftLeft | Key::ShiftRight | Key::ControlLeft | Key::ControlRight | Key::AltLeft | Key::AltRight | Key::SuperLeft | Key::SuperRight)
}

fn capitalized(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase()).unwrap_or_default()
}

pub const DIGITS: [Key; 9] = [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9];
pub const PAD_KEYS: [Key; 12] = [Key::F1, Key::F2, Key::F3, Key::F4, Key::F5, Key::F6, Key::F7, Key::F8, Key::F9, Key::F10, Key::F11, Key::F12];

/// The shortcuts Cutaway comes with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Builtin {
    Settings,
    Undo,
    Redo,
    Record,
    Transition,
    Edit,
    Marker,
    Auto,
    Take,
    CutPreview,
    Captions,
    NextSegment,
    PreviousSegment,
    Clip,
    Talkback,
    /// Back to the scene on air before this one.
    Back,
    /// A marker now, then type its name.
    NamedMarker,
    /// ⌘K: search settings, pages and the desk.
    Search,
}

impl Builtin {
    pub const ALL: [Builtin; 18] = [
        Builtin::Take,
        Builtin::CutPreview,
        Builtin::Back,
        Builtin::Transition,
        Builtin::Auto,
        Builtin::Captions,
        Builtin::Marker,
        Builtin::NamedMarker,
        Builtin::Clip,
        Builtin::Talkback,
        Builtin::NextSegment,
        Builtin::PreviousSegment,
        Builtin::Edit,
        Builtin::Undo,
        Builtin::Redo,
        Builtin::Record,
        Builtin::Settings,
        Builtin::Search,
    ];

    /// Its name in the settings file.
    pub fn id(self) -> &'static str {
        match self {
            Builtin::Settings => "settings",
            Builtin::Undo => "undo",
            Builtin::Redo => "redo",
            Builtin::Record => "record",
            Builtin::Transition => "transition",
            Builtin::Edit => "edit",
            Builtin::Marker => "marker",
            Builtin::Auto => "auto",
            Builtin::Take => "take",
            Builtin::CutPreview => "cut",
            Builtin::Captions => "captions",
            Builtin::NextSegment => "next",
            Builtin::PreviousSegment => "previous",
            Builtin::Clip => "clip",
            Builtin::Talkback => "talkback",
            Builtin::Back => "back",
            Builtin::NamedMarker => "named_marker",
            Builtin::Search => "search",
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            Builtin::Settings => "Open the settings area you last had open",
            Builtin::Undo => "Undo a change to a scene, a title's text or a setting",
            Builtin::Redo => "Redo",
            Builtin::Record => "Start / stop recording",
            Builtin::Transition => "Switch between Cut and your transition",
            Builtin::Edit => "Edit the live layout",
            Builtin::Marker => "Add a marker while recording",
            Builtin::Auto => "Automatic switching on / off",
            Builtin::Take => "Take the preview scene to program (preview mode)",
            Builtin::CutPreview => "Cut the preview scene to program (preview mode)",
            Builtin::Captions => "Live captions on screen / off",
            Builtin::NextSegment => "Next rundown segment",
            Builtin::PreviousSegment => "Previous rundown segment",
            Builtin::Clip => "Save a clip of the last few seconds while recording",
            Builtin::Talkback => "Talkback: your mic in the presenters' ears while held",
            Builtin::Back => "Back to the last scene (the one on air before this)",
            Builtin::NamedMarker => "Add a marker and type its name while recording",
            Builtin::Search => "Search settings, pages and the desk (works while typing too)",
        }
    }

    pub fn defaults(self) -> Vec<KeyCombo> {
        let cmd = |key| KeyCombo { key, command: true, shift: false, alt: false };
        match self {
            Builtin::Settings => vec![cmd(Key::Comma)],
            Builtin::Undo => vec![cmd(Key::Z)],
            Builtin::Redo => {
                let mut v = vec![KeyCombo { key: Key::Z, command: true, shift: true, alt: false }];
                if !cfg!(target_os = "macos") {
                    v.push(cmd(Key::Y));
                }
                v
            }
            Builtin::Record => vec![cmd(Key::R)],
            Builtin::Transition => vec![KeyCombo::plain(Key::T)],
            Builtin::Edit => vec![KeyCombo::plain(Key::E)],
            Builtin::Marker => vec![KeyCombo::plain(Key::M)],
            Builtin::Auto => vec![KeyCombo::plain(Key::A)],
            Builtin::Take => vec![KeyCombo::plain(Key::Space)],
            Builtin::CutPreview => vec![KeyCombo::plain(Key::Enter)],
            Builtin::Captions => vec![KeyCombo::plain(Key::C)],
            Builtin::NextSegment => vec![KeyCombo::plain(Key::N)],
            Builtin::PreviousSegment => vec![KeyCombo { key: Key::N, command: false, shift: true, alt: false }],
            Builtin::Clip => vec![KeyCombo::plain(Key::K)],
            Builtin::Talkback => vec![KeyCombo::plain(Key::B)],
            Builtin::Back => vec![KeyCombo::plain(Key::Backspace)],
            Builtin::NamedMarker => vec![KeyCombo { key: Key::M, command: false, shift: true, alt: false }],
            Builtin::Search => vec![cmd(Key::K)],
        }
    }
}

/// What a key does.
#[derive(Clone, Debug, PartialEq)]
pub enum Binding {
    Builtin(Builtin),
    /// One of yours: a command (its text, as typed).
    Command(String),
}

/// Every shortcut, resolved from the settings.
#[derive(Clone, Debug, Default)]
pub struct Keymap {
    pub keys: Vec<(KeyCombo, Binding)>,
    /// Keys given two jobs, and other mistakes, in plain words.
    pub problems: Vec<String>,
}

/// The keys a built-in shortcut is on now: as set, or its defaults.
pub fn builtin_keys(cfg: &KeysConfig, b: Builtin) -> Vec<KeyCombo> {
    match cfg.builtin.get(b.id()) {
        Some(set) => split_keys(set).into_iter().filter_map(KeyCombo::parse).collect(),
        None => b.defaults(),
    }
}

/// "Cmd+R, F5" → ["Cmd+R", "F5"]. A comma where a key goes is the comma
/// key: "Cmd+," stays whole.
pub fn split_keys(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, c) in s.char_indices() {
        let part = s[start..i].trim();
        if c == ',' && !part.is_empty() && !part.ends_with('+') {
            out.push(part);
            start = i + 1;
        }
    }
    let last = s[start..].trim();
    if !last.is_empty() {
        out.push(last);
    }
    out
}

impl Keymap {
    pub fn new(cfg: &KeysConfig) -> Keymap {
        let mut map = Keymap::default();
        let add = |map: &mut Keymap, combo: KeyCombo, binding: Binding, what: String| {
            if combo.is_numbered() {
                map.problems.push(format!("{} is already used by the numbered shortcuts (scenes, graphics and pads), so it can't also be {what}", combo.label()));
                return;
            }
            if let Some((_, other)) = map.keys.iter().find(|(c, _)| *c == combo) {
                let other = match other {
                    Binding::Builtin(b) => b.describe().to_string(),
                    Binding::Command(c) => format!("\"{c}\""),
                };
                map.problems.push(format!("{} is set for both {other} and {what}: only the first works", combo.label()));
                return;
            }
            map.keys.push((combo, binding));
        };
        // Yours first: they win over a built-in on the same key.
        for (i, k) in cfg.custom.iter().enumerate() {
            if k.key.trim().is_empty() && k.command.trim().is_empty() {
                continue;
            }
            let Some(combo) = KeyCombo::parse(&k.key) else {
                map.problems.push(format!("Shortcut {}: \"{}\" isn't a key Cutaway knows", i + 1, k.key));
                continue;
            };
            if let Err(e) = Action::parse(&k.command) {
                map.problems.push(format!("{}: {e}", combo.label()));
                continue;
            }
            add(&mut map, combo, Binding::Command(k.command.trim().to_string()), format!("\"{}\"", k.command.trim()));
        }
        for b in Builtin::ALL {
            if let Some(set) = cfg.builtin.get(b.id()) {
                for part in split_keys(set) {
                    if KeyCombo::parse(part).is_none() {
                        map.problems.push(format!("{}: \"{}\" isn't a key Cutaway knows", b.describe(), part.trim()));
                    }
                }
            }
            for combo in builtin_keys(cfg, b) {
                // (A built-in on a key you've taken just steps aside.)
                if map.keys.iter().any(|(c, bind)| *c == combo && matches!(bind, Binding::Command(_))) {
                    continue;
                }
                add(&mut map, combo, Binding::Builtin(b), b.describe().to_lowercase());
            }
        }
        map
    }

    pub fn find(&self, combo: KeyCombo) -> Option<&Binding> {
        self.keys.iter().find(|(c, _)| *c == combo).map(|(_, b)| b)
    }

    /// The keys for a built-in, as people write them ("" when it's off).
    pub fn label_of(&self, b: Builtin) -> String {
        self.keys.iter().filter(|(_, x)| *x == Binding::Builtin(b)).map(|(c, _)| c.label()).collect::<Vec<_>>().join(" / ")
    }
}

/// A command that's held, like a sound pad: press on the way down, let go
/// on the way up.
pub fn release_of(command: &str) -> Option<Action> {
    match Action::parse(command).ok()? {
        Action::Pad(t, super::PadCmd::Press) => Some(Action::Pad(t, super::PadCmd::Release)),
        Action::Talkback(super::Toggle::On) => Some(Action::Talkback(super::Toggle::Off)),
        _ => None,
    }
}

/// A page listing every shortcut, to print and keep by the keyboard.
pub fn cheat_sheet(cfg: &crate::config::Config) -> String {
    let map = Keymap::new(&cfg.keys);
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let (alt, shift) = if cfg!(target_os = "macos") { ("⌥", "⇧") } else { ("Alt+", "Shift+") };
    let mut rows: Vec<(String, String)> = Vec::new();
    let scenes = cfg.effective_scenes();
    for (i, s) in scenes.iter().take(9).enumerate() {
        rows.push(((i + 1).to_string(), format!("Take {} ({shift}{} cuts straight to it)", s.name, i + 1)));
    }
    for (i, g) in cfg.graphics.iter().take(9).enumerate() {
        rows.push((format!("{alt}{}", i + 1), format!("Show / hide {}", g.name)));
    }
    for (i, p) in cfg.pads.iter().take(12).enumerate() {
        rows.push((format!("F{}", i + 1), format!("Sound pad: {}", p.name)));
    }
    for (combo, b) in &map.keys {
        let what = match b {
            Binding::Builtin(b) => b.describe().to_string(),
            Binding::Command(c) => describe_command(cfg, c),
        };
        rows.push((combo.label(), what));
    }
    let body: String = rows.iter().map(|(k, w)| format!("<tr><td><kbd>{}</kbd></td><td>{}</td></tr>\n", esc(k), esc(w))).collect();
    let show = if cfg.ui.profile.is_empty() { String::new() } else { format!(" · {}", esc(&cfg.ui.profile)) };
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Cutaway shortcuts</title><style>\
         body{{font:14px -apple-system,BlinkMacSystemFont,'Segoe UI',sans-serif;margin:32px;color:#111}}\
         h1{{font-size:20px;margin:0 0 4px}} p{{color:#666;margin:0 0 18px}}\
         table{{border-collapse:collapse;width:100%;columns:2}} td{{padding:5px 10px;border-bottom:1px solid #ddd;vertical-align:top}}\
         td:first-child{{white-space:nowrap;width:1%}} kbd{{font:600 13px ui-monospace,Menlo,Consolas,monospace;background:#f2f2f2;border:1px solid #ccc;border-radius:4px;padding:1px 6px}}\
         @media print{{body{{margin:12mm}}}}</style></head><body>\
         <h1>Cutaway shortcuts{show}</h1><p>Going live has no key unless you give it one, so a stray key can't start or end a broadcast.</p>\
         <table>{body}</table><script>window.print&&setTimeout(()=>window.print(),300)</script></body></html>"
    )
}

/// A command in words, from the same list Settings offers.
pub fn describe_command(cfg: &crate::config::Config, command: &str) -> String {
    super::suggestions(cfg).into_iter().find(|(code, _)| code == command.trim()).map(|(_, words)| words).unwrap_or_else(|| command.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CustomKey, KeysConfig};

    #[test]
    fn keys_read_and_write() {
        let c = KeyCombo::parse("cmd+shift+g").unwrap();
        assert_eq!((c.key, c.command, c.shift, c.alt), (Key::G, true, true, false));
        assert_eq!(c.code(), "Cmd+Shift+G");
        assert_eq!(KeyCombo::parse(&c.code()), Some(c));
        assert_eq!(KeyCombo::parse("Ctrl+Alt+F5").map(|c| c.code()), Some("Cmd+Alt+F5".into()));
        assert_eq!(KeyCombo::parse("space"), Some(KeyCombo::plain(Key::Space)));
        assert_eq!(KeyCombo::parse("Option+1").map(|c| (c.key, c.alt)), Some((Key::Num1, true)));
        assert_eq!(KeyCombo::parse("Cmd++").map(|c| c.key), Some(Key::Plus));
        assert_eq!(KeyCombo::parse("Cmd+,").map(|c| c.key), Some(Key::Comma));
        assert_eq!(split_keys("Cmd+R, F5"), ["Cmd+R", "F5"]);
        assert_eq!(split_keys("Cmd+,"), ["Cmd+,"]);
        assert_eq!(split_keys("Cmd+,, F5"), ["Cmd+,", "F5"]);
        assert_eq!(split_keys(","), [","]);
        assert!(split_keys("  ").is_empty());
        let mut cfg = KeysConfig::default();
        cfg.builtin.insert(Builtin::Settings.id().into(), "Cmd+,".into());
        assert!(Keymap::new(&cfg).problems.is_empty());
        assert_eq!(builtin_keys(&cfg, Builtin::Settings), Builtin::Settings.defaults());
        assert_eq!(KeyCombo::parse("Shift"), None, "a key is needed");
        assert_eq!(KeyCombo::parse("Ctrl+ControlLeft"), None);
        assert_eq!(KeyCombo::parse("G+H"), None, "one key only");
        assert_eq!(KeyCombo::parse("Hyper+Q"), None);
    }

    #[test]
    fn numbered_keys_stay_put() {
        assert!(KeyCombo::plain(Key::Num3).is_numbered());
        assert!(KeyCombo::parse("Shift+3").unwrap().is_numbered());
        assert!(KeyCombo::parse("Alt+3").unwrap().is_numbered());
        assert!(KeyCombo::plain(Key::F4).is_numbered());
        assert!(!KeyCombo::parse("Cmd+3").unwrap().is_numbered());
        assert!(!KeyCombo::parse("Shift+F4").unwrap().is_numbered());
        assert!(!KeyCombo::plain(Key::G).is_numbered());
    }

    #[test]
    fn yours_and_the_built_in_ones() {
        let mut cfg = KeysConfig::default();
        let map = Keymap::new(&cfg);
        assert!(map.problems.is_empty(), "{:?}", map.problems);
        assert_eq!(map.find(KeyCombo::plain(Key::M)), Some(&Binding::Builtin(Builtin::Marker)));
        assert_eq!(map.find(KeyCombo::parse("Cmd+R").unwrap()), Some(&Binding::Builtin(Builtin::Record)));
        assert_eq!(map.find(KeyCombo::parse("Shift+N").unwrap()), Some(&Binding::Builtin(Builtin::PreviousSegment)));

        // Marker moved to Cmd+M, captions turned off, a key of your own on C.
        cfg.builtin.insert("marker".into(), "Cmd+M".into());
        cfg.builtin.insert("captions".into(), String::new());
        cfg.custom.push(CustomKey { key: "G".into(), command: "graphic:Logo".into() });
        cfg.custom.push(CustomKey { key: "C".into(), command: "pad:Applause".into() });
        let map = Keymap::new(&cfg);
        assert!(map.problems.is_empty(), "{:?}", map.problems);
        assert_eq!(map.find(KeyCombo::plain(Key::M)), None);
        assert_eq!(map.find(KeyCombo::parse("Cmd+M").unwrap()), Some(&Binding::Builtin(Builtin::Marker)));
        assert_eq!(map.find(KeyCombo::plain(Key::G)), Some(&Binding::Command("graphic:Logo".into())));
        assert_eq!(map.find(KeyCombo::plain(Key::C)), Some(&Binding::Command("pad:Applause".into())));
        assert_eq!(map.label_of(Builtin::Captions), "");

        // Yours win over a built-in; two of yours on one key is a problem.
        cfg.custom.push(CustomKey { key: "T".into(), command: "record".into() });
        cfg.custom.push(CustomKey { key: "g".into(), command: "marker".into() });
        cfg.custom.push(CustomKey { key: "2".into(), command: "marker".into() });
        cfg.custom.push(CustomKey { key: "Cmd+J".into(), command: "explode".into() });
        let map = Keymap::new(&cfg);
        assert_eq!(map.find(KeyCombo::plain(Key::T)), Some(&Binding::Command("record".into())));
        assert_eq!(map.label_of(Builtin::Transition), "", "it stepped aside");
        assert_eq!(map.find(KeyCombo::plain(Key::G)), Some(&Binding::Command("graphic:Logo".into())));
        assert_eq!(map.problems.len(), 3, "{:?}", map.problems);
        assert!(map.problems.iter().any(|p| p.contains("both")));
        assert!(map.problems.iter().any(|p| p.contains("numbered")));
        assert!(map.problems.iter().any(|p| p.contains("explode")));
    }

    #[test]
    fn held_commands_let_go() {
        assert_eq!(release_of("pad:Applause"), Some(Action::Pad(crate::control::Target::Name("Applause".into()), crate::control::PadCmd::Release)));
        assert_eq!(release_of("record"), None);
    }

    #[test]
    fn a_sheet_to_print() {
        let mut cfg = crate::config::Config::default();
        cfg.keys.custom.push(CustomKey { key: "G".into(), command: "record".into() });
        let html = cheat_sheet(&cfg);
        assert!(html.contains("<kbd>G</kbd>"));
        assert!(html.contains("Add a marker while recording"));
    }
}
