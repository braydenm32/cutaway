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
    let notes: String = v["body"].as_str().unwrap_or("").trim().chars().take(1200).collect();
    Ok(Some(Release {
        version: tag.trim_start_matches(['v', 'V']).to_string(),
        url: v["html_url"].as_str().map(str::to_string).unwrap_or_else(|| format!("https://github.com/{REPO}/releases/latest")),
        notes,
    }))
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
    }
}
