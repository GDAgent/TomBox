//! `ID3D12Resource::Map` on GPU-only resources (unified memory contract).
//!
//! On the console every resource lives in CPU-visible unified memory, so a title may `Map`
//! a committed DEFAULT-heap texture and write it. Observed in a GDK title: a generic
//! "create committed resource" helper calls `CreateCommittedResource`, then `Map(0, NULL,
//! &p)` without checking the HRESULT, then `memset(p, 0, GetResourceAllocationInfo(desc)
//! .SizeInBytes)`, then `Unmap`, for every heap kind including render targets. PC D3D12
//! refuses `Map` on a resource without CPU access and leaves `p` NULL, so the title faults.
//!
//! Like `heap_wrap`, the real `ID3D12Resource` vtable is patched in place (slots 8/9) so
//! the resource objects stay real. The patched `Map` forwards CPU-heap resources; for a
//! GPU-only resource (which the runtime refuses) it returns a zero-filled CPU shadow, sized with the same
//! allocation model as our `GetResourceAllocationInfo`, so the title's writes stay inside it.
//!
//! On the last `Unmap` the shadow is compared with what the GPU resource is known to hold:
//!   * a committed resource mapped for the first time after creation, on a heap without
//!     `D3D12_HEAP_FLAG_CREATE_NOT_ZEROED`, is zero-initialised by D3D12, so an all-zero
//!     shadow means the GPU contents already match;
//!   * anything else (non-zero data, a resource the GPU may have written) would need a
//!     layout-aware upload that is not implemented. It is reported as an error, not hidden.
//!
//! A `CreatePlacedResourceX` resource maps to its title-chosen address: that memory is the
//! resource on the console, and upload.rs mirrors it into the PC resource.
//!
//! Limits: only subresource 0 (Xbox `Map` of other subresources returns an offset in the
//! console's tiled layout); a CPU *read* through the shadow sees zeros, not GPU contents.

use core::ffi::c_void;
use core::sync::atomic::{AtomicUsize, Ordering};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use compat_common::record;

extern "system" {
    fn VirtualProtect(addr: *mut c_void, size: usize, new_protect: u32, old_protect: *mut u32) -> i32;
    fn VirtualAlloc(addr: *mut c_void, size: usize, ty: u32, protect: u32) -> *mut c_void;
    fn VirtualFree(addr: *mut c_void, size: usize, ty: u32) -> i32;
}
const PAGE_READWRITE: u32 = 0x04;
const MEM_COMMIT_RESERVE: u32 = 0x3000;
const MEM_RELEASE: u32 = 0x8000;

const SLOT_MAP: usize = 8;
const SLOT_UNMAP: usize = 9;
const SLOT_GET_DESC: usize = 10;
const SLOT_GET_HEAP_PROPERTIES: usize = 14;

const HEAP_TYPE_DEFAULT: u32 = 1;
const HEAP_TYPE_CUSTOM: u32 = 4;
const CPU_PAGE_PROPERTY_NOT_AVAILABLE: u32 = 1;
const HEAP_FLAG_CREATE_NOT_ZEROED: u32 = 0x1000;

static ORIG_MAP: AtomicUsize = AtomicUsize::new(0);
static ORIG_UNMAP: AtomicUsize = AtomicUsize::new(0);
static PATCHED_VTABLES: Mutex<Vec<usize>> = Mutex::new(Vec::new());

struct Shadow {
    ptr: usize,
    size: usize,
    maps: u32,
    /// The pointer is our VirtualAlloc (false: title memory behind a placed resource).
    owned: bool,
    /// GPU contents are known to be zero (fresh committed resource, zeroed heap).
    gpu_zero: bool,
}

/// Resources currently mapped through a shadow.
static SHADOWS: Mutex<Option<HashMap<usize, Shadow>>> = Mutex::new(None);
/// Committed resources not mapped since creation.
static FRESH: Mutex<Option<HashSet<usize>>> = Mutex::new(None);

/// Is a resource on a heap with these properties CPU-inaccessible on PC?
fn gpu_only(heap_type: u32, cpu_page_property: u32) -> bool {
    heap_type == HEAP_TYPE_DEFAULT
        || (heap_type == HEAP_TYPE_CUSTOM && cpu_page_property == CPU_PAGE_PROPERTY_NOT_AVAILABLE)
}

#[derive(Debug, PartialEq, Eq)]
enum UnmapOutcome {
    /// The GPU resource already holds what the CPU wrote.
    InSync,
    /// The CPU wrote data that did not reach the GPU resource.
    Lost,
}

