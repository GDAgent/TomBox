//! Texel transfer for placed (unified-memory) textures.
//!
//! On console a texture created with `CreatePlacedResourceX` lives at a title-chosen
//! address and the title fills it by writing memory in the console's tiled layout; there is
//! no API call when texels change. This module mirrors that memory into the PC texture:
//!
//!   * layout: RGBA8 and BC1 UNORM layout 261 support full-tile single levels and power-of-two
//!     mip chains, including the packed 4 KiB tail, in the native decoder.
//!     For other descriptors the *real* `xg_xs.dll` shipped with the title (it runs natively) is asked for
//!     a texture address computer for the title's own resource desc. Verified with
//!     `tools/dev/xgprobe.rs`: vtable slot 2 = GetResourceLayout, 3 = size, 6 =
//!     GetTexelElementOffsetBytes(plane, mip, x: u64, y, zOrSlice, sample, u32* bitPos),
//!     8 = CopyIntoSubresource; slot 6 offsets matched slot 8's output for every element of
//!     five texture shapes (BC1/BC3/BGRA8/R8, 2D + cube, layout 261). Slot 9
//!     (CopyFromSubresource) refuses to run off-console (needs an exclusive-lock object),
//!     so de-tiling is done here from the slot-6 element offsets.
//!   * change detection: each `PresentX` hashes the title memory of tracked subresources
//!     (never-uploaded ones first, the rest round-robin under a byte budget) and re-uploads
//!     the ones whose hash changed.
//!   * lifetime: the registry holds one reference per placed texture; when it is the only
//!     one left the title has released the texture and tracking stops.
//!
//! Assumptions / limits (see docs/TECHNICAL_DEBT.md):
//!   * explicit title transitions are replayed in queue submission order; pending split
//!     transitions and subresources last transitioned on another queue are deferred;
//!   * memory that is not committed/readable is skipped and retried later;
//!   * a write racing the read is repaired on a later frame (hash changes again), but a
//!     decommit racing the read would fault;
//!   * Texture3D, multi-plane and render-target/depth placed textures are not mirrored.

use core::ffi::c_void;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use compat_common::record;
use crate::placed::Placed;

extern "system" {
    fn VirtualQuery(addr: *const c_void, mbi: *mut Mbi, len: usize) -> usize;
    fn GetModuleHandleA(n: *const u8) -> *mut c_void;
    fn LoadLibraryA(n: *const u8) -> *mut c_void;
    fn GetProcAddress(h: *mut c_void, n: *const u8) -> *mut c_void;
}

#[repr(C)]
struct Mbi {
    base: usize,
    alloc_base: usize,
    alloc_protect: u32,
    partition: u32,
    region_size: usize,
    state: u32,
    protect: u32,
    ty: u32,
    _pad: u32,
}

#[repr(C)]
struct Guid(u32, u16, u16, [u8; 8]);
const IID_DEVICE: Guid = Guid(0x189819f1, 0x1db6, 0x4b57, [0xbe, 0x54, 0x18, 0x21, 0x33, 0x9b, 0x85, 0xf7]);
const IID_ALLOCATOR: Guid = Guid(0x6102dee4, 0xaf59, 0x4b09, [0xb9, 0x99, 0xb4, 0x4d, 0x73, 0xf0, 0x9b, 0x24]);
const IID_GFX_LIST: Guid = Guid(0x5b160d0f, 0xac1b, 0x4185, [0x8b, 0xa8, 0xb3, 0xae, 0x42, 0xa5, 0xa4, 0x55]);
const IID_FENCE: Guid = Guid(0x0a753dcf, 0xc4d8, 0x4b91, [0xad, 0xf6, 0xbe, 0x5a, 0x60, 0xd9, 0x5a, 0x76]);
const IID_RESOURCE: Guid = Guid(0x696442be, 0xa72e, 0x4059, [0xbc, 0x79, 0x5b, 0x5c, 0x98, 0x04, 0x0f, 0xad]);

unsafe fn vt(obj: *mut c_void) -> *const usize {
    *(obj as *const *const usize)
}
macro_rules! vcall {
    ($obj:expr, $slot:expr, fn($($at:ty),*) -> $rt:ty $(, $arg:expr)*) => {{
        let f: unsafe extern "system" fn(*mut c_void $(, $at)*) -> $rt =
            core::mem::transmute(*vt($obj).add($slot));
        f($obj $(, $arg)*)
    }};
}

/// True if `[addr, addr+len)` is committed and readable right now.
fn readable(addr: usize, len: usize) -> bool {
    let end = addr + len;
    let mut p = addr;
    while p < end {
        let mut m: Mbi = unsafe { core::mem::zeroed() };
        if unsafe { VirtualQuery(p as *const c_void, &mut m, core::mem::size_of::<Mbi>()) } == 0 {
            return false;
        }
        let prot = m.protect;
        let ok = m.state == 0x1000 && prot & 0x100 == 0 && matches!(prot & 0xff, 0x02 | 0x04 | 0x08 | 0x20 | 0x40 | 0x80);
        if !ok {
            return false;
        }
        p = m.base + m.region_size;
    }
    true
}

