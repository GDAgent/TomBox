//! A COM wrapper around a real PC `ID3D12Device` that lets us intercept
//! `QueryInterface` (to supply Xbox-only interfaces such as the merged DXGI
//! device) while forwarding every other `ID3D12Device` method straight to the
//! real device via naked tail-call thunks.
//!
//! Layout: `{ vtable, real, refs }`. The forwarding thunks replace `this` (our
//! wrapper) with `this->real` and jump to the real method at the same vtable
//! slot — ABI-exact for any signature.

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;
use compat_common::record;

const S_OK: i32 = 0;
const E_POINTER: i32 = 0x8000_4003u32 as i32;
const E_NOINTERFACE: i32 = 0x8000_4002u32 as i32;

#[repr(C)]
#[derive(PartialEq)]
pub struct Guid {
    pub d1: u32,
    pub d2: u16,
    pub d3: u16,
    pub d4: [u8; 8],
}

const IID_IUNKNOWN: Guid = Guid { d1: 0, d2: 0, d3: 0, d4: [0xC0, 0, 0, 0, 0, 0, 0, 0x46] };

/// ID3D12Device through ID3D12Device8 — all resolve to our wrapper so every
/// device call (CreateDescriptorHeap etc.) stays intercepted.
const DEVICE_IIDS: [Guid; 9] = [
    Guid { d1: 0x1898_19F1, d2: 0x1DB6, d3: 0x4B57, d4: [0xBE, 0x54, 0x18, 0x21, 0x33, 0x9B, 0x85, 0xF7] }, // Device
    Guid { d1: 0x77ac_ce80, d2: 0x638e, d3: 0x4e65, d4: [0x88, 0x95, 0xc1, 0xf2, 0x33, 0x86, 0x86, 0x3e] }, // Device1
    Guid { d1: 0x30ba_a41e, d2: 0xb15b, d3: 0x475c, d4: [0xa0, 0xbb, 0x1a, 0xf5, 0xc5, 0xb6, 0x43, 0x28] }, // Device2
    Guid { d1: 0x81da_dc15, d2: 0x2bad, d3: 0x4392, d4: [0x93, 0xc5, 0x10, 0x13, 0x45, 0xc4, 0xaa, 0x98] }, // Device3
    Guid { d1: 0xe865_df17, d2: 0xa9ee, d3: 0x46f9, d4: [0xa4, 0x63, 0x30, 0x98, 0x31, 0x5a, 0xa2, 0xe5] }, // Device4
    Guid { d1: 0x8b4f_173b, d2: 0x2fea, d3: 0x4b80, d4: [0x8f, 0x58, 0x43, 0x07, 0x19, 0x1a, 0xb9, 0x5d] }, // Device5
    Guid { d1: 0xc70b_221b, d2: 0x40e4, d3: 0x4a17, d4: [0x89, 0xaf, 0x02, 0x5a, 0x07, 0x27, 0xa6, 0xdc] }, // Device6
    Guid { d1: 0x5c01_4b53, d2: 0x68a1, d3: 0x4b9b, d4: [0x8b, 0xd1, 0xdd, 0x60, 0x46, 0xb9, 0x35, 0x8b] }, // Device7
    Guid { d1: 0x9218_e6bb, d2: 0xf944, d3: 0x4f7e, d4: [0xa7, 0x5c, 0xb1, 0xb2, 0xc7, 0xb7, 0x01, 0xf3] }, // Device8
];

fn is_device_iid(g: &Guid) -> bool {
    DEVICE_IIDS.iter().any(|d| d == g)
}

#[repr(C)]
pub struct DeviceWrapper {
    vtable: *const usize,
    real: *mut c_void,
    refs: AtomicU32,
}

fn guid_str(g: &Guid) -> String {
    format!(
        "{{{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}}}",
        g.d1, g.d2, g.d3, g.d4[0], g.d4[1], g.d4[2], g.d4[3], g.d4[4], g.d4[5], g.d4[6], g.d4[7]
    )
}

unsafe fn real_vtbl(real: *mut c_void) -> *const usize {
    unsafe { *(real as *const *const usize) }
}

unsafe extern "system" fn qi(this: *mut DeviceWrapper, riid: *const Guid, ppv: *mut *mut c_void) -> i32 {
    if ppv.is_null() || riid.is_null() {
        return E_POINTER;
    }
    unsafe { *ppv = core::ptr::null_mut() };
    let g = unsafe { &*riid };
    if is_device_iid(g) || *g == IID_IUNKNOWN {
        unsafe {
            *ppv = this as *mut c_void;
            (*this).refs.fetch_add(1, Ordering::SeqCst);
        }
        return S_OK;
    }
    // Xbox "DXGIDevice" interface: synthesize it (PC D3D12 devices lack IDXGIDevice).
    if *g == crate::dxgi_device::IID_XBOX_DXGIDEVICE {
        let shim = crate::dxgi_device::create();
        unsafe { *ppv = shim };
        record("d3d12_xs", "device.QueryInterface", "Xbox DXGIDevice -> synthesized shim");
        return S_OK;
    }
    // Forward anything else to the real device (higher ID3D12Device versions,
    // info-queue, etc.). Log the IID + result so unknown Xbox IIDs surface.
    let real = unsafe { (*this).real };
    let rvt = unsafe { real_vtbl(real) };
    let real_qi: unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> i32 =
        unsafe { core::mem::transmute(*rvt) };
    let hr = unsafe { real_qi(real, riid, ppv) };
    record(
        "d3d12_xs",
        "device.QueryInterface",
        &format!("{} -> hr={hr:#010x}{}", guid_str(g), if hr != S_OK { " (UNHANDLED)" } else { "" }),
    );
    if hr == S_OK { hr } else { E_NOINTERFACE }
}

unsafe extern "system" fn addref(this: *mut DeviceWrapper) -> u32 {
    unsafe { (*this).refs.fetch_add(1, Ordering::SeqCst) + 1 }
}

unsafe extern "system" fn release(this: *mut DeviceWrapper) -> u32 {
    let n = unsafe { (*this).refs.fetch_sub(1, Ordering::SeqCst) } - 1;
    if n == 0 {
        unsafe {
            let real = (*this).real;
            let rvt = real_vtbl(real);
            let real_rel: unsafe extern "system" fn(*mut c_void) -> u32 =
                core::mem::transmute(*rvt.add(2));
            real_rel(real);
            drop(Box::from_raw(this));
        }
    }
    n
}

/// Intercept `CheckFeatureSupport` (slot 13): the Xbox feature structs differ in
/// size from PC, so a verbatim forward gets E_INVALIDARG. Zero the caller buffer
/// and negotiate the largest size <= the requested size that the real device
/// accepts (D3D12 feature structs are append-only, so filling the PC-known prefix
/// and leaving Xbox-only trailing fields zero is sound).
unsafe extern "system" fn check_feature_support(
    this: *mut DeviceWrapper, feature: u32, pdata: *mut c_void, size: u32,
) -> i32 {
    let real = unsafe { (*this).real };
    let rvt = unsafe { real_vtbl(real) };
    let f: unsafe extern "system" fn(*mut c_void, u32, *mut c_void, u32) -> i32 =
        unsafe { core::mem::transmute(*rvt.add(13)) };
    let hr = unsafe { f(real, feature, pdata, size) };
    if hr == S_OK {
        return S_OK;
    }
    if !pdata.is_null() && size > 0 && size <= 65536 {
        unsafe { core::ptr::write_bytes(pdata as *mut u8, 0, size as usize) };
        let mut try_size = size & !3; // 4-byte aligned
        while try_size >= 4 {
            let hr2 = unsafe { f(real, feature, pdata, try_size) };
            if hr2 == S_OK {
                record("d3d12_xs", "CheckFeatureSupport",
                    &format!("feature={feature} xbox_size={size} -> PC size={try_size} S_OK"));
                return S_OK;
            }
            try_size -= 4;
        }
    }
    record("d3d12_xs", "CheckFeatureSupport", &format!("feature={feature} size={size} UNHANDLED hr={hr:#010x}"));
    hr
}

/// Intercept `CreateDescriptorHeap` (slot 14) and wrap the returned heap so the
/// descriptor-handle getters get the Xbox RAX return-convention bridge.
unsafe extern "system" fn create_descriptor_heap(
    this: *mut DeviceWrapper, pdesc: *const c_void, riid: *const Guid, ppv: *mut *mut c_void,
) -> i32 {
    let real = unsafe { (*this).real };
    let f: unsafe extern "system" fn(*mut c_void, *const c_void, *const Guid, *mut *mut c_void) -> i32 =
        unsafe { core::mem::transmute(*real_vtbl(real).add(14)) };
    let hr = unsafe { f(real, pdesc, riid, ppv) };
    if hr == S_OK && !ppv.is_null() {
        // Keep the heap REAL (so it works when passed to SetDescriptorHeaps etc.);
        // patch the shared heap vtable once to fix the handle-getter ABI.
        crate::heap_wrap::ensure_patched(unsafe { *ppv });
        if crate::rtcap::enabled() && !pdesc.is_null() {
            // Binding trace: remember shader-visible heaps (GPU handle -> CPU handle).
            let d = pdesc as *const u32;
            let (htype, numd, flags) = unsafe { (*d.add(0), *d.add(1), *d.add(2)) };
            let inc: unsafe extern "system" fn(*mut c_void, u32) -> u32 = unsafe { core::mem::transmute(*real_vtbl(real).add(15)) };
            for t in 0..4 {
                crate::bindtrace::set_increment(t, unsafe { inc(real, t) });
            }
            if flags & 1 != 0 {
                let heap = unsafe { *ppv };
                let hv = unsafe { *(heap as *const *const usize) };
                let c: unsafe extern "system" fn(*mut c_void) -> u64 = unsafe { core::mem::transmute(*hv.add(9)) };
                let g: unsafe extern "system" fn(*mut c_void) -> u64 = unsafe { core::mem::transmute(*hv.add(10)) };
                crate::bindtrace::note_heap(unsafe { c(heap) } as usize, unsafe { g(heap) }, numd, htype);
            }
        }
        // Log DESC { Type@0, NumDescriptors@4, Flags@8, NodeMask@0xC } + GPU base so
        // we can correlate the shader-visible online heap with the handles the game
        // later binds. Only log shader-visible (Flags bit0) or the first few.
        static N: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
        if !pdesc.is_null() {
            let d = pdesc as *const u32;
            let (htype, numd, flags) = unsafe { (*d.add(0), *d.add(1), *d.add(2)) };
            let shader_visible = flags & 1 != 0;
            if shader_visible || N.fetch_add(1, Ordering::Relaxed) < 10 {
                let heap = unsafe { *ppv };
                // GetGPUDescriptorHandleForHeapStart (slot 10, now our bridge -> u64)
                let gpu_base: u64 = if shader_visible {
                    let g: unsafe extern "system" fn(*mut c_void) -> u64 =
                        unsafe { core::mem::transmute(*(*(heap as *const *const usize)).add(10)) };
                    unsafe { g(heap) }
                } else { 0 };
                record("d3d12_xs", "CreateDescriptorHeap",
                    &format!("type={htype} num={numd} flags={flags:#x} shaderVis={shader_visible} heap={heap:p} gpuBase={gpu_base:#x}"));
            }
        }
    }
    hr
}

