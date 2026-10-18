//! AAC decoding with AudioToolbox.

use crate::engine::decode::AudioDecoder;
use crate::media::{mp4, AudioConfig, AAC_FRAME};
use anyhow::{bail, Result};
use objc2_audio_toolbox::{
    kAudioConverterDecompressionMagicCookie, AudioConverterDispose, AudioConverterFillComplexBuffer, AudioConverterNew, AudioConverterRef,
    AudioConverterSetProperty,
};
use objc2_core_audio_types::{
    kAudioFormatFlagIsFloat, kAudioFormatFlagIsPacked, kAudioFormatLinearPCM, kAudioFormatMPEG4AAC, AudioBuffer, AudioBufferList,
    AudioStreamBasicDescription, AudioStreamPacketDescription,
};
use std::ffi::c_void;
use std::ptr::NonNull;

/// The one packet waiting to be pulled by the converter.
struct Packet {
    data: Vec<u8>,
    desc: AudioStreamPacketDescription,
    taken: bool,
}

const NO_DATA: i32 = 0x6e6f6461; // 'noda'

unsafe extern "C-unwind" fn input_proc(
    _conv: AudioConverterRef,
    packets: NonNull<u32>,
    data: NonNull<AudioBufferList>,
    desc: *mut *mut AudioStreamPacketDescription,
    user: *mut c_void,
) -> i32 {
    let p = unsafe { &mut *(user as *mut Packet) };
    let list = unsafe { &mut *data.as_ptr() };
    if p.taken || p.data.is_empty() {
        unsafe { *packets.as_ptr() = 0 };
        list.mBuffers[0].mDataByteSize = 0;
        list.mBuffers[0].mData = std::ptr::null_mut();
        return NO_DATA;
    }
    p.taken = true;
    list.mNumberBuffers = 1;
    list.mBuffers[0].mDataByteSize = p.data.len() as u32;
    list.mBuffers[0].mData = p.data.as_mut_ptr().cast();
    p.desc = AudioStreamPacketDescription { mStartOffset: 0, mVariableFramesInPacket: 0, mDataByteSize: p.data.len() as u32 };
    if !desc.is_null() {
        unsafe { *desc = &mut p.desc };
    }
    unsafe { *packets.as_ptr() = 1 };
    0
}

pub struct AtAacDecoder {
    conv: AudioConverterRef,
    rate: u32,
    channels: u16,
    packet: Box<Packet>,
}

unsafe impl Send for AtAacDecoder {}

impl AtAacDecoder {
    pub fn new(asc: &[u8]) -> Result<Self> {
        let (rate, channels) = crate::media::demux::parse_asc(asc).unwrap_or((48_000, 2));
        let ch = channels as u32;
        let mut input = AudioStreamBasicDescription {
            mSampleRate: rate as f64,
            mFormatID: kAudioFormatMPEG4AAC,
            mFormatFlags: 0,
            mBytesPerPacket: 0,
            mFramesPerPacket: AAC_FRAME,
            mBytesPerFrame: 0,
            mChannelsPerFrame: ch,
            mBitsPerChannel: 0,
            mReserved: 0,
        };
        let mut output = AudioStreamBasicDescription {
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
        let mut conv: AudioConverterRef = std::ptr::null_mut();
        let st = unsafe { AudioConverterNew(NonNull::from(&mut input), NonNull::from(&mut output), NonNull::from(&mut conv)) };
        if st != 0 || conv.is_null() {
            bail!("AudioToolbox can't decode this AAC audio (error {st})");
        }
        let mut cookie = mp4::es_descriptor(&AudioConfig { rate, channels, asc: asc.to_vec(), priming: 0 });
        let st = unsafe { AudioConverterSetProperty(conv, kAudioConverterDecompressionMagicCookie, cookie.len() as u32, NonNull::new(cookie.as_mut_ptr().cast()).unwrap()) };
        if st != 0 {
            eprintln!("AAC decoder: the stream description wasn't accepted (error {st})");
        }
        let packet = Box::new(Packet { data: Vec::new(), desc: AudioStreamPacketDescription { mStartOffset: 0, mVariableFramesInPacket: 0, mDataByteSize: 0 }, taken: true });
        Ok(AtAacDecoder { conv, rate, channels, packet })
    }
}

impl AudioDecoder for AtAacDecoder {
    fn decode(&mut self, frame: &[u8]) -> Result<Vec<f32>> {
        self.packet.data = frame.to_vec();
        self.packet.taken = false;
        let ch = self.channels as usize;
        let mut out = vec![0f32; AAC_FRAME as usize * ch];
        let mut list = AudioBufferList {
            mNumberBuffers: 1,
            mBuffers: [AudioBuffer { mNumberChannels: ch as u32, mDataByteSize: (out.len() * 4) as u32, mData: out.as_mut_ptr().cast() }],
        };
        let mut frames: u32 = AAC_FRAME;
        let user: *mut Packet = &mut *self.packet;
        let st = unsafe { AudioConverterFillComplexBuffer(self.conv, Some(input_proc), user.cast(), NonNull::from(&mut frames), NonNull::from(&mut list), std::ptr::null_mut()) };
        if st != 0 && st != NO_DATA {
            bail!("AAC decoding failed (error {st})");
        }
        out.truncate(frames as usize * ch);
        Ok(out)
    }

    fn rate(&self) -> u32 {
        self.rate
    }

    fn channels(&self) -> u16 {
        self.channels
    }
}

impl Drop for AtAacDecoder {
    fn drop(&mut self) {
        unsafe { AudioConverterDispose(self.conv) };
    }
}
