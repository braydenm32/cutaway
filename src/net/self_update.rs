//! Updating Cutaway from inside the app (the Mac and Windows apps).
//!
//! The new version for this computer is downloaded from the release, checked
//! against the release's fingerprints (SHA256SUMS.txt), unpacked beside the
//! app and run once (`--version`) to make sure it starts. When Cutaway
//! closes, a small helper (a shell script on the Mac, PowerShell on Windows)
//! waits for it to finish, moves the old version aside, puts the new one in
//! its place and, for "Restart to update", opens it. The old version is kept
//! until the new one has been running for a few seconds: if the new one
//! closes before then, the helper puts the old one back and opens that.

use super::update::Release;
use crate::engine::Waker;
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The release's fingerprints file.
pub const SUMS: &str = "SHA256SUMS.txt";
const ZIP_MAGIC: &[u8] = b"PK\x03\x04";
/// How long a new version has to run before it counts as started properly.
pub const SETTLE: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Mac,
    Windows,
}

impl Platform {
    pub fn this() -> Option<Platform> {
        if cfg!(target_os = "macos") {
            Some(Platform::Mac)
        } else if cfg!(windows) {
            Some(Platform::Windows)
        } else {
            None
        }
    }

    /// The release's download for this platform.
    pub fn asset_name(self, version: &str) -> String {
        match self {
            Platform::Mac => format!("Cutaway-{version}-macos-arm64.zip"),
            Platform::Windows => format!("Cutaway-{version}-windows-x64.zip"),
        }
    }

    /// What the download holds: the app (Mac) or the Cutaway folder (Windows).
    fn item_name(self) -> &'static str {
        match self {
            Platform::Mac => "Cutaway.app",
            Platform::Windows => "Cutaway",
        }
    }

    /// The program inside it.
    fn program(self, item: &Path) -> PathBuf {
        match self {
            Platform::Mac => item.join("Contents").join("MacOS").join("Cutaway"),
            Platform::Windows => item.join("Cutaway.exe"),
        }
    }
}

/// Where this copy of Cutaway is, and where its updates are prepared.
#[derive(Clone, Debug, PartialEq)]
pub struct Install {
    pub platform: Platform,
    /// What's replaced: Cutaway.app on a Mac, the Cutaway folder on Windows.
    pub item: PathBuf,
    /// Updates are unpacked here, and the old version kept until the new one
    /// has started (on the same drive as the app on Windows).
    pub work: PathBuf,
}

impl Install {
    /// From the program's path. `support` is Cutaway's folder in Application
    /// Support (where a Mac's updates are prepared).
    pub fn of(platform: Platform, exe: &Path, support: &Path) -> Result<Install, String> {
        match platform {
            Platform::Mac => {
                let item = exe.ancestors().find(|a| a.extension().is_some_and(|e| e.eq_ignore_ascii_case("app"))).ok_or("this copy of Cutaway isn't the app (it runs from a build)")?;
                Ok(Install { platform, item: item.to_path_buf(), work: support.join("Updates") })
            }
            Platform::Windows => {
                let dir = exe.parent().ok_or("this copy of Cutaway isn't in a folder")?;
                let name = exe.file_name().and_then(|n| n.to_str()).unwrap_or("");
                let built = dir.file_name().is_some_and(|n| n == "release" || n == "debug") && dir.parent().and_then(|p| p.file_name()).is_some_and(|n| n == "target");
                if !name.eq_ignore_ascii_case("Cutaway.exe") || built {
                    return Err("this copy of Cutaway runs from a build, not the download".into());
                }
                Ok(Install { platform, item: dir.to_path_buf(), work: dir.join(".update") })
            }
        }
    }

    /// Where this copy is (whether or not it can replace itself).
    pub fn here() -> Result<Install, String> {
        let platform = Platform::this().ok_or("updating from inside Cutaway is for the Mac and Windows apps")?;
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let support = crate::config::default_config_path().parent().map(Path::to_path_buf).unwrap_or_default();
        Install::of(platform, &exe, &support)
    }

    /// This copy, if it can replace itself (and why not, if it can't).
    pub fn this() -> Result<Install, String> {
        let me = Install::here()?;
        let need: Vec<PathBuf> = match me.platform {
            // Moving the app needs its folder and the app itself.
            Platform::Mac => vec![me.item.parent().map(Path::to_path_buf).unwrap_or_default(), me.item.clone()],
            Platform::Windows => vec![me.item.clone()],
        };
        if let Some(dir) = need.iter().find(|d| !writable(d)) {
            return Err(format!("Cutaway isn't allowed to change {} from this account", dir.display()));
        }
        Ok(me)
    }

    fn record(&self) -> PathBuf {
        self.work.join("pending.json")
    }

    /// Where the old version goes while the new one proves itself.
    fn previous(&self) -> PathBuf {
        self.work.join("previous")
    }

    pub fn log(&self) -> PathBuf {
        self.work.join("update.log")
    }

    fn staging(&self, version: &str) -> PathBuf {
        self.work.join(version)
    }

    fn started(&self, version: &str) -> PathBuf {
        self.work.join(format!("started-{version}"))
    }

    fn rolled_back(&self, version: &str) -> PathBuf {
        self.work.join(format!("rolled-back-{version}"))
    }
}

fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".cutaway-write-test-{}", std::process::id()));
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// A new version unpacked and ready to go in.
#[derive(Clone, Debug, PartialEq)]
pub struct Staged {
    pub version: String,
    /// The new Cutaway.app, or the new Cutaway folder.
    pub new_item: PathBuf,
}