fn unmap_outcome(gpu_zero: bool, shadow: &[u8]) -> UnmapOutcome {
    if gpu_zero && all_zero(shadow) { UnmapOutcome::InSync } else { UnmapOutcome::Lost }
}

fn all_zero(b: &[u8]) -> bool {
    let (head, words, tail) = unsafe { b.align_to::<u64>() };
    head.iter().all(|&x| x == 0) && words.iter().all(|&x| x == 0) && tail.iter().all(|&x| x == 0)
}

unsafe fn vslot(obj: *mut c_void, slot: usize) -> usize {
    unsafe { *(*(obj as *const *const usize)).add(slot) }
}

/// Record a committed resource created by the title (zero-initialised unless its heap flags
/// say otherwise) and make sure its vtable is patched.
pub fn note_committed(res: *mut c_void, heap_flags: u32) {
    if res.is_null() {
        return;
    }
    let mut fresh = FRESH.lock().unwrap();
    let set = fresh.get_or_insert_with(HashSet::new);
    if heap_flags & HEAP_FLAG_CREATE_NOT_ZEROED == 0 {
        set.insert(res as usize);
    } else {
        set.remove(&(res as usize));
    }
    drop(fresh);
    ensure_patched(res);
}

/// A placed or reserved resource may reuse the address of a released committed one.
pub fn note_other(res: *mut c_void) {
    if res.is_null() {
        return;
    }
    if let Some(s) = FRESH.lock().unwrap().as_mut() {
        s.remove(&(res as usize));
    }
    ensure_patched(res);
}

fn ensure_patched(res: *mut c_void) {
    let vtbl = unsafe { *(res as *const *mut usize) };
    let mut patched = PATCHED_VTABLES.lock().unwrap();
    if patched.contains(&(vtbl as usize)) {
        return;
    }
    unsafe {
        let cur_map = *vtbl.add(SLOT_MAP);
        let cur_unmap = *vtbl.add(SLOT_UNMAP);
        if cur_map != map as *const () as usize {
            // Several resource vtables (e.g. per resource kind) share one implementation in
            // the runtime. A different implementation would need its own saved original.
            let prev = ORIG_MAP.swap(cur_map, Ordering::Relaxed);
            let prev_u = ORIG_UNMAP.swap(cur_unmap, Ordering::Relaxed);
            if (prev != 0 && prev != cur_map) || (prev_u != 0 && prev_u != cur_unmap) {
                ORIG_MAP.store(prev, Ordering::Relaxed);
                ORIG_UNMAP.store(prev_u, Ordering::Relaxed);
                record("d3d12_xs", "resource.patch",
                       &format!("vtable {vtbl:p} has a different Map/Unmap implementation; not patched"));
                patched.push(vtbl as usize);
                return;
            }
        }
        let addr = vtbl.add(SLOT_MAP) as *mut c_void;
        let mut old = 0u32;
        if VirtualProtect(addr, 16, PAGE_READWRITE, &mut old) != 0 {
            *vtbl.add(SLOT_MAP) = map as *const () as usize;
            *vtbl.add(SLOT_UNMAP) = unmap as *const () as usize;
            let mut tmp = 0u32;
            VirtualProtect(addr, 16, old, &mut tmp);
            patched.push(vtbl as usize);
            record("d3d12_xs", "resource.patch", &format!("vtable {vtbl:p} slots 8/9 (Map/Unmap) patched ({} total)", patched.len()));
        } else {
            record("d3d12_xs", "resource.patch", "VirtualProtect failed");
        }
    }
}

/// Bytes a title may touch through `Map` of subresource 0: the allocation size our
/// `GetResourceAllocationInfo` reports for the same description.
unsafe fn shadow_size(res: *mut c_void) -> u64 {
    #[repr(C, align(8))]
    struct Desc([u8; 0x38]);
    let mut d = Desc([0; 0x38]);
    let get_desc: unsafe extern "system" fn(*mut c_void, *mut Desc) -> *mut Desc =
        unsafe { core::mem::transmute(vslot(res, SLOT_GET_DESC)) };
    unsafe { get_desc(res, &mut d) };
    let b = &d.0;
    let u16at = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]) as u64;
    let u32at = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    crate::device_wrap::alloc_size_for(
        u32at(0),
        u64::from_le_bytes(b[0x10..0x18].try_into().unwrap()).max(1),
        (u32at(0x18) as u64).max(1),
        u16at(0x1c).max(1),
        u16at(0x1e).max(1),
        u32at(0x20),
        (u32at(0x24) as u64).max(1),
    )
}