fn hash_bytes(b: &[u8]) -> u64 {
    let mut h: u64 = 0x9E37_79B9_7F4A_7C15 ^ b.len() as u64;
    let mut chunks = b.chunks_exact(8);
    for c in &mut chunks {
        let w = u64::from_le_bytes(c.try_into().unwrap());
        h = (h ^ w).wrapping_mul(0x0000_0100_0000_01B3).rotate_left(23);
    }
    for &x in chunks.remainder() {
        h = (h ^ x as u64).wrapping_mul(0x0000_0100_0000_01B3);
    }
    h
}

// ---- Xbox texture layout ---------------------------------------------------

/// Element offsets of one subresource inside the Xbox resource memory.
enum ElementOffsets {
    Native4K(TileFormat),
    FromXg(Vec<u32>),
}

struct SubMap {
    w_e: u32,
    h_e: u32,
    offsets: ElementOffsets,
    lo: usize,
    hi: usize, // exclusive
}

enum OffsetSource {
    Native4K { maps: Vec<Arc<SubMap>>, slice_bytes: u32 },
    Xg(usize), // XGTextureAddressComputer*
}

struct TexLayout {
    source: OffsetSource,
    total: u64,
    bpe: u32,
    planes: u32,
    mips: u32,
    subs: Mutex<HashMap<u32, Option<Arc<SubMap>>>>,
}

impl TexLayout {
    unsafe fn create(p: &Placed) -> Result<TexLayout, String> {
        if let Some(format) = native_4k_format(p) {
            let (maps, slice_bytes) = native_4k_mips(format, p.width() as u32, p.height(), u32::from(p.mips()));
            return Ok(TexLayout {
                source: OffsetSource::Native4K { maps, slice_bytes },
                total: u64::from(slice_bytes) * u64::from(p.depth_or_array()),
                bpe: format.element_bytes(),
                planes: 1,
                mips: u32::from(p.mips()),
                subs: Mutex::new(HashMap::new()),
            });
        }
        let desc = &p.desc;
        let mut h = GetModuleHandleA(b"xg_xs.dll\0".as_ptr());
        if h.is_null() {
            h = LoadLibraryA(b"xg_xs.dll\0".as_ptr());
        }
        if h.is_null() {
            return Err("xg_xs.dll not loadable".into());
        }
        let f = GetProcAddress(h, b"XGCreateTextureComputer\0".as_ptr());
        if f.is_null() {
            return Err("XGCreateTextureComputer export missing".into());
        }
        let create: unsafe extern "system" fn(*const u8, *mut *mut c_void) -> i32 = core::mem::transmute(f);
        let mut comp: *mut c_void = core::ptr::null_mut();
        let hr = create(desc.as_ptr(), &mut comp);
        if hr < 0 || comp.is_null() {
            return Err(format!("XGCreateTextureComputer hr={hr:#010x}"));
        }
        // XG_RESOURCE_LAYOUT: u64 size, u64 alignment, u32 mips, u32 planes, planes[] of 0x6C8
        // bytes each (title's FD3D12TextureLayout::FillFromDesc copies exactly that); the
        // plane's BytesPerElement is at plane+0x28 (probe).
        let mut lay = vec![0u8; 0x4000];
        let hr = vcall!(comp, 2, fn(*mut u8) -> i32, lay.as_mut_ptr());
        if hr < 0 {
            return Err(format!("GetResourceLayout hr={hr:#010x}"));
        }
        let u64at = |o: usize| u64::from_le_bytes(lay[o..o + 8].try_into().unwrap());
        let u32at = |o: usize| u32::from_le_bytes(lay[o..o + 4].try_into().unwrap());
        Ok(TexLayout {
            source: OffsetSource::Xg(comp as usize),
            total: u64at(0),
            mips: u32at(0x10),
            planes: u32at(0x14),
            bpe: u32at(0x18 + 0x28),
            subs: Mutex::new(HashMap::new()),
        })
    }