/// Intercept the Xbox extension `CreateCommandQueueX` (slot 48) and translate to
/// the real PC `CreateCommandQueue` (slot 8). The Xbox
/// `D3D12XBOX_COMMAND_QUEUE_DESC` extends the PC `D3D12_COMMAND_QUEUE_DESC`; the
/// first 16 bytes ({Type, Priority, Flags, NodeMask}) are layout-compatible.
unsafe extern "system" fn create_command_queue_x(
    this: *mut DeviceWrapper, pdescx: *const c_void, riid: *const Guid, ppv: *mut *mut c_void,
) -> i32 {
    let real = unsafe { (*this).real };
    let mut pc_desc = [0u8; 16];
    if !pdescx.is_null() {
        unsafe { core::ptr::copy_nonoverlapping(pdescx as *const u8, pc_desc.as_mut_ptr(), 16) };
    }
    let f: unsafe extern "system" fn(*mut c_void, *const c_void, *const Guid, *mut *mut c_void) -> i32 =
        unsafe { core::mem::transmute(*real_vtbl(real).add(8)) }; // PC CreateCommandQueue
    let hr = unsafe { f(real, pc_desc.as_ptr() as *const c_void, riid, ppv) };
    let qtype = u32::from_le_bytes([pc_desc[0], pc_desc[1], pc_desc[2], pc_desc[3]]);
    let qflags = u32::from_le_bytes([pc_desc[8], pc_desc[9], pc_desc[10], pc_desc[11]]);
    if hr == 0 && !ppv.is_null() {
        // Give the queue an Xbox-sized vtable (PresentX at slot 30, other extensions no-op).
        crate::present::apply_queue_vtable(unsafe { *ppv });
    }
    record("d3d12_xs", "CreateCommandQueueX", &format!("type={qtype} flags={qflags} -> hr={hr:#010x}"));
    hr
}

/// Xbox slot 73 = command-list create. Xbox signature (read from the call site):
///   HRESULT (this, const Params* [rdx], ID3D12CommandAllocator* [r8],
///            ID3D12PipelineState* [r9], REFIID, void** ppList)
/// where Params is `{ u32 NodeMask@0; u32 _@4; D3D12_COMMAND_LIST_TYPE Type@8; u32 _@0xC }`.
/// PC `CreateCommandList` (slot 12) takes (NodeMask, Type, alloc, initState, riid, ppList).
unsafe extern "system" fn create_command_list_x(
    this: *mut DeviceWrapper, params: *const u32, alloc: *mut c_void,
    init_state: *mut c_void, riid: *const Guid, ppv: *mut *mut c_void,
) -> i32 {
    // Struct is { Type@0, _@4, NodeMask@8, _@0xC } (confirmed: type=0 DIRECT,
    // nodeMask=1). PC requires a non-zero node mask.
    let (list_type, mut node_mask) = if params.is_null() {
        (0u32, 1u32)
    } else {
        unsafe { (*params.add(0), *params.add(2)) }
    };
    if node_mask == 0 {
        node_mask = 1;
    }
    let real = unsafe { (*this).real };
    let f: unsafe extern "system" fn(*mut c_void, u32, u32, *mut c_void, *mut c_void, *const Guid, *mut *mut c_void) -> i32 =
        unsafe { core::mem::transmute(*real_vtbl(real).add(12)) };
    let hr = unsafe { f(real, node_mask, list_type, alloc, init_state, riid, ppv) };
    if hr == S_OK && !ppv.is_null() {
        // Hand the title a shim object (Xbox vtable layout + inert inline-buffer fields)
        // instead of the real PC command list; see cmdlist_wrap.
        unsafe { *ppv = crate::cmdlist_wrap::wrap(*ppv, init_state) };
    }
    record("d3d12_xs", "CreateCommandList", &format!("nodeMask={node_mask} type={list_type} -> hr={hr:#010x}"));
    hr
}

/// Slot 15: GetDescriptorHandleIncrementSize(type) -> UINT. Log what the game
/// queries and what the real (NVIDIA) device reports, then forward.
unsafe extern "system" fn get_descriptor_increment(this: *mut DeviceWrapper, heap_type: u32) -> u32 {
    let real = unsafe { (*this).real };
    let f: unsafe extern "system" fn(*mut c_void, u32) -> u32 =
        unsafe { core::mem::transmute(*real_vtbl(real).add(15)) };
    let sz = unsafe { f(real, heap_type) };
    static N: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
    if N.fetch_add(1, Ordering::Relaxed) < 8 {
        record("d3d12_xs", "GetDescriptorHandleIncrementSize", &format!("type={heap_type} -> {sz}"));
    }
    sz
}

/// ID3D12Device::GetResourceAllocationInfo (slot 25) returns D3D12_RESOURCE_ALLOCATION_INFO
/// {UINT64 SizeInBytes; UINT64 Alignment;} by value -> sret ABI.
#[repr(C)]
#[derive(Clone, Copy)]
struct AllocInfo { size: u64, align: u64 }

/// Intercept GetResourceAllocationInfo to see pResourceDescs (the crash is a null
/// deref inside the real method), dump the desc, and avoid crashing on a null/bad
/// desc. Rust's sret for a 16-byte return matches the PC ABI
/// (rcx=retval, rdx=this, r8=visibleMask, r9=numDescs, stack=pResourceDescs).
unsafe extern "system" fn get_resource_alloc_info(
    this: *mut DeviceWrapper, visible_mask: u32, num_descs: u32, pdescs: *const u32,
) -> AllocInfo {
    static N: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    if pdescs.is_null() {
        if n < 20 { record("d3d12_xs", "GetResourceAllocationInfo", &format!("numDescs={num_descs} pDescs=NULL -> default{{64KB,64KB}}")); }
        return AllocInfo { size: 0x10000, align: 0x10000 };
    }
    // Copy and sanitize the desc(s): Xbox may leave SampleDesc.Count / DepthOrArraySize
    // / MipLevels as 0, which are invalid on PC D3D12 and crash GetResourceAllocationInfo.
    // D3D12_RESOURCE_DESC is 0x40 bytes: Dimension@0, Alignment@8, Width@16, Height@24,
    // DepthOrArraySize@28(u16), MipLevels@30(u16), Format@32, SampleDesc{Count@36,Quality@40},
    // Layout@44, Flags@48.
    let count = num_descs.max(1) as usize;
    // Zeroed, DESC1-sized (0x48) copies so a runtime reading past D3D12_RESOURCE_DESC
    // sees zeros, not adjacent heap garbage.
    let mut descs: Vec<u8> = vec![0u8; count * 0x48];
    for i in 0..count {
        let src = unsafe { core::slice::from_raw_parts(pdescs.cast::<u8>().add(i * 0x40), 0x40) };
        descs[i * 0x48..i * 0x48 + 0x40].copy_from_slice(src);
        let b = &mut descs[i * 0x48..];
        let u16at = |b: &[u8], o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
        let u32at = |b: &[u8], o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        let u64at = |b: &[u8], o: usize| u64::from_le_bytes(b[o..o+8].try_into().unwrap());
        let align = u64at(b, 8);
        if align != 0 && align != 0x1000 && align != 0x10000 && align != 0x400000 {
            b[8..16].copy_from_slice(&0u64.to_le_bytes()); // bad alignment -> 0 (let D3D12 pick)
        }
        if u16at(b, 28) == 0 { b[28] = 1; } // DepthOrArraySize 0 -> 1
        if u16at(b, 30) == 0 { b[30] = 1; } // MipLevels 0 -> 1
        if u32at(b, 36) == 0 { b[36] = 1; } // SampleDesc.Count 0 -> 1
        if n < 8 && i == 0 {
            record("d3d12_xs", "GetResourceAllocationInfo",
                &format!("dim={} align={:#x} w={} h={} depthArr={} mips={} fmt={} sampleCount={} layout={} flags={:#x}",
                    u32at(b, 0), align, u64at(b, 16), u32at(b, 24),
                    u16at(b, 28), u16at(b, 30), u32at(b, 32), u32at(b, 36), u32at(b, 44), u32at(b, 48)));
        }
    }
    // The real D3D12Core GetResourceAllocationInfo faults deep in its texture-tiling
    // layout sub-function for the Xbox-origin texture descriptors (verified: valid
    // device + fully-sanitized valid desc still crashes at vtbl+0x25829). Compute the
    // info ourselves from the D3D12 64 KiB standard-swizzle tile model. We OVER-estimate
    // (round each mip up to whole tiles) so the pool allocator never under-reserves.
    let total = (0..count).map(|i| {
        let b = &descs[i * 0x48..];
        let u16at = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
        let u32at = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        let u64at = |o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap());
        let dim = u32at(0);
        let width = u64at(16).max(1);
        let height = (u32at(24) as u64).max(1);
        let arr = (u16at(28) as u64).max(1);
        let mips = (u16at(30) as u64).max(1);
        let samples = (u32at(36) as u64).max(1);
        alloc_size_for(dim, width, height, arr, mips, u32at(32), samples)
    }).max().unwrap_or(0x10000);
    let align = if descs.len() >= 16 && u64::from_le_bytes(descs[8..16].try_into().unwrap()) == 0x1000 && total <= 0x10000 {
        0x1000
    } else {
        0x10000
    };
    let info = AllocInfo { size: total, align };
    if n < 8 {
        record("d3d12_xs", "GetResourceAllocationInfo", &format!("COMPUTED size={:#x} align={:#x}", info.size, info.align));
    }
    let _ = (this, visible_mask);
    info
}