/// The fingerprint `SHA256SUMS.txt` gives for `name`.
pub fn expected_sum(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|l| {
        let mut parts = l.split_whitespace();
        let hash = parts.next()?;
        let file = parts.next()?.trim_start_matches('*');
        (file == name && hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit())).then(|| hash.to_ascii_lowercase())
    })
}

pub fn sha256(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Download the release for this computer, check it, unpack it and make
/// sure it starts.
pub fn prepare(release: &Release, me: &Install, cancel: &AtomicBool, progress: &mut dyn FnMut(u64, Option<u64>), downloaded: &mut dyn FnMut()) -> Result<Staged> {
    let name = me.platform.asset_name(&release.version);
    let zip = release.assets.iter().find(|a| a.name == name).ok_or_else(|| anyhow!("this release has no download for this computer"))?;
    let sums = release.assets.iter().find(|a| a.name == SUMS).ok_or_else(|| anyhow!("this release has no fingerprints to check a download against"))?;
    let dir = me.staging(&release.version);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).with_context(|| format!("couldn't make {}", dir.display()))?;
    let sums_file = super::fetch::fetch(&sums.url, &dir.join(SUMS), b"", "GitHub", "the release's fingerprints", cancel, &mut |_, _| {})?;
    let want = expected_sum(&std::fs::read_to_string(&sums_file).unwrap_or_default(), &name).ok_or_else(|| anyhow!("the release's fingerprints don't list {name}"))?;
    let file = super::fetch::fetch(&zip.url, &dir.join(&name), ZIP_MAGIC, "GitHub", "the update", cancel, progress)?;
    downloaded();
    if sha256(&file)? != want {
        let _ = std::fs::remove_file(&file);
        bail!("the download doesn't match the release's fingerprint, so it wasn't used");
    }
    let into = dir.join("new");
    let new_item = into.join(me.platform.item_name());
    let program = me.platform.program(&new_item);
    let unpacked = unpack(&file, &into);
    // (Windows' own unzipper, if tar didn't manage.)
    #[cfg(windows)]
    let unpacked = if unpacked.is_err() || !program.is_file() { expand_archive(&file, &into) } else { unpacked };
    unpacked?;
    let _ = std::fs::remove_file(&file);
    if !program.is_file() {
        bail!("the download doesn't hold {}", me.platform.item_name());
    }
    check_starts(&program, &release.version)?;
    Ok(Staged { version: release.version.clone(), new_item })
}

