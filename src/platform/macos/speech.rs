//! Live captions on macOS: the Speech framework, on-device only (nothing
//! leaves the Mac, and it works offline).

use crate::platform::speech::{Event, Recognizer, Word};
use anyhow::{anyhow, bail, Result};
use block2::RcBlock;
use crossbeam_channel::{bounded, unbounded, Receiver, Sender};
use objc2::rc::Retained;
use objc2::AllocAnyThread;
use objc2_avf_audio::{AVAudioFormat, AVAudioPCMBuffer};
use objc2_foundation::{NSBundle, NSError, NSLocale, NSString};
use objc2_speech::{
    SFSpeechAudioBufferRecognitionRequest, SFSpeechRecognitionResult, SFSpeechRecognitionTask, SFSpeechRecognitionTaskHint, SFSpeechRecognizer,
    SFSpeechRecognizerAuthorizationStatus,
};
use std::time::Duration;

/// Why captions can't run, if they can't.
pub fn problem() -> Option<String> {
    // Without the usage text in the app's Info.plist, macOS stops the app
    // the moment speech recognition is touched (e.g. run from Terminal).
    let key = NSString::from_str("NSSpeechRecognitionUsageDescription");
    if NSBundle::mainBundle().objectForInfoDictionaryKey(&key).is_none() {
        return Some("live captions need Cutaway.app (not run from Terminal)".into());
    }
    match unsafe { SFSpeechRecognizer::authorizationStatus() } {
        SFSpeechRecognizerAuthorizationStatus::Denied => Some("speech recognition is turned off for Cutaway: turn it on in System Settings → Privacy & Security → Speech Recognition".into()),
        SFSpeechRecognizerAuthorizationStatus::Restricted => Some("speech recognition isn't allowed on this Mac".into()),
        _ => None,
    }
}

pub fn languages() -> Vec<(String, String)> {
    if problem().is_some() {
        return Vec::new();
    }
    let set = unsafe { SFSpeechRecognizer::supportedLocales() };
    let all = set.allObjects();
    let here = NSLocale::currentLocale();
    let mut out: Vec<(String, String)> = (0..all.count())
        .map(|i| {
            let id = all.objectAtIndex(i).localeIdentifier();
            let name = here.localizedStringForLocaleIdentifier(&id).to_string();
            (id.to_string().replace('_', "-"), name)
        })
        .collect();
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out
}

/// Ask for permission (once); waits for the answer.
fn authorize() -> Result<()> {
    if let Some(p) = problem() {
        bail!("{p}");
    }
    if unsafe { SFSpeechRecognizer::authorizationStatus() } == SFSpeechRecognizerAuthorizationStatus::Authorized {
        return Ok(());
    }
    let (tx, rx) = bounded(1);
    let block = RcBlock::new(move |status: SFSpeechRecognizerAuthorizationStatus| {
        let _ = tx.try_send(status);
    });
    unsafe { SFSpeechRecognizer::requestAuthorization(&block) };
    match rx.recv_timeout(Duration::from_secs(120)) {
        Ok(SFSpeechRecognizerAuthorizationStatus::Authorized) => Ok(()),
        Ok(_) => bail!("speech recognition wasn't allowed; turn it on in System Settings → Privacy & Security → Speech Recognition"),
        Err(_) => bail!("no answer to the speech recognition permission request"),
    }
}

struct Session {
    request: Retained<SFSpeechAudioBufferRecognitionRequest>,
    _task: Retained<SFSpeechRecognitionTask>,
}

pub struct MacRecognizer {
    recognizer: Retained<SFSpeechRecognizer>,
    format: Retained<AVAudioFormat>,
    current: Session,
    /// Requests ended but not yet finished, kept alive until they are.
    finishing: Vec<(u64, Session, std::time::Instant)>,
    request: u64,
    tx: Sender<Event>,
    rx: Receiver<Event>,
}

// The Speech objects are only touched from the captions thread; results
// arrive through the channel.
unsafe impl Send for MacRecognizer {}

