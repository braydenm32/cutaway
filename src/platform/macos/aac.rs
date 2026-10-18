//! AAC encoding with AudioToolbox.

use crate::engine::encode::AudioEncoder;
use crate::media::{h264, AudioConfig, AAC_FRAME};
use anyhow::{bail, Result};
use objc2_audio_toolbox::{
    kAudioConverterEncodeBitRate, kAudioConverterPrimeInfo, kAudioConverterPropertyMaximumOutputPacketSize, AudioConverterDispose,
    AudioConverterFillComplexBuffer, AudioConverterGetProperty, AudioConverterNew, AudioConverterPrimeInfo, AudioConverterRef,
    AudioConverterSetProperty,
};
use objc2_core_audio_types::{
    kAudioFormatFlagIsFloat, kAudioFormatFlagIsPacked, kAudioFormatLinearPCM, kAudioFormatMPEG4AAC, AudioBuffer, AudioBufferList,
    AudioStreamBasicDescription, AudioStreamPacketDescription,
};
use std::ffi::c_void;
use std::ptr::NonNull;

/// Returned by the input callback when it has nothing more right now.
const NEED_MORE: i32 = 0x6d6f7265; // 'more'
const MPEG4_OBJECT_AAC_LC: u32 = 2;

/// Input waiting to be pulled by the converter.
struct Feed {
    data: Vec<f32>,
    /// Frames already handed to the converter.
    offset: usize,
    channels: usize,
    end: bool,
}

unsafe extern "C-unwind" fn input_proc(
    _conv: AudioConverterRef,
    packets: NonNull<u32>,
    data: NonNull<AudioBufferList>,
    _desc: *mut *mut AudioStreamPacketDescription,
    user: *mut c_void,
) -> i32 {
    let feed = unsafe { &mut *(user as *mut Feed) };
    let available = feed.data.len() / feed.channels - feed.offset;
    let list = unsafe { &mut *data.as_ptr() };
    if available == 0 {
        unsafe { *packets.as_ptr() = 0 };
        list.mBuffers[0].mDataByteSize = 0;
        list.mBuffers[0].mData = std::ptr::null_mut();
        return if feed.end { 0 } else { NEED_MORE };
    }
    let n = (unsafe { *packets.as_ptr() } as usize).min(available);
    list.mNumberBuffers = 1;
    list.mBuffers[0].mNumberChannels = feed.channels as u32;
    list.mBuffers[0].mDataByteSize = (n * feed.channels * 4) as u32;
    list.mBuffers[0].mData = unsafe { feed.data.as_mut_ptr().add(feed.offset * feed.channels) }.cast();
    feed.offset += n;
    unsafe { *packets.as_ptr() = n as u32 };
    0
}

pub struct AtAacEncoder {
    conv: AudioConverterRef,
    config: AudioConfig,
    feed: Box<Feed>,
    max_packet: usize,
}

// The converter is only used from the thread that owns the encoder.
unsafe impl Send for AtAacEncoder {}