/// Bytes per pixel for the common DXGI formats (block-compressed reported as their
/// effective per-pixel cost so the tile estimate stays an over-estimate).
fn format_bpp(fmt: u32) -> u64 {
    match fmt {
        1..=4 => 16,                   // R32G32B32A32
        5..=8 => 12,                   // R32G32B32
        9..=14 => 8,                   // R16G16B16A16
        15..=22 => 8,                  // R32G32 / depth-stencil combos
        70..=72 => 1,                  // BC1 (0.5 bpp, rounded up)
        73..=78 => 1,                  // BC2/BC3
        79..=84 => 1,                  // BC4/BC5
        94..=99 => 1,                  // BC6H/BC7
        54..=59 => 2,                  // R16
        60..=65 => 1,                  // R8
        _ => 4,                        // R8G8B8A8 / B8G8R8A8 / R10G10B10A2 / etc.
    }
}

/// D3D12 64 KiB standard-swizzle tile size, over-estimated (whole tiles per mip).
fn alloc_size_for(dim: u32, width: u64, height: u64, arr: u64, mips: u64, fmt: u32, samples: u64) -> u64 {
    const TILE: u64 = 0x10000;
    if dim <= 1 {
        // Buffer: 64 KiB-aligned length.
        return ((width + TILE - 1) & !(TILE - 1)).max(TILE);
    }
    let bpp = format_bpp(fmt);
    // 64 KiB tile pixel extents by bpp (D3D12 standard-swizzle 2D tile shapes).
    let (tw, th) = match bpp {
        16 => (64u64, 64u64),
        12 | 8 => (128, 64),
        4 => (128, 128),
        2 => (256, 128),
        _ => (256, 256), // 1 bpp and BC (approx)
    };
    let mut tiles = 0u64;
    let (mut w, mut h) = (width, height);
    for _ in 0..mips {
        let tx = (w + tw - 1) / tw;
        let ty = (h + th - 1) / th;
        tiles += (tx * ty).max(1);
        w = (w / 2).max(1);
        h = (h / 2).max(1);
    }
    (tiles * arr * samples * TILE).max(TILE)
}

/// Intercept CreateConstantBufferView (slot 17). Xbox allows a CBV SizeInBytes that
/// is not a multiple of 256; PC D3D12 rejects it and REMOVES THE DEVICE
/// (DXGI_ERROR_INVALID_CALL). Round SizeInBytes up to 256.
/// Args: rcx=this, rdx=pDesc, r8=DestDescriptor (CPU handle, by value).
/// D3D12_CONSTANT_BUFFER_VIEW_DESC { u64 BufferLocation; u32 SizeInBytes; }.
unsafe extern "system" fn create_cbv(this: *mut DeviceWrapper, pdesc: *const u8, dest: usize) {
    let real = unsafe { (*this).real };
    let f: unsafe extern "system" fn(*mut c_void, *const u8, usize) =
        unsafe { core::mem::transmute(*real_vtbl(real).add(17)) };
    if pdesc.is_null() {
        unsafe { f(real, pdesc, dest) };
        return;
    }
    let mut desc = [0u8; 16];
    unsafe { core::ptr::copy_nonoverlapping(pdesc, desc.as_mut_ptr(), 12) };
    let size = u32::from_le_bytes([desc[8], desc[9], desc[10], desc[11]]);
    let aligned = (size + 255) & !255;
    if aligned != size {
        desc[8..12].copy_from_slice(&aligned.to_le_bytes());
        static N: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
        if N.fetch_add(1, Ordering::Relaxed) < 6 {
            record("d3d12_xs", "CreateConstantBufferView", &format!("size {size} -> {aligned} (256-aligned)"));
        }
    }
    unsafe { f(real, desc.as_ptr(), dest) };
}

/// PC IID_ID3D12Resource = {696442be-a72e-4059-bc79-5b5c98040fad}
static IID_ID3D12RESOURCE: Guid = Guid {
    d1: 0x6964_42be, d2: 0xa72e, d3: 0x4059, d4: [0xbc, 0x79, 0x5b, 0x5c, 0x98, 0x04, 0x0f, 0xad],
};

/// Intercept CreateCommittedResource (slot 27). Logs heap/desc/riid and, like the
/// device/file IIDs, substitutes the PC ID3D12Resource IID so a mismatched Xbox
/// resource IID doesn't make the real call return a null resource.
/// Args (system ABI): rcx=this, rdx=pHeapProps, r8=HeapFlags, r9=pDesc, then
/// stack: InitialState, pOptimizedClearValue, riid, ppvResource.
unsafe extern "system" fn create_committed_resource(
    this: *mut DeviceWrapper, pheap: *const u32, heap_flags: u32, pdesc: *const u32,
    init_state: u32, pclear: *const c_void, riid: *const Guid, ppv: *mut *mut c_void,
) -> i32 {
    let real = unsafe { (*this).real };
    let f: unsafe extern "system" fn(*mut c_void, *const u32, u32, *const u32, u32, *const c_void, *const Guid, *mut *mut c_void) -> i32 =
        unsafe { core::mem::transmute(*real_vtbl(real).add(27)) };
    let use_riid: *const Guid = &IID_ID3D12RESOURCE;
    // Mask D3D12_RESOURCE_DESC::Flags (offset 0x30) to PC-valid bits: Xbox sets
    // console-only flags (e.g. 0x300000) that PC D3D12 rejects, removing the device.
    #[repr(align(8))]
    struct AlignedDesc([u8; 0x38]);
    let mut storage = AlignedDesc([0; 0x38]);
    let desc_copy = &mut storage.0;
    let desc_ptr = if pdesc.is_null() {
        pdesc
    } else {
        // PC fields end at byte 52. The remaining four bytes can contain Xbox DCC options.
        unsafe { core::ptr::copy_nonoverlapping(pdesc.cast::<u8>(), desc_copy.as_mut_ptr(), 0x34) };
        let flags = u32::from_le_bytes(desc_copy[0x30..0x34].try_into().unwrap());
        // The Scarlett upscaler selects layout 0x101 for private GPU textures
        // (April DLL RVA 0x3732). Their backing bytes never cross the API.
        // D3D12_TEXTURE_LAYOUT_UNKNOWN lets the desktop driver tile them.
        let dim = u32::from_le_bytes(desc_copy[..4].try_into().unwrap());
        let layout = u32::from_le_bytes(desc_copy[0x2c..0x30].try_into().unwrap());
        if dim == 3 && layout == 0x101 && heap_flags == 0 && !pheap.is_null()
            && unsafe { *pheap } == 1 && flags & 0x10 == 0 {
            desc_copy[0x2c..0x30].fill(0);
        }
        const PC_RESOURCE_FLAGS: u32 = 0xFF; // RT|DSV|UAV|DENY_SRV|CROSS_ADAPTER|SIMUL|VIDEO*
        desc_copy[0x30..0x34].copy_from_slice(&(flags & PC_RESOURCE_FLAGS).to_le_bytes());
        desc_copy.as_ptr() as *const u32
    };
    let hr = unsafe { f(real, pheap, heap_flags, desc_ptr, init_state, pclear, use_riid, ppv) };
    if hr >= 0 && !ppv.is_null() && !pdesc.is_null() {
        unsafe { crate::rtcap::register(*ppv, &desc_copy[..0x38], init_state) };
        if crate::rtcap::enabled() && desc_copy[0] == 1 && !pheap.is_null() {
            let width = u64::from_le_bytes(desc_copy[0x10..0x18].try_into().unwrap());
            unsafe { crate::bindtrace::note_buffer(*ppv, width, *pheap) };
        }
    }
    static N: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
    if hr < 0 && !pdesc.is_null() {
        let orig = unsafe { core::slice::from_raw_parts(pdesc as *const u8, 0x38) };
        let hp = if pheap.is_null() { String::new() } else { unsafe { format!("heap{{type={} cpu={} pool={}}}", *pheap, *pheap.add(1), *pheap.add(2)) } };
        record("d3d12_xs", "CreateCommittedResource", &format!(
            "FAILED hr={hr:#010x} {hp} heapFlags={heap_flags:#x} state={init_state:#x} clear={pclear:p} {}", desc_str(orig)));
    }
    if N.fetch_add(1, Ordering::Relaxed) < 10 {
        let htype = if pheap.is_null() { 0xffff_ffff } else { unsafe { *pheap } };
        let (dim, w) = if pdesc.is_null() { (0xffff_ffff, 0) } else {
            (u32::from_le_bytes(desc_copy[..4].try_into().unwrap()),
             u64::from_le_bytes(desc_copy[16..24].try_into().unwrap()))
        };
        let orig_riid = if riid.is_null() { String::from("null") } else { guid_str(unsafe { &*riid }) };
        let out = if ppv.is_null() { core::ptr::null_mut() } else { unsafe { *ppv } };
        record("d3d12_xs", "CreateCommittedResource",
            &format!("heapType={htype} dim={dim} width={w} initState={init_state:#x} riid={orig_riid} -> hr={hr:#010x} res={out:p}"));
    }
    hr
}

/// Device slot 67. The Scarlett upscaler retries slot 62 when offline ACO PSO
/// deserialization fails. Reporting success here would leave it with a null PSO.
unsafe extern "system" fn deserialize_compute_pso_x(
    _this: *mut DeviceWrapper, _desc: *const c_void, _iid: *const Guid, out: *mut *mut c_void,
) -> i32 {
    if out.is_null() {
        return E_POINTER;
    }
    unsafe { *out = core::ptr::null_mut() };
    record("d3d12_xs", "DeserializeComputePipelineStateX", "offline ACO packets unsupported");
    0x8000_4001u32 as i32 // E_NOTIMPL
}