    /// Returns the byte origin and element map for mip + slice * mips. Native maps
    /// are shared across slices. XG returns absolute offsets from the resource origin.
    unsafe fn submap(&self, sub: u32, w_e: u32, h_e: u32) -> Option<(usize, Arc<SubMap>)> {
        let comp = match &self.source {
            OffsetSource::Native4K { maps, slice_bytes } => {
                let origin = u64::from(sub / self.mips) * u64::from(*slice_bytes);
                let map = &maps[(sub % self.mips) as usize];
                return (origin < self.total && map.w_e == w_e && map.h_e == h_e)
                    .then(|| (origin as usize, Arc::clone(map)));
            }
            OffsetSource::Xg(ptr) => *ptr as *mut c_void,
        };
        let mut g = self.subs.lock().unwrap();
        if let Some(v) = g.get(&sub) {
            return v.clone().map(|map| (0, map));
        }
        let (mip, slice) = (sub % self.mips, sub / self.mips);
        let mut offs = Vec::with_capacity((w_e * h_e) as usize);
        let (mut lo, mut hi) = (usize::MAX, 0usize);
        let mut ok = true;
        'outer: for y in 0..h_e {
            for x in 0..w_e {
                let o = vcall!(comp, 6, fn(u32, u32, u64, u32, u32, u32, *mut u32) -> u64,
                    0, mip, x as u64, y, slice, 0, core::ptr::null_mut());
                if o >= self.total || o + self.bpe as u64 > self.total || o > u32::MAX as u64 {
                    ok = false;
                    break 'outer;
                }
                lo = lo.min(o as usize);
                hi = hi.max(o as usize + self.bpe as usize);
                offs.push(o as u32);
            }
        }
        let v = if ok { Some(Arc::new(SubMap { w_e, h_e, offsets: ElementOffsets::FromXg(offs), lo, hi })) } else { None };
        g.insert(sub, v.clone());
        v.map(|map| (0, map))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TileFormat { Rgba8, Bc1 }

impl TileFormat {
    fn element_side(self) -> u32 {
        match self { Self::Rgba8 => 1, Self::Bc1 => 4 }
    }
    fn element_bytes(self) -> u32 {
        match self { Self::Rgba8 => 4, Self::Bc1 => 8 }
    }
    // Tile size in image pixels. A BC1 tile contains 32×16 compressed blocks.
    fn tile_pixels(self) -> (u32, u32) {
        match self { Self::Rgba8 => (32, 32), Self::Bc1 => (128, 64) }
    }
    fn offset(self, width: u32, x: u32, y: u32) -> u32 {
        match self {
            Self::Rgba8 => rgba8_4k_offset(width, x, y),
            Self::Bc1 => {
                // GFX10 SW_4K_S for 8-byte elements. Byte-address bits 3..11
                // select X0 Y0 Y1 X1 X2 Y2 X3 Y3 X4 in block coordinates.
                let tile = (y / 16) * width.div_ceil(32) + x / 32;
                let x_bits = ((x & 1) << 3) | ((x & 6) << 5) | ((x & 8) << 6) | ((x & 16) << 7);
                let y_bits = ((y & 3) << 4) | ((y & 4) << 6) | ((y & 8) << 7);
                (tile << 12) | x_bits | y_bits
            }
        }
    }
}

// Both dimensions of a multilevel base must be powers of two.
fn native_4k_format(p: &Placed) -> Option<TileFormat> {
    let format = match p.format() {
        28 => TileFormat::Rgba8,
        71 => TileFormat::Bc1,
        _ => return None,
    };
    let width = u32::try_from(p.width()).ok()?;
    let height = p.height();
    let (tile_w, tile_h) = format.tile_pixels();
    let full_tiles = |n, tile| (tile..=16384).contains(&n) && n % tile == 0;
    let levels = u32::from(p.mips());
    let supported_levels = levels == 1 || (width.is_power_of_two() && height.is_power_of_two()
        && (2..=width.max(height).ilog2() + 1).contains(&levels));
    (p.dim() == 3 && p.layout() == 261 && p.flags() == 0
        && supported_levels && (1..=2048).contains(&p.depth_or_array())
        && p.sample_count() == 1 && p.sample_quality() == 0
        && full_tiles(width, tile_w) && full_tiles(height, tile_h)).then_some(format)
}

// Large levels occupy full tiles in reverse mip order. Small levels share a
// tile. Advance the tail slot by image mip, including BC1's three one-block mips.
fn native_4k_mips(format: TileFormat, width: u32, height: u32, levels: u32) -> (Vec<Arc<SubMap>>, u32) {
    const TAIL_ORIGINS: [u32; 8] = [2048, 1536, 1280, 1024, 768, 512, 256, 0];
    let (tile_w, tile_h) = format.tile_pixels();
    let bytes = format.element_bytes();
    // A thin GFX10 tail fits in half a tile's width and its full height.
    // Count slots by mip index even after one image dimension reaches one.
    let first_tail = (0..levels).find(|&level| {
        (width >> level).max(1) <= tile_w / 2 && (height >> level).max(1) <= tile_h
            && levels - level <= TAIL_ORIGINS.len() as u32
    }).unwrap_or(levels);
    let mut cursor = if first_tail < levels { 4096 } else { 0 };
    let mut maps = Vec::with_capacity(levels as usize);
    for level in (0..levels).rev() {
        let pixel_w = (width >> level).max(1);
        let pixel_h = (height >> level).max(1);
        let w = pixel_w.div_ceil(format.element_side());
        let h = pixel_h.div_ceil(format.element_side());
        let origin = if level >= first_tail {
            TAIL_ORIGINS[(level - first_tail) as usize]
        } else {
            let base = cursor;
            cursor += pixel_w.div_ceil(tile_w) * pixel_h.div_ceil(tile_h) * 4096;
            base
        };
        let end = origin + format.offset(w, w - 1, h - 1) + bytes;
        maps.push(Arc::new(SubMap {
            w_e: w, h_e: h, offsets: ElementOffsets::Native4K(format),
            lo: origin as usize, hi: end as usize,
        }));
    }
    maps.reverse();
    (maps, cursor)
}

// AMD GFX10 SW_4K_S, four bytes per element. Inside each row-major tile,
// byte address bits 2..11 are X0 X1 Y0 Y1 Y2 X2 Y3 X3 Y4 X4.
fn rgba8_4k_offset(width: u32, x: u32, y: u32) -> u32 {
    let tile = (y / 32) * width.div_ceil(32) + x / 32;
    let x_bits = ((x & 3) << 2) | ((x & 4) << 5) | ((x & 8) << 6) | ((x & 16) << 7);
    let y_bits = ((y & 7) << 4) | ((y & 8) << 5) | ((y & 16) << 6);
    (tile << 12) | x_bits | y_bits
}

fn layout_for(p: &Placed) -> Result<Arc<TexLayout>, String> {
    static CACHE: Mutex<Option<HashMap<[u8; 0x38], Result<Arc<TexLayout>, String>>>> = Mutex::new(None);
    let mut key = p.desc;
    key[8..16].fill(0); // alignment does not affect the layout request we make
    let mut g = CACHE.lock().unwrap();
    let m = g.get_or_insert_with(HashMap::new);
    if let Some(v) = m.get(&key) {
        return v.clone();
    }
    let v = unsafe { TexLayout::create(p) }.map(Arc::new);
    m.insert(key, v.clone());
    v
}

// ---- tracking + upload -----------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Footprint {
    raw: [u8; 32], // D3D12_PLACED_SUBRESOURCE_FOOTPRINT
    rows: u32,
    row_size: u64,
}
impl Footprint {
    fn offset(&self) -> u64 { u64::from_le_bytes(self.raw[0..8].try_into().unwrap()) }
    fn row_pitch(&self) -> u32 { u32::from_le_bytes(self.raw[0x18..0x1c].try_into().unwrap()) }
}

