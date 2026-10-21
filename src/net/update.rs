//! Is there a newer Cutaway? Asks GitHub for the latest release (once, at
//! start, when allowed in Settings → General). Nothing else is sent.

use anyhow::{anyhow, Result};
use std::time::Duration;

pub const REPO: &str = "braydenm32/cutaway";

/// A release newer than this one.
#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    /// "0.6.1"
    pub version: String,
    /// Its download page.
    pub url: String,
    /// What's new (as written on GitHub, trimmed).
    pub notes: String,
    /// Its files.
    pub assets: Vec<Asset>,
}

/// A file attached to a release.
#[derive(Clone, Debug, PartialEq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

/// The numbers in a version ("v0.6.0" → [0, 6, 0]); anything after a '-'
/// (a pre-release) is ignored.
fn numbers(v: &str) -> Vec<u64> {
    v.trim().trim_start_matches(['v', 'V']).split('-').next().unwrap_or("").split('.').map(|p| p.trim().parse().unwrap_or(0)).collect()
}

/// Whether `tag` is a newer version than `current`.
pub fn is_newer(tag: &str, current: &str) -> bool {
    let (a, b) = (numbers(tag), numbers(current));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
        if x != y {
            return x > y;
        }
    }
    false
}

/// GitHub's answer for the latest release, if it's newer than `current`.
pub fn parse(json: &str, current: &str) -> Result<Option<Release>> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|_| anyhow!("GitHub answered with something unexpected"))?;
    let tag = v["tag_name"].as_str().ok_or_else(|| anyhow!("no release found"))?;
    if v["draft"].as_bool() == Some(true) || v["prerelease"].as_bool() == Some(true) || !is_newer(tag, current) {
        return Ok(None);
    }
    let notes = readable_notes(v["body"].as_str().unwrap_or(""));
    let assets = v["assets"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|a| Some(Asset { name: a["name"].as_str()?.to_string(), url: a["browser_download_url"].as_str()?.to_string(), size: a["size"].as_u64().unwrap_or(0) }))
                .collect()
        })
        .unwrap_or_default();
    Ok(Some(Release {
        version: tag.trim_start_matches(['v', 'V']).to_string(),
        url: v["html_url"].as_str().map(str::to_string).unwrap_or_else(|| format!("https://github.com/{REPO}/releases/latest")),
        notes,
        assets,
    }))
}

/// The release notes as plain text for the update window: what's new
/// (not the download table above it), without the Markdown marks, cut to a
/// readable length at a line.
pub fn readable_notes(body: &str) -> String {
    let lines: Vec<&str> = body.lines().collect();
    let from = lines.iter().position(|l| l.trim_start().starts_with('#') && l.to_lowercase().contains("new in")).map(|i| i + 1).unwrap_or(0);
    let mut out: Vec<String> = Vec::new();
    for l in &lines[from..] {
        let t = l.trim();
        if t.starts_with('|') {
            continue;
        }
        let t = t.trim_start_matches('#').trim();
        let t = match t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
            Some(rest) => format!("• {rest}"),
            None => t.to_string(),
        };
        let t = plain_links(&t.replace("**", "").replace('`', ""));
        if t.is_empty() && out.last().is_none_or(|l| l.is_empty()) {
            continue;
        }
        out.push(t);
    }
    while out.last().is_some_and(|l| l.is_empty()) {
        out.pop();
    }
    let mut text = String::new();
    for l in out {
        if text.chars().count() + l.chars().count() > 1200 {
            text.push('…');
            break;
        }
        text.push_str(&l);
        text.push('\n');
    }
    text.trim_end().to_string()
}

/// "[the README](https://…)" → "the README".
fn plain_links(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(open) = rest.find('[') {
        let Some(mid) = rest[open..].find("](").map(|i| open + i) else { break };
        let Some(close) = rest[mid..].find(')').map(|i| mid + i) else { break };
        out.push_str(&rest[..open]);
        out.push_str(&rest[open + 1..mid]);
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    out
}

/// Ask GitHub (a few seconds at most).
pub fn check(current: &str) -> Result<Option<Release>> {
    let agent = crate::util::https_agent(Duration::from_secs(10));
    let mut resp = agent
        .get(format!("https://api.github.com/repos/{REPO}/releases/latest"))
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", &format!("Cutaway/{current}"))
        .call()
        .map_err(|e| anyhow!("couldn't reach GitHub ({e})"))?;
    match resp.status().as_u16() {
        200 => {}
        404 => return Ok(None), // no releases yet
        c => return Err(anyhow!("GitHub answered with error {c}")),
    }
    let text = resp.body_mut().read_to_string().map_err(|e| anyhow!("GitHub's answer was cut off ({e})"))?;
    parse(&text, current)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(is_newer("v0.6.1", "0.6.0"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(is_newer("v1.0", "0.9.12"));
        assert!(!is_newer("v0.6.0", "0.6.0"));
        assert!(!is_newer("0.5.9", "0.6.0"));
        assert!(!is_newer("v0.6.0-beta.1", "0.6.0"));
    }

    /// Against GitHub itself (needs the internet): `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn asks_github() {
        let r = check("0.0.1");
        println!("{r:?}");
        assert!(r.is_ok());
    }

    #[test]
    fn notes_read_as_plain_text() {
        let body = "## Download\n\n| For | File |\n|---|---|\n| Mac | **Cutaway.zip** |\n\nThe apps aren't signed yet.\n\n## New in 0.6.1\n\n**Production**\n- Fade to black, `ftb`.\n- See [the README](https://github.com/x#readme).\n\n\n";
        assert_eq!(readable_notes(body), "Production\n• Fade to black, ftb.\n• See the README.");
        // Without a "New in" heading: all of it, tables left out.
        assert_eq!(readable_notes("Fixes.\n| a | b |\n"), "Fixes.");
        let long = format!("## New in 1.0\n{}", "- a line of notes\n".repeat(200));
        let short = readable_notes(&long);
        assert!(short.chars().count() <= 1201 && short.ends_with('…'), "{}", short.len());
    }

    #[test]
    fn reads_github_releases() {
        let json = r#"{"tag_name":"v0.7.0","html_url":"https://github.com/braydenm32/cutaway/releases/tag/v0.7.0","draft":false,"prerelease":false,"body":"  New: things.\n"}"#;
        let r = parse(json, "0.6.0").unwrap().unwrap();
        assert_eq!(r.version, "0.7.0");
        assert!(r.url.ends_with("/v0.7.0"));
        assert_eq!(r.notes, "New: things.");
        assert_eq!(parse(json, "0.7.0").unwrap(), None, "already on it");
        let pre = json.replace("\"prerelease\":false", "\"prerelease\":true");
        assert_eq!(parse(&pre, "0.6.0").unwrap(), None, "pre-releases aren't offered");
        assert!(parse("{}", "0.6.0").is_err());
        assert!(r.assets.is_empty());
        let with = json.replace("\"body\"", r#""assets":[{"name":"Cutaway-0.7.0-macos-arm64.zip","browser_download_url":"https://github.com/x/a.zip","size":1234},{"name":"SHA256SUMS.txt","browser_download_url":"https://github.com/x/s.txt","size":99}],"body""#);
        let r = parse(&with, "0.6.0").unwrap().unwrap();
        assert_eq!(r.assets.len(), 2);
        assert_eq!((r.assets[0].name.as_str(), r.assets[0].size), ("Cutaway-0.7.0-macos-arm64.zip", 1234));
        assert_eq!(r.assets[1].url, "https://github.com/x/s.txt");
    }
}