/// Device slot 76. Both Scarlett upscalers pass the ordinary resource fields through
/// Flags at byte 48, followed by DCC options. The PC driver chooses its physical layout.
unsafe extern "system" fn create_committed_resource_x(
    this: *mut DeviceWrapper, heap: *const u32, heap_flags: u32, desc: *const u32,
    state: u32, clear: *const c_void, iid: *const Guid, out: *mut *mut c_void,
) -> i32 {
    if !out.is_null() {
        unsafe { *out = core::ptr::null_mut() };
    }
    if heap.is_null() || desc.is_null() || iid.is_null() {
        return E_POINTER;
    }
    if unsafe { &*iid } != &IID_ID3D12RESOURCE {
        return E_NOINTERFACE;
    }
    // The public FFX binary enables DCC and force-texture-compatibility (0xc00000).
    // Leave allocation of compression metadata to the host for this committed resource.
    let flags = unsafe { *desc.add(12) };
    let dcc_options = unsafe { *desc.add(13) };
    let extension = unsafe { desc.add(14).cast::<u64>().read_unaligned() };
    if flags & !(0x00c0_0000 | 0xff) != 0 || !matches!(dcc_options, 0 | 1 | 3) || extension != 0 {
        record("d3d12_xs", "CreateCommittedResourceX",
               &format!("unsupported description: flags={flags:#x} dcc={dcc_options:#x} extension={extension:#x}"));
        return 0x8000_4001u32 as i32;
    }
    unsafe { create_committed_resource(this, heap, heap_flags, desc, state, clear, iid, out) }
}

// Forwarding thunk: this(rcx) -> this->real; jmp real->vtbl[slot].
macro_rules! fwd {
    ($name:ident, $slot:expr) => {
        #[unsafe(naked)]
        unsafe extern "system" fn $name() {
            core::arch::naked_asm!(
                "mov rcx, [rcx+8]",
                "mov rax, [rcx]",
                "jmp qword ptr [rax + {o}]",
                o = const ($slot * 8usize),
            );
        }
    };
}

// sret forwarding thunk: for methods returning a struct >8 bytes BY VALUE, the
// x64 ABI puts the hidden return pointer in rcx and `this` in rdx. So we must
// redirect rdx (this -> this->real), leaving rcx (the return buffer) untouched.
// ID3D12Device::GetResourceAllocationInfo (25, returns 16B) and
// GetCustomHeapProperties (26, returns 20B) are the two such device methods.
macro_rules! fwd_sret {
    ($name:ident, $slot:expr) => {
        #[unsafe(naked)]
        unsafe extern "system" fn $name() {
            core::arch::naked_asm!(
                "mov rdx, [rdx+8]",
                "mov rax, [rdx]",
                "jmp qword ptr [rax + {o}]",
                o = const ($slot * 8usize),
            );
        }
    };
}

// ID3D12Device methods 3..=43 (0,1,2 are IUnknown, handled above).
fwd!(f3, 3); fwd!(f4, 4); fwd!(f5, 5); fwd!(f6, 6); fwd!(f7, 7); fwd!(f8, 8);
fwd!(f9, 9); fwd!(f10, 10); fwd!(f11, 11); fwd!(f12, 12);
/* 13 = check_feature_support, 14 = create_descriptor_heap */
fwd!(f15, 15); fwd!(f16, 16); fwd!(f17, 17); fwd!(f18, 18); fwd!(f19, 19); fwd!(f20, 20);
fwd!(f21, 21); fwd!(f22, 22); fwd!(f23, 23); fwd!(f24, 24);
fwd_sret!(f25, 25); fwd_sret!(f26, 26); // return structs by value: `this` is in rdx
fwd!(f27, 27); fwd!(f28, 28); fwd!(f29, 29); fwd!(f30, 30); fwd!(f31, 31); fwd!(f32, 32);
fwd!(f33, 33); fwd!(f34, 34); fwd!(f35, 35); fwd!(f36, 36); fwd!(f37, 37); fwd!(f38, 38);
fwd!(f39, 39); fwd!(f40, 40); fwd!(f41, 41); fwd!(f42, 42); fwd!(f43, 43);
// Slots 44+ are Xbox-specific device extensions (NOT PC ID3D12Device1-8), so they
// are NOT forwarded by slot number. Unrecognized ones get this fail-loud stub
// (returns E_NOTIMPL) so the game fails that call cleanly and names it in its log,
// instead of reading past the vtable and crashing. Recognized Xbox methods are
// intercepted individually (e.g. slot 48 = CreateCommandQueueX).
unsafe extern "system" fn dev_stub() -> i32 {
    0x8000_4001u32 as i32 // E_NOTIMPL
}

/// Xbox graphics-PSO create (device slot 60).
///
/// The desc is layout-compatible with PC `D3D12_GRAPHICS_PIPELINE_STATE_DESC`, but the
/// shaders are Xbox-native RDNA2 (XDAT/FSSM, DXIL stripped). Translate VS/PS to PC
/// bytecode (see `shader_xlat`), patch a copy of the desc, and create a real PC PSO.
/// If any stage cannot be translated the PSO honestly fails (no placeholder shaders).
/// Signature (from the RHI call site at EXE+0xd5564c): `(this, pDesc, numExt, pExtDesc, riid, ppv)`
/// — the Xbox variant carries an extra extension-struct array, which has no PC equivalent
/// and is currently ignored (logged once).
unsafe extern "system" fn create_gfx_pso_x(
    this: *mut DeviceWrapper, pdesc: *const u8, _num_ext: u32, _pext: *const u8,
    riid: *const Guid, ppv: *mut *mut c_void,
) -> i32 {
    const E_FAIL: i32 = 0x8000_4005u32 as i32;
    const DESC_SIZE: usize = 0x290;
    static OK: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
    static FAIL: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
    static XFAIL: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
    let real = unsafe { (*this).real };
    if pdesc.is_null() {
        return E_FAIL;
    }
    let mut desc = unsafe { core::slice::from_raw_parts(pdesc, DESC_SIZE).to_vec() };
    let rd = |d: &[u8], o: usize| u64::from_le_bytes(d[o..o + 8].try_into().unwrap());
    // Keep translated bytecode alive until the PSO is created.
    let mut keep: Vec<std::sync::Arc<Vec<u8>>> = Vec::new();
    let mut failure: Option<String> = None;
    let present = |off: usize| rd(&desc, off) != 0 && rd(&desc, off + 8) != 0;
    // Compile the pixel shader first so its bytecode can be compared with the
    // final producer before choosing a producer layout.
    let feeder = if present(0x48) { 0x48 } else if present(0x28) { 0x28 } else { 0x08 };
    let mut pixel_shader = None;
    for (stage, off) in [("PS", 0x18usize), ("VS", 0x08), ("DS", 0x28), ("HS", 0x38), ("GS", 0x48)] {
        let (p, n) = (rd(&desc, off) as *const u8, rd(&desc, off + 8) as usize);
        if p.is_null() || n == 0 {
            continue;
        }
        let result = match pixel_shader.as_ref().filter(|_| off == feeder) {
            Some(pixel) => unsafe { crate::shader_xlat::translate_producer(p, n, pixel) },
            None => unsafe { crate::shader_xlat::translate(p, n) },
        };
        match result {
            Ok(shader) => {
                if off == 0x18 {
                    pixel_shader = Some(shader.clone());
                }
                let code = shader.code;
                desc[off..off + 8].copy_from_slice(&(code.as_ptr() as u64).to_le_bytes());
                desc[off + 8..off + 16].copy_from_slice(&(code.len() as u64).to_le_bytes());
                keep.push(code);
            }
            Err(e) => {
                failure = Some(format!("{stage}: {}", e.lines().next().unwrap_or("")));
                break;
            }
        }
    }
    let hr = if let Some(why) = failure {
        let n = XFAIL.fetch_add(1, Ordering::Relaxed);
        if n < 6 {
            record("d3d12_xs", "CreateGraphicsPSO_X", &format!("untranslatable shader -> E_FAIL ({why})"));
        }
        E_FAIL
    } else {
        static DUMP1: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
        if DUMP1.fetch_add(1, Ordering::Relaxed) < 2 {
            let ptr = rd(&desc, 0x228) as *const u8;
            let n = (u32::from_le_bytes(desc[0x230..0x234].try_into().unwrap()) as usize).min(8);
            let mut s = format!("inputLayout ptr={ptr:p} n={n} rootSig={:#x} riid={:p}", rd(&desc, 0), riid);
            if !ptr.is_null() {
                let raw = unsafe { core::slice::from_raw_parts(ptr, n * 32) };
                for (i, c) in raw.chunks(8).enumerate() {
                    if i % 4 == 0 { s.push_str("\n   "); }
                    s.push_str(&format!(" {:016x}", u64::from_le_bytes(c.try_into().unwrap())));
                }
            }
            record("d3d12_xs", "CreateGraphicsPSO_X", &s);
        }
        // Diagnostic: depth-only pipelines (no colour target) — input layout + original VS.
        {
            static DO: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
            let u32at = |o: usize| u32::from_le_bytes(desc[o..o + 4].try_into().unwrap());
            if u32at(0x240) == 0 && u32at(0x264) != 0 && DO.fetch_add(1, Ordering::Relaxed) < 6 {
                let ptr = rd(&desc, 0x228) as *const u8;
                let n = (u32at(0x230) as usize).min(16);
                let mut s = format!("depth-only PSO: {n} input elements");
                for i in 0..n {
                    let e = unsafe { core::slice::from_raw_parts(ptr.add(i * 32), 32) };
                    let name_ptr = u64::from_le_bytes(e[0..8].try_into().unwrap()) as *const u8;
                    let mut name = String::new();
                    if !name_ptr.is_null() {
                        let mut k = 0;
                        while k < 32 && unsafe { *name_ptr.add(k) } != 0 { name.push(unsafe { *name_ptr.add(k) } as char); k += 1; }
                    }
                    let f = |o: usize| u32::from_le_bytes(e[o..o + 4].try_into().unwrap());
                    s.push_str(&format!("\n    {name}{} fmt={} slot={} off={} class={} step={}", f(8), f(12), f(16), f(20), f(24), f(28)));
                }
                record("d3d12_xs", "PSOFMT", &s);
                let (p, len) = (unsafe { core::ptr::read_unaligned(pdesc.add(0x08) as *const u64) } as *const u8, unsafe { core::ptr::read_unaligned(pdesc.add(0x10) as *const u64) } as usize);
                if !p.is_null() && len > 0 && len < 0x100000 {
                    let dir = std::env::temp_dir().join("tombox_shaders");
                    let _ = std::fs::create_dir_all(&dir);
                    let _ = std::fs::write(dir.join(format!("depthonly_vs_{}.dxbc", DO.load(Ordering::Relaxed))), unsafe { core::slice::from_raw_parts(p, len) });
                }
            }
        }
        // Diagnostic: output-merger formats of the first PSOs plus the (so far ignored)
        // Xbox extension struct, to check whether formats live outside the PC-layout desc.
        static FMTLOG: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
        if FMTLOG.fetch_add(1, Ordering::Relaxed) < 60 {
            let u32at = |o: usize| u32::from_le_bytes(desc[o..o + 4].try_into().unwrap());
            let ext = if _pext.is_null() || _num_ext == 0 { String::from("none") } else {
                unsafe { core::slice::from_raw_parts(_pext, 0x40) }.chunks(4).map(|c| format!("{:x}", u32::from_le_bytes(c.try_into().unwrap()))).collect::<Vec<_>>().join(" ")
            };
            record("d3d12_xs", "PSOFMT", &format!(
                "base numRT={} rtv=[{} {} {} {}] dsv={} depthEnable={} depthWrite={} samples={} numExt={} ext[{}]",
                u32at(0x240), u32at(0x244), u32at(0x248), u32at(0x24c), u32at(0x250), u32at(0x264), u32at(0x1f0), u32at(0x1f4), u32at(0x268), _num_ext, ext));
        }
        let f: unsafe extern "system" fn(*mut c_void, *const u8, *const Guid, *mut *mut c_void) -> i32 =
            unsafe { core::mem::transmute(*real_vtbl(real).add(10)) };
        let hr = unsafe { f(real, desc.as_ptr(), riid, ppv) };
        if hr == 0 {
            let n = OK.fetch_add(1, Ordering::Relaxed) + 1;
            if n <= 5 {
                record("d3d12_xs", "CreateGraphicsPSO_X", &format!("PSO #{n} created from translated shaders"));
            }
        } else {
            let n = FAIL.fetch_add(1, Ordering::Relaxed);
            if n < 10 {
                let u32at = |o: usize| u32::from_le_bytes(desc[o..o + 4].try_into().unwrap());
                record("d3d12_xs", "CreateGraphicsPSO_X", &format!(
                    "real CreateGraphicsPipelineState FAILED hr={hr:#010x} (translated shaders ok) topo={} numRT={} rtv0={} dsv={} samples={} inputElems={}",
                    u32at(0x23c), u32at(0x240), u32at(0x244), u32at(0x264), u32at(0x268), u32at(0x230)));
                // Keep the title's original stage bytecode so the failure can be reproduced
                // offline (`xscan`), e.g. an inter-stage signature mismatch.
                let dir = std::env::temp_dir().join("tombox_shaders");
                let _ = std::fs::create_dir_all(&dir);
                let mut saved = Vec::new();
                for (stage, off) in [("vs", 0x08usize), ("ps", 0x18), ("ds", 0x28), ("hs", 0x38), ("gs", 0x48)] {
                    let p = unsafe { core::ptr::read_unaligned(pdesc.add(off) as *const u64) } as *const u8;
                    let len = unsafe { core::ptr::read_unaligned(pdesc.add(off + 8) as *const u64) } as usize;
                    if p.is_null() || len == 0 || len >= 0x100_0000 {
                        continue;
                    }
                    let path = dir.join(format!("psofail_{n}_{stage}.dxbc"));
                    if std::fs::write(&path, unsafe { core::slice::from_raw_parts(p, len) }).is_ok() {
                        saved.push(path.display().to_string());
                    }
                }
                record("d3d12_xs", "CreateGraphicsPSO_X", &format!("failed PSO #{n} stages saved: {}", saved.join(", ")));
            }
        }
        if hr == 0 && !ppv.is_null() {
            unsafe { register_pso(*ppv, desc.clone(), keep.clone()) };
        }
        hr
    };
    drop(keep);
    static LOGN: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
    if LOGN.fetch_add(1, Ordering::Relaxed) % 32 == 0 {
        let (sok, sfail) = crate::shader_xlat::counts();
        record("d3d12_xs", "CreateGraphicsPSO_X", &format!(
            "counts: pso_ok={} pso_fail={} untranslatable={} | shaders ok={sok} fail={sfail}",
            OK.load(Ordering::Relaxed), FAIL.load(Ordering::Relaxed), XFAIL.load(Ordering::Relaxed)));
    }
    hr
}

