//! Shared Media Foundation plumbing (Windows).
//!
//! Only the pieces the agent needs: runtime lifetime, MFT instantiation and
//! media-type helpers. Decoding deliberately runs on the synchronous software
//! MFTs — NV12 frames have to land in system memory for the preview and for
//! the virtual camera's shared memory anyway; a D3D11/DXVA path can be added
//! later if the CPU becomes the bottleneck.

use anyhow::{bail, Context, Result};
use windows::core::GUID;
use windows::Win32::Foundation::VARIANT_BOOL;
use windows::Win32::Media::MediaFoundation::{
    IMFActivate, IMFMediaType, IMFTransform, MFCreateMediaType, MFShutdown, MFStartup, MFTEnumEx,
    MFT_ENUM_FLAG_HARDWARE, MFT_ENUM_FLAG_SORTANDFILTER, MFT_ENUM_FLAG_SYNCMFT,
    MFT_REGISTER_TYPE_INFO, MF_VERSION,
};
use windows::Win32::System::Com::{
    CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_MULTITHREADED,
};
use windows::Win32::System::Variant::{
    VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_BOOL, VT_UI4,
};

/// Keeps Media Foundation (and COM) alive for the lifetime of the guard.
pub struct MediaFoundation;

impl MediaFoundation {
    pub fn start() -> Result<Self> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok().context("CoInitializeEx")?;
            MFStartup(MF_VERSION, 0).context("MFStartup")?;
        }
        Ok(Self)
    }
}

impl Drop for MediaFoundation {
    fn drop(&mut self) {
        unsafe {
            let _ = MFShutdown();
            CoUninitialize();
        }
    }
}

/// `MF_MT_FRAME_SIZE` style packing: high 32 bits = width, low = height.
pub fn pack_size(width: u32, height: u32) -> u64 {
    (u64::from(width) << 32) | u64::from(height)
}

pub fn unpack_size(packed: u64) -> (u32, u32) {
    ((packed >> 32) as u32, packed as u32)
}

/// Fraction attributes (`MF_MT_FRAME_RATE`, `MF_MT_PIXEL_ASPECT_RATIO`).
pub fn pack_ratio(numerator: u32, denominator: u32) -> u64 {
    (u64::from(numerator) << 32) | u64::from(denominator)
}

pub fn unpack_ratio(packed: u64) -> (u32, u32) {
    ((packed >> 32) as u32, packed as u32)
}

pub fn new_media_type() -> Result<IMFMediaType> {
    unsafe { MFCreateMediaType().context("MFCreateMediaType") }
}

/// First synchronous MFT of `category` matching the given types.
///
/// `input`/`output` are the major type + subtype pairs used to filter the
/// enumeration; pass `None` to ignore that side.
pub fn create_transform(
    category: GUID,
    input: Option<(GUID, GUID)>,
    output: Option<(GUID, GUID)>,
    prefer_hardware: bool,
) -> Result<IMFTransform> {
    let input_info = input.map(|(major, subtype)| MFT_REGISTER_TYPE_INFO {
        guidMajorType: major,
        guidSubtype: subtype,
    });
    let output_info = output.map(|(major, subtype)| MFT_REGISTER_TYPE_INFO {
        guidMajorType: major,
        guidSubtype: subtype,
    });

    let mut flags = MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER;
    if prefer_hardware {
        flags |= MFT_ENUM_FLAG_HARDWARE;
    }

    let mut activates: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0u32;
    unsafe {
        MFTEnumEx(
            category,
            flags,
            input_info.as_ref().map(|info| info as *const _),
            output_info.as_ref().map(|info| info as *const _),
            &mut activates,
            &mut count,
        )
        .context("MFTEnumEx")?;
    }

    if activates.is_null() || count == 0 {
        bail!("aucun MFT trouvé pour cette catégorie");
    }

    let entries = unsafe { std::slice::from_raw_parts(activates, count as usize) };
    let mut chosen = None;
    for entry in entries.iter().flatten() {
        if let Ok(transform) = unsafe { entry.ActivateObject::<IMFTransform>() } {
            chosen = Some(transform);
            break;
        }
    }
    unsafe { CoTaskMemFree(Some(activates as *const core::ffi::c_void)) };

    chosen.context("activation du MFT impossible")
}

/// `VARIANT` with `VT_BOOL` (used by the `ICodecAPI` setters).
pub fn variant_bool(value: bool) -> VARIANT {
    VARIANT {
        Anonymous: VARIANT_0 {
            Anonymous: core::mem::ManuallyDrop::new(VARIANT_0_0 {
                vt: VT_BOOL,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: VARIANT_0_0_0 {
                    boolVal: VARIANT_BOOL(if value { -1 } else { 0 }),
                },
            }),
        },
    }
}

/// `VARIANT` with `VT_UI4` (the union carries the value in its low 32 bits).
pub fn variant_u32(value: u32) -> VARIANT {
    VARIANT {
        Anonymous: VARIANT_0 {
            Anonymous: core::mem::ManuallyDrop::new(VARIANT_0_0 {
                vt: VT_UI4,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: VARIANT_0_0_0 { lVal: value as i32 },
            }),
        },
    }
}
