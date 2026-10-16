//! WebcamLink PC agent library: usbmux transport, the WCLN protocol and the
//! Media Foundation video pipeline.

pub mod clip;
pub mod color;
pub mod h264;
pub mod protocol;
pub mod session;
pub mod usbmux;

#[cfg(windows)]
pub mod decode;
#[cfg(windows)]
pub mod encode;
#[cfg(windows)]
pub mod mf;
#[cfg(windows)]
pub mod preview;