/// Unpack a zip with the system's own tool: ditto on a Mac (it keeps the
/// app's links and attributes), tar on Windows, unzip elsewhere.
pub fn unpack(zip: &Path, into: &Path) -> Result<()> {
    let _ = std::fs::remove_dir_all(into);
    std::fs::create_dir_all(into).with_context(|| format!("couldn't make {}", into.display()))?;
    let mut c = if cfg!(target_os = "macos") {
        let mut c = Command::new("/usr/bin/ditto");
        c.arg("-x").arg("-k").arg(zip).arg(into);
        c
    } else if cfg!(windows) {
        let root = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("C:\\Windows"));
        let mut c = Command::new(root.join("System32").join("tar.exe"));
        c.arg("-x").arg("-f").arg(zip).arg("-C").arg(into);
        c
    } else {
        let mut c = Command::new("unzip");
        c.arg("-q").arg(zip).arg("-d").arg(into);
        c
    };
    quiet(&mut c);
    let out = c.stdin(Stdio::null()).output().context("couldn't unpack the update")?;
    if !out.status.success() {
        bail!("couldn't unpack the update ({})", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

/// Unpack with PowerShell's Expand-Archive.
#[cfg(windows)]
fn expand_archive(zip: &Path, into: &Path) -> Result<()> {
    let _ = std::fs::remove_dir_all(into);
    std::fs::create_dir_all(into)?;
    let root = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("C:\\Windows"));
    let mut c = Command::new(root.join("System32").join("WindowsPowerShell").join("v1.0").join("powershell.exe"));
    c.args(["-NoProfile", "-NonInteractive", "-Command"]).arg(format!("Expand-Archive -LiteralPath {} -DestinationPath {} -Force", ps(zip), ps(into)));
    quiet(&mut c);
    let out = c.stdin(Stdio::null()).output().context("couldn't unpack the update")?;
    if !out.status.success() {
        bail!("couldn't unpack the update ({})", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

/// No console window for helpers on Windows.
fn quiet(c: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    #[cfg(not(windows))]
    let _ = c;
}

/// Run the new version once (`--version`): it has to start and say it's the
/// version expected.
pub fn check_starts(program: &Path, version: &str) -> Result<()> {
    let mut c = Command::new(program);
    c.arg("--version").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    quiet(&mut c);
    let mut child = c.spawn().context("the new version didn't start")?;
    let until = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break s;
        }
        if Instant::now() > until {
            let _ = child.kill();
            let _ = child.wait();
            bail!("the new version didn't answer");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let mut out = String::new();
    if let Some(mut o) = child.stdout.take() {
        let _ = o.read_to_string(&mut out);
    }
    if !status.success() || !out.contains(version) {
        bail!("the new version didn't start properly ({})", out.trim());
    }
    Ok(())
}

/// What the helper is asked to do.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct Record {
    from: String,
    to: String,
    relaunch: bool,
}

/// The helper's instructions: everything it needs, written into the script.
#[derive(Clone, Debug)]
pub struct Swap {
    /// The Cutaway to wait for.
    pub pid: u32,
    pub item: PathBuf,
    pub new_item: PathBuf,
    /// The old version is moved here (the old app on a Mac; a folder the
    /// old files go into on Windows).
    pub previous: PathBuf,
    /// Written by the new version once it has run for a while.
    pub started: PathBuf,
    /// Written by the helper when it had to put the old version back.
    pub rolled_back: PathBuf,
    pub log: PathBuf,
    /// Open the new version afterwards (and watch that it starts).
    pub relaunch: bool,
    /// Script code that opens the app and waits for it to close (Mac), or
    /// opens it and gives its process (Windows).
    pub launch: String,
    /// Script code that opens the app without waiting (after going back).
    pub launch_old: String,
}

impl Swap {
    pub fn new(me: &Install, staged: &Staged, pid: u32, relaunch: bool) -> Swap {
        let item = me.item.clone();
        let (launch, launch_old) = match me.platform {
            Platform::Mac => (format!("open -W {}", sh(&item)), format!("open {}", sh(&item))),
            Platform::Windows => {
                let exe = ps(&item.join("Cutaway.exe"));
                (format!("Start-Process -FilePath {exe} -WorkingDirectory {} -PassThru", ps(&item)), format!("$null = Start-Process -FilePath {exe} -WorkingDirectory {}", ps(&item)))
            }
        };
        Swap {
            pid,
            previous: match me.platform {
                Platform::Mac => me.previous().join("Cutaway.app"),
                Platform::Windows => me.previous(),
            },
            item,
            new_item: staged.new_item.clone(),
            started: me.started(&staged.version),
            rolled_back: me.rolled_back(&staged.version),
            log: me.log(),
            relaunch,
            launch,
            launch_old,
        }
    }

    /// The Mac's helper (POSIX shell).
    pub fn shell_script(&self) -> String {
        let relaunch = if self.relaunch { 1 } else { 0 };
        format!(
            r#"#!/bin/sh
# Cutaway's updater, written by Cutaway: it waits for Cutaway to close, puts
# the new version in place and, if asked, opens it. Safe to delete.
PID={pid}
ITEM={item}
NEW={new}
OLD={old}
STARTED={started}
BACK={back}
LOG={log}
RELAUNCH={relaunch}
say() {{ echo "$(date '+%Y-%m-%d %H:%M:%S') $*" >> "$LOG"; }}
launch() {{ {launch}; }}
launch_old() {{ {launch_old}; }}
say "waiting for Cutaway ($PID) to close"
n=0
while kill -0 "$PID" 2>/dev/null; do
  sleep 0.2
  n=$((n + 1))
  if [ "$n" -gt 9000 ]; then say "Cutaway didn't close; not updating"; exit 1; fi
done
rm -rf "$OLD"
mkdir -p "$(dirname "$OLD")"
if ! mv "$ITEM" "$OLD"; then
  say "couldn't move the old version aside; not updating"
  [ "$RELAUNCH" = 1 ] && launch_old
  exit 1
fi
if ! mv "$NEW" "$ITEM"; then
  say "couldn't put the new version in place; putting the old one back"
  rm -rf "$ITEM"
  mv "$OLD" "$ITEM"
  [ "$RELAUNCH" = 1 ] && launch_old
  exit 1
fi
say "the new version is in place"
[ "$RELAUNCH" = 1 ] || exit 0
launch &
W=$!
n=0
while [ ! -f "$STARTED" ]; do
  if ! kill -0 "$W" 2>/dev/null; then
    sleep 1
    [ -f "$STARTED" ] && break
    say "the new version closed before it had started properly; going back"
    rm -rf "$ITEM"
    mv "$OLD" "$ITEM"
    : > "$BACK"
    launch_old
    exit 1
  fi
  sleep 0.5
  n=$((n + 1))
  [ "$n" -gt 240 ] && break
done
say "the new version started"
exit 0
"#,
            pid = self.pid,
            item = sh(&self.item),
            new = sh(&self.new_item),
            old = sh(&self.previous),
            started = sh(&self.started),
            back = sh(&self.rolled_back),
            log = sh(&self.log),
            launch = self.launch,
            launch_old = self.launch_old,
        )
    }

    /// Windows' helper (PowerShell). Everything in the Cutaway folder but the
    /// update folder is moved aside, and the new files moved in.
    pub fn powershell_script(&self) -> String {
        let work_name = self.previous.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| ".update".into());
        format!(
            r#"# Cutaway's updater, written by Cutaway: it waits for Cutaway to close, puts
# the new version in place and, if asked, opens it. Safe to delete.
$ErrorActionPreference = 'Stop'
$Id = {pid}
$Dir = {item}
$Keep = {keep}
$New = {new}
$Old = {old}
$Started = {started}
$Back = {back}
$Log = {log}
$Relaunch = ${relaunch}
function Say($m) {{ Add-Content -LiteralPath $Log -Value ("{{0}} {{1}}" -f (Get-Date -Format 'yyyy-MM-dd HH:mm:ss'), $m) }}
function Again([scriptblock]$b) {{
  for ($i = 0; ; $i++) {{ try {{ & $b; return }} catch {{ if ($i -ge 20) {{ throw }}; Start-Sleep -Milliseconds 500 }} }}
}}
function Restore {{
  foreach ($x in @(Get-ChildItem -LiteralPath $Dir -Force)) {{ if ($x.Name -ne $Keep) {{ Again {{ Remove-Item -LiteralPath $x.FullName -Recurse -Force }} }} }}
  foreach ($x in @(Get-ChildItem -LiteralPath $Old -Force)) {{ Again {{ Move-Item -LiteralPath $x.FullName -Destination $Dir }} }}
}}
Say "waiting for Cutaway ($Id) to close"
try {{ Wait-Process -Id $Id -Timeout 1800 -ErrorAction SilentlyContinue }} catch {{}}
if (Get-Process -Id $Id -ErrorAction SilentlyContinue) {{ Say "Cutaway didn't close; not updating"; exit 1 }}
Start-Sleep -Milliseconds 300
try {{
  if (Test-Path -LiteralPath $Old) {{ Remove-Item -LiteralPath $Old -Recurse -Force }}
  New-Item -ItemType Directory -Path $Old | Out-Null
  foreach ($x in @(Get-ChildItem -LiteralPath $Dir -Force)) {{ if ($x.Name -ne $Keep) {{ Again {{ Move-Item -LiteralPath $x.FullName -Destination $Old }} }} }}
  foreach ($x in @(Get-ChildItem -LiteralPath $New -Force)) {{ Again {{ Move-Item -LiteralPath $x.FullName -Destination $Dir }} }}
}} catch {{
  Say "couldn't put the new version in place ($_); putting the old one back"
  try {{ Restore }} catch {{ Say "couldn't put everything back ($_)" }}
  if ($Relaunch) {{ {launch_old} }}
  exit 1
}}
Say "the new version is in place"
if (-not $Relaunch) {{ exit 0 }}
$p = {launch}
for ($n = 0; $n -lt 240 -and -not (Test-Path -LiteralPath $Started); $n++) {{
  if ($p.HasExited) {{
    Start-Sleep -Seconds 1
    if (Test-Path -LiteralPath $Started) {{ break }}
    Say "the new version closed before it had started properly; going back"
    try {{ Restore }} catch {{ Say "couldn't put everything back ($_)" }}
    New-Item -ItemType File -Path $Back -Force | Out-Null
    {launch_old}
    exit 1
  }}
  Start-Sleep -Milliseconds 500
}}
Say "the new version started"
exit 0
"#,
            pid = self.pid,
            item = ps(&self.item),
            keep = ps(Path::new(&work_name)),
            new = ps(&self.new_item),
            old = ps(&self.previous),
            started = ps(&self.started),
            back = ps(&self.rolled_back),
            log = ps(&self.log),
            relaunch = if self.relaunch { "true" } else { "false" },
            launch = self.launch,
            launch_old = self.launch_old,
        )
    }
}

/// A path for a POSIX shell, single-quoted.
fn sh(p: &Path) -> String {
    format!("'{}'", p.to_string_lossy().replace('\'', "'\\''"))
}

/// A path for PowerShell, single-quoted.
fn ps(p: &Path) -> String {
    format!("'{}'", p.to_string_lossy().replace('\'', "''"))
}

/// Hand over to the helper: it does the rest once Cutaway has closed.
pub fn schedule(me: &Install, staged: &Staged, from: &str, relaunch: bool) -> Result<()> {
    std::fs::create_dir_all(&me.work)?;
    let rec = Record { from: from.to_string(), to: staged.version.clone(), relaunch };
    std::fs::write(me.record(), serde_json::to_vec_pretty(&rec)?).context("couldn't write down the update")?;
    let _ = std::fs::remove_file(me.started(&staged.version));
    let _ = std::fs::remove_file(me.rolled_back(&staged.version));
    let swap = Swap::new(me, staged, std::process::id(), relaunch);
    match me.platform {
        Platform::Mac => {
            let script = me.work.join("install.sh");
            std::fs::write(&script, swap.shell_script())?;
            let mut c = Command::new("/bin/sh");
            c.arg(&script).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                c.process_group(0);
            }
            c.spawn().context("couldn't start the updater")?;
        }
        Platform::Windows => {
            let script = me.work.join("install.ps1");
            // With a byte-order mark, so Windows PowerShell reads any
            // non-English letters in the folder names right.
            let mut text = "\u{feff}".to_string();
            text.push_str(&swap.powershell_script());
            std::fs::write(&script, text)?;
            spawn_powershell(&script)?;
        }
    }
    Ok(())
}

#[cfg(windows)]
fn spawn_powershell(script: &Path) -> Result<()> {
    use std::os::windows::process::CommandExt;
    let root = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("C:\\Windows"));
    let exe = root.join("System32").join("WindowsPowerShell").join("v1.0").join("powershell.exe");
    let run = |flags: u32| {
        Command::new(&exe)
            .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-WindowStyle", "Hidden", "-File"])
            .arg(script)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(flags)
            .spawn()
    };
    // No window, its own group, and not closed along with Cutaway (when
    // Cutaway's in a job that allows leaving it).
    const NO_WINDOW: u32 = 0x0800_0000;
    const NEW_GROUP: u32 = 0x0000_0200;
    const BREAKAWAY: u32 = 0x0100_0000;
    run(NO_WINDOW | NEW_GROUP | BREAKAWAY).or_else(|_| run(NO_WINDOW | NEW_GROUP)).context("couldn't start the updater")?;
    Ok(())
}

