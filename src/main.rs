//! Cutaway: record many video sources at once (one file each), switch a
//! composited program feed live, stream it, and record audio from an
//! interface/DAW alongside — a software take on hardware like the
//! RØDECaster Video.
//!
//! Everything is built in: capture, GPU compositing, encoding, file
//! writing and streaming need nothing installed besides the app.

// Windows: an app, not a console program (no black window behind it).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod config;
mod control;
mod devices;
mod engine;
mod logging;
mod media;
mod net;
mod platform;
mod recovery;
mod report;
mod scene;
mod secrets;
mod settings;
mod ui;
mod util;

/// Tests on Windows: an access violation takes the whole test run down
/// without saying which test caused it. libtest names each test's thread
/// after the test, so a first-chance exception handler can say.
#[cfg(all(test, windows))]
mod crash_report {
    use std::ffi::c_void;
    use std::io::Write;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[repr(C)]
    struct ExceptionRecord {
        code: u32,
        flags: u32,
        next: *mut ExceptionRecord,
        address: *mut c_void,
        params: u32,
        info: [usize; 15],
    }

    #[repr(C)]
    struct ExceptionPointers {
        record: *mut ExceptionRecord,
        context: *mut c_void,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn AddVectoredExceptionHandler(first: u32, handler: unsafe extern "system" fn(*mut ExceptionPointers) -> i32) -> *mut c_void;
    }

    static SEEN: AtomicU32 = AtomicU32::new(0);

    unsafe extern "system" fn handler(p: *mut ExceptionPointers) -> i32 {
        // SAFETY: Windows passes valid exception pointers to the handler.
        let rec = unsafe { &*(*p).record };
        if rec.code == 0xC000_0005 && SEEN.fetch_add(1, Ordering::Relaxed) < 3 {
            let what = if rec.info[0] == 0 { "reading" } else if rec.info[0] == 1 { "writing" } else { "running" };
            let thread = std::thread::current();
            let mut err = std::io::stderr().lock();
            let _ = writeln!(err, "\n*** crash: access violation {what} {:#x} at {:p}, in test thread {:?}", rec.info[1], rec.address, thread.name());
            let _ = err.flush();
            let _ = writeln!(err, "{}", std::backtrace::Backtrace::force_capture());
            let _ = err.flush();
        }
        0 // keep searching: this only reports
    }

    extern "C" fn install() {
        // SAFETY: registers a handler that only reads the record and prints.
        unsafe {
            AddVectoredExceptionHandler(1, handler);
        }
    }

    // Runs when the test program starts, before main.
    #[used]
    #[unsafe(link_section = ".CRT$XCU")]
    static INSTALL: extern "C" fn() = install;
}

use crate::util::LockExt;
use anyhow::{Context, Result};
use clap::Parser;
use config::{AudioConfig, Config, SourceConfig, TransitionMode};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Parser, Debug)]
#[command(name = "cutaway", version, about = "Multi-camera ISO recorder and live switcher")]
struct Args {
    /// Settings file. Created with your detected devices on first run.
    #[arg(long)]
    config: Option<PathBuf>,

    /// List cameras, capture cards and audio inputs, then exit.
    #[arg(long)]
    list_devices: bool,

    /// Show which video/audio encoders this machine will use, then exit.
    #[arg(long)]
    probe_encoders: bool,

    /// Create the settings file if needed (auto-detecting devices), print it and exit.
    #[arg(long)]
    init: bool,

    /// Use N test pictures and a test tone instead of real devices.
    #[arg(long, value_name = "N")]
    demo: Option<usize>,

    /// Run without a window: record a take of --seconds and exit.
    #[arg(long)]
    headless: bool,

    /// Length of the headless take, in seconds.
    #[arg(long, default_value_t = 10.0)]
    seconds: f64,

    /// In headless mode, go to the next scene every this many seconds
    /// (alternating cuts and mixes).
    #[arg(long)]
    switch_every: Option<f64>,