fn session(recognizer: &SFSpeechRecognizer, id: u64, tx: Sender<Event>) -> Session {
    unsafe {
        let request = SFSpeechAudioBufferRecognitionRequest::new();
        request.setShouldReportPartialResults(true);
        request.setRequiresOnDeviceRecognition(true);
        request.setAddsPunctuation(true);
        request.setTaskHint(SFSpeechRecognitionTaskHint::Dictation);
        let handler = RcBlock::new(move |result: *mut SFSpeechRecognitionResult, error: *mut NSError| {
            if let Some(r) = result.as_ref() {
                let best = r.bestTranscription();
                let text = best.formattedString().to_string();
                if r.isFinal() {
                    let segs = best.segments();
                    let words = (0..segs.count())
                        .map(|i| {
                            let s = segs.objectAtIndex(i);
                            Word { text: s.substring().to_string(), start: s.timestamp(), duration: s.duration() }
                        })
                        .collect();
                    let _ = tx.send(Event::Final { request: id, text, words });
                } else {
                    let _ = tx.send(Event::Partial { request: id, text });
                }
            } else if let Some(e) = error.as_ref() {
                let message = e.localizedDescription().to_string();
                // Ending a request with nothing said isn't a problem.
                if e.code() != 1110 && e.code() != 216 && !message.to_lowercase().contains("no speech") {
                    let _ = tx.send(Event::Error { request: id, message });
                } else {
                    let _ = tx.send(Event::Final { request: id, text: String::new(), words: Vec::new() });
                }
            }
        });
        let task = recognizer.recognitionTaskWithRequest_resultHandler(&request, &handler);
        Session { request, _task: task }
    }
}

pub fn start(language: &str) -> Result<Box<dyn Recognizer>> {
    authorize()?;
    let recognizer = unsafe {
        if language.trim().is_empty() {
            SFSpeechRecognizer::new()
        } else {
            let locale = NSLocale::localeWithLocaleIdentifier(&NSString::from_str(&language.replace('-', "_")));
            SFSpeechRecognizer::initWithLocale(SFSpeechRecognizer::alloc(), &locale).ok_or_else(|| anyhow!("speech recognition doesn't support {language}"))?
        }
    };
    if !unsafe { recognizer.supportsOnDeviceRecognition() } {
        bail!("on-device recognition isn't available for this language on this Mac. Turn on Dictation for it in System Settings → Keyboard (that downloads it), or choose another language");
    }
    let format = unsafe { AVAudioFormat::initStandardFormatWithSampleRate_channels(AVAudioFormat::alloc(), 48_000.0, 1) }.ok_or_else(|| anyhow!("couldn't set up the audio format"))?;
    let (tx, rx) = unbounded();
    let current = session(&recognizer, 1, tx.clone());
    Ok(Box::new(MacRecognizer { recognizer, format, current, finishing: Vec::new(), request: 1, tx, rx }))
}

impl Recognizer for MacRecognizer {
    fn push(&mut self, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }
        unsafe {
            let Some(buf) = AVAudioPCMBuffer::initWithPCMFormat_frameCapacity(AVAudioPCMBuffer::alloc(), &self.format, samples.len() as u32) else { return };
            buf.setFrameLength(samples.len() as u32);
            let data = buf.floatChannelData();
            if data.is_null() {
                return;
            }
            let ch0 = (*data).as_ptr();
            std::ptr::copy_nonoverlapping(samples.as_ptr(), ch0, samples.len());
            self.current.request.appendAudioPCMBuffer(&buf);
        }
        // Forget ended requests that never reported back.
        self.finishing.retain(|(_, _, at)| at.elapsed() < Duration::from_secs(20));
    }

    fn restart(&mut self) -> u64 {
        self.request += 1;
        let next = session(&self.recognizer, self.request, self.tx.clone());
        let old = std::mem::replace(&mut self.current, next);
        unsafe { old.request.endAudio() };
        self.finishing.push((self.request - 1, old, std::time::Instant::now()));
        self.request
    }

    fn events(&self) -> Receiver<Event> {
        self.rx.clone()
    }
}

impl Drop for MacRecognizer {
    fn drop(&mut self) {
        unsafe { self.current.request.endAudio() };
    }
}