unsafe extern "system" fn map(res: *mut c_void, sub: u32, range: *const c_void, out: *mut *mut c_void) -> i32 {
    let orig: unsafe extern "system" fn(*mut c_void, u32, *const c_void, *mut *mut c_void) -> i32 =
        unsafe { core::mem::transmute(ORIG_MAP.load(Ordering::Relaxed)) };
    // Ask for the heap first: the runtime refuses Map on a GPU-only heap (and the debug
    // layer reports it as an execution error), so such resources never reach it.
    let mut props = [0u32; 5];
    let mut flags = 0u32;
    let get_props: unsafe extern "system" fn(*mut c_void, *mut u32, *mut u32) -> i32 =
        unsafe { core::mem::transmute(vslot(res, SLOT_GET_HEAP_PROPERTIES)) };
    if out.is_null() || unsafe { get_props(res, props.as_mut_ptr(), &mut flags) } < 0 || !gpu_only(props[0], props[1]) {
        return unsafe { orig(res, sub, range, out) }; // CPU heap, reserved resource, or no pointer wanted
    }
    const E_INVALIDARG: i32 = 0x8007_0057u32 as i32;
    let hr = E_INVALIDARG; // what the runtime returns for a GPU-only heap
    let fresh = FRESH.lock().unwrap().as_mut().is_some_and(|s| s.remove(&(res as usize)));
    let placed_va = crate::placed::lookup(res).map(|p| p.va);
    let mut g = SHADOWS.lock().unwrap();
    let shadows = g.get_or_insert_with(HashMap::new);
    if let Some(s) = shadows.get_mut(&(res as usize)) {
        if sub == 0 {
            s.maps += 1;
            unsafe { *out = s.ptr as *mut c_void };
            return 0;
        }
    }
    if sub != 0 {
        record("d3d12_xs", "resource.Map",
               &format!("res={res:p} subresource {sub} of a GPU-only resource: console tiled offset unknown, refused hr={hr:#010x}"));
        return hr;
    }
    if let Some(va) = placed_va {
        // `CreatePlacedResourceX`: on the console the resource IS the title memory at `va`,
        // which upload.rs mirrors into the PC resource.
        shadows.insert(res as usize, Shadow { ptr: va as usize, size: 0, maps: 1, owned: false, gpu_zero: false });
        unsafe { *out = va as *mut c_void };
        return 0;
    }
    let size = unsafe { shadow_size(res) } as usize;
    let ptr = unsafe { VirtualAlloc(core::ptr::null_mut(), size, MEM_COMMIT_RESERVE, PAGE_READWRITE) };
    if ptr.is_null() {
        record("d3d12_xs", "resource.Map", &format!("res={res:p} shadow of {size:#x} bytes: VirtualAlloc failed"));
        return hr;
    }
    let gpu_zero = fresh && flags & HEAP_FLAG_CREATE_NOT_ZEROED == 0;
    shadows.insert(res as usize, Shadow { ptr: ptr as usize, size, maps: 1, owned: true, gpu_zero });
    static N: AtomicUsize = AtomicUsize::new(0);
    if N.fetch_add(1, Ordering::Relaxed) < 8 {
        record("d3d12_xs", "resource.Map",
               &format!("res={res:p} heapType={} GPU-only: CPU shadow {size:#x} bytes (fresh={fresh})", props[0]));
    }
    unsafe { *out = ptr };
    0
}

unsafe extern "system" fn unmap(res: *mut c_void, sub: u32, written: *const c_void) {
    let mut g = SHADOWS.lock().unwrap();
    let Some(shadows) = g.as_mut() else {
        drop(g);
        return unsafe { forward_unmap(res, sub, written) };
    };
    let Some(s) = shadows.get_mut(&(res as usize)) else {
        drop(g);
        return unsafe { forward_unmap(res, sub, written) };
    };
    if sub != 0 {
        return; // Map refused it; nothing is mapped
    }
    s.maps -= 1;
    if s.maps > 0 {
        return;
    }
    let s = shadows.remove(&(res as usize)).unwrap();
    drop(g);
    if !s.owned {
        return;
    }
    let bytes = unsafe { core::slice::from_raw_parts(s.ptr as *const u8, s.size) };
    match unmap_outcome(s.gpu_zero, bytes) {
        UnmapOutcome::InSync => {}
        UnmapOutcome::Lost => {
            static N: AtomicUsize = AtomicUsize::new(0);
            let n = N.fetch_add(1, Ordering::Relaxed);
            if n < 16 || n.is_power_of_two() {
                record("d3d12_xs", "resource.Unmap", &format!(
                    "ERROR res={res:p}: CPU writes to a GPU-only resource were NOT transferred \
                     (zero={} knownZeroGpu={}, {} occurrences)", all_zero(bytes), s.gpu_zero, n + 1));
            }
        }
    }
    unsafe { VirtualFree(s.ptr as *mut c_void, 0, MEM_RELEASE) };
}