    /// In headless mode, also go live to the destinations in Settings.
    #[arg(long)]
    stream: bool,

    /// Add a stream destination just for this run (rtmp://server/app/key,
    /// rtmps://... or srt://host:port). Can be given more than once.
    #[arg(long, value_name = "URL")]
    stream_to: Vec<String>,

    /// In headless mode, don't record (stream only).
    #[arg(long)]
    no_record: bool,

    /// In headless mode, drop a marker at every scene change.
    #[arg(long)]
    markers: bool,

    /// Measure lip sync (clap once within --seconds), print the delays it
    /// suggests and exit.
    #[arg(long)]
    measure_sync: bool,

    /// Test hook: end the stream halfway through and go live again.
    #[arg(long, hide = true)]
    restream_test: bool,

    /// Scripted UI actions, for automated screenshots.
    #[arg(long, hide = true)]
    ui_test: bool,

    /// Windows: install the virtual camera (run elevated by Cutaway).
    #[arg(long, hide = true)]
    vcam_install: bool,

    /// Windows: remove the virtual camera (run elevated by Cutaway).
    #[arg(long, hide = true)]
    vcam_uninstall: bool,
}

fn main() {
    // Started from a terminal on Windows: print there (--list-devices etc.).
    #[cfg(windows)]
    platform::win::attach_console();
    if let Err(e) = run() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    // --version (as the updater asks a new version) answers without touching
    // the log.
    if std::env::args().skip(1).any(|a| a == "--version" || a == "-V") {
        Args::parse();
    }
    logging::init();
    let args = Args::parse();

    if args.list_devices {
        devices::print_devices();
        return Ok(());
    }

    #[cfg(windows)]
    if args.vcam_install || args.vcam_uninstall {
        return platform::win::vcam::install_elevated(args.vcam_uninstall).context("virtual camera");
    }

    let cfg_path = args.config.clone().unwrap_or_else(config::default_config_path);
    let first_run = args.demo.is_none() && !cfg_path.exists();
    let mut cfg = if let Some(n) = args.demo {
        demo_config(n)
    } else if cfg_path.exists() {
        Config::load(&cfg_path)?
    } else {
        let cfg = auto_config();
        cfg.save(&cfg_path)?;
        eprintln!("Created settings file {}", cfg_path.display());
        cfg
    };
    // The remote always has a PIN: one is made (and kept) for settings
    // from before that.
    let made_pin = cfg.remote.ensure_pin() && args.demo.is_none();
    if made_pin {
        cfg.save(&cfg_path)?;
        eprintln!("Made a PIN for the remote: {}", cfg.remote.pin);
    }
    if args.demo.is_some() {
        // Keep demo output next to the real recordings but don't overwrite settings.
        if let Ok(real) = Config::load(&cfg_path) {
            cfg.output.folder = real.output.folder;
        }
    }

    if args.init {
        println!("Settings: {}", cfg_path.display());
        println!("{}", toml::to_string_pretty(&cfg)?);
        return Ok(());
    }

    if args.probe_encoders {
        use crate::media::VideoCodec;
        for codec in [VideoCodec::H264, VideoCodec::Hevc] {
            let list = engine::encode::video_backends(codec);
            let names: Vec<&str> = list.iter().map(|b| engine::encode::describe_backend(b)).collect();
            println!("{}: {}", engine::encode::codec_name(codec), if names.is_empty() { "not available".into() } else { names.join(", ") });
        }
        match engine::encode::codec_problem(config::Codec::Prores.video_codec()) {
            None => println!("ProRes: available"),
            Some(why) => println!("ProRes: {why}"),
        }
        match engine::encode::create_aac(48_000, 2, 192) {
            Ok(_) => println!("AAC: available"),
            Err(e) => println!("AAC: {e:#}"),
        }
        println!("NDI: {}", if engine::ndi::available() { "installed" } else { "not installed (optional: NDI Tools from ndi.video)" });
        match engine::compositor::create_device() {
            Ok((_, _, name)) => println!("Compositor GPU: {name}"),
            Err(e) => println!("Compositor GPU: {e:#}"),
        }
        return Ok(());
    }

    for (i, url) in args.stream_to.iter().enumerate() {
        let srt = url.to_lowercase().starts_with("srt://");
        cfg.streaming.destinations.push(config::StreamDestination {
            name: format!("Command line {}", i + 1),
            service: if srt { config::Service::Srt } else { config::Service::CustomRtmp },
            server: url.clone(),
            key: String::new(),
            enabled: true,
            vertical: false,
        });
    }

    // Camera/microphone prompts (first launch only); answered in the background.
    platform::request_permissions();

    // Takes a crash left unfinished are repaired first: here for the
    // command line, in the background (with a progress card) in the window.
    if args.measure_sync || args.headless {
        for folder in recovery::folders(&cfg) {
            for take in recovery::recover_unfinished(&folder) {
                eprintln!("Recovered an unfinished take: {}", take.display());
            }
        }
    }
    if args.measure_sync {
        return measure_sync(&cfg, args.seconds);
    }
    if args.headless {
        let stream = args.stream || !args.stream_to.is_empty();
        return headless(&cfg, args.seconds, args.switch_every, stream, !args.no_record, args.restream_test, args.markers);
    }

    // With --demo, edits from the UI shouldn't overwrite the real settings.
    let save_path = if args.demo.is_some() { None } else { Some(cfg_path) };
    ui::run(cfg, save_path, args.ui_test, made_pin && !first_run, first_run)
}

