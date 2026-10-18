//! Cutaway's virtual camera for Windows 11.
//!
//! A Media Foundation media source that Windows' camera service (Frame
//! Server) loads when an app opens the "Cutaway" camera, which Cutaway
//! creates with `MFCreateVirtualCamera` while its Webcam button is on. The
//! pictures come from Cutaway through shared memory (see `shared.rs`);
//! when none arrive, the camera shows a dark picture.
//!
//! COM pieces: a class factory → an activation object (IMFActivate) → the
//! media source (IMFMediaSourceEx, plus IKsControl and IMFGetService that
//! Frame Server asks for) → one video stream (IMFMediaStream2).

#![allow(non_snake_case)]

pub mod shared;

#[cfg(windows)]
mod com;

#[cfg(windows)]
pub use com::*;