struct Tracked {
    res: usize,
    p: Placed,
    layout: Arc<TexLayout>,
    fps: Vec<Footprint>,
    total_bytes: u64,
    hashes: Vec<Option<u64>>,
}

struct Uploader {
    device: *mut c_void,
    alloc: *mut c_void,
    list: *mut c_void,
    fence: *mut c_void,
    fence_value: u64,
    tracked: Vec<Tracked>,
    retire: Vec<(u64, usize)>,
    cursor: usize,
    pumps: u64,
    uploaded_subs: u64,
    uploaded_bytes: u64,
    unreadable: u64,
    rejected: u64,
}
unsafe impl Send for Uploader {}
static UP: Mutex<Option<Uploader>> = Mutex::new(None);

unsafe fn init(queue: *mut c_void) -> Result<Uploader, String> {
    let mut device: *mut c_void = core::ptr::null_mut();
    let hr = vcall!(queue, 7, fn(*const Guid, *mut *mut c_void) -> i32, &IID_DEVICE, &mut device);
    if hr < 0 || device.is_null() {
        return Err(format!("queue->GetDevice hr={hr:#010x}"));
    }
    let mut alloc: *mut c_void = core::ptr::null_mut();
    let hr = vcall!(device, 9, fn(u32, *const Guid, *mut *mut c_void) -> i32, 0, &IID_ALLOCATOR, &mut alloc);
    if hr < 0 {
        return Err(format!("CreateCommandAllocator hr={hr:#010x}"));
    }
    let mut list: *mut c_void = core::ptr::null_mut();
    let hr = vcall!(device, 12, fn(u32, u32, *mut c_void, *mut c_void, *const Guid, *mut *mut c_void) -> i32,
        0, 0, alloc, core::ptr::null_mut(), &IID_GFX_LIST, &mut list);
    if hr < 0 {
        return Err(format!("CreateCommandList hr={hr:#010x}"));
    }
    vcall!(list, 9, fn() -> i32);
    let mut fence: *mut c_void = core::ptr::null_mut();
    let hr = vcall!(device, 36, fn(u64, u32, *const Guid, *mut *mut c_void) -> i32, 0, 0, &IID_FENCE, &mut fence);
    if hr < 0 {
        return Err(format!("CreateFence hr={hr:#010x}"));
    }
    Ok(Uploader {
        device, alloc, list, fence, fence_value: 0, tracked: Vec::new(), retire: Vec::new(), cursor: 0,
        pumps: 0, uploaded_subs: 0, uploaded_bytes: 0, unreadable: 0, rejected: 0,
    })
}

/// Why a placed resource is not mirrored (None = it is).
fn unsupported_reason(p: &Placed) -> Option<String> {
    if p.dim() != 3 {
        return Some(format!("dimension {} (only Texture2D/array/cube are mirrored)", p.dim()));
    }
    if p.flags() & 0x3 != 0 {
        return Some("render-target/depth-stencil texture (GPU-written, nothing to mirror)".into());
    }
    None
}