/// Listen for a clap and print the delays that would line everything up.
fn measure_sync(cfg: &Config, seconds: f64) -> Result<()> {
    let engine = engine::Engine::start(cfg, Arc::new(|| {}))?;
    std::thread::sleep(Duration::from_millis(1500));
    println!("Clap once, sharply, where every camera can see it ({seconds:.0} s)…");
    let job = engine.measure_sync(seconds as f32);
    while job.result.lock_ok().is_none() {
        std::thread::sleep(Duration::from_millis(100));
    }
    let r = job.result.lock_ok().clone().unwrap();
    if let Some(p) = &r.problem {
        println!("Problem: {p}");
    }
    for (kind, items) in [("Video", &r.video), ("Audio", &r.audio)] {
        for i in items {
            let found = if i.event.is_some() { format!("found (clarity {:.0})", i.confidence) } else { "not found".into() };
            println!("{kind:5}  {:<20} {found:<20} delay {} ms -> {} ms", i.name, i.current_ms, i.suggested_ms);
        }
    }
    Ok(())
}

/// First-run settings from whatever is plugged in.
fn auto_config() -> Config {
    let mut cfg = Config::default();
    let video = devices::list(devices::Kind::Video);
    for (i, d) in video.iter().take(8).enumerate() {
        // Two of the same camera (or names alike in their first letters)
        // still get names of their own.
        let base = short_name(&d.name, i);
        let mut name = base.clone();
        let mut n = 2;
        while cfg.sources.iter().any(|s| s.name.eq_ignore_ascii_case(&name)) {
            name = format!("{base} {n}");
            n += 1;
        }
        cfg.sources.push(SourceConfig {
            name,
            device: d.name.clone(),
            ..Default::default()
        });
    }
    if cfg.sources.is_empty() {
        for i in 0..2 {
            cfg.sources.push(SourceConfig { name: format!("Test {}", i + 1), device: "test".into(), ..Default::default() });
        }
    }
    let audio = devices::list(devices::Kind::Audio);
    if let Some(d) = audio.first() {
        cfg.audio.push(AudioConfig { name: "DAW".into(), device: d.name.clone(), ..Default::default() });
    }
    cfg
}