#[cfg(not(windows))]
fn spawn_powershell(_script: &Path) -> Result<()> {
    bail!("PowerShell is for Windows")
}

/// How the last update went, found when Cutaway starts.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// This is the new version.
    Updated { from: String },
    /// The new version didn't start, so the helper put this one back.
    RolledBack { to: String },
    /// The update didn't go in (the reason is from the helper's log).
    NotInstalled { to: String, why: String },
}

/// Look at what the last update left, and tidy up.
pub fn after_start(me: &Install, current: &str) -> Option<Outcome> {
    let rec: Option<Record> = std::fs::read(me.record()).ok().and_then(|b| serde_json::from_slice(&b).ok());
    let Some(rec) = rec else {
        // Nothing waiting: unpacked updates that were never used.
        if let Ok(list) = std::fs::read_dir(&me.work) {
            for e in list.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if e.path().is_dir() && name != "previous" {
                    let _ = std::fs::remove_dir_all(e.path());
                }
            }
        }
        return None;
    };
    if rec.to == current {
        // The record is kept until this version has run for a while (`settled`).
        return Some(Outcome::Updated { from: rec.from });
    }
    let outcome = if me.rolled_back(&rec.to).exists() {
        Outcome::RolledBack { to: rec.to.clone() }
    } else {
        let why = std::fs::read_to_string(me.log()).ok().and_then(|l| l.lines().last().map(|l| l.splitn(3, ' ').nth(2).unwrap_or(l).to_string())).unwrap_or_else(|| "the updater didn't run".into());
        Outcome::NotInstalled { to: rec.to.clone(), why }
    };
    // The old version is back (or never left): the update is done with. The
    // folder the old version was kept in stays until the next update, in
    // case anything of it didn't make it back.
    let _ = std::fs::remove_file(me.record());
    let _ = std::fs::remove_file(me.rolled_back(&rec.to));
    let _ = std::fs::remove_dir_all(me.staging(&rec.to));
    Some(outcome)
}