unsafe fn adopt(up: &mut Uploader, res: *mut c_void, p: Placed) -> Result<Tracked, String> {
    if let Some(r) = unsupported_reason(&p) {
        return Err(r);
    }
    let layout = layout_for(&p)?;
    if layout.planes != 1 {
        return Err(format!("{} planes", layout.planes));
    }
    if layout.bpe == 0 || layout.total == 0 || layout.total > u32::MAX as u64 {
        return Err(format!("layout size {:#x} bpe {}", layout.total, layout.bpe));
    }
    if p.va.checked_add(layout.total).is_none() {
        return Err("texture address range overflow".into());
    }
    let nsubs = p.mips() as u32 * p.depth_or_array() as u32;
    if layout.mips != p.mips() as u32 || nsubs == 0 || nsubs > 4096 {
        return Err(format!("mip mismatch xg={} desc={}", layout.mips, p.mips()));
    }
    let mut pc_desc = [0u8; 0x38];
    vcall!(res, 10, fn(*mut u8) -> *mut u8, pc_desc.as_mut_ptr());
    let mut raw = vec![[0u8; 32]; nsubs as usize];
    let mut rows = vec![0u32; nsubs as usize];
    let mut row_sizes = vec![0u64; nsubs as usize];
    let mut total = 0u64;
    vcall!(up.device, 38, fn(*const u8, u32, u32, u64, *mut [u8; 32], *mut u32, *mut u64, *mut u64) -> (),
        pc_desc.as_ptr(), 0, nsubs, 0, raw.as_mut_ptr(), rows.as_mut_ptr(), row_sizes.as_mut_ptr(), &mut total);
    if total == 0 || total == u64::MAX {
        return Err("GetCopyableFootprints failed".into());
    }
    let fps: Vec<Footprint> = (0..nsubs as usize).map(|i| Footprint { raw: raw[i], rows: rows[i], row_size: row_sizes[i] }).collect();
    if fps.iter().any(|f| f.row_size % layout.bpe as u64 != 0) {
        return Err(format!("row size not a multiple of bytes-per-element {}", layout.bpe));
    }
    Ok(Tracked { res: res as usize, p, layout, fps, total_bytes: total, hashes: vec![None; nsubs as usize] })
}

struct Staged {
    res: usize,
    tracked_index: usize,
    buf: *mut c_void,
    subs: Vec<(u32, Footprint, u64)>,
}

/// Check one tracked texture; stage an upload for every changed, readable subresource.
/// `fresh_only`: look only at subresources that have never been uploaded.
unsafe fn check(up: &mut Uploader, idx: usize, hashed: &mut u64, fresh_only: bool) -> Option<Staged> {
    let device = up.device;
    let t = &mut up.tracked[idx];
    let bpe = t.layout.bpe as usize;
    let mut changed = Vec::new();
    let mut unreadable = 0;
    for sub in 0..t.fps.len() as u32 {
        if fresh_only == t.hashes[sub as usize].is_some() {
            continue;
        }
        let fp = t.fps[sub as usize];
        let (w_e, h_e) = ((fp.row_size / bpe as u64) as u32, fp.rows);
        let Some((origin, map)) = t.layout.submap(sub, w_e, h_e) else {
            continue; // layout query failed for this subresource (counted at adoption time)
        };
        let base = t.p.va as usize + origin;
        if !readable(base + map.lo, map.hi - map.lo) {
            unreadable += 1;
            continue;
        }
        let mem = core::slice::from_raw_parts((base + map.lo) as *const u8, map.hi - map.lo);
        let h = hash_bytes(mem);
        *hashed += mem.len() as u64;
        if t.hashes[sub as usize].is_none() && mem.iter().all(|&b| b == 0) {
            // Never written by the title (fresh committed pages are zero). Do not upload:
            // the title may instead fill this subresource with a GPU copy
            // (CopyTextureRegion), which an upload of zeros could overwrite.
            t.hashes[sub as usize] = Some(h);
            continue;
        }
        if t.hashes[sub as usize] != Some(h) {
            changed.push((sub, base as *const u8, map, h));
        }
    }
    up.unreadable += unreadable;
    if changed.is_empty() {
        return None;
    }
    // One upload buffer for this texture's changed subresources.
    let heap: [u32; 5] = [2 /* UPLOAD */, 0, 0, 1, 1];
    let mut bd = [0u8; 0x38];
    bd[0..4].copy_from_slice(&1u32.to_le_bytes());
    bd[0x10..0x18].copy_from_slice(&t.total_bytes.to_le_bytes());
    bd[0x18..0x1c].copy_from_slice(&1u32.to_le_bytes());
    bd[0x1c..0x1e].copy_from_slice(&1u16.to_le_bytes());
    bd[0x1e..0x20].copy_from_slice(&1u16.to_le_bytes());
    bd[0x24..0x28].copy_from_slice(&1u32.to_le_bytes());
    bd[0x2c..0x30].copy_from_slice(&1u32.to_le_bytes());
    let mut buf: *mut c_void = core::ptr::null_mut();
    let hr = vcall!(device, 27, fn(*const u32, u32, *const u8, u32, *const c_void, *const Guid, *mut *mut c_void) -> i32,
        heap.as_ptr(), 0, bd.as_ptr(), 0x2C3, core::ptr::null(), &IID_RESOURCE, &mut buf);
    if hr < 0 || buf.is_null() {
        record("d3d12_xs", "upload", &format!("upload buffer ({} bytes) FAILED hr={hr:#010x}", t.total_bytes));
        return None;
    }
    let mut data: *mut u8 = core::ptr::null_mut();
    let hr = vcall!(buf, 8, fn(u32, *const c_void, *mut *mut u8) -> i32, 0, core::ptr::null(), &mut data);
    if hr < 0 || data.is_null() {
        vcall!(buf, 2, fn() -> u32);
        record("d3d12_xs", "upload", &format!("upload buffer Map FAILED hr={hr:#010x}"));
        return None;
    }
    let mut subs = Vec::with_capacity(changed.len());
    for (sub, base, map, h) in changed {
        let fp = t.fps[sub as usize];
        let dst0 = data.add(fp.offset() as usize);
        let pitch = fp.row_pitch() as usize;
        for y in 0..map.h_e as usize {
            let row = dst0.add(y * pitch);
            match &map.offsets {
                ElementOffsets::Native4K(format) => {
                    // The low four byte-address bits form a contiguous 16-byte run.
                    let run = map.w_e.min(16 / format.element_bytes()) as usize;
                    for x in (0..map.w_e).step_by(run) {
                        let offset = format.offset(map.w_e, x, y as u32);
                        core::ptr::copy_nonoverlapping(base.add(map.lo + offset as usize), row.add(x as usize * bpe), run * bpe);
                    }
                }
                ElementOffsets::FromXg(offs) => {
                    let start = y * map.w_e as usize;
                    for (x, &o) in offs[start..start + map.w_e as usize].iter().enumerate() {
                        core::ptr::copy_nonoverlapping(base.add(o as usize), row.add(x * bpe), bpe);
                    }
                }
            }
        }
        subs.push((sub, fp, h));
    }
    vcall!(buf, 9, fn(u32, *const c_void) -> (), 0, core::ptr::null());
    Some(Staged { res: t.res, tracked_index: idx, buf, subs })
}