/// A created PSO's PC desc plus everything it points at, so slot 55 can derive variants.
struct PsoRecord {
    desc: Vec<u8>,
    _shaders: Vec<std::sync::Arc<Vec<u8>>>,
    _layout: Vec<u8>,
}
// SAFETY: the raw pointers inside `desc` only reference `_shaders`/`_layout` (owned here)
// and static title data (root signature object, semantic-name strings).
unsafe impl Send for PsoRecord {}
unsafe impl Sync for PsoRecord {}

fn pso_registry() -> &'static std::sync::Mutex<std::collections::HashMap<usize, std::sync::Arc<PsoRecord>>> {
    static R: OnceLock<std::sync::Mutex<std::collections::HashMap<usize, std::sync::Arc<PsoRecord>>>> = OnceLock::new();
    R.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Snapshot a (translated) desc so it stays valid after the title frees its copy:
/// deep-copies the input-element array and repoints the desc at the copy.
unsafe fn register_pso(pso: *mut c_void, mut desc: Vec<u8>, shaders: Vec<std::sync::Arc<Vec<u8>>>) {
    let ptr = u64::from_le_bytes(desc[0x228..0x230].try_into().unwrap()) as *const u8;
    let n = u32::from_le_bytes(desc[0x230..0x234].try_into().unwrap()) as usize;
    let layout = if ptr.is_null() || n == 0 || n > 64 {
        Vec::new()
    } else {
        unsafe { core::slice::from_raw_parts(ptr, n * 32).to_vec() }
    };
    if !layout.is_empty() {
        desc[0x228..0x230].copy_from_slice(&(layout.as_ptr() as u64).to_le_bytes());
    }
    let rec = std::sync::Arc::new(PsoRecord { desc, _shaders: shaders, _layout: layout });
    pso_registry().lock().unwrap().insert(pso as usize, rec);
}

/// For the diagnostic draw trace: (translated VS pointer, translated PS pointer, state text).
pub fn pso_summary(pso: usize) -> Option<(usize, usize, String)> {
    let rec = pso_registry().lock().unwrap().get(&pso).cloned()?;
    let d = &rec.desc;
    let q = |o: usize| u64::from_le_bytes(d[o..o + 8].try_into().unwrap()) as usize;
    let u = |o: usize| u32::from_le_bytes(d[o..o + 4].try_into().unwrap());
    // D3D12_BLEND_DESC at 0x78: AlphaToCoverage, IndependentBlend, then RT0 {BlendEnable, LogicOpEnable, Src, Dest, Op, SrcA, DestA, OpA, LogicOp, WriteMask}
    let state = format!("depth(en={} write={} func={}) blend0(en={} src={} dst={} op={} srcA={} dstA={} mask={:#x}) nRT={}",
        u(0x1f0), u(0x1f4), u(0x1f8), u(0x80), u(0x88), u(0x8c), u(0x90), u(0x94), u(0x98), d[0xa4], u(0x240));
    Some((q(0x08), q(0x18), state))
}