/// The new version has been running a while: tell the helper (if it's
/// watching) and let go of the old version.
pub fn settled(me: &Install, current: &str) {
    let _ = std::fs::write(me.started(current), b"");
    let _ = std::fs::remove_dir_all(me.previous());
    let _ = std::fs::remove_dir_all(me.staging(current));
    let _ = std::fs::remove_file(me.record());
}

// ---- the app's side: one update at a time ------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub enum Stage {
    Idle,
    Downloading { got: u64, total: Option<u64> },
    /// Checking the fingerprint, unpacking, trying it.
    Preparing,
    Ready(Staged),
    Failed(String),
}

/// When a ready update goes in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Finish {
    /// Not chosen yet.
    Ask,
    /// Now: Cutaway closes and opens again.
    Restart,
    /// When Cutaway is next closed.
    OnQuit,
}

struct State {
    stage: Stage,
    finish: Finish,
    cancel: Arc<AtomicBool>,
}

static STATE: std::sync::LazyLock<Mutex<State>> = std::sync::LazyLock::new(|| Mutex::new(State { stage: Stage::Idle, finish: Finish::Ask, cancel: Arc::new(AtomicBool::new(false)) }));

pub fn stage() -> Stage {
    STATE.lock().unwrap().stage.clone()
}

pub fn finish() -> Finish {
    STATE.lock().unwrap().finish
}

/// For scripted screenshots of the update window.
pub fn show_stage(stage: Stage, finish: Finish) {
    let mut st = STATE.lock().unwrap();
    st.stage = stage;
    st.finish = finish;
}

pub fn choose(f: Finish) {
    STATE.lock().unwrap().finish = f;
}

/// Fetch and prepare `release` in the background; `then`: when it goes in
/// once ready (Ask: wait to be told).
pub fn start(release: Release, then: Finish, waker: Waker) {
    let me = match Install::this() {
        Ok(me) => me,
        Err(e) => {
            STATE.lock().unwrap().stage = Stage::Failed(e);
            waker();
            return;
        }
    };
    let cancel = {
        let mut st = STATE.lock().unwrap();
        if matches!(st.stage, Stage::Downloading { .. } | Stage::Preparing) || (matches!(st.stage, Stage::Ready(ref s) if s.version == release.version)) {
            return;
        }
        st.cancel.store(true, Ordering::Relaxed);
        st.cancel = Arc::new(AtomicBool::new(false));
        st.stage = Stage::Downloading { got: 0, total: None };
        st.finish = then;
        st.cancel.clone()
    };
    let _ = std::thread::Builder::new().name("update".into()).spawn(move || {
        let w = waker.clone();
        let mut last = Instant::now();
        let r = prepare(
            &release,
            &me,
            &cancel,
            &mut |got, total| {
                let mut st = STATE.lock().unwrap();
                if !cancel.load(Ordering::Relaxed) {
                    st.stage = Stage::Downloading { got, total };
                }
                drop(st);
                if last.elapsed() > Duration::from_millis(100) {
                    last = Instant::now();
                    w();
                }
            },
            &mut || {
                STATE.lock().unwrap().stage = Stage::Preparing;
                waker();
            },
        );
        let mut st = STATE.lock().unwrap();
        if cancel.load(Ordering::Relaxed) {
            st.stage = Stage::Idle;
            let _ = std::fs::remove_dir_all(me.staging(&release.version));
        } else {
            st.stage = match r {
                Ok(s) => Stage::Ready(s),
                Err(e) => {
                    crate::logging::note(&format!("update to {}: {e:#}", release.version));
                    Stage::Failed(format!("{e:#}"))
                }
            };
        }
        drop(st);
        waker();
    });
}