fn short_name(device: &str, i: usize) -> String {
    let n: String = device.chars().take(18).collect();
    if n.trim().is_empty() { format!("Camera {}", i + 1) } else { n.trim().to_string() }
}

fn demo_config(n: usize) -> Config {
    let mut cfg = Config::default();
    for i in 0..n.clamp(1, 16) {
        cfg.sources.push(SourceConfig { name: format!("Cam {}", i + 1), device: "test".into(), ..Default::default() });
    }
    cfg.audio.push(AudioConfig { name: "DAW".into(), device: "test".into(), ..Default::default() });
    cfg
}

fn headless(cfg: &Config, seconds: f64, switch_every: Option<f64>, stream: bool, record: bool, restream: bool, markers: bool) -> Result<()> {
    let engine = engine::Engine::start(cfg, Arc::new(|| {}))?;
    println!("{} | {}", engine.encoder_description(), engine.info.compositor);
    std::thread::sleep(Duration::from_millis(800));
    if record {
        engine.start_recording()?;
    }
    if stream {
        engine.start_stream()?;
    }
    let scenes = cfg.effective_scenes();
    let start = Instant::now();
    let mut next_switch = switch_every;
    let mut next_report = 2.0;
    let mut restream_at = if restream { Some(seconds / 2.0) } else { None };
    let mut n = 0usize;
    while start.elapsed().as_secs_f64() < seconds {
        std::thread::sleep(Duration::from_millis(20));
        let t = start.elapsed().as_secs_f64();
        if let (Some(at), Some(every)) = (next_switch, switch_every) {
            if t >= at && !scenes.is_empty() {
                n += 1;
                let scene = &scenes[n % scenes.len()];
                let mode = if n.is_multiple_of(2) { TransitionMode::Cut } else { TransitionMode::Mix };
                println!("[{t:6.2}s] {} ({mode:?})", scene.name);
                engine.take_scene(scene, mode, cfg.transition.duration_ms);
                if markers && record {
                    engine.add_marker(Some(scene.name.clone()))?;
                }
                next_switch = Some(at + every);
            }
        }
        if restream_at.is_some_and(|at| t >= at) {
            restream_at = None;
            println!("[{t:6.2}s] ending the stream and going live again");
            engine.stop_stream();
            while engine.status.lock_ok().stream.stopping {
                std::thread::sleep(Duration::from_millis(20));
            }
            engine.start_stream()?;
        }
        let st = engine.status.lock_ok();
        if let Some(f) = &st.fatal {
            anyhow::bail!("engine failed: {f}");
        }
        if let Some(f) = &st.rec_failed {
            anyhow::bail!("recording failed: {f}");
        }
        if stream && t >= next_report {
            next_report += 2.0;
            let line: Vec<String> = st
                .stream
                .destinations
                .iter()
                .map(|d| format!("{}: {} {:.0} kbps", d.name, engine::streamer::describe(&d.state), d.kbps))
                .collect();
            println!("[{t:6.2}s] stream  {}", line.join(" | "));
        }
    }
    if stream {
        engine.stop_stream();
    }
    if record {
        engine.stop_recording();
    }
    engine.wait_finalized();
    // Stream shutdown runs in the background too; give it a moment.
    let deadline = Instant::now() + Duration::from_secs(8);
    while engine.is_streaming() || engine.status.lock_ok().stream.stopping {
        if Instant::now() > deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let st = engine.status.lock_ok();
    for line in st.log.iter().filter(|l| l.level != engine::LogLevel::Info) {
        println!("  {:?}: {}", line.level, line.text);
    }
    if record {
        let report = st.last_take.clone().context("take was not finalized")?;
        for (f, size) in &report.files {
            println!("  {}  {}", util::human_size(*size), f.display());
        }
        if !report.problems.is_empty() {
            anyhow::bail!("take finished with problems: {}", report.problems.join("; "));
        }
    }
    Ok(())
}