/// CreateRenderTargetView (20) / CreateDepthStencilView (21): forwarded unchanged; the
/// (descriptor handle -> resource) pair is remembered for the diagnostic draw trace.
// Binding-trace hooks (diagnostic capture only). They are patched into the REAL device's
// method table rather than installed on the wrapper, because the title also reaches the
// real device directly (ID3D12DeviceChild::GetDevice on a resource or command list returns
// it) and views created that way would otherwise go unrecorded.
static REAL_ORIG: [core::sync::atomic::AtomicUsize; 32] = [const { core::sync::atomic::AtomicUsize::new(0) }; 32];
fn real_orig(slot: usize) -> usize {
    REAL_ORIG[slot].load(Ordering::Relaxed)
}
unsafe extern "system" fn real_create_cbv(real: *mut c_void, pdesc: *const u8, dest: usize) {
    if !pdesc.is_null() {
        let va = unsafe { core::ptr::read_unaligned(pdesc as *const u64) };
        let size = unsafe { core::ptr::read_unaligned(pdesc.add(8) as *const u32) };
        crate::bindtrace::note_desc(dest, crate::bindtrace::Desc::Cbv { va, size });
    }
    let f: unsafe extern "system" fn(*mut c_void, *const u8, usize) = unsafe { core::mem::transmute(real_orig(17)) };
    unsafe { f(real, pdesc, dest) }
}
unsafe extern "system" fn real_create_srv(real: *mut c_void, res: *mut c_void, pdesc: *const u8, dest: usize) {
    let desc = if pdesc.is_null() { None } else { Some(unsafe { core::ptr::read_unaligned(pdesc as *const [u8; 40]) }) };
    crate::bindtrace::note_desc(dest, crate::bindtrace::Desc::Srv { res: res as usize, desc });
    let f: unsafe extern "system" fn(*mut c_void, *mut c_void, *const u8, usize) = unsafe { core::mem::transmute(real_orig(18)) };
    unsafe { f(real, res, pdesc, dest) }
}
unsafe extern "system" fn real_create_uav(real: *mut c_void, res: *mut c_void, counter: *mut c_void, pdesc: *const u8, dest: usize) {
    let desc = if pdesc.is_null() { None } else { Some(unsafe { core::ptr::read_unaligned(pdesc as *const [u8; 40]) }) };
    crate::bindtrace::note_desc(dest, crate::bindtrace::Desc::Uav { res: res as usize, desc });
    let f: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void, *const u8, usize) = unsafe { core::mem::transmute(real_orig(19)) };
    unsafe { f(real, res, counter, pdesc, dest) }
}
unsafe extern "system" fn real_create_sampler(real: *mut c_void, pdesc: *const u8, dest: usize) {
    if !pdesc.is_null() {
        let desc = unsafe { core::ptr::read_unaligned(pdesc as *const [u8; 0x34]) };
        crate::bindtrace::note_desc(dest, crate::bindtrace::Desc::Sampler { desc });
    }
    let f: unsafe extern "system" fn(*mut c_void, *const u8, usize) = unsafe { core::mem::transmute(real_orig(22)) };
    unsafe { f(real, pdesc, dest) }
}
/// CopyDescriptors (slot 23): a null size array means every range has one descriptor.
unsafe extern "system" fn real_copy_descriptors(
    real: *mut c_void, nd: u32, dstarts: *const usize, dsizes: *const u32,
    ns: u32, sstarts: *const usize, ssizes: *const u32, heap_type: u32,
) {
    if !dstarts.is_null() && !sstarts.is_null() {
        // Walk both range lists in step, as the API does.
        let size = |p: *const u32, i: u32| if p.is_null() { 1 } else { unsafe { *p.add(i as usize) } };
        let inc = crate::bindtrace::increment(heap_type);
        let (mut di, mut doff, mut si, mut soff) = (0u32, 0u32, 0u32, 0u32);
        while di < nd && si < ns {
            let (dn, sn) = (size(dsizes, di), size(ssizes, si));
            let n = (dn - doff).min(sn - soff);
            if n == 0 {
                break;
            }
            unsafe {
                crate::bindtrace::copy_desc(*dstarts.add(di as usize) + doff as usize * inc, *sstarts.add(si as usize) + soff as usize * inc, n, heap_type);
            }
            doff += n;
            soff += n;
            if doff == dn { di += 1; doff = 0; }
            if soff == sn { si += 1; soff = 0; }
        }
    }
    let f: unsafe extern "system" fn(*mut c_void, u32, *const usize, *const u32, u32, *const usize, *const u32, u32) =
        unsafe { core::mem::transmute(real_orig(23)) };
    unsafe { f(real, nd, dstarts, dsizes, ns, sstarts, ssizes, heap_type) }
}
unsafe extern "system" fn real_copy_descriptors_simple(real: *mut c_void, n: u32, dest: usize, src: usize, heap_type: u32) {
    crate::bindtrace::copy_desc(dest, src, n, heap_type);
    let f: unsafe extern "system" fn(*mut c_void, u32, usize, usize, u32) = unsafe { core::mem::transmute(real_orig(24)) };
    unsafe { f(real, n, dest, src, heap_type) }
}
/// Patch the real device's method table (shared by every interface version of the device).
unsafe fn patch_real_device(real: *mut c_void) {
    extern "system" {
        fn VirtualProtect(addr: *mut c_void, size: usize, new_protect: u32, old_protect: *mut u32) -> i32;
    }
    static DONE: std::sync::Once = std::sync::Once::new();
    DONE.call_once(|| unsafe {
        let vt = *(real as *const *mut usize);
        let hooks: [(usize, usize); 6] = [
            (17, real_create_cbv as usize), (18, real_create_srv as usize), (19, real_create_uav as usize),
            (22, real_create_sampler as usize), (23, real_copy_descriptors as usize), (24, real_copy_descriptors_simple as usize),
        ];
        let mut old = 0u32;
        if VirtualProtect(vt.add(17) as *mut c_void, 8 * 8, 0x04, &mut old) == 0 {
            record("d3d12_xs", "bindtrace", "could not unprotect the device method table - descriptor trace disabled");
            return;
        }
        for (slot, f) in hooks {
            REAL_ORIG[slot].store(*vt.add(slot), Ordering::Relaxed);
            *vt.add(slot) = f;
        }
        let mut tmp = 0u32;
        VirtualProtect(vt.add(17) as *mut c_void, 8 * 8, old, &mut tmp);
        record("d3d12_xs", "bindtrace", "descriptor hooks installed on the real device method table");
    });
}

unsafe extern "system" fn create_rtv_traced(this: *mut DeviceWrapper, res: *mut c_void, pdesc: *const u8, dest: usize) {
    let real = unsafe { (*this).real };
    let inc: unsafe extern "system" fn(*mut c_void, u32) -> u32 = unsafe { core::mem::transmute(*real_vtbl(real).add(15)) };
    crate::rtcap::note_view(dest, res as usize, unsafe { inc(real, 2) } as usize);
    let f: unsafe extern "system" fn(*mut c_void, *mut c_void, *const u8, usize) = unsafe { core::mem::transmute(*real_vtbl(real).add(20)) };
    unsafe { f(real, res, pdesc, dest) }
}
unsafe extern "system" fn create_dsv_traced(this: *mut DeviceWrapper, res: *mut c_void, pdesc: *const u8, dest: usize) {
    let real = unsafe { (*this).real };
    crate::rtcap::note_view(dest, res as usize, 0);
    let f: unsafe extern "system" fn(*mut c_void, *mut c_void, *const u8, usize) = unsafe { core::mem::transmute(*real_vtbl(real).add(21)) };
    unsafe { f(real, res, pdesc, dest) }
}

/// Xbox "derived pipeline" create (device slot 55), from the RHI call site at EXE+0xd56979:
/// `(this, pBasePSO, numPatches, pPatches, riid, ppv)`. Each patch is a 0x14C-byte tagged
/// struct (u32 tag, payload at +4) overriding one block of the base pipeline's state:
///   0 = D3D12_DEPTH_STENCIL_DESC (0x34)   1 = D3D12_RASTERIZER_DESC (0x2C)
///   2 = D3D12_BLEND_DESC (0x148)          3 = SampleMask (u32)
///   5 = Xbox misc (stencil masks / flags) — no PC desc field; ignored.
/// Tags/payload sizes are read off the code that fills them (FD3D12 CreatePipelineState).
unsafe extern "system" fn create_derived_pso_x(
    this: *mut DeviceWrapper, base: *mut c_void, num: u32, patches: *const u8,
    riid: *const Guid, ppv: *mut *mut c_void,
) -> i32 {
    const E_FAIL: i32 = 0x8000_4005u32 as i32;
    const STRIDE: usize = 0x14C;
    static OK: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
    static FAIL: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
    let real = unsafe { (*this).real };
    let rec = pso_registry().lock().unwrap().get(&(base as usize)).cloned();
    let Some(rec) = rec else {
        if FAIL.fetch_add(1, Ordering::Relaxed) < 6 {
            record("d3d12_xs", "CreateDerivedPSO_X", &format!("base PSO {base:p} unknown (its creation failed?) -> E_FAIL"));
        }
        return E_FAIL;
    };
    let mut desc = rec.desc.clone();
    let mut tags = String::new();
    if !patches.is_null() {
        for i in 0..(num as usize).min(16) {
            let p = unsafe { patches.add(i * STRIDE) };
            let tag = unsafe { core::ptr::read_unaligned(p as *const u32) };
            let payload = |n: usize| unsafe { core::slice::from_raw_parts(p.add(4), n) };
            tags.push_str(&format!("{tag} "));
            match tag {
                0 => {
                    static T0: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
                    if u32::from_le_bytes(desc[0x264..0x268].try_into().unwrap()) != 0 && T0.fetch_add(1, Ordering::Relaxed) < 400 {
                        let hex = payload(0x34).chunks(4).map(|c| format!("{:x}", u32::from_le_bytes(c.try_into().unwrap()))).collect::<Vec<_>>().join(" ");
                        let u32at = |o: usize| u32::from_le_bytes(desc[o..o + 4].try_into().unwrap());
                        record("d3d12_xs", "PSOFMT", &format!("tag0 numRT={} dsv={} payload[{hex}]", u32at(0x240), u32at(0x264)));
                    }
                    desc[0x1f0..0x1f0 + 0x34].copy_from_slice(payload(0x34))
                }
                1 => desc[0x1c4..0x1c4 + 0x2c].copy_from_slice(payload(0x2c)),
                2 => desc[0x78..0x78 + 0x148].copy_from_slice(payload(0x148)),
                3 => desc[0x1c0..0x1c4].copy_from_slice(payload(4)),
                5 => {
                    static T5: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
                    if T5.fetch_add(1, Ordering::Relaxed) < 40 {
                        let hex = payload(0x30).chunks(4).map(|c| format!("{:x}", u32::from_le_bytes(c.try_into().unwrap()))).collect::<Vec<_>>().join(" ");
                        record("d3d12_xs", "PSOFMT", &format!("derived tag5 payload[{hex}]"));
                    }
                }
                other => {
                    if FAIL.fetch_add(1, Ordering::Relaxed) < 6 {
                        record("d3d12_xs", "CreateDerivedPSO_X", &format!("unknown patch tag {other} -> E_FAIL"));
                    }
                    return E_FAIL;
                }
            }
        }
    }
    {
        static DL: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
        if DL.fetch_add(1, Ordering::Relaxed) < 60 {
            let u32at = |o: usize| u32::from_le_bytes(desc[o..o + 4].try_into().unwrap());
            record("d3d12_xs", "PSOFMT", &format!(
                "derived tags=[{}] numRT={} rtv0={} dsv={} depthEnable={} depthWrite={} depthFunc={} stencil={}",
                tags.trim(), u32at(0x240), u32at(0x244), u32at(0x264), u32at(0x1f0), u32at(0x1f4), u32at(0x1f8), u32at(0x1fc)));
        }
    }
    let f: unsafe extern "system" fn(*mut c_void, *const u8, *const Guid, *mut *mut c_void) -> i32 =
        unsafe { core::mem::transmute(*real_vtbl(real).add(10)) };
    let hr = unsafe { f(real, desc.as_ptr(), riid, ppv) };
    if hr == 0 && !ppv.is_null() {
        let n = OK.fetch_add(1, Ordering::Relaxed) + 1;
        if n <= 4 || n % 100 == 0 {
            record("d3d12_xs", "CreateDerivedPSO_X", &format!("derived PSO #{n} created (patch tags: {tags})"));
        }
        unsafe { register_pso(*ppv, desc, rec._shaders.clone()) };
    } else if FAIL.fetch_add(1, Ordering::Relaxed) < 10 {
        record("d3d12_xs", "CreateDerivedPSO_X", &format!("real CreateGraphicsPipelineState FAILED hr={hr:#010x} (tags: {tags})"));
    }
    hr
}

