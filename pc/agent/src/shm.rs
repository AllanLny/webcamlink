//! Shared memory bridge between the agent (user session) and the virtual
//! camera DLL (Frame Server, session 0) — see `docs/shm.md`.
//!
//! The DLL creates the `Global\` objects; the agent only opens them. Tests (and
//! a future dev tool) can create local-namespace objects with the same layout.

use anyhow::{bail, Context, Result};
use std::ffi::c_void;
use std::sync::atomic::{compiler_fence, Ordering};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, GENERIC_ALL, GENERIC_READ,
    GENERIC_WRITE, GetLastError, HANDLE,
};
use windows::Win32::Security::{
    ACL, ACL_REVISION, AddAccessAllowedAce, CreateWellKnownSid, InitializeAcl,
    InitializeSecurityDescriptor, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES,
    SECURITY_DESCRIPTOR, SetSecurityDescriptorDacl, WinBuiltinUsersSid, WinLocalServiceSid,
    WinLocalSystemSid,
};
use windows::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_READ, FILE_MAP_WRITE, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
    OpenFileMappingW, PAGE_READWRITE, UnmapViewOfFile,
};
use windows::Win32::System::Threading::{
    CreateEventW, EVENT_MODIFY_STATE, OpenEventW, SYNCHRONIZATION_ACCESS_RIGHTS, SetEvent,
};

/// `SYNCHRONIZE` access right (lives in `Win32::Storage::FileSystem`, which the
/// agent does not otherwise need).
const SYNCHRONIZE_ACCESS: u32 = 0x0010_0000;
const SECURITY_DESCRIPTOR_REVISION: u32 = 1;

pub const MAGIC: &[u8; 8] = b"WCLSHM01";
pub const VERSION: u32 = 1;
pub const HEADER_SIZE: usize = 128;
pub const SLOT_HEADER_SIZE: usize = 64;
pub const SLOT_COUNT: usize = 3;
pub const MAX_SLOT_CAPACITY: u32 = 3840 * 2160 * 3 / 2;
pub const NO_SLOT: u32 = u32::MAX;

// Header offsets (see docs/shm.md).
const OFF_MAGIC: usize = 0;
const OFF_VERSION: usize = 8;
const OFF_HEADER_SIZE: usize = 12;
const OFF_SLOT_COUNT: usize = 16;
const OFF_SLOT_CAPACITY: usize = 20;
const OFF_WIDTH: usize = 24;
const OFF_HEIGHT: usize = 28;
const OFF_STRIDE: usize = 32;
const OFF_PIXEL_FORMAT: usize = 36;
const OFF_FRAME_BYTES: usize = 40;
const OFF_PRODUCER_STATE: usize = 44;
const OFF_PRODUCER_ERROR: usize = 48;
const OFF_LATEST_SLOT: usize = 52;
const OFF_CONSUMER_ACTIVE: usize = 56;
const OFF_CONSUMER_WIDTH: usize = 60;
const OFF_CONSUMER_HEIGHT: usize = 64;
const OFF_CONSUMER_FPS: usize = 68;
const OFF_CONSUMER_GENERATION: usize = 72;
const OFF_PRODUCER_GENERATION: usize = 76;
const OFF_PTS_US: usize = 80;
const OFF_SEQUENCE: usize = 88;
const OFF_FRAMES_PUBLISHED: usize = 96;
const OFF_FRAMES_DROPPED: usize = 104;

// Slot-relative offsets.
const SLOT_STATE: usize = 0;
const SLOT_SEQUENCE: usize = 4;
const SLOT_PTS_US: usize = 8;
const SLOT_FRAME_BYTES: usize = 16;

pub const SLOT_FREE: u32 = 0;
pub const SLOT_WRITING: u32 = 1;
pub const SLOT_READY: u32 = 2;

/// `0` arrêté, `1` flux actif, `2` erreur (voir `docs/shm.md`).
pub const PRODUCER_STOPPED: u32 = 0;
pub const PRODUCER_STREAMING: u32 = 1;
pub const PRODUCER_ERROR: u32 = 2;

#[derive(Debug, Clone)]
pub struct ShmNames {
    pub mapping: String,
    pub event: String,
}

impl ShmNames {
    pub fn global() -> Self {
        Self { mapping: "Global\\WclShm".to_string(), event: "Global\\WclFrameReady".to_string() }
    }