impl AtAacEncoder {
    pub fn new(rate: u32, channels: u16, bitrate_kbps: u32) -> Result<Self> {
        let ch = channels as u32;
        let mut input = AudioStreamBasicDescription {
            mSampleRate: rate as f64,
            mFormatID: kAudioFormatLinearPCM,
            mFormatFlags: kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked,
            mBytesPerPacket: 4 * ch,
            mFramesPerPacket: 1,
            mBytesPerFrame: 4 * ch,
            mChannelsPerFrame: ch,
            mBitsPerChannel: 32,
            mReserved: 0,
        };
        let mut output = AudioStreamBasicDescription {
            mSampleRate: rate as f64,
            mFormatID: kAudioFormatMPEG4AAC,
            mFormatFlags: MPEG4_OBJECT_AAC_LC,
            mBytesPerPacket: 0,
            mFramesPerPacket: AAC_FRAME,
            mBytesPerFrame: 0,
            mChannelsPerFrame: ch,
            mBitsPerChannel: 0,
            mReserved: 0,
        };
        let mut conv: AudioConverterRef = std::ptr::null_mut();
        let st = unsafe { AudioConverterNew(NonNull::from(&mut input), NonNull::from(&mut output), NonNull::from(&mut conv)) };
        if st != 0 || conv.is_null() {
            bail!("AudioToolbox can't make an AAC encoder (error {st})");
        }
        let mut bps: u32 = bitrate_kbps * 1000;
        let st = unsafe { AudioConverterSetProperty(conv, kAudioConverterEncodeBitRate, 4, NonNull::from(&mut bps).cast()) };
        if st != 0 {
            eprintln!("AAC: {bitrate_kbps} kbps isn't supported here (error {st}); using the default");
        }
        let mut prime = AudioConverterPrimeInfo { leadingFrames: 0, trailingFrames: 0 };
        let mut size = std::mem::size_of::<AudioConverterPrimeInfo>() as u32;
        unsafe { AudioConverterGetProperty(conv, kAudioConverterPrimeInfo, NonNull::from(&mut size), NonNull::from(&mut prime).cast()) };
        let mut max_packet: u32 = 0;
        let mut size = 4u32;
        unsafe { AudioConverterGetProperty(conv, kAudioConverterPropertyMaximumOutputPacketSize, NonNull::from(&mut size), NonNull::from(&mut max_packet).cast()) };
        Ok(AtAacEncoder {
            conv,
            config: AudioConfig {
                rate,
                channels,
                asc: h264::aac_asc(rate, channels),
                priming: if prime.leadingFrames > 0 { prime.leadingFrames } else { 2112 },
            },
            feed: Box::new(Feed { data: Vec::new(), offset: 0, channels: channels as usize, end: false }),
            max_packet: (max_packet as usize).max(2048),
        })
    }

    /// One AAC frame, if the converter has enough input for it.
    fn pull(&mut self) -> Result<Option<Vec<u8>>> {
        let mut out = vec![0u8; self.max_packet];
        let mut list = AudioBufferList {
            mNumberBuffers: 1,
            mBuffers: [AudioBuffer { mNumberChannels: self.config.channels as u32, mDataByteSize: out.len() as u32, mData: out.as_mut_ptr().cast() }],
        };
        let mut packets: u32 = 1;
        let mut desc = AudioStreamPacketDescription { mStartOffset: 0, mVariableFramesInPacket: 0, mDataByteSize: 0 };
        let feed: *mut Feed = &mut *self.feed;
        let st = unsafe {
            AudioConverterFillComplexBuffer(self.conv, Some(input_proc), feed.cast(), NonNull::from(&mut packets), NonNull::from(&mut list), &mut desc)
        };
        // Forget input the converter has taken.
        let f = &mut *self.feed;
        f.data.drain(..f.offset * f.channels);
        f.offset = 0;
        if st != 0 && st != NEED_MORE {
            bail!("AAC encoding failed (error {st})");
        }
        if packets == 0 {
            return Ok(None);
        }
        let size = if desc.mDataByteSize > 0 { desc.mDataByteSize } else { list.mBuffers[0].mDataByteSize } as usize;
        out.truncate(size);
        Ok(Some(out))
    }
}

impl AudioEncoder for AtAacEncoder {
    fn config(&self) -> AudioConfig {
        self.config.clone()
    }

    fn push(&mut self, samples: &[f32]) -> Result<Vec<Vec<u8>>> {
        self.feed.data.extend_from_slice(samples);
        let mut frames = Vec::new();
        while self.feed.data.len() / self.feed.channels >= AAC_FRAME as usize {
            match self.pull()? {
                Some(f) => frames.push(f),
                None => break,
            }
        }
        Ok(frames)
    }

    fn finish(&mut self) -> Result<Vec<Vec<u8>>> {
        self.feed.end = true;
        let mut frames = Vec::new();
        for _ in 0..64 {
            match self.pull()? {
                Some(f) => frames.push(f),
                None => break,
            }
        }
        Ok(frames)
    }
}

impl Drop for AtAacEncoder {
    fn drop(&mut self) {
        unsafe { AudioConverterDispose(self.conv) };
    }
}