/// Xbox compute-PSO create (device slot 62), call site EXE+0xd54c30:
/// `(this, pDesc, numExt, pExtDesc, riid, ppv)` with a PC-layout
/// D3D12_COMPUTE_PIPELINE_STATE_DESC { pRootSignature@0, CS{ptr@8,len@0x10}, NodeMask@0x18,
/// CachedPSO@0x20, Flags@0x30 }. The CS is translated like graphics stages.
unsafe extern "system" fn create_compute_pso_x(
    this: *mut DeviceWrapper, pdesc: *const u8, _num_ext: u32, _pext: *const u8,
    riid: *const Guid, ppv: *mut *mut c_void,
) -> i32 {
    const E_FAIL: i32 = 0x8000_4005u32 as i32;
    static OK: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
    static FAIL: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
    let real = unsafe { (*this).real };
    if pdesc.is_null() {
        return E_FAIL;
    }
    let mut desc = unsafe { core::slice::from_raw_parts(pdesc, 0x38).to_vec() };
    let p = u64::from_le_bytes(desc[8..16].try_into().unwrap()) as *const u8;
    let n = u64::from_le_bytes(desc[16..24].try_into().unwrap()) as usize;
    let shader = match unsafe { crate::shader_xlat::translate(p, n) } {
        Ok(c) => c,
        Err(e) => {
            if FAIL.fetch_add(1, Ordering::Relaxed) < 6 {
                record("d3d12_xs", "CreateComputePSO_X",
                    &format!("untranslatable compute shader -> E_FAIL ({})", e.lines().next().unwrap_or("")));
            }
            return E_FAIL;
        }
    };
    let code = shader.code;
    desc[8..16].copy_from_slice(&(code.as_ptr() as u64).to_le_bytes());
    desc[16..24].copy_from_slice(&(code.len() as u64).to_le_bytes());
    let f: unsafe extern "system" fn(*mut c_void, *const u8, *const Guid, *mut *mut c_void) -> i32 =
        unsafe { core::mem::transmute(*real_vtbl(real).add(11)) }; // PC CreateComputePipelineState
    let hr = unsafe { f(real, desc.as_ptr(), riid, ppv) };
    if hr == S_OK && !ppv.is_null() && !(*ppv).is_null() {
        let pso = *ppv;
        if let Some(group) = shader.portable_compute_group {
            crate::cmdlist_wrap::tag_compute_pso(pso, group);
        }
        if crate::rtcap::enabled() {
            crate::rtcap::note_compute_pso(pso as usize, code.as_ptr() as usize);
        }
    }
    if hr == 0 {
        let k = OK.fetch_add(1, Ordering::Relaxed) + 1;
        if k <= 4 || k % 100 == 0 {
            record("d3d12_xs", "CreateComputePSO_X", &format!("compute PSO #{k} created from translated shader"));
        }
    } else if FAIL.fetch_add(1, Ordering::Relaxed) < 10 {
        record("d3d12_xs", "CreateComputePSO_X", &format!("real CreateComputePipelineState FAILED hr={hr:#010x}"));
    }
    hr
}

/// ID3D12Device::CreateHeap (slot 28). Logs the desc and result; Xbox-only heap flag bits
/// are masked off (PC accepts only the low D3D12_HEAP_FLAGS bits).
unsafe extern "system" fn create_heap(
    this: *mut DeviceWrapper, pdesc: *const u8, riid: *const Guid, ppv: *mut *mut c_void,
) -> i32 {
    let real = unsafe { (*this).real };
    let f: unsafe extern "system" fn(*mut c_void, *const u8, *const Guid, *mut *mut c_void) -> i32 =
        unsafe { core::mem::transmute(*real_vtbl(real).add(28)) };
    if pdesc.is_null() {
        return unsafe { f(real, pdesc, riid, ppv) };
    }
    let mut d = unsafe { core::slice::from_raw_parts(pdesc, 0x30).to_vec() };
    let u32at = |d: &[u8], o: usize| u32::from_le_bytes(d[o..o + 4].try_into().unwrap());
    let u64at = |d: &[u8], o: usize| u64::from_le_bytes(d[o..o + 8].try_into().unwrap());
    let (size, htype, align, flags) = (u64at(&d, 0), u32at(&d, 8), u64at(&d, 0x20), u32at(&d, 0x28));
    let mut hr = unsafe { f(real, d.as_ptr(), riid, ppv) };
    let mut note = String::new();
    if hr < 0 {
        // Retry with PC-valid values: flags limited to the documented PC bits, and a
        // PC-legal alignment (0 = default 64 KiB).
        // PC-defined D3D12_HEAP_FLAGS bits: 0x1 0x4 0x8 0x20 0x40 0x80 0x100 0x200 0x400 0x800 0x1000.
        // (The title passes 0x2, which is not a PC flag.)
        let pc_flags = flags & 0x1fed;
        d[0x28..0x2c].copy_from_slice(&pc_flags.to_le_bytes());
        if align != 0 && align != 0x10000 && align != 0x400000 {
            d[0x20..0x28].copy_from_slice(&0u64.to_le_bytes());
        }
        let hr2 = unsafe { f(real, d.as_ptr(), riid, ppv) };
        note = format!(" | retry flags={pc_flags:#x} -> hr={hr2:#010x}");
        hr = hr2;
    }
    if hr >= 0 && crate::rtcap::enabled() && !ppv.is_null() {
        crate::bindtrace::note_heap_type(unsafe { *ppv } as usize, htype);
    }
    static N: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
    if N.fetch_add(1, Ordering::Relaxed) < 8 || hr < 0 {
        record("d3d12_xs", "CreateHeap", &format!(
            "size={size:#x} type={htype} cpuPage={} pool={} align={align:#x} flags={flags:#x} -> hr={hr:#010x}{note}",
            u32at(&d, 0xc), u32at(&d, 0x10)));
    }
    hr
}

/// Describe a D3D12_RESOURCE_DESC for diagnostics.
fn desc_str(d: &[u8]) -> String {
    let u32at = |o: usize| u32::from_le_bytes(d[o..o + 4].try_into().unwrap());
    let u16at = |o: usize| u16::from_le_bytes(d[o..o + 2].try_into().unwrap());
    let u64at = |o: usize| u64::from_le_bytes(d[o..o + 8].try_into().unwrap());
    format!("dim={} align={:#x} {}x{}x{} mips={} fmt={} samples={} layout={} flags={:#x}",
        u32at(0), u64at(8), u64at(0x10), u32at(0x18), u16at(0x1c), u16at(0x1e), u32at(0x20), u32at(0x24), u32at(0x2c), u32at(0x30))
}

/// ID3D12Device::CreatePlacedResource (slot 29): (pHeap, HeapOffset, pDesc, InitialState,
/// pClearValue, riid, ppv). Same Xbox→PC desc fixups as committed resources.
unsafe extern "system" fn create_placed_resource(
    this: *mut DeviceWrapper, heap: *mut c_void, offset: u64, pdesc: *const u8,
    init_state: u32, pclear: *const c_void, riid: *const Guid, ppv: *mut *mut c_void,
) -> i32 {
    let real = unsafe { (*this).real };
    let f: unsafe extern "system" fn(*mut c_void, *mut c_void, u64, *const u8, u32, *const c_void, *const Guid, *mut *mut c_void) -> i32 =
        unsafe { core::mem::transmute(*real_vtbl(real).add(29)) };
    if pdesc.is_null() {
        return unsafe { f(real, heap, offset, pdesc, init_state, pclear, riid, ppv) };
    }
    let mut d = unsafe { core::slice::from_raw_parts(pdesc, 0x38).to_vec() };
    let flags = u32::from_le_bytes(d[0x30..0x34].try_into().unwrap());
    d[0x30..0x34].copy_from_slice(&(flags & 0xFF).to_le_bytes());
    let hr = unsafe { f(real, heap, offset, d.as_ptr(), init_state, pclear, &IID_ID3D12RESOURCE, ppv) };
    if hr >= 0 && crate::rtcap::enabled() && d[0] == 1 && !ppv.is_null() {
        let width = u64::from_le_bytes(d[0x10..0x18].try_into().unwrap());
        unsafe { crate::bindtrace::note_buffer(*ppv, width, crate::bindtrace::heap_type_of(heap as usize)) };
    }
    static N: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
    if hr < 0 || N.fetch_add(1, Ordering::Relaxed) < 4 {
        let orig = unsafe { core::slice::from_raw_parts(pdesc, 0x38) };
        record("d3d12_xs", "CreatePlacedResource", &format!(
            "heap={heap:p} off={offset:#x} state={init_state:#x} {} -> hr={hr:#010x}", desc_str(orig)));
    }
    hr
}