    /// Local-namespace variant used by tests and dev tools.
    pub fn local(prefix: &str) -> Self {
        Self { mapping: format!("Local\\{prefix}Shm"), event: format!("Local\\{prefix}FrameReady") }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Control {
    pub consumer_active: bool,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub generation: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ShmStats {
    pub latest_slot: u32,
    pub pts_us: u64,
    pub sequence: u64,
    pub published: u64,
    pub dropped: u64,
    pub width: u32,
    pub height: u32,
}

pub struct ShmBlock {
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    mapping: HANDLE,
    event: HANDLE,
    slot_capacity: u32,
    view_size: usize,
    next_slot: usize,
}

// The mapping is only touched from the agent's single worker thread.
unsafe impl Send for ShmBlock {}

impl ShmBlock {
    fn base(&self) -> *mut u8 {
        self.view.Value as *mut u8
    }

    /// Open the objects created by the virtual camera. `Ok(None)` means the
    /// camera is not in use (objects not created yet).
    pub fn open(names: &ShmNames) -> Result<Option<Self>> {
        let mapping_name = wide(&names.mapping);
        let mapping = unsafe { OpenFileMappingW(FILE_MAP_READ.0 | FILE_MAP_WRITE.0, false, PCWSTR(mapping_name.as_ptr())) };
        let mapping = match mapping {
            Ok(handle) => handle,
            Err(error) if is_not_found(&error) => return Ok(None),
            Err(error) => return Err(error).context("OpenFileMappingW"),
        };

        let event_name = wide(&names.event);
        let event = unsafe {
            OpenEventW(
                SYNCHRONIZATION_ACCESS_RIGHTS(SYNCHRONIZE_ACCESS | EVENT_MODIFY_STATE.0),
                false,
                PCWSTR(event_name.as_ptr()),
            )
        };
        let event = match event {
            Ok(handle) => handle,
            Err(error) => {
                unsafe { let _ = CloseHandle(mapping); }
                if is_not_found(&error) {
                    return Ok(None);
                }
                return Err(error).context("OpenEventW");
            }
        };

        let view = unsafe { MapViewOfFile(mapping, FILE_MAP_READ | FILE_MAP_WRITE, 0, 0, 0) };
        if view.Value.is_null() {
            unsafe {
                let _ = CloseHandle(event);
                let _ = CloseHandle(mapping);
            }
            bail!("MapViewOfFile a échoué");
        }

        let block = Self {
            view,
            mapping,
            event,
            slot_capacity: 0,
            view_size: 0,
            next_slot: 0,
        };

        let magic = block.read_bytes(OFF_MAGIC, 8);
        if magic != MAGIC {
            bail!("mémoire partagée invalide (magic {magic:?})");
        }
        let version = block.read_u32(OFF_VERSION);
        if version != VERSION {
            bail!("version de mémoire partagée {version} non supportée");
        }

        let slot_capacity = block.read_u32(OFF_SLOT_CAPACITY);
        let mut block = block;
        block.slot_capacity = slot_capacity;
        block.view_size = HEADER_SIZE + SLOT_COUNT * (SLOT_HEADER_SIZE + slot_capacity as usize);
        Ok(Some(block))
    }

    /// Create the objects (DLL role, or tests in the local namespace).
    pub fn create(names: &ShmNames, slot_capacity: u32) -> Result<Self> {
        let capacity = slot_capacity.clamp(4096, MAX_SLOT_CAPACITY);
        let size = HEADER_SIZE + SLOT_COUNT * (SLOT_HEADER_SIZE + capacity as usize);
        let security = SecurityAttributes::build()?;

        let mapping_name = wide(&names.mapping);
        let mapping = unsafe {
            CreateFileMappingW(
                HANDLE::default(),
                Some(&security.attributes),
                PAGE_READWRITE,
                0,
                size as u32,
                PCWSTR(mapping_name.as_ptr()),
            )
        }
        .context("CreateFileMappingW")?;
        let already_exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;

        let event_name = wide(&names.event);
        let event = unsafe {
            CreateEventW(
                Some(&security.attributes),
                false,
                false,
                PCWSTR(event_name.as_ptr()),
            )
        }
        .context("CreateEventW")?;

        let view = unsafe { MapViewOfFile(mapping, FILE_MAP_READ | FILE_MAP_WRITE, 0, 0, 0) };
        if view.Value.is_null() {
            unsafe {
                let _ = CloseHandle(event);
                let _ = CloseHandle(mapping);
            }
            bail!("MapViewOfFile a échoué");
        }

        let mut block = Self {
            view,
            mapping,
            event,
            slot_capacity: capacity,
            view_size: size,
            next_slot: 0,
        };

        if !already_exists {
            for offset in (0..HEADER_SIZE).step_by(4) {
                block.write_u32(offset, 0);
            }
            block.write_bytes(OFF_MAGIC, MAGIC);
            block.write_u32(OFF_VERSION, VERSION);
            block.write_u32(OFF_HEADER_SIZE, HEADER_SIZE as u32);
            block.write_u32(OFF_SLOT_COUNT, SLOT_COUNT as u32);
            block.write_u32(OFF_SLOT_CAPACITY, capacity);
            block.write_u32(OFF_PIXEL_FORMAT, 0);
            block.write_u32(OFF_LATEST_SLOT, NO_SLOT);
            for slot in 0..SLOT_COUNT {
                block.write_slot_u32(slot, SLOT_STATE, SLOT_FREE);
                block.write_slot_u32(slot, SLOT_SEQUENCE, 0);
            }
        }

        Ok(block)
    }

    pub fn control(&self) -> Control {
        Control {
            consumer_active: self.read_u32(OFF_CONSUMER_ACTIVE) != 0,
            width: self.read_u32(OFF_CONSUMER_WIDTH),
            height: self.read_u32(OFF_CONSUMER_HEIGHT),
            fps: self.read_u32(OFF_CONSUMER_FPS),
            generation: self.read_u32(OFF_CONSUMER_GENERATION),
        }
    }

    /// Writer side of the control block (tests + future dev tools).
    pub fn set_control(&mut self, control: &Control) {
        self.write_u32(OFF_CONSUMER_ACTIVE, u32::from(control.consumer_active));
        self.write_u32(OFF_CONSUMER_WIDTH, control.width);
        self.write_u32(OFF_CONSUMER_HEIGHT, control.height);
        self.write_u32(OFF_CONSUMER_FPS, control.fps);
        self.write_u32(OFF_CONSUMER_GENERATION, control.generation);
    }

    pub fn slot_capacity(&self) -> u32 {
        self.slot_capacity
    }

    pub fn stats(&self) -> ShmStats {
        ShmStats {
            latest_slot: self.read_u32(OFF_LATEST_SLOT),
            pts_us: self.read_u64(OFF_PTS_US),
            sequence: self.read_u64(OFF_SEQUENCE),
            published: self.read_u64(OFF_FRAMES_PUBLISHED),
            dropped: self.read_u64(OFF_FRAMES_DROPPED),
            width: self.read_u32(OFF_WIDTH),
            height: self.read_u32(OFF_HEIGHT),
        }
    }

    /// Declare the current video format (compact NV12).
    pub fn set_video_format(&mut self, width: u32, height: u32) {
        let frame_bytes = width * height * 3 / 2;
        self.write_u32(OFF_WIDTH, width);
        self.write_u32(OFF_HEIGHT, height);
        self.write_u32(OFF_STRIDE, width);
        self.write_u32(OFF_FRAME_BYTES, frame_bytes);
        let generation = self.read_u32(OFF_PRODUCER_GENERATION).wrapping_add(1);
        self.write_u32(OFF_PRODUCER_GENERATION, generation);
    }

    pub fn set_producer_state(&mut self, state: u32, error_code: u32) {
        self.write_u32(OFF_PRODUCER_ERROR, error_code);
        self.write_u32(OFF_PRODUCER_STATE, state);
    }

    /// Publish one compact NV12 frame into the next slot.
    pub fn publish(&mut self, nv12: &[u8], pts_us: u64) -> Result<()> {
        if nv12.len() > self.slot_capacity as usize {
            bail!("frame {} octets > capacité {}", nv12.len(), self.slot_capacity);
        }
        if self.view_size == 0 {
            self.view_size = HEADER_SIZE + SLOT_COUNT * (SLOT_HEADER_SIZE + self.slot_capacity as usize);
        }

        let slot = self.next_slot % SLOT_COUNT;
        self.next_slot = self.next_slot.wrapping_add(1);

        // Dropped counter: the slot we are about to overwrite was still ready.
        if self.read_slot_u32(slot, SLOT_STATE) == SLOT_READY {
            let dropped = self.read_u64(OFF_FRAMES_DROPPED).wrapping_add(1);
            self.write_u64(OFF_FRAMES_DROPPED, dropped);
        }

        self.write_slot_u32(slot, SLOT_STATE, SLOT_WRITING);
        let sequence = self.read_slot_u32(slot, SLOT_SEQUENCE).wrapping_add(1);
        self.write_slot_u32(slot, SLOT_SEQUENCE, sequence);
        compiler_fence(Ordering::SeqCst);

        let base = self.slot_base(slot);
        self.write_bytes(base + SLOT_HEADER_SIZE, nv12);
        self.write_slot_u32(slot, SLOT_FRAME_BYTES, nv12.len() as u32);
        self.write_slot_u64(slot, SLOT_PTS_US, pts_us);

        self.write_slot_u32(slot, SLOT_SEQUENCE, sequence.wrapping_add(1));
        self.write_slot_u32(slot, SLOT_STATE, SLOT_READY);
        compiler_fence(Ordering::SeqCst);

        self.write_u32(OFF_LATEST_SLOT, slot as u32);
        self.write_u64(OFF_PTS_US, pts_us);
        let global_sequence = self.read_u64(OFF_SEQUENCE).wrapping_add(1);
        self.write_u64(OFF_SEQUENCE, global_sequence);
        let published = self.read_u64(OFF_FRAMES_PUBLISHED).wrapping_add(1);
        self.write_u64(OFF_FRAMES_PUBLISHED, published);

        unsafe {
            let _ = SetEvent(self.event);
        }
        Ok(())
    }

    fn slot_base(&self, slot: usize) -> usize {
        HEADER_SIZE + slot * (SLOT_HEADER_SIZE + self.slot_capacity as usize)
    }

    fn read_bytes(&self, offset: usize, count: usize) -> Vec<u8> {
        let mut out = vec![0u8; count];
        unsafe { std::ptr::copy_nonoverlapping(self.base().add(offset), out.as_mut_ptr(), count) };
        out
    }

    fn read_u32(&self, offset: usize) -> u32 {
        unsafe { std::ptr::read_volatile(self.base().add(offset) as *const u32) }
    }

    fn read_u64(&self, offset: usize) -> u64 {
        unsafe { std::ptr::read_volatile(self.base().add(offset) as *const u64) }
    }

    fn write_u32(&mut self, offset: usize, value: u32) {
        unsafe { std::ptr::write_volatile(self.base().add(offset) as *mut u32, value) };
    }

    fn write_u64(&mut self, offset: usize, value: u64) {
        unsafe { std::ptr::write_volatile(self.base().add(offset) as *mut u64, value) };
    }

    fn write_bytes(&mut self, offset: usize, data: &[u8]) {
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), self.base().add(offset), data.len()) };
    }