// x64 D3D12_RESOURCE_BARRIER with the Transition union member active.
#[repr(C)]
struct TransitionBarrier {
    kind: u32,
    flags: u32,
    resource: usize,
    sub: u32,
    before: u32,
    after: u32,
}
const _: () = assert!(core::mem::size_of::<TransitionBarrier>() == 32);

unsafe fn upload_barriers(list: *mut c_void, barriers: &[TransitionBarrier]) {
    if barriers.is_empty() { return }
    vcall!(list, 26, fn(u32, *const TransitionBarrier) -> (), barriers.len() as u32, barriers.as_ptr());
    static TRACE: std::sync::LazyLock<bool> = std::sync::LazyLock::new(||
        std::env::var("TOMBOX_TRACE_UPLOAD_BARRIERS").as_deref() == Ok("1"));
    if *TRACE {
        for b in barriers {
            record("d3d12_xs", "upload.barrier", &format!("sub={} {:#x}->{:#x}", b.sub, b.before, b.after));
        }
    }
}

/// Called once per `PresentX`, on the presenting thread, before the frame is copied out.
pub unsafe fn pump(queue: *mut c_void) {
    static FAILED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
    if FAILED.load(core::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let mut guard = UP.lock().unwrap();
    if guard.is_none() {
        match init(queue) {
            Ok(u) => *guard = Some(u),
            Err(e) => {
                FAILED.store(true, core::sync::atomic::Ordering::Relaxed);
                record("d3d12_xs", "upload", &format!("init FAILED: {e} - placed textures will stay empty"));
                return;
            }
        }
    }
    let up = guard.as_mut().unwrap();
    up.pumps += 1;

    // Retire upload buffers whose copy has completed.
    let done = vcall!(up.fence, 8, fn() -> u64);
    up.retire.retain(|&(v, b)| {
        if v <= done {
            vcall!(b as *mut c_void, 2, fn() -> u32);
            false
        } else {
            true
        }
    });

    // Adopt newly placed resources (the registry already holds a reference on each).
    for (res, p) in crate::placed::drain_new() {
        match adopt(up, res as *mut c_void, p.clone()) {
            Ok(t) => up.tracked.push(t),
            Err(e) => {
                up.rejected += 1;
                if up.rejected <= 12 {
                    record("d3d12_xs", "upload", &format!(
                        "NOT mirrored: va={:#x} {}x{}x{} mips={} fmt={} layout={} flags={:#x}: {e}",
                        p.va, p.width(), p.height(), p.depth_or_array(), p.mips(), p.format(), p.layout(), p.flags()));
                }
                crate::placed::forget(res as *mut c_void);
                vcall!(res as *mut c_void, 2, fn() -> u32);
            }
        }
    }

    // Drop textures the title has released (ours is the last reference).
    let mut i = 0;
    while i < up.tracked.len() {
        let r = up.tracked[i].res as *mut c_void;
        vcall!(r, 1, fn() -> u32);
        if vcall!(r, 2, fn() -> u32) == 1 {
            crate::placed::forget(r);
            vcall!(r, 2, fn() -> u32);
            up.tracked.swap_remove(i);
        } else {
            i += 1;
        }
    }

    // The single upload list/allocator can be reused only once the previous copy is done.
    if done < up.fence_value {
        return;
    }

    const FRESH_BUDGET: u64 = 96 << 20; // bytes hashed per frame for never-uploaded textures
    const RECHECK_BUDGET: u64 = 8 << 20; // bytes hashed per frame re-checking uploaded ones
    let mut staged: Vec<Staged> = Vec::new();
    let mut hashed = 0u64;
    let n = up.tracked.len();
    for idx in 0..n {
        if hashed >= FRESH_BUDGET {
            break;
        }
        if up.tracked[idx].hashes.iter().any(|h| h.is_none()) {
            if let Some(s) = check(up, idx, &mut hashed, true) {
                staged.push(s);
            }
        }
    }
    let mut rehashed = 0u64;
    let mut visited = 0;
    while n > 0 && visited < n && rehashed < RECHECK_BUDGET {
        let idx = up.cursor % n;
        up.cursor = (up.cursor + 1) % n;
        visited += 1;
        if let Some(s) = check(up, idx, &mut rehashed, false) {
            staged.push(s);
        }
    }

    if !staged.is_empty() {
        const COPY_DEST: u32 = 0x400;
        let tracker = crate::texture_state::TRACKER.lock().unwrap();
        // Keep the snapshot valid through ExecuteCommandLists. CPU hashing and staging
        // above run outside this lock. Deferred writes retain their previous hashes.
        staged.retain_mut(|s| {
            s.subs.retain(|(sub, _, _)| tracker.upload_state(s.res, *sub, queue as usize).is_some());
            if s.subs.is_empty() {
                vcall!(s.buf, 2, fn() -> u32);
            }
            !s.subs.is_empty()
        });
        if staged.is_empty() { return }
        if vcall!(up.alloc, 8, fn() -> i32) < 0 ||
            vcall!(up.list, 10, fn(*mut c_void, *mut c_void) -> i32, up.alloc, core::ptr::null_mut()) < 0 {
            for s in staged { vcall!(s.buf, 2, fn() -> u32); }
            return;
        }
        for s in &staged {
            let mut barriers: Vec<_> = s.subs.iter().filter_map(|&(sub, _, _)| {
                let before = tracker.upload_state(s.res, sub, queue as usize)?;
                (before != COPY_DEST).then_some(TransitionBarrier {
                    kind: 0, flags: 0, resource: s.res, sub, before, after: COPY_DEST,
                })
            }).collect();
            upload_barriers(up.list, &barriers);
            for (sub, fp, _) in &s.subs {
                // D3D12_TEXTURE_COPY_LOCATION { pResource; Type; union { PlacedFootprint | SubresourceIndex } }
                let mut dst = [0u8; 48];
                dst[0..8].copy_from_slice(&(s.res as u64).to_le_bytes());
                dst[16..20].copy_from_slice(&sub.to_le_bytes()); // Type 0 = SUBRESOURCE_INDEX
                let mut src = [0u8; 48];
                src[0..8].copy_from_slice(&(s.buf as u64).to_le_bytes());
                src[8..12].copy_from_slice(&1u32.to_le_bytes()); // Type 1 = PLACED_FOOTPRINT
                src[16..48].copy_from_slice(&fp.raw);
                vcall!(up.list, 16, fn(*const u8, u32, u32, u32, *const u8, *const c_void) -> (),
                    dst.as_ptr(), 0, 0, 0, src.as_ptr(), core::ptr::null());
            }
            for b in &mut barriers {
                core::mem::swap(&mut b.before, &mut b.after);
            }
            upload_barriers(up.list, &barriers);
        }
        let hr = vcall!(up.list, 9, fn() -> i32);
        if hr < 0 {
            record("d3d12_xs", "upload", &format!("upload list Close FAILED hr={hr:#010x}"));
            for s in staged { vcall!(s.buf, 2, fn() -> u32); }
            return;
        }
        let lists = [up.list];
        vcall!(queue, 10, fn(u32, *const *mut c_void) -> (), 1, lists.as_ptr());
        drop(tracker);
        for s in &staged {
            for &(sub, fp, hash) in &s.subs {
                up.tracked[s.tracked_index].hashes[sub as usize] = Some(hash);
                up.uploaded_subs += 1;
                up.uploaded_bytes += u64::from(fp.rows) * fp.row_size;
            }
        }
        up.fence_value += 1;
        vcall!(queue, 14, fn(*mut c_void, u64) -> i32, up.fence, up.fence_value);
        for s in &staged {
            up.retire.push((up.fence_value, s.buf as usize));
        }
    }

    if (!staged.is_empty() && up.pumps < 40) || up.pumps % 600 == 0 {
        record("d3d12_xs", "upload", &format!(
            "pump {}: staged {} textures; tracked={} total uploaded {} subresources / {} bytes; unreadable-skips={} not-mirrored={}",
            up.pumps, staged.len(), up.tracked.len(), up.uploaded_subs, up.uploaded_bytes, up.unreadable, up.rejected));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_decoder_checks_shapes_and_resource_flags() {
        let mut p = Placed { desc: [0; 0x38], va: 0, init_state: 0 };
        for (at, value) in [(0, 3u32), (0x10, 64), (0x18, 32), (0x1c, 0x00010001),
                            (0x20, 28), (0x24, 1), (0x2c, 261)] {
            p.desc[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        assert_eq!(native_4k_format(&p), Some(TileFormat::Rgba8));
        let mut rect = p.clone();
        rect.desc[0x1e..0x20].copy_from_slice(&7u16.to_le_bytes());
        assert_eq!(native_4k_format(&rect), Some(TileFormat::Rgba8));
        rect.desc[0x10..0x18].copy_from_slice(&96u64.to_le_bytes());
        assert_eq!(native_4k_format(&rect), None);
        let mut square = p.clone();
        square.desc[0x18..0x1c].copy_from_slice(&64u32.to_le_bytes());
        for (levels, supported) in [(0u16, false), (2, true), (7, true), (8, false)] {
            square.desc[0x1e..0x20].copy_from_slice(&levels.to_le_bytes());
            assert_eq!(native_4k_format(&square).is_some(), supported);
        }
        for (at, value) in [(0, 4u32), (0x10, 65), (0x14, 1), (0x18, 31), (0x18, 0),
                            (0x1c, 0x00080001), (0x1c, 0x00010000), (0x1c, 0x00010801), (0x20, 71), (0x24, 2),
                            (0x28, 1), (0x2c, 3), (0x30, 1)] {
            let mut other = p.clone();
            other.desc[at..at + 4].copy_from_slice(&value.to_le_bytes());
            assert_eq!(native_4k_format(&other), None, "field {at:#x} = {value}");
        }
    }

    #[test]
    fn full_and_truncated_mips_have_distinct_placement_ranges() {
        // Observed byte origins for the 256-square Scarlett reference mip chain.
        let (maps, size) = native_4k_mips(TileFormat::Rgba8, 256, 256, 9);
        assert_eq!(size, 352256);
        let starts: Vec<_> = maps.iter().map(|m| m.lo).collect();
        assert_eq!(starts, [90112, 24576, 8192, 4096, 2048, 1536, 1280, 1024, 768]);
        assert!(maps.windows(2).all(|p| p[1].hi <= p[0].lo));
        assert_eq!(maps[8].hi - maps[8].lo, 4);
        let (large, size) = native_4k_mips(TileFormat::Rgba8, 64, 64, 2);
        assert_eq!((large[0].lo, large[1].lo, size), (4096, 0, 20480));
        let (tail, size) = native_4k_mips(TileFormat::Rgba8, 64, 64, 3);
        assert_eq!((tail[0].lo, tail[2].lo, size), (8192, 2048, 24576));
    }
    #[test]
    fn rectangular_mips_follow_padded_tiles_and_pack_the_tail_by_level() {
        // Addr2ComputeSurfaceInfo and Addr2ComputeSurfaceAddrFromCoord, GFX10 SW_4K_S.
        let (wide, size) = native_4k_mips(TileFormat::Rgba8, 256, 64, 9);
        assert_eq!((size, wide[3].lo, wide[8].lo), (98304, 4096, 768));
        let (tall, size) = native_4k_mips(TileFormat::Rgba8, 64, 256, 9);
        assert_eq!((size, tall[3].lo, tall[8].lo), (94208, 2048, 512));
        assert_eq!((tall[7].w_e, tall[7].h_e), (1, 2));
        assert_eq!(TileFormat::Rgba8.offset(16, 0, 32), 4096);
        let (truncated, size) = native_4k_mips(TileFormat::Rgba8, 64, 256, 3);
        assert_eq!((size, truncated[2].lo), (90112, 0));
    }

    #[test]
    fn large_native_arrays_keep_the_complete_layout_size() {
        // Four 1 GiB slices exceed the mirror's 32-bit resource size limit.
        let mut p = Placed { desc: [0; 0x38], va: 0, init_state: 0 };
        for (at, value) in [(0, 3u32), (0x10, 16384), (0x18, 16384), (0x1c, 0x00010004),
                            (0x20, 28), (0x24, 1), (0x2c, 261)] {
            p.desc[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        let layout = unsafe { TexLayout::create(&p) }.unwrap();
        assert_eq!(layout.total, 1u64 << 32);
        let (origin, _) = unsafe { layout.submap(3, 16384, 16384) }.unwrap();
        assert_eq!(origin, 3usize << 30);
        assert!(unsafe { layout.submap(4, 16384, 16384) }.is_none());
    }

    #[test]
    fn bc1_address_covers_one_tile_and_advances_at_block_boundaries() {
        let format = TileFormat::Bc1;
        let mut offsets: Vec<_> = (0..16).flat_map(|y| (0..32).map(move |x| format.offset(64, x, y))).collect();
        offsets.sort_unstable();
        assert_eq!(offsets, (0..4096).step_by(8).collect::<Vec<_>>());
        assert_eq!(format.offset(64, 32, 0), 4096);
        assert_eq!(format.offset(64, 0, 16), 8192);
        let (maps, bytes) = native_4k_mips(format, 128, 64, 1);
        assert_eq!((maps[0].w_e, maps[0].h_e, maps[0].hi, bytes), (32, 16, 4096, 4096));
    }

    #[test]
    fn bc1_mips_keep_sub_block_levels_in_separate_slots() {
        let (maps, size) = native_4k_mips(TileFormat::Bc1, 256, 256, 9);
        assert_eq!(size, 45056);
        let starts: Vec<_> = maps.iter().map(|m| m.lo).collect();
        assert_eq!(starts, [12288, 4096, 2048, 1536, 1280, 1024, 768, 512, 256]);
        assert!(maps.windows(2).all(|p| p[1].hi <= p[0].lo));
        assert!(maps[6..].iter().all(|m| (m.w_e, m.h_e, m.hi - m.lo) == (1, 1, 8)));
    }

}