pub fn cancel() {
    let mut st = STATE.lock().unwrap();
    st.cancel.store(true, Ordering::Relaxed);
    if !matches!(st.stage, Stage::Ready(_)) {
        st.stage = Stage::Idle;
    }
}

/// Back to the start after a failure (to try again).
pub fn reset() {
    let mut st = STATE.lock().unwrap();
    if matches!(st.stage, Stage::Failed(_)) {
        st.stage = Stage::Idle;
    }
}

/// Cutaway is closing: if an update is ready and has been told when to go
/// in, hand it to the helper. Returns what went wrong, if anything.
pub fn on_exit() -> Option<String> {
    let st = STATE.lock().unwrap();
    let Stage::Ready(staged) = st.stage.clone() else { return None };
    if st.finish == Finish::Ask {
        return None;
    }
    let relaunch = st.finish == Finish::Restart;
    drop(st);
    let r = Install::this().map_err(|e| anyhow!(e)).and_then(|me| schedule(&me, &staged, env!("CARGO_PKG_VERSION"), relaunch));
    match r {
        Ok(()) => None,
        Err(e) => {
            crate::logging::note(&format!("update to {}: {e:#}", staged.version));
            Some(format!("{e:#}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cutaway-update-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn finds_itself() {
        let mac = Install::of(Platform::Mac, Path::new("/Applications/Cutaway.app/Contents/MacOS/Cutaway"), Path::new("/Users/me/Library/Application Support/Cutaway")).unwrap();
        assert_eq!(mac.item, Path::new("/Applications/Cutaway.app"));
        assert_eq!(mac.work, Path::new("/Users/me/Library/Application Support/Cutaway/Updates"));
        assert!(Install::of(Platform::Mac, Path::new("/Users/me/cutaway/target/release/cutaway"), Path::new("/x")).is_err(), "a build isn't the app");
        let win = Install::of(Platform::Windows, Path::new("D:/Shows/Cutaway/Cutaway.exe"), Path::new("")).unwrap();
        assert_eq!(win.item, Path::new("D:/Shows/Cutaway"));
        assert_eq!(win.work, Path::new("D:/Shows/Cutaway/.update"));
        assert!(Install::of(Platform::Windows, Path::new("C:/src/cutaway/target/release/cutaway.exe"), Path::new("")).is_err());
        assert_eq!(Platform::Mac.asset_name("0.8.0"), "Cutaway-0.8.0-macos-arm64.zip");
        assert_eq!(Platform::Windows.asset_name("0.8.0"), "Cutaway-0.8.0-windows-x64.zip");
    }

    #[test]
    fn reads_fingerprints() {
        let a = "a".repeat(64);
        let b = "B".repeat(64);
        let sums = format!("{a}  Cutaway-0.8.0-macos-arm64.zip\n{b} *Cutaway-0.8.0-windows-x64.zip\nnonsense\n");
        assert_eq!(expected_sum(&sums, "Cutaway-0.8.0-macos-arm64.zip"), Some(a));
        assert_eq!(expected_sum(&sums, "Cutaway-0.8.0-windows-x64.zip"), Some("b".repeat(64)));
        assert_eq!(expected_sum(&sums, "Cutaway-0.8.0-linux.zip"), None);
        let d = temp("sha");
        std::fs::write(d.join("x"), b"abc").unwrap();
        assert_eq!(sha256(&d.join("x")).unwrap(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn quotes_paths_for_the_helpers() {
        assert_eq!(sh(Path::new("/Users/o'neil/x y")), r#"'/Users/o'\''neil/x y'"#);
        assert_eq!(ps(Path::new("C:\\Users\\O'Neil\\x y")), "'C:\\Users\\O''Neil\\x y'");
    }

    /// A Mac-style app folder with a file saying which version it is.
    #[cfg(unix)]
    fn fake_app(at: &Path, which: &str) {
        std::fs::create_dir_all(at.join("Contents/MacOS")).unwrap();
        std::fs::write(at.join("Contents/which"), which).unwrap();
    }

    #[cfg(unix)]
    fn which(at: &Path) -> String {
        std::fs::read_to_string(at.join("Contents/which")).unwrap_or_default()
    }

    /// The Mac's helper, run for real with sh: it waits for Cutaway to
    /// close, swaps the app, and goes back if the new one doesn't start.
    #[cfg(unix)]
    #[test]
    fn the_shell_helper_swaps_and_goes_back() {
        for starts in [true, false] {
            let d = temp(if starts { "sh-ok" } else { "sh-back" });
            let me = Install { platform: Platform::Mac, item: d.join("Applications/Cutaway.app"), work: d.join("Updates") };
            fake_app(&me.item, "old");
            let staged = Staged { version: "9.9.9".into(), new_item: me.work.join("9.9.9/new/Cutaway.app") };
            fake_app(&staged.new_item, "new");
            // Cutaway, still closing for a moment (and seen to by a parent,
            // as the real one is: an unreaped one would look alive).
            let mut cutaway = Command::new("sleep").arg("0.6").spawn().unwrap();
            let pid = cutaway.id();
            let reaper = std::thread::spawn(move || cutaway.wait());
            let mut swap = Swap::new(&me, &staged, pid, true);
            let opened = d.join("opened-old");
            // "Opening" the new version: it starts properly, or closes at once.
            swap.launch = if starts { format!("touch {}", sh(&swap.started)) } else { "true".into() };
            swap.launch_old = format!("touch {}", sh(&opened));
            let script = d.join("install.sh");
            std::fs::write(&script, swap.shell_script()).unwrap();
            let t = Instant::now();
            let status = Command::new("/bin/sh").arg(&script).status().unwrap();
            let _ = reaper.join();
            assert!(t.elapsed() >= Duration::from_millis(500), "it waited for Cutaway to close");
            let log = std::fs::read_to_string(&swap.log).unwrap_or_default();
            if starts {
                assert!(status.success(), "{log}");
                assert_eq!(which(&me.item), "new");
                assert_eq!(which(&swap.previous), "old", "the old version is kept until the new one settles");
                assert!(!opened.exists());
            } else {
                assert!(!status.success());
                assert_eq!(which(&me.item), "old", "back to the old version: {log}");
                assert!(swap.rolled_back.exists() && opened.exists());
                assert!(log.contains("going back"), "{log}");
            }
        }
    }

    /// Updating when Cutaway quits: swapped, nothing opened; the next start
    /// says so and, once settled, lets the old version go.
    #[cfg(unix)]
    #[test]
    fn installs_on_quit_and_tidies_up() {
        let d = temp("quit");
        let me = Install { platform: Platform::Mac, item: d.join("Applications/Cutaway.app"), work: d.join("Updates") };
        fake_app(&me.item, "old");
        let staged = Staged { version: "9.9.9".into(), new_item: me.work.join("9.9.9/new/Cutaway.app") };
        fake_app(&staged.new_item, "new");
        std::fs::write(me.record(), serde_json::to_vec(&Record { from: "0.7.1".into(), to: "9.9.9".into(), relaunch: false }).unwrap()).unwrap();
        let mut swap = Swap::new(&me, &staged, 999_999, false);
        swap.launch = "exit 9".into();
        swap.launch_old = "exit 9".into();
        std::fs::write(d.join("i.sh"), swap.shell_script()).unwrap();
        assert!(Command::new("/bin/sh").arg(d.join("i.sh")).status().unwrap().success());
        assert_eq!(which(&me.item), "new");
        assert_eq!(after_start(&me, "9.9.9"), Some(Outcome::Updated { from: "0.7.1".into() }));
        settled(&me, "9.9.9");
        assert!(!me.previous().exists() && !me.staging("9.9.9").exists() && !me.record().exists());
        assert_eq!(after_start(&me, "9.9.9"), None);
    }

    /// The old version came back: the next start says why.
    #[test]
    fn reports_an_update_that_did_not_go_in() {
        let d = temp("report");
        let me = Install { platform: Platform::Mac, item: d.join("Cutaway.app"), work: d.join("Updates") };
        std::fs::create_dir_all(&me.work).unwrap();
        std::fs::write(me.record(), serde_json::to_vec(&Record { from: "0.7.1".into(), to: "9.9.9".into(), relaunch: true }).unwrap()).unwrap();
        std::fs::write(me.rolled_back("9.9.9"), b"").unwrap();
        assert_eq!(after_start(&me, "0.7.1"), Some(Outcome::RolledBack { to: "9.9.9".into() }));
        assert_eq!(after_start(&me, "0.7.1"), None, "said once");
        std::fs::write(me.record(), serde_json::to_vec(&Record { from: "0.7.1".into(), to: "9.9.9".into(), relaunch: false }).unwrap()).unwrap();
        std::fs::write(me.log(), "2026-10-22 09:00:00 waiting\n2026-10-22 09:00:01 couldn't move the old version aside; not updating\n").unwrap();
        assert_eq!(after_start(&me, "0.7.1"), Some(Outcome::NotInstalled { to: "9.9.9".into(), why: "couldn't move the old version aside; not updating".into() }));
    }

    /// The whole preparation against a stand-in for GitHub: download,
    /// fingerprint, unpack and a first run of the "new version".
    #[cfg(unix)]
    #[test]
    fn prepares_a_release() {
        if Command::new("zip").arg("-v").output().is_err() || (cfg!(not(target_os = "macos")) && Command::new("unzip").arg("-v").output().is_err()) {
            eprintln!("skipping: no zip/unzip here");
            return;
        }
        let d = temp("prep");
        // The release's app: its program answers --version like Cutaway.
        let app = d.join("src/Cutaway.app");
        std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        let prog = app.join("Contents/MacOS/Cutaway");
        std::fs::write(&prog, "#!/bin/sh\necho cutaway 9.9.9\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&prog, std::fs::Permissions::from_mode(0o755)).unwrap();
        let zip_name = Platform::Mac.asset_name("9.9.9");
        let zip_path = d.join(&zip_name);
        assert!(Command::new("zip").current_dir(d.join("src")).arg("-qr").arg(&zip_path).arg("Cutaway.app").status().unwrap().success());
        let zip = std::fs::read(&zip_path).unwrap();
        let good = format!("{}  {zip_name}\n", sha256(&zip_path).unwrap());
        for (sums, ok) in [(good, true), (format!("{}  {zip_name}\n", "0".repeat(64)), false)] {
            let url = serve(vec![("/zip".into(), zip.clone()), ("/sums".into(), sums.into_bytes())]);
            let release = Release {
                version: "9.9.9".into(),
                url: String::new(),
                notes: String::new(),
                assets: vec![
                    super::super::update::Asset { name: zip_name.clone(), url: format!("{url}/zip"), size: zip.len() as u64 },
                    super::super::update::Asset { name: SUMS.into(), url: format!("{url}/sums"), size: 0 },
                ],
            };
            let me = Install { platform: Platform::Mac, item: d.join("Cutaway.app"), work: d.join(if ok { "w1" } else { "w2" }) };
            let mut seen = 0;
            let r = prepare(&release, &me, &AtomicBool::new(false), &mut |got, _| seen = seen.max(got), &mut || {});
            if ok {
                let s = r.unwrap();
                assert_eq!(s.new_item, me.work.join("9.9.9/new/Cutaway.app"));
                assert!(s.new_item.join("Contents/MacOS/Cutaway").is_file());
                assert_eq!(seen, zip.len() as u64);
                assert!(!me.work.join("9.9.9").join(&zip_name).exists(), "the download is removed once unpacked");
            } else {
                let e = r.unwrap_err().to_string();
                assert!(e.contains("fingerprint"), "{e}");
                assert!(!me.work.join("9.9.9/new").exists());
            }
        }
        // A release without fingerprints isn't used.
        let release = Release { version: "9.9.9".into(), url: String::new(), notes: String::new(), assets: vec![super::super::update::Asset { name: zip_name, url: "http://127.0.0.1:9/zip".into(), size: 1 }] };
        let me = Install { platform: Platform::Mac, item: d.join("Cutaway.app"), work: d.join("w3") };
        assert!(prepare(&release, &me, &AtomicBool::new(false), &mut |_, _| {}, &mut || {}).unwrap_err().to_string().contains("fingerprints"));
    }

    /// A tiny web server answering each path with its body.
    #[cfg(unix)]
    fn serve(files: Vec<(String, Vec<u8>)>) -> String {
        use std::io::{BufRead, BufReader, Write};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in l.incoming().take(files.len()) {
                let Ok(mut s) = stream else { continue };
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut first = String::new();
                r.read_line(&mut first).unwrap();
                loop {
                    let mut line = String::new();
                    if r.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                        break;
                    }
                }
                let path = first.split_whitespace().nth(1).unwrap_or("/").to_string();
                let body = files.iter().find(|(p, _)| *p == path).map(|(_, b)| b.clone()).unwrap_or_default();
                let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = s.write_all(&body);
            }
        });
        format!("http://{addr}")
    }

    /// Windows' helper, run for real with PowerShell (on Windows).
    #[cfg(windows)]
    #[test]
    fn the_powershell_helper_swaps_and_goes_back() {
        for starts in [true, false] {
            let d = temp(if starts { "ps-ok" } else { "ps-back" });
            let dir = d.join("Cutaway");
            let me = Install { platform: Platform::Windows, item: dir.clone(), work: dir.join(".update") };
            std::fs::create_dir_all(dir.join("licenses")).unwrap();
            std::fs::write(dir.join("Cutaway.exe"), "old").unwrap();
            std::fs::write(dir.join("old-only.dll"), "old").unwrap();
            let staged = Staged { version: "9.9.9".into(), new_item: me.work.join("9.9.9\\new\\Cutaway") };
            std::fs::create_dir_all(staged.new_item.join("licenses")).unwrap();
            std::fs::write(staged.new_item.join("Cutaway.exe"), "new").unwrap();
            std::fs::create_dir_all(&me.work).unwrap();
            let mut cutaway = Command::new("cmd").args(["/c", "ping -n 2 127.0.0.1 > nul"]).spawn().unwrap();
            let pid = cutaway.id();
            let reaper = std::thread::spawn(move || cutaway.wait());
            let mut swap = Swap::new(&me, &staged, pid, true);
            let opened = d.join("opened-old");
            let marker = |p: &Path| format!("Start-Process -FilePath 'cmd.exe' -ArgumentList '/c','type nul > \"{}\"' -WindowStyle Hidden -PassThru", p.display());
            swap.launch = if starts { marker(&swap.started) } else { "Start-Process -FilePath 'cmd.exe' -ArgumentList '/c','exit 1' -WindowStyle Hidden -PassThru".into() };
            swap.launch_old = format!("New-Item -ItemType File -Path {} -Force | Out-Null", ps(&opened));
            let script = d.join("install.ps1");
            std::fs::write(&script, format!("\u{feff}{}", swap.powershell_script())).unwrap();
            let status = Command::new("powershell.exe").args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"]).arg(&script).status().unwrap();
            let _ = reaper.join();
            let log = std::fs::read_to_string(&swap.log).unwrap_or_default();
            let exe = std::fs::read_to_string(dir.join("Cutaway.exe")).unwrap_or_default();
            if starts {
                assert!(status.success(), "{log}");
                assert_eq!(exe, "new", "{log}");
                assert!(!dir.join("old-only.dll").exists(), "the old version's files are set aside");
                assert!(swap.previous.join("old-only.dll").exists() && swap.previous.join("Cutaway.exe").exists());
                assert!(dir.join(".update").exists(), "the update folder stays where it was");
            } else {
                assert!(!status.success());
                assert_eq!(exe, "old", "back to the old version: {log}");
                assert!(dir.join("old-only.dll").exists() && dir.join("licenses").exists());
                assert!(swap.rolled_back.exists() && opened.exists(), "{log}");
            }
        }
    }
}