    fn read_slot_u32(&self, slot: usize, offset: usize) -> u32 {
        self.read_u32(self.slot_base(slot) + offset)
    }

    fn write_slot_u32(&mut self, slot: usize, offset: usize, value: u32) {
        let base = self.slot_base(slot);
        self.write_u32(base + offset, value);
    }

    fn write_slot_u64(&mut self, slot: usize, offset: usize, value: u64) {
        let base = self.slot_base(slot);
        self.write_u64(base + offset, value);
    }
}

impl Drop for ShmBlock {
    fn drop(&mut self) {
        unsafe {
            let _ = UnmapViewOfFile(self.view);
            let _ = CloseHandle(self.event);
            let _ = CloseHandle(self.mapping);
        }
    }
}

/// SECURITY_ATTRIBUTES granting SYSTEM/LOCAL SERVICE full access and Users
/// read/write/synchronize, so the Frame Server and the agent can share it.
struct SecurityAttributes {
    attributes: SECURITY_ATTRIBUTES,
    // Keeps the descriptor (and its ACL/SIDs) alive: `attributes` only holds
    // a raw pointer to them.
    _descriptor: Box<SECURITY_DESCRIPTOR>,
    _acl: Box<[u8; 512]>,
    _system_sid: Box<[u8; 68]>,
    _service_sid: Box<[u8; 68]>,
    _users_sid: Box<[u8; 68]>,
}