/// Xbox slot 44 `CreatePlacedResourceX` (see placed.rs for evidence and model).
unsafe extern "system" fn create_placed_resource_x(
    this: *mut DeviceWrapper, va: u64, pdesc: *const u8, init_state: u32,
    pclear: *const c_void, _riid: *const Guid, ppv: *mut *mut c_void,
) -> i32 {
    let real = unsafe { (*this).real };
    if pdesc.is_null() || ppv.is_null() {
        record("d3d12_xs", "CreatePlacedResourceX", "null desc/out -> E_INVALIDARG");
        return 0x80070057u32 as i32;
    }
    let orig: [u8; 0x38] = unsafe { core::slice::from_raw_parts(pdesc, 0x38) }.try_into().unwrap();
    let mut d = orig;
    let dim = u32::from_le_bytes(d[0..4].try_into().unwrap());
    let flags = u32::from_le_bytes(d[0x30..0x34].try_into().unwrap());
    d[8..16].copy_from_slice(&0u64.to_le_bytes()); // Alignment: let the host choose
    d[0x2c..0x30].copy_from_slice(&(if dim == 1 { 1u32 } else { 0u32 }).to_le_bytes()); // Layout: ROW_MAJOR for buffers, UNKNOWN for textures (Xbox tile modes have no PC meaning)
    d[0x30..0x34].copy_from_slice(&(flags & 0xFF).to_le_bytes());
    let heap: [u32; 5] = [1 /* DEFAULT */, 0, 0, 1, 1];
    let f: unsafe extern "system" fn(*mut c_void, *const u32, u32, *const u8, u32, *const c_void, *const Guid, *mut *mut c_void) -> i32 =
        unsafe { core::mem::transmute(*real_vtbl(real).add(27)) };
    let hr = unsafe { f(real, heap.as_ptr(), 0, d.as_ptr(), init_state, pclear, &IID_ID3D12RESOURCE, ppv) };
    static N: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    if hr < 0 || n < 12 {
        record("d3d12_xs", "CreatePlacedResourceX", &format!(
            "#{n} va={va:#x} state={init_state:#x} clear={pclear:p} {} -> hr={hr:#010x} res={:p}",
            desc_str(&orig), unsafe { *ppv }));
    }
    if hr >= 0 {
        // One reference for the placement registry (released by upload.rs).
        let res = unsafe { *ppv };
        let addref: unsafe extern "system" fn(*mut c_void) -> u32 = unsafe { core::mem::transmute(*real_vtbl(res).add(1)) };
        unsafe { addref(res) };
        crate::placed::register(res, crate::placed::Placed { va, desc: orig, init_state });
    }
    hr
}

/// Xbox slot 61: extended sampler create, `(this, const XboxSamplerDesc*, D3D12_CPU_DESCRIPTOR_HANDLE)`.
///
/// Evidence: `FD3D12Device::CreateSamplerInternal` (EXE+0xd2c908). For every filter except
/// `D3D12_FILTER_ANISOTROPIC` (0x55) it calls the standard `CreateSampler` (slot 22). For
/// anisotropic it zero-fills a 0x6C-byte block and copies the standard desc into it:
///   +0x18/+0x1C/+0x20 AddressU/V/W, +0x24 MinLOD, +0x28 MaxLOD, +0x2C MipLODBias,
///   +0x34 MaxAnisotropy, +0x40 ComparisonFunc
/// plus constants at +0x04/+0x08 (3), +0x0C (2) and two console-variable clamps at +0x3C
/// (<= 7) and +0x54 (<= 15) whose meaning is unverified (they look like per-axis anisotropy
/// limits). The border colour is left zero.
///
/// Implemented as the standard anisotropic sampler those fields were copied from; the
/// unverified extra fields are ignored (so any Xbox-specific anisotropy bias is not modelled).
unsafe extern "system" fn create_sampler_x(this: *mut DeviceWrapper, xdesc: *const u8, dest: usize) {
    let real = unsafe { (*this).real };
    if xdesc.is_null() {
        record("d3d12_xs", "CreateSamplerX", "null desc - ignored");
        return;
    }
    let x = unsafe { core::slice::from_raw_parts(xdesc, 0x6c) };
    let u = |o: usize| u32::from_le_bytes(x[o..o + 4].try_into().unwrap());
    let mut d = [0u8; 0x34]; // D3D12_SAMPLER_DESC
    let mut put = |o: usize, v: u32| d[o..o + 4].copy_from_slice(&v.to_le_bytes());
    put(0x00, 0x55); // D3D12_FILTER_ANISOTROPIC
    put(0x04, u(0x18));
    put(0x08, u(0x1c));
    put(0x0c, u(0x20));
    put(0x10, u(0x2c)); // MipLODBias
    put(0x14, u(0x34).clamp(1, 16)); // MaxAnisotropy
    put(0x18, u(0x40)); // ComparisonFunc
    put(0x2c, u(0x24)); // MinLOD
    put(0x30, u(0x28)); // MaxLOD
    let f: unsafe extern "system" fn(*mut c_void, *const u8, usize) = unsafe { core::mem::transmute(*real_vtbl(real).add(22)) };
    unsafe { f(real, d.as_ptr(), dest) };
    static N: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
    if N.fetch_add(1, Ordering::Relaxed) < 4 {
        record("d3d12_xs", "CreateSamplerX", &format!(
            "addr={}/{}/{} maxAniso={} cmp={} lod=[{},{}] bias={} extra(+3c={},+54={}) dest={dest:#x}",
            u(0x18), u(0x1c), u(0x20), u(0x34), u(0x40), f32::from_bits(u(0x24)), f32::from_bits(u(0x28)),
            f32::from_bits(u(0x2c)), u(0x3c), u(0x54)));
    }
}

// Keep the legacy hint result for slots 44..90. Unknown higher slots return
// E_NOTIMPL. Record the calling module and return-address RVA for future ABI work.
unsafe extern "system" fn dev_stub_impl(_this: *mut c_void, _a: u64, caller: usize, slot: usize) -> i32 {
    static PER_SLOT: [core::sync::atomic::AtomicUsize; 160] = [const { core::sync::atomic::AtomicUsize::new(0) }; 160];
    let (hr, result) = if slot > 90 { (0x8000_4001u32 as i32, "E_NOTIMPL") } else { (S_OK, "S_OK") };
    if PER_SLOT[slot].fetch_add(1, Ordering::Relaxed) < 3 {
        let location = crate::gxdk_edition::caller_location(caller);
        record("d3d12_xs", "dev_ext", &format!("slot={slot} caller={location} -> {result}"));
    }
    hr
}
#[unsafe(naked)]
unsafe extern "system" fn dev_diagnostic<const SLOT: usize>() -> i32 {
    core::arch::naked_asm!("mov r8, [rsp]", "mov r9d, {s}", "jmp {i}", s = const SLOT, i = sym dev_stub_impl);
}

macro_rules! install_dev_stubs {
    ($v:ident, $($slot:literal),+ $(,)?) => { $( $v[$slot] = dev_diagnostic::<$slot> as *const () as usize; )+ };
}

fn vtable() -> *const usize {
    static VT: OnceLock<usize> = OnceLock::new();
    *VT.get_or_init(|| {
        let mut v: Vec<usize> = vec![
            qi as usize, addref as usize, release as usize,
            f3 as usize, f4 as usize, f5 as usize, f6 as usize, f7 as usize, f8 as usize,
            f9 as usize, f10 as usize, f11 as usize, f12 as usize,
            check_feature_support as usize,   // 13
            create_descriptor_heap as usize,  // 14
            f15 as usize, f16 as usize, f17 as usize, f18 as usize, f19 as usize, f20 as usize,
            f21 as usize, f22 as usize, f23 as usize, f24 as usize, f25 as usize, f26 as usize,
            f27 as usize, f28 as usize, f29 as usize, f30 as usize, f31 as usize, f32 as usize,
            f33 as usize, f34 as usize, f35 as usize, f36 as usize, f37 as usize, f38 as usize,
            f39 as usize, f40 as usize, f41 as usize, f42 as usize, f43 as usize,
        ];
        // 44..160: Xbox-extension slots — fail-loud stub unless intercepted below.
        while v.len() < 160 {
            v.push(dev_stub as usize);
        }
        install_dev_stubs!(v,
            45,46,47,49,50,51,52,53,54,56,57,58,59,63,64,65,66,68,69,70,71,72,74,75,77,
            82,83,84,85,86,87,88,89,90,
            91,92,93,94,95,96,97,98,99,100,101,102,103,104,105,106,107,108,109,110,
            111,112,113,114,115,116,117,118,119,120,121,122,123,124,125,126,127,128,129,130,
            131,132,133,134,135,136,137,138,139,140,141,142,143,144,145,146,147,148,149,150,
            151,152,153,154,155,156,157,158,159,
        );
        // Frame pacing (see frame.rs).
        v[78] = crate::frame::set_frame_interval_x as usize;
        v[79] = crate::frame::schedule_frame_event_x as usize;
        v[80] = crate::frame::wait_frame_event_x as usize;
        v[81] = crate::frame::frame_statistics_x as usize;
        v[44] = create_placed_resource_x as usize; // CreatePlacedResourceX
        v[61] = create_sampler_x as usize; // Xbox extended (anisotropic) sampler create
        v[60] = create_gfx_pso_x as usize; // Xbox graphics PSO create -> PC CreateGraphicsPipelineState
        v[55] = create_derived_pso_x as usize; // Xbox derived-PSO create (base PSO + state patches)
        v[62] = create_compute_pso_x as usize; // Xbox compute PSO create
        v[28] = create_heap as usize; // CreateHeap (logged; Xbox-only flag bits masked on retry)
        v[29] = create_placed_resource as usize; // CreatePlacedResource (desc flag fixups)
        v[48] = create_command_queue_x as usize; // CreateCommandQueueX
        v[8] = create_command_queue_x as usize; // PC-slot CreateCommandQueue: same desc; needs the Xbox-sized queue vtable too
        v[15] = get_descriptor_increment as usize; // GetDescriptorHandleIncrementSize
        v[25] = get_resource_alloc_info as usize; // GetResourceAllocationInfo (null-safe)
        v[17] = create_cbv as usize; // CreateConstantBufferView (256-align size)
        v[27] = create_committed_resource as usize; // CreateCommittedResource (IID subst)
        v[67] = deserialize_compute_pso_x as usize;
        v[76] = create_committed_resource_x as usize;
        v[73] = create_command_list_x as usize;  // Xbox command-list create
        if crate::rtcap::enabled() {
            v[20] = create_rtv_traced as usize;
            v[21] = create_dsv_traced as usize;
        }
        Box::leak(v.into_boxed_slice()).as_ptr() as usize
    }) as *const usize
}

/// Wrap a real `ID3D12Device` so we can intercept QueryInterface.
pub fn wrap_device(real: *mut c_void) -> *mut c_void {
    if crate::rtcap::enabled() && !real.is_null() {
        unsafe { patch_real_device(real) };
    }
    let w = Box::new(DeviceWrapper { vtable: vtable(), real, refs: AtomicU32::new(1) });
    Box::into_raw(w) as *mut c_void
}
