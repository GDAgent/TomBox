//! Xbox `CreatePlacedResourceX` (device slot 44): resources placed at a title-chosen
//! virtual address in unified (CPU == GPU) memory.
//!
//! Evidence: `FXboxCommonD3D12Adapter::CreatePlacedResourceX` (EXE+0x5c4610) calls slot 44 as
//!   (this, VirtualAddress, const D3D12_RESOURCE_DESC*, InitialState, pClearValue = null,
//!    riid, ppvResource)
//! which matches the public GDK signature. Its caller `CreateVirtualTexture` (EXE+0x5c4524)
//! obtains the address either by reserving a 64 KiB-aligned virtual block (streamed
//! textures; pages are mapped later) or from a small-texture pool.
//!
//! On console the texel memory at that address *is* the texture. A PC GPU cannot sample
//! title memory, so each placed resource gets a committed PC resource plus a registry entry
//! remembering the address; texel transfer from title memory is a separate step (upload.rs).
//!
//! Status: resource creation is implemented; see `Placed` for what is recorded.

use core::ffi::c_void;
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Clone, Debug)]
pub struct Placed {
    pub va: u64,
    /// The title's original (Xbox) D3D12_RESOURCE_DESC, 0x38 bytes.
    pub desc: [u8; 0x38],
    pub init_state: u32,
}

impl Placed {
    fn u32at(&self, o: usize) -> u32 { u32::from_le_bytes(self.desc[o..o + 4].try_into().unwrap()) }
    pub fn dim(&self) -> u32 { self.u32at(0) }
    pub fn width(&self) -> u64 { u64::from_le_bytes(self.desc[0x10..0x18].try_into().unwrap()) }
    pub fn height(&self) -> u32 { self.u32at(0x18) }
    pub fn depth_or_array(&self) -> u16 { u16::from_le_bytes(self.desc[0x1c..0x1e].try_into().unwrap()) }
    pub fn mips(&self) -> u16 { u16::from_le_bytes(self.desc[0x1e..0x20].try_into().unwrap()) }
    pub fn format(&self) -> u32 { self.u32at(0x20) }
    pub fn sample_count(&self) -> u32 { self.u32at(0x24) }
    pub fn sample_quality(&self) -> u32 { self.u32at(0x28) }
    pub fn layout(&self) -> u32 { self.u32at(0x2c) }
    pub fn flags(&self) -> u32 { self.u32at(0x30) }
}

/// PC resource pointer -> placement record.
static REG: Mutex<Option<HashMap<usize, Placed>>> = Mutex::new(None);

/// Placements not yet seen by the uploader.
static NEW: Mutex<Vec<(usize, Placed)>> = Mutex::new(Vec::new());

/// Record a placed resource. The caller has taken one COM reference on `res` for the
/// registry; it is released by the uploader when the title drops the resource (or when the
/// resource is not mirrored).
pub fn register(res: *mut c_void, p: Placed) {
    let count = usize::from(p.mips()) * usize::from(p.depth_or_array());
    if p.dim() == 3 && p.flags() & 3 == 0 && (1..=4096).contains(&count) {
        crate::texture_state::TRACKER.lock().unwrap().register(
            res as usize, count, p.init_state, p.flags() & 0x20 != 0,
        );
    }
    REG.lock().unwrap().get_or_insert_with(HashMap::new).insert(res as usize, p.clone());
    NEW.lock().unwrap().push((res as usize, p));
}

pub fn drain_new() -> Vec<(usize, Placed)> {
    core::mem::take(&mut *NEW.lock().unwrap())
}

pub fn forget(res: *mut c_void) {
    crate::texture_state::TRACKER.lock().unwrap().forget(res as usize);
    if let Some(m) = REG.lock().unwrap().as_mut() {
        m.remove(&(res as usize));
    }
}

pub fn lookup(res: *mut c_void) -> Option<Placed> {
    REG.lock().unwrap().as_ref().and_then(|m| m.get(&(res as usize)).cloned())
}

pub fn count() -> usize {
    REG.lock().unwrap().as_ref().map_or(0, |m| m.len())
}