impl SecurityAttributes {
    fn build() -> Result<Self> {
        let mut acl = Box::new([0u8; 512]);
        let mut system_sid = Box::new([0u8; 68]);
        let mut service_sid = Box::new([0u8; 68]);
        let mut users_sid = Box::new([0u8; 68]);

        unsafe {
            InitializeAcl(acl.as_mut_ptr() as *mut ACL, acl.len() as u32, ACL_REVISION)
                .context("InitializeAcl")?;

            for (kind, sid) in [
                (WinLocalSystemSid, system_sid.as_mut()),
                (WinLocalServiceSid, service_sid.as_mut()),
                (WinBuiltinUsersSid, users_sid.as_mut()),
            ] {
                let mut size = sid.len() as u32;
                CreateWellKnownSid(
                    kind,
                    None,
                    Some(PSID(sid.as_mut_ptr() as *mut c_void)),
                    &mut size,
                )
                .context("CreateWellKnownSid")?;
            }

            let acl_ptr = acl.as_mut_ptr() as *mut ACL;
            AddAccessAllowedAce(acl_ptr, ACL_REVISION, GENERIC_ALL.0, PSID(system_sid.as_mut_ptr() as *mut c_void))
                .context("AddAccessAllowedAce(system)")?;
            AddAccessAllowedAce(acl_ptr, ACL_REVISION, GENERIC_ALL.0, PSID(service_sid.as_mut_ptr() as *mut c_void))
                .context("AddAccessAllowedAce(service)")?;
            AddAccessAllowedAce(
                acl_ptr,
                ACL_REVISION,
                GENERIC_READ.0 | GENERIC_WRITE.0 | SYNCHRONIZE_ACCESS,
                PSID(users_sid.as_mut_ptr() as *mut c_void),
            )
            .context("AddAccessAllowedAce(users)")?;
        }

        let mut descriptor: Box<SECURITY_DESCRIPTOR> = Box::new(unsafe { std::mem::zeroed() });
        let descriptor_ptr = PSECURITY_DESCRIPTOR(
            descriptor.as_mut() as *mut SECURITY_DESCRIPTOR as *mut c_void,
        );
        unsafe {
            InitializeSecurityDescriptor(descriptor_ptr, SECURITY_DESCRIPTOR_REVISION)
                .context("InitializeSecurityDescriptor")?;
            SetSecurityDescriptorDacl(
                descriptor_ptr,
                true,
                Some(acl.as_ptr() as *const ACL),
                false,
            )
            .context("SetSecurityDescriptorDacl")?;
        }

        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.as_mut() as *mut SECURITY_DESCRIPTOR as *mut c_void,
            bInheritHandle: false.into(),
        };