unsafe fn forward_unmap(res: *mut c_void, sub: u32, written: *const c_void) {
    let orig: unsafe extern "system" fn(*mut c_void, u32, *const c_void) =
        unsafe { core::mem::transmute(ORIG_UNMAP.load(Ordering::Relaxed)) };
    unsafe { orig(res, sub, written) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu_only_heaps() {
        assert!(gpu_only(HEAP_TYPE_DEFAULT, 0));
        assert!(!gpu_only(2, 0)); // UPLOAD
        assert!(!gpu_only(3, 0)); // READBACK
        assert!(gpu_only(HEAP_TYPE_CUSTOM, CPU_PAGE_PROPERTY_NOT_AVAILABLE));
        assert!(!gpu_only(HEAP_TYPE_CUSTOM, 3)); // WRITE_BACK
    }

    #[test]
    fn unmap_outcomes() {
        let mut b = vec![0u8; 4099];
        assert_eq!(unmap_outcome(true, &b), UnmapOutcome::InSync);
        assert_eq!(unmap_outcome(false, &b), UnmapOutcome::Lost);
        b[4098] = 1; // unaligned tail
        assert_eq!(unmap_outcome(true, &b), UnmapOutcome::Lost);
        b[4098] = 0;
        b[1] = 1; // unaligned head
        assert_eq!(unmap_outcome(true, &b[1..]), UnmapOutcome::Lost);
    }

    /// Real device: the PC runtime refuses `Map` on a DEFAULT-heap render target, and the
    /// patched vtable provides a shadow covering the allocation the title zeroes.
    #[test]
    fn default_heap_texture_map_gets_shadow() {
        #[repr(C)]
        struct Guid(u32, u16, u16, [u8; 8]);
        const IID_DEVICE: Guid = Guid(0x189819f1, 0x1db6, 0x4b57, [0xbe, 0x54, 0x18, 0x21, 0x33, 0x9b, 0x85, 0xf7]);
        const IID_RESOURCE: Guid = Guid(0x696442be, 0xa72e, 0x4059, [0xbc, 0x79, 0x5b, 0x5c, 0x98, 0x04, 0x0f, 0xad]);
        extern "system" {
            fn LoadLibraryA(n: *const u8) -> *mut c_void;
            fn GetProcAddress(h: *mut c_void, n: *const u8) -> *mut c_void;
        }
        unsafe {
            let d3d12 = LoadLibraryA(b"d3d12.dll\0".as_ptr());
            let create = if d3d12.is_null() { core::ptr::null_mut() } else { GetProcAddress(d3d12, b"D3D12CreateDevice\0".as_ptr()) };
            if create.is_null() {
                eprintln!("skipped: no d3d12.dll");
                return;
            }
            let create: unsafe extern "system" fn(*mut c_void, u32, *const Guid, *mut *mut c_void) -> i32 = core::mem::transmute(create);
            let mut dev = core::ptr::null_mut();
            if create(core::ptr::null_mut(), 0xb000, &IID_DEVICE, &mut dev) < 0 {
                eprintln!("skipped: no D3D12 device");
                return;
            }
            // DEFAULT heap, RGBA8 1904x1041 render target (the shape seen in the title).
            let heap = [HEAP_TYPE_DEFAULT, 0, 0, 1, 1];
            let mut desc = [0u8; 0x38];
            desc[0..4].copy_from_slice(&3u32.to_le_bytes());
            desc[0x10..0x18].copy_from_slice(&1904u64.to_le_bytes());
            desc[0x18..0x1c].copy_from_slice(&1041u32.to_le_bytes());
            desc[0x1c..0x1e].copy_from_slice(&1u16.to_le_bytes());
            desc[0x1e..0x20].copy_from_slice(&1u16.to_le_bytes());
            desc[0x20..0x24].copy_from_slice(&28u32.to_le_bytes());
            desc[0x24..0x28].copy_from_slice(&1u32.to_le_bytes());
            desc[0x30..0x34].copy_from_slice(&1u32.to_le_bytes()); // ALLOW_RENDER_TARGET
            let ccr: unsafe extern "system" fn(*mut c_void, *const u32, u32, *const u8, u32, *const c_void, *const Guid, *mut *mut c_void) -> i32 =
                core::mem::transmute(vslot(dev, 27));
            let mut res = core::ptr::null_mut();
            assert!(ccr(dev, heap.as_ptr(), 0, desc.as_ptr(), 4 /* RENDER_TARGET */, core::ptr::null(), &IID_RESOURCE, &mut res) >= 0);

            let map_fn: unsafe extern "system" fn(*mut c_void, u32, *const c_void, *mut *mut c_void) -> i32 =
                core::mem::transmute(vslot(res, SLOT_MAP));
            let mut p = core::ptr::null_mut();
            assert!(map_fn(res, 0, core::ptr::null(), &mut p) < 0, "PC runtime unexpectedly maps a DEFAULT-heap texture");
            assert!(p.is_null());

            note_committed(res, 0);
            let map_fn: unsafe extern "system" fn(*mut c_void, u32, *const c_void, *mut *mut c_void) -> i32 =
                core::mem::transmute(vslot(res, SLOT_MAP));
            let unmap_fn: unsafe extern "system" fn(*mut c_void, u32, *const c_void) = core::mem::transmute(vslot(res, SLOT_UNMAP));
            assert_eq!(map_fn(res, 0, core::ptr::null(), &mut p), 0);
            assert!(!p.is_null());
            let size = crate::device_wrap::alloc_size_for(3, 1904, 1041, 1, 1, 28, 1) as usize;
            assert_eq!(size, 0x870000); // the size the title memsets (logged in game)
            core::ptr::write_bytes(p.cast::<u8>(), 0, size);
            let mut p2 = core::ptr::null_mut();
            assert_eq!(map_fn(res, 0, core::ptr::null(), &mut p2), 0); // nested map: same shadow
            assert_eq!(p, p2);
            assert!(map_fn(res, 1, core::ptr::null(), &mut p2) < 0); // other subresources refused
            unmap_fn(res, 0, core::ptr::null());
            assert!(SHADOWS.lock().unwrap().as_ref().unwrap().contains_key(&(res as usize)));
            unmap_fn(res, 0, core::ptr::null());
            assert!(!SHADOWS.lock().unwrap().as_ref().unwrap().contains_key(&(res as usize)));

            // An upload buffer still maps through the runtime, not a shadow.
            let up = [2u32, 0, 0, 1, 1];
            let mut bdesc = [0u8; 0x38];
            bdesc[0..4].copy_from_slice(&1u32.to_le_bytes());
            bdesc[0x10..0x18].copy_from_slice(&256u64.to_le_bytes());
            bdesc[0x18..0x1c].copy_from_slice(&1u32.to_le_bytes());
            bdesc[0x1c..0x1e].copy_from_slice(&1u16.to_le_bytes());
            bdesc[0x1e..0x20].copy_from_slice(&1u16.to_le_bytes());
            bdesc[0x24..0x28].copy_from_slice(&1u32.to_le_bytes());
            bdesc[0x2c..0x30].copy_from_slice(&1u32.to_le_bytes()); // ROW_MAJOR
            let mut buf = core::ptr::null_mut();
            assert!(ccr(dev, up.as_ptr(), 0, bdesc.as_ptr(), 0xac3 /* GENERIC_READ */, core::ptr::null(), &IID_RESOURCE, &mut buf) >= 0);
            note_committed(buf, 0);
            let map_fn: unsafe extern "system" fn(*mut c_void, u32, *const c_void, *mut *mut c_void) -> i32 =
                core::mem::transmute(vslot(buf, SLOT_MAP));
            assert_eq!(map_fn(buf, 0, core::ptr::null(), &mut p), 0);
            assert!(!SHADOWS.lock().unwrap().as_ref().unwrap().contains_key(&(buf as usize)));
            let unmap_fn: unsafe extern "system" fn(*mut c_void, u32, *const c_void) = core::mem::transmute(vslot(buf, SLOT_UNMAP));
            unmap_fn(buf, 0, core::ptr::null());

            let release = |o: *mut c_void| {
                let f: unsafe extern "system" fn(*mut c_void) -> u32 = core::mem::transmute(vslot(o, 2));
                f(o)
            };
            release(buf);
            release(res);
            release(dev);
        }
    }
}
