//! Network outputs for live streaming.

pub mod rtmp;
pub mod rtmp_server;
pub mod srt;
pub mod chat;
pub mod tunnel;

#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    use crate::engine::encode::{create_aac, create_video, Purpose, VideoSettings};
    use crate::media::testsrc::{TestPattern, TestTone};
    use crate::media::{flv, ts::TsMuxer, VideoCodec, VideoFrame, VideoPacket};
    use std::process::{Command, Stdio};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    fn have_ffmpeg() -> bool {
        Command::new("ffmpeg").arg("-version").stdout(Stdio::null()).status().is_ok()
    }

    /// Two seconds of encoded test video + audio.
    fn media() -> (Vec<VideoPacket>, Vec<Vec<u8>>, crate::media::AudioConfig) {
        let packets = Arc::new(Mutex::new(Vec::new()));
        let p2 = packets.clone();
        let s = VideoSettings { codec: VideoCodec::H264, width: 320, height: 180, fps: 30, bitrate_kbps: 600, purpose: Purpose::Stream, backend: "auto".into() };
        let mut enc = create_video(&s, Arc::new(move |p| p2.lock().unwrap().push(p))).unwrap();
        let mut tp = TestPattern::new("smpte", 320, 180);
        for i in 0..60i64 {
            let pts = i * 1_000_000_000 / 30;
            let img = tp.render(pts, 30);
            enc.encode(&VideoFrame::new_nv12(320, 180, pts, img.y, img.uv, i as u64), pts, false).unwrap();
        }
        let mut aac = create_aac(48000, 2, 128).unwrap();
        let tone = TestTone::new(48000, 2, 440.0);
        let mut frames = Vec::new();
        for i in 0..200 {
            frames.extend(aac.push(&tone.render(i * 10_000_000, 480)).unwrap());
        }
        let v = packets.lock().unwrap().clone();
        (v, frames, aac.config())
    }

    fn probe(path: &std::path::Path) -> String {
        let o = Command::new("ffprobe").args(["-v", "error", "-show_entries", "stream=codec_name:format=duration", "-of", "csv=p=0"]).arg(path).output().unwrap();
        String::from_utf8_lossy(&o.stdout).to_string()
    }

    #[test]
    fn publishes_rtmp_and_srt() {
        if !have_ffmpeg() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("cutaway-net-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (video, audio, acfg) = media();

        // RTMP
        let flv_path = dir.join("rtmp.flv");
        let mut server = Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y", "-listen", "1", "-i", "rtmp://127.0.0.1:19350/live/test", "-c", "copy"])
            .arg(&flv_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(700));
        let mut p = super::rtmp::RtmpPublisher::connect("rtmp://127.0.0.1:19350/live/test", Duration::from_secs(5)).unwrap();
        p.send_metadata(&flv::metadata(&video[0].config, 30, 600, &acfg, 128)).unwrap();
        p.send_video(0, &flv::video_sequence_header(&video[0].config)).unwrap();
        p.send_audio(0, &flv::audio_sequence_header(&acfg)).unwrap();
        let mut ai = 0;
        for v in &video {
            let ms = (v.pts / 1_000_000) as u32;
            while ai < audio.len() && (ai as u64 * 1024 * 1000 / 48000) as u32 <= ms {
                p.send_audio((ai as u64 * 1024 * 1000 / 48000) as u32, &flv::audio_frame(&audio[ai])).unwrap();
                ai += 1;
            }
            p.send_video(ms, &flv::video_frame(&v.data, v.keyframe, 0)).unwrap();
            p.poll().unwrap();
        }
        assert!(p.bytes_sent() > 10_000);
        p.close();
        let _ = server.wait();
        let s = probe(&flv_path);
        assert!(s.contains("h264") && s.contains("aac"), "rtmp: {s}");

        // SRT
        let ts_path = dir.join("srt.ts");
        let mut server = Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y", "-i", "srt://127.0.0.1:19351?mode=listener", "-c", "copy"])
            .arg(&ts_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(700));
        let mut s = super::srt::SrtSender::connect("srt://127.0.0.1:19351", Duration::from_secs(5)).unwrap();
        let mut m = TsMuxer::new((*video[0].config).clone(), acfg.clone());
        let mut ai = 0;
        for v in &video {
            let mut out = Vec::new();
            while ai < audio.len() && (ai as i64 * 1024 * 1_000_000_000 / 48000) <= v.pts {
                out.extend(m.audio(ai as i64 * 1024 * 1_000_000_000 / 48000, &audio[ai]));
                ai += 1;
            }
            out.extend(m.video(v.pts, v.dts, v.keyframe, &v.data));
            s.send(&out).unwrap();
            std::thread::sleep(Duration::from_millis(5));
        }
        s.close();
        std::thread::sleep(Duration::from_millis(1500));
        let _ = server.kill();
        let _ = server.wait();
        let out = probe(&ts_path);
        assert!(out.contains("h264") && out.contains("aac"), "srt: {out}");
    }
}