        Ok(Self {
            attributes,
            _descriptor: descriptor,
            _acl: acl,
            _system_sid: system_sid,
            _service_sid: service_sid,
            _users_sid: users_sid,
        })
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn is_not_found(error: &windows::core::Error) -> bool {
    error.code() == windows::core::HRESULT::from_win32(ERROR_FILE_NOT_FOUND.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each test gets its own object names: they run in parallel.
    fn names(label: &str) -> ShmNames {
        ShmNames::local(&format!("WclTest{}-{}-", std::process::id(), label))
    }

    #[test]
    fn publish_then_read_back() {
        let names = names("readback");
        let mut producer = ShmBlock::create(&names, 256 * 1024).unwrap();
        producer.set_video_format(320, 240);

        let frame = vec![42u8; 320 * 240 * 3 / 2];
        producer.publish(&frame, 123_456).unwrap();

        let stats = producer.stats();
        assert_eq!(stats.latest_slot, 0);
        assert_eq!(stats.pts_us, 123_456);
        assert_eq!(stats.published, 1);
        assert_eq!((stats.width, stats.height), (320, 240));

        // A second "consumer" view reads the same bytes through the mapping.
        let consumer = ShmBlock::open(&names).unwrap().expect("mapping présent");
        assert_eq!(consumer.stats(), stats);
        let base = consumer.slot_base(0) + SLOT_HEADER_SIZE;
        let copy = consumer.read_bytes(base, frame.len());
        assert_eq!(copy, frame);

        // Control block round-trip.
        let mut writer = ShmBlock::open(&names).unwrap().unwrap();
        writer.set_control(&Control {
            consumer_active: true,
            width: 1280,
            height: 720,
            fps: 30,
            generation: 7,
        });
        let control = producer.control();
        assert!(control.consumer_active);
        assert_eq!(
            (control.width, control.height, control.fps, control.generation),
            (1280, 720, 30, 7)
        );
    }

    #[test]
    fn slots_rotate_and_count_drops() {
        let names = names("rotate");
        let mut producer = ShmBlock::create(&names, 4096).unwrap();
        producer.set_video_format(32, 16);
        let frame = vec![7u8; 32 * 16 * 3 / 2];

        for index in 0..4 {
            producer.publish(&frame, index as u64).unwrap();
        }
        let stats = producer.stats();
        assert_eq!(stats.latest_slot, 0, "4 frames sur 3 slots → retour au slot 0");
        assert_eq!(stats.published, 4);
        assert!(stats.dropped >= 1, "le slot écrasé non lu est compté");
    }

    #[test]
    fn open_returns_none_when_absent() {
        let names = ShmNames::local(&format!("WclAbsent{}-", std::process::id()));
        assert!(ShmBlock::open(&names).unwrap().is_none());
    }
}
