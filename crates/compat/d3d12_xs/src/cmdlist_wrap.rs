//! Xbox D3D12.x `ID3D12GraphicsCommandList` vtable translation.
//!
//! The Xbox shim forwards slots 0–26 to the desktop slots with the same numbers.
//! Slots 32–55 forward with an offset of four. Later slots depend on the guest
//! interface layout. The shim keeps the real command list and its PC vtable intact.

use core::ffi::c_void;
use core::sync::atomic::{AtomicUsize, Ordering};
#[allow(unused_imports)] use core::sync::atomic::AtomicU64;
use compat_common::record;

extern "system" {
    fn VirtualProtect(addr: *mut c_void, size: usize, new_protect: u32, old_protect: *mut u32) -> i32;
}
const PAGE_READWRITE: u32 = 0x04;

/// Slot 148 is used by the GXDK 250405 BVH builder.
const N: usize = 149;
static XBOX_VT: [AtomicUsize; N] = [const { AtomicUsize::new(0) }; N];
static BUILT: std::sync::Once = std::sync::Once::new();
static mut REAL_VT_PTR: *const usize = core::ptr::null();
static INSERTED_HITS: AtomicUsize = AtomicUsize::new(0);

/// Last method guaranteed by the base PC graphics command-list interface.
const PC_LAST_SLOT: usize = 59;

fn map_slot(i: usize) -> Option<usize> {
    match i {
        0..=26 => Some(i),
        32..=55 => Some(i - 4),
        _ => None,
    }
}

/// Fail-loud-ish stub for an Xbox-inserted slot of unknown signature. Returns 0
/// in rax (valid for both void- and HRESULT(S_OK)-returning methods) so a stray
/// call doesn't corrupt state; logs the first few so we learn if the game needs one.
#[unsafe(naked)]
unsafe extern "system" fn inserted_stub() {
    core::arch::naked_asm!(
        "lea rax, [rip + {counter}]",
        "lock inc qword ptr [rax]",
        "xor eax, eax",
        "ret",
        counter = sym INSERTED_HITS,
    )
}

// Unidentified guest slots can have unrelated desktop methods at the same index.
// Capture the first argument and the guest return address without recording GPU work.
unsafe extern "system" fn probe_impl(_this: *mut c_void, a: usize, caller: usize, slot: usize) -> usize {
    static PER: [AtomicUsize; N] = [const { AtomicUsize::new(0) }; N];
    INSERTED_HITS.fetch_add(1, Ordering::Relaxed);
    if PER[slot].fetch_add(1, Ordering::Relaxed) < 4 {
        let caller = crate::gxdk_edition::caller_location(caller);
        record("d3d12_xs", "cmdlist.hi", &format!("slot={slot} arg1={a:#x} caller={caller} -> 0"));
    }
    // Unknown return ABI. Preserve the zero fallback, including for pointer-valued entries.
    0
}

macro_rules! make_probes {
    ($(($name:ident, $slot:literal)),* $(,)?) => {
        $(
            #[unsafe(naked)]
            unsafe extern "system" fn $name() {
                core::arch::naked_asm!(
                    "mov r8, [rsp]",        // caller return address -> 3rd arg
                    "mov r9d, {s}",         // slot -> 4th arg
                    "jmp {i}",
                    s = const $slot, i = sym probe_impl,
                );
            }
        )*
        /// Install diagnostics first and replace them with the identified PC methods below.
        fn install_probes(xbox: &[AtomicUsize; N], pc: &[usize; PC_LAST_SLOT + 1]) {
            $( xbox[$slot].store($name as usize, Ordering::Relaxed); )*
            for i in 47..=49 { xbox[i].store(pc[i - 4], Ordering::Relaxed); }
            // Verified from FD3D12CommandContext::RHIClearMRTImpl call sites:
            // Xbox 51 = ClearDepthStencilView (PC 47), Xbox 52 = ClearRenderTargetView (PC 48).
            // The Scarlett FidelityFX clear job passes GPU/CPU handles, a resource, four
            // clear values and the rect count/pointer to slots 53 (UINT) and 54 (FLOAT).
            // Its discard job at RVA 0x6dcb calls slot 55 with the resource and null region
            // after flushing barriers, matching PC DiscardResource (51).
            for i in 51..=55 { xbox[i].store(pc[i - 4], Ordering::Relaxed); }
            xbox[67].store(flush_texture_cache_x as usize, Ordering::Relaxed);
            // Xbox 68 = OMSetDepthBounds (PC ID3D12GraphicsCommandList1 slot 62): called from
            // FD3D12StateCacheBase::ApplyState (EXE+0x4e0480) with the two cached depth-bound
            // floats in xmm1/xmm2 when its depth-bounds dirty flag is set.
            xbox[68].store(set_depth_bounds as usize, Ordering::Relaxed);
            xbox[83].store(copy_texture_region_x as usize, Ordering::Relaxed);
            // The two Scarlett FidelityFX binaries use the same copy ABI at slot 85.
            xbox[85].store(copy_texture_region_fidelityfx as usize, Ordering::Relaxed);
            xbox[146].store(route146 as usize, Ordering::Relaxed);
            xbox[59].store(route59 as usize, Ordering::Relaxed);
            xbox[60].store(route60 as usize, Ordering::Relaxed);
            xbox[61].store(route61 as usize, Ordering::Relaxed);
            xbox[2].store(release_list as usize, Ordering::Relaxed);
            xbox[26].store(resource_barrier_traced as usize, Ordering::Relaxed);
            if crate::rtcap::enabled() {
                xbox[50].store(om_set_targets_traced as usize, Ordering::Relaxed);
                xbox[12].store(draw_traced as usize, Ordering::Relaxed);
                xbox[13].store(draw_indexed_traced as usize, Ordering::Relaxed);
                xbox[14].store(dispatch_traced as usize, Ordering::Relaxed);
                xbox[48].store(ia_set_vbs_traced as usize, Ordering::Relaxed);
                xbox[35].store(set_compute_table_traced as usize, Ordering::Relaxed);
                xbox[36].store(set_gfx_table_traced as usize, Ordering::Relaxed);
            }
        }
    };
}
make_probes!(
    (p27,27),(p28,28),(p29,29),(p30,30),(p31,31),
    (p47,47),(p48,48),(p49,49),(p51,51),(p52,52),(p53,53),(p54,54),(p55,55),(p56,56),(p57,57),
    (p58,58),(p59,59),(p60,60),(p61,61),(p62,62),(p63,63),(p64,64),(p65,65),(p66,66),(p67,67),
    (p68,68),(p69,69),(p70,70),(p71,71),(p72,72),(p73,73),(p74,74),(p75,75),(p76,76),(p77,77),
    (p78,78),(p79,79),(p80,80),(p81,81),(p82,82),(p83,83),(p84,84),(p85,85),(p86,86),(p87,87),
    (p88,88),(p89,89),(p90,90),(p91,91),(p92,92),(p93,93),(p94,94),(p95,95),(p96,96),(p97,97),
    (p98,98),(p99,99),(p100,100),(p101,101),(p102,102),(p103,103),(p104,104),(p105,105),(p106,106),
    (p107,107),(p108,108),(p109,109),(p110,110),(p111,111),(p112,112),(p113,113),(p114,114),(p115,115),
    (p116,116),(p117,117),(p118,118),(p119,119),(p120,120),(p121,121),(p122,122),(p123,123),(p124,124),
    (p125,125),(p126,126),(p127,127),
    (p128,128),(p129,129),(p130,130),(p131,131),(p132,132),(p133,133),(p134,134),
    (p135,135),(p136,136),(p137,137),(p138,138),(p139,139),(p140,140),(p141,141),
    (p142,142),(p143,143),(p144,144),(p145,145),(p146,146),(p147,147),(p148,148),
);

unsafe extern "system" fn release_list(this: *mut c_void) -> u32 {
    let f: unsafe extern "system" fn(*mut c_void) -> u32 = core::mem::transmute(*REAL_VT_PTR.add(2));
    let mut tracker = crate::texture_state::TRACKER.lock().unwrap();
    let refs = f(this);
    if refs == 0 { tracker.reset(this as usize); }
    refs
}

unsafe extern "system" fn resource_barrier_traced(this: *mut c_void, n: u32, barriers: *const u8) {
    crate::rtcap::note_barriers(n, barriers);
    // Copy-on-write: only transitions of textures carrying buffer-only state bits change.
    let mut fixed: Option<Vec<u8>> = None;
    if !barriers.is_null() {
        let mut tracker = crate::texture_state::TRACKER.lock().unwrap();
        // The x64 D3D12_RESOURCE_BARRIER ABI has a 32-byte stride.
        for i in 0..n as usize {
            let b = barriers.add(i * 32);
            let word = |offset| b.add(offset).cast::<u32>().read_unaligned();
            if word(0) == 0 {
                let resource = b.add(8).cast::<usize>().read_unaligned();
                let (mut before, mut after) = (word(20), word(24));
                if (before | after) & crate::device_wrap::BUFFER_ONLY_STATES != 0 && resource != 0
                    && !crate::device_wrap::is_buffer(resource as *mut c_void) {
                    before = crate::device_wrap::texture_state(before);
                    after = crate::device_wrap::texture_state(after);
                    let v = fixed.get_or_insert_with(|| core::slice::from_raw_parts(barriers, n as usize * 32).to_vec());
                    v[i * 32 + 20..i * 32 + 24].copy_from_slice(&before.to_le_bytes());
                    v[i * 32 + 24..i * 32 + 28].copy_from_slice(&after.to_le_bytes());
                    static N: AtomicUsize = AtomicUsize::new(0);
                    if N.fetch_add(1, Ordering::Relaxed) < 4 {
                        record("d3d12_xs", "ResourceBarrier", &format!(
                            "texture {resource:#x}: buffer-only state bits dropped ({:#x}->{:#x} becomes {before:#x}->{after:#x})",
                            word(20), word(24)));
                    }
                }
                let _ = before;
                let after = (word(4) != 1).then_some(after);
                tracker.record(this as usize, resource, word(16), after);
            }
        }
    }
    let f: unsafe extern "system" fn(*mut c_void, u32, *const u8) = core::mem::transmute(*REAL_VT_PTR.add(26));
    match &fixed {
        Some(v) => f(this, n, v.as_ptr()),
        None => f(this, n, barriers),
    }
}

// Draw-trace hooks (installed only with the diagnostic capture): note, then forward.
unsafe extern "system" fn om_set_targets_traced(this: *mut c_void, n: u32, handles: *const usize, single: i32, dsv: *const usize) {
    crate::rtcap::on_set_targets(this as usize, n, handles, single != 0, dsv);
    let f: unsafe extern "system" fn(*mut c_void, u32, *const usize, i32, *const usize) = core::mem::transmute(*REAL_VT_PTR.add(46));
    f(this, n, handles, single, dsv)
}
/// SetCompute/GraphicsRootDescriptorTable (PC 31/32 = Xbox 35/36): (RootParameterIndex,
/// D3D12_GPU_DESCRIPTOR_HANDLE by value). Recorded for the binding trace.
unsafe extern "system" fn set_compute_table_traced(this: *mut c_void, index: u32, handle: u64) {
    crate::bindtrace::set_table(this as usize, true, index, handle);
    let f: unsafe extern "system" fn(*mut c_void, u32, u64) = core::mem::transmute(*REAL_VT_PTR.add(31));
    f(this, index, handle)
}
unsafe extern "system" fn set_gfx_table_traced(this: *mut c_void, index: u32, handle: u64) {
    crate::bindtrace::set_table(this as usize, false, index, handle);
    let f: unsafe extern "system" fn(*mut c_void, u32, u64) = core::mem::transmute(*REAL_VT_PTR.add(32));
    f(this, index, handle)
}
unsafe extern "system" fn draw_traced(this: *mut c_void, a: u32, b: u32, c: u32, d: u32) {
    crate::rtcap::trace_draw(this as usize, false, &format!("DrawInstanced(vtx={a} inst={b} startVtx={c} startInst={d})"));
    if !crate::rtcap::on_draw(this as usize, 0) {
        return;
    }
    let f: unsafe extern "system" fn(*mut c_void, u32, u32, u32, u32) = core::mem::transmute(*REAL_VT_PTR.add(12));
    f(this, a, b, c, d)
}
/// IASetVertexBuffers (PC 44 = Xbox 48): (StartSlot, NumViews, pViews); views are 16 bytes.
unsafe extern "system" fn ia_set_vbs_traced(this: *mut c_void, start: u32, n: u32, views: *const u8) {
    crate::rtcap::on_set_vbs(this as usize, start, n, views);
    let f: unsafe extern "system" fn(*mut c_void, u32, u32, *const u8) = core::mem::transmute(*REAL_VT_PTR.add(44));
    f(this, start, n, views)
}
unsafe extern "system" fn draw_indexed_traced(this: *mut c_void, a: u32, b: u32, c: u32, d: i32, e: u32) {
    crate::rtcap::trace_draw(this as usize, false, &format!("DrawIndexedInstanced(idx={a} inst={b} startIdx={c} baseVtx={d} startInst={e})"));
    if !crate::rtcap::on_draw(this as usize, 1) {
        return;
    }
    let f: unsafe extern "system" fn(*mut c_void, u32, u32, u32, i32, u32) = core::mem::transmute(*REAL_VT_PTR.add(13));
    f(this, a, b, c, d, e)
}
unsafe extern "system" fn dispatch_traced(this: *mut c_void, a: u32, b: u32, c: u32) {
    crate::rtcap::trace_draw(this as usize, true, &format!("Dispatch({a},{b},{c})"));
    if !crate::rtcap::on_draw(this as usize, 2) {
        return;
    }
    let f: unsafe extern "system" fn(*mut c_void, u32, u32, u32) = core::mem::transmute(*REAL_VT_PTR.add(14));
    f(this, a, b, c)
}
unsafe extern "system" fn indirect_traced(this: *mut c_void, sig: *mut c_void, n: u32, buf: *mut c_void, off: u64, cbuf: *mut c_void, coff: u64) {
    if !crate::rtcap::on_draw(this as usize, 3) {
        return;
    }
    let f: unsafe extern "system" fn(*mut c_void, *mut c_void, u32, *mut c_void, u64, *mut c_void, u64) = core::mem::transmute(*REAL_VT_PTR.add(59));
    f(this, sig, n, buf, off, cbuf, coff)
}

// The original RHI issues ExecuteIndirect at slot 59. GXDK 260402/261000
// callers use 59 for a PIX packet and 61 for ExecuteIndirect. Preserve all
// arguments (including the untouched stack arguments) until the ABI is known.
macro_rules! route_by_caller {
    ($name:ident, $slot:literal) => {
        #[unsafe(naked)]
        unsafe extern "system" fn $name() {
            core::arch::naked_asm!(
                ".seh_proc {entry}", "sub rsp, 40", ".seh_stackalloc 40", ".seh_endprologue",
                "mov [rsp+48], rcx", "mov [rsp+56], rdx", "mov [rsp+64], r8", "mov [rsp+72], r9",
                "mov rcx, [rsp+40]", "mov edx, {slot}", "call {select}",
                "mov rcx, [rsp+48]", "mov rdx, [rsp+56]", "mov r8, [rsp+64]", "mov r9, [rsp+72]",
                // REX.W on the tail jump identifies it as a Win64 unwind epilogue.
                "add rsp, 40", ".byte 0x48", "jmp rax", ".seh_endproc",
                entry = sym $name, slot = const $slot, select = sym select_command,
            );
        }
    };
}
route_by_caller!(route59, 59);
route_by_caller!(route60, 60);
route_by_caller!(route61, 61);
route_by_caller!(route80, 80);
route_by_caller!(route90, 90);
route_by_caller!(route146, 146);

unsafe extern "system" fn select_command(caller: usize, slot: u32) -> usize {
    use crate::gxdk_edition::{at_address, Edition::*};
    match (at_address(caller), slot) {
        (AprilUpdate2 | October, 59) => pix_packet as usize,
        (AprilUpdate2, 60) => pix_timing as usize,
        (AprilUpdate2 | October, 61) | (Unknown | Gxdk250405, 59) => {
            if crate::rtcap::enabled() { indirect_traced as usize } else { unsafe { *REAL_VT_PTR.add(59) } }
        }
        (_, 59) => p59 as usize,
        (_, 60) => p60 as usize,
        (Gxdk250405, 80) => dispatch_threads as usize,
        (_, 80) => p80 as usize,
        (Gxdk250405, 90) => clear_bvh_counter as *const () as usize,
        (_, 90) => p90 as *const () as usize,
        (Gxdk250405, 146) => compute_uav_barrier as usize,
        (_, 146) => p146 as usize,
        _ => p61 as usize,
    }
}

#[repr(C)]
struct BarrierGroup {
    kind: u32,
    count: u32,
    barriers: *const [u32; 4],
}

// All observed 250405 calls carry one global barrier with compute sync and UAV
// access on both sides. A legacy UAV barrier with a null resource orders all
// UAV accesses and works on hosts without enhanced-barrier support.
unsafe extern "system" fn compute_uav_barrier(this: *mut c_void, count: u32, groups: *const BarrierGroup) {
    const COMPUTE_SHADING: u32 = 0x80;
    const UNORDERED_ACCESS: u32 = 0x10;
    if count != 1 || groups.is_null() {
        record("d3d12_xs", "BarrierX", "unsupported barrier group count or null group");
        return;
    }
    let group = unsafe { &*groups };
    if group.kind != 0 || group.count != 1 || group.barriers.is_null()
        || unsafe { *group.barriers } != [COMPUTE_SHADING, COMPUTE_SHADING, UNORDERED_ACCESS, UNORDERED_ACCESS]
    {
        record("d3d12_xs", "BarrierX", "unsupported barrier group or access masks");
        return;
    }
    #[repr(C)]
    struct UavBarrier {
        kind: u32,
        flags: u32,
        resource: *mut c_void,
        union_padding: [u64; 2],
    }
    const _: () = assert!(size_of::<UavBarrier>() == 32);
    let barrier = UavBarrier { kind: 2, flags: 0, resource: core::ptr::null_mut(), union_padding: [0; 2] };
    let methods = unsafe { *this.cast::<*const usize>() };
    let record_barrier: unsafe extern "system" fn(*mut c_void, u32, *const UavBarrier) =
        unsafe { core::mem::transmute(*methods.add(26)) };
    unsafe { record_barrier(this, 1, &barrier) };
    static FIRST: std::sync::Once = std::sync::Once::new();
    FIRST.call_once(|| record("d3d12_xs", "BarrierX", "slot=146 global compute UAV barrier recorded"));
}

unsafe fn host_annotation(this: *mut c_void, kind: u8, metadata: u32, data: *const u8, size: u32) {
    let vt = unsafe { *this.cast::<*const usize>() };
    match kind {
        0 => {
            let end: unsafe extern "system" fn(*mut c_void) = unsafe { core::mem::transmute(*vt.add(58)) };
            unsafe { end(this) };
        }
        1 | 2 => {
            let slot = if kind == 1 { 57 } else { 56 }; // BeginEvent / SetMarker
            let emit: unsafe extern "system" fn(*mut c_void, u32, *const u8, u32) =
                unsafe { core::mem::transmute(*vt.add(slot)) };
            unsafe { emit(this, metadata, data, size) };
        }
        _ => record("d3d12_xs", "PIX", &format!("unsupported event type {kind}")),
    }
}

unsafe extern "system" fn pix_packet(this: *mut c_void, kind: u8, data: *const u8, size: u32) {
    // WinPixEventRuntime's WINPIX_EVENT_PIX3BLOB_V2 tag for the encoded packet.
    unsafe { host_annotation(this, kind, 6345127, data, size) };
}

unsafe extern "system" fn pix_timing(this: *mut c_void, kind: u8) {
    // This form carries only the event type. The Xbox PIX thread buffer owns
    // the label, so keep GPU event boundaries on PC with a plain ANSI label.
    let label = b"Xbox PIX timing event\0";
    unsafe { host_annotation(this, kind, 1, label.as_ptr(), label.len() as u32) };
}

/// OMSetDepthBounds belongs to the PC List1 interface. Query it on the actual
/// command list before reading past the base vtable.
unsafe extern "system" fn set_depth_bounds(this: *mut c_void, min: f32, max: f32) {
    use crate::device_wrap::Guid;
    const LIST1: Guid = Guid { d1: 0x553103fb, d2: 0x1fe7, d3: 0x4557,
                               d4: [0xbb, 0x38, 0x94, 0x6d, 0x7d, 0x0e, 0x7c, 0xa7] };
    let vt = unsafe { *this.cast::<*const usize>() };
    let qi: unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> i32 =
        unsafe { core::mem::transmute(*vt) };
    let mut list1 = core::ptr::null_mut();
    if unsafe { qi(this, &LIST1, &mut list1) } < 0 || list1.is_null() {
        record("d3d12_xs", "OMSetDepthBounds", "host List1 unavailable");
        return;
    }
    unsafe {
        let methods = *list1.cast::<*const usize>();
        let set: unsafe extern "system" fn(*mut c_void, f32, f32) = core::mem::transmute(*methods.add(62));
        let release: unsafe extern "system" fn(*mut c_void) -> u32 = core::mem::transmute(*methods.add(2));
        set(list1, min, max);
        release(list1);
    }
}

/// Xbox slot 67: reached (by tail call) from `FD3D12CommandContext::FlushTextureCacheIfNeeded`
/// (EXE+0x4e0718). The console GPU's texture cache has no PC counterpart, so there is
/// nothing to do; this is a deliberate no-op, not a placeholder.
unsafe extern "system" fn flush_texture_cache_x(_this: *mut c_void) {}

/// Xbox slot 83 = CopyTextureRegion with one extra trailing argument.
///
/// Evidence: `FD3D12TextureBase::UpdateTexture` (EXE+0xc7cc5f) and
/// `FD3D12RHICommandInitializeTexture::ExecuteNoCmdList` (EXE+0x13e7da9) call it as
/// (this, pDst, DstX, DstY, DstZ, pSrc, pSrcBox, 0) with 48-byte copy locations laid out as
/// on PC (resource, type, 32-byte placed footprint / subresource index). The eighth argument
/// is always 0 at both sites (probably Xbox copy flags) and is dropped.
unsafe extern "system" fn copy_texture_region_x(
    this: *mut c_void, dst: *const u8, x: u32, y: u32, z: u32, src: *const u8, src_box: *const c_void, flags: u32,
) {
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    let n = CALLS.fetch_add(1, Ordering::Relaxed);
    if dst.is_null() || src.is_null() {
        record("d3d12_xs", "CopyTextureRegionX", "null copy location - ignored");
        return;
    }
    let q = |p: *const u8, o: usize| core::ptr::read_unaligned(p.add(o) as *const u64);
    if n < 6 || flags != 0 {
        record("d3d12_xs", "CopyTextureRegionX", &format!(
            "#{n} dst={:#x} at ({x},{y},{z}) src={:#x} box={src_box:p} flags={flags:#x}",
            q(dst, 0), q(src, 0)));
    }
    if q(dst, 0) == 0 || q(src, 0) == 0 {
        record("d3d12_xs", "CopyTextureRegionX", "null resource in copy location - ignored");
        return;
    }
    let f: unsafe extern "system" fn(*mut c_void, *const u8, u32, u32, u32, *const u8, *const c_void) =
        core::mem::transmute(*REAL_VT_PTR.add(16));
    f(this, dst, x, y, z, src, src_box);
}

/// Slot 85 receives a trailing zero in both branches of the Scarlett FFX copy job.
unsafe extern "system" fn copy_texture_region_fidelityfx(
    this: *mut c_void, dst: *const u8, x: u32, y: u32, z: u32, src: *const u8,
    src_box: *const c_void, flags: u32,
) {
    if flags != 0 {
        record("d3d12_xs", "CopyTextureRegionX", &format!("slot 85 unsupported flags={flags:#x}"));
        return;
    }
    unsafe { copy_texture_region_x(this, dst, x, y, z, src, src_box, flags) };
}

/// Point a freshly created real command list at the Xbox-layout vtable.
pub fn apply(real_list: *mut c_void) {
    if real_list.is_null() {
        return;
    }
    unsafe {
        let real_vt = *(real_list as *const *const usize);
        BUILT.call_once(|| {
            for i in 0..N {
                let p = match map_slot(i) {
                    Some(src) => *real_vt.add(src),
                    None => inserted_stub as usize,
                };
                XBOX_VT[i].store(p, Ordering::Relaxed);
            }
            extern "system" { fn GetModuleHandleW(n: *const u16) -> *mut c_void; }
            REAL_VT_PTR = real_vt;
            // SAFETY: a valid ID3D12GraphicsCommandList has all 60 base methods.
            install_probes(&XBOX_VT, &*real_vt.cast::<[usize; PC_LAST_SLOT + 1]>());
            record("d3d12_xs", "cmdlist.vtable",
                &format!("built Xbox-layout cmdlist vtable from real {real_vt:p} (+4 shift at PC28)"));
            // Diagnostic: RVAs of the real PC methods inside D3D12Core, to attribute crash frames.
            let core = GetModuleHandleW("D3D12Core.dll ".encode_utf16().collect::<Vec<u16>>().as_ptr()) as usize;
            let mut s = String::new();
            for i in 0..=PC_LAST_SLOT { s.push_str(&format!("{i}:{:x} ", (*real_vt.add(i)).wrapping_sub(core))); }
            record("d3d12_xs", "cmdlist.vtable", &format!("XBOX_VT at {:p}, slot98={:#x} stub={:#x}", XBOX_VT.as_ptr(), XBOX_VT[98].load(Ordering::Relaxed), inserted_stub as usize));
            record("d3d12_xs", "cmdlist.vtable", &format!("PC cmdlist method RVAs in D3D12Core: {s}"));
        });
        // The real object keeps its PC vtable; the title only ever sees a shim (below).
        let _ = (VirtualProtect as usize, PAGE_READWRITE);
    }
}

// ---- command-list shim object ------------------------------------------------------------
//
// On Xbox the command list is a public struct with an inline GPU command buffer: the title
// writes raw command packets straight into it for hot state changes, e.g. in
// FD3D12StateCacheBase::ApplyState (EXE+0x4e0433):
//
//     mov rcx,[list+18h]             ; current write pointer
//     cmp rcx,[list+30h]             ; end of buffer -> jae slow path
//     mov dword ptr [rcx],0C000B600h ; mov [rcx+4],eax ; lea rax,[rcx+8] ; mov [list+18h],rax
//
// and only when the buffer is full does it call the method (slow path = vtable slot 20,
// IASetPrimitiveTopology). Handing the title the real PC object therefore lets it read
// D3D12-internal fields as buffer pointers and scribble through them. Instead the title
// gets this shim: our own zeroed block whose write pointer == end pointer (both 0), so
// every inline fast path of that form falls back to the API call, with a vtable of thunks
// that swap the shim for the real list and jump through the Xbox-layout table.
//
// Status: the layout fact (+0x18 write ptr, +0x30 end ptr) is observed at one call site;
// other inline fields the title may read are served zeros (unverified).

/// Offset of the real ID3D12GraphicsCommandList pointer inside the shim.
const SHIM_REAL_OFF: usize = 0x38;
/// Shim size: generous and zero-filled, for any other inline field the title reads.
const SHIM_SIZE: usize = 0x4000;
// TomBox state lives beyond the guest-visible inline packet fields. Recording
// a D3D12 command list is single-threaded. No borrowed PSO survives a bind.
#[repr(C, align(16))]
struct ListShim {
    vtable: *const AtomicUsize,
    packet_fields: [u8; SHIM_REAL_OFF - size_of::<usize>()],
    real: *mut c_void,
    guest_tail: [u8; SHIM_SIZE - SHIM_REAL_OFF - size_of::<usize>()],
    group: [u32; 3],
    copy_dest: core::ops::Range<u64>,
}

const _: () = assert!(core::mem::offset_of!(ListShim, real) == SHIM_REAL_OFF);
const _: () = assert!(core::mem::offset_of!(ListShim, group) == SHIM_SIZE);

// Private data on the host PSO follows its COM lifetime (including address reuse).
static HAS_GROUP_METADATA: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
const GROUP_GUID: crate::device_wrap::Guid = crate::device_wrap::Guid {
    d1: 0x968a5ef2, d2: 0x0e91, d3: 0x411d, d4: [0x83, 0x16, 0x7d, 0x54, 0x09, 0xc7, 0x12, 0x18],
};
pub(super) unsafe fn tag_compute_pso(created: *mut c_void, group: [u32; 3]) {
    // Creation returns the interface requested by the caller, possibly IUnknown.
    const OBJECT: crate::device_wrap::Guid = crate::device_wrap::Guid {
        d1: 0xc4fec28f, d2: 0x7966, d3: 0x4e95, d4: [0x9f, 0x94, 0xf4, 0x31, 0xcb, 0x56, 0xc3, 0xb8],
    };
    let vt = *created.cast::<*const usize>();
    let query: unsafe extern "system" fn(*mut c_void, &crate::device_wrap::Guid, &mut *mut c_void) -> i32 =
        core::mem::transmute(*vt);
    let mut pso = core::ptr::null_mut();
    if query(created, &OBJECT, &mut pso) < 0 || pso.is_null() { return; }
    let vt = *pso.cast::<*const usize>();
    let set: unsafe extern "system" fn(*mut c_void, &crate::device_wrap::Guid, u32, &[u32; 3]) -> i32 =
        core::mem::transmute(*vt.add(4));
    if set(pso, &GROUP_GUID, 12, &group) == 0 {
        HAS_GROUP_METADATA.store(true, Ordering::Release);
    } else {
        record("d3d12_xs", "DispatchThreads", "could not attach group dimensions to pipeline");
    }
    let release: unsafe extern "system" fn(*mut c_void) -> u32 = core::mem::transmute(*vt.add(2));
    release(pso);
}

unsafe fn pipeline_group(pso: *mut c_void) -> [u32; 3] {
    if pso.is_null() || !HAS_GROUP_METADATA.load(Ordering::Acquire) { return [0; 3]; }
    let vt = *pso.cast::<*const usize>();
    let get: unsafe extern "system" fn(*mut c_void, &crate::device_wrap::Guid, &mut u32, &mut [u32; 3]) -> i32 =
        core::mem::transmute(*vt.add(3));
    let mut size = 12;
    let mut group = [0; 3];
    if get(pso, &GROUP_GUID, &mut size, &mut group) == 0 && size == 12 { group } else { [0; 3] }
}

impl ListShim {
    unsafe fn remember_pipeline(&mut self, pso: *mut c_void) {
        self.group = pipeline_group(pso);
        if crate::rtcap::enabled() {
            crate::rtcap::on_set_pso(self.real as usize, pso as usize);
        }
    }
}

unsafe extern "system" fn shim_reset(shim: &mut ListShim, allocator: *mut c_void, pso: *mut c_void) -> i32 {
    let reset: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void) -> i32 =
        core::mem::transmute(*REAL_VT_PTR.add(10));
    let hr = reset(shim.real, allocator, pso);
    if hr >= 0 {
        crate::texture_state::TRACKER.lock().unwrap().reset(shim.real as usize);
        shim.copy_dest = 0..0;
        shim.remember_pipeline(pso);
    }
    hr
}

// Track only the most recent explicit COPY_DEST buffer transition in this recording.
// No resource pointer is retained. A split, aliasing or later transition invalidates the range.
unsafe extern "system" fn shim_barrier(shim: &mut ListShim, count: u32, barriers: *const u8) {
    resource_barrier_traced(shim.real, count, barriers);
    if count == 0 || barriers.is_null() { return; }
    shim.copy_dest = 0..0;
    let last = barriers.add((count as usize - 1) * 32);
    let word = |offset| last.add(offset).cast::<u32>().read_unaligned();
    if word(0) != 0 || word(4) != 0 || !matches!(word(16), 0 | u32::MAX) || word(24) != 0x400 { return; }
    let resource = last.add(8).cast::<*mut c_void>().read_unaligned();
    if resource.is_null() { return; }
    let vt = *resource.cast::<*const usize>();
    let describe: unsafe extern "system" fn(*mut c_void, *mut u64) -> *mut u64 = core::mem::transmute(*vt.add(10));
    let address: unsafe extern "system" fn(*mut c_void) -> u64 = core::mem::transmute(*vt.add(11));
    let mut desc = [0u64; 8];
    describe(resource, desc.as_mut_ptr());
    if desc[0] as u32 == 1 { // D3D12_RESOURCE_DIMENSION_BUFFER, Width at offset 16.
        let base = address(resource);
        if let Some(end) = base.checked_add(desc[2]) {
            if base != 0 { shim.copy_dest = base..end; }
        }
    }
}

// GXDK 250405 issues slot 90 once per scratch counter. Only the zero write
// without the final-batch flag has a verified desktop equivalent.
unsafe extern "system" fn clear_bvh_counter(shim: &ListShim, address: u64, value: u32, flags: u32) {
    if value != 0 || flags != 0 || address % 4 != 0 || !shim.copy_dest.contains(&address)
        || shim.copy_dest.end - address < 4 {
        record("d3d12_xs", "cmdlist.counter", "slot=90 unsupported counter range, value or flags");
        return;
    }
    use crate::device_wrap::Guid;
    type Query = unsafe extern "system" fn(*mut c_void, *const u8, *mut *mut c_void) -> i32;
    type Release = unsafe extern "system" fn(*mut c_void) -> u32;
    let vt = *shim.real.cast::<*const usize>();
    let get_device: Query = core::mem::transmute(*vt.add(7));
    let list_type: unsafe extern "system" fn(*mut c_void) -> u32 = core::mem::transmute(*vt.add(8));
    let mut device = core::ptr::null_mut();
    if get_device(shim.real, crate::IID_ID3D12Device.as_ptr(), &mut device) < 0 { return; }
    let dev_vt = *device.cast::<*const usize>();
    let feature: unsafe extern "system" fn(*mut c_void, u32, *mut u32, u32) -> i32 = core::mem::transmute(*dev_vt.add(13));
    let mut options3 = [0u32; 5];
    let hr = feature(device, 21, options3.as_mut_ptr(), core::mem::size_of_val(&options3) as u32);
    let release: Release = core::mem::transmute(*dev_vt.add(2));
    release(device);
    if hr < 0 || options3[2] & 1u32.checked_shl(list_type(shim.real)).unwrap_or(0) == 0 {
        record("d3d12_xs", "cmdlist.counter", "slot=90 host does not support immediate writes on this list");
        return;
    }

    const LIST2: Guid = Guid { d1: 0x38c3e585, d2: 0xff17, d3: 0x412c, d4: [0x91,0x50,0x4f,0xc6,0xf9,0xd7,0x2a,0x28] };
    let query: Query = core::mem::transmute(*vt);
    let mut list = core::ptr::null_mut();
    if query(shim.real, (&LIST2 as *const Guid).cast(), &mut list) < 0 { return; }
    #[repr(C)]
    struct Write { destination: u64, value: u32, pad: u32 }
    let methods = *list.cast::<*const usize>();
    let write: unsafe extern "system" fn(*mut c_void, u32, *const Write, *const u32) = core::mem::transmute(*methods.add(66));
    write(list, 1, &Write { destination: address, value, pad: 0 }, core::ptr::null());
    let release: Release = core::mem::transmute(*methods.add(2));
    release(list);
    record("d3d12_xs", "cmdlist.counter", "slot=90 recorded 32-bit zero write");
}

unsafe extern "system" fn shim_clear(shim: &mut ListShim, pso: *mut c_void) {
    let clear: unsafe extern "system" fn(*mut c_void, *mut c_void) = core::mem::transmute(*REAL_VT_PTR.add(11));
    clear(shim.real, pso);
    shim.remember_pipeline(pso);
}

unsafe extern "system" fn shim_set_pso(shim: &mut ListShim, pso: *mut c_void) {
    let set: unsafe extern "system" fn(*mut c_void, *mut c_void) = core::mem::transmute(*REAL_VT_PTR.add(25));
    set(shim.real, pso);
    shim.remember_pipeline(pso);
}

fn dispatch_groups(threads: [u32; 3], group: [u32; 3], flags: u32) -> Option<[u32; 3]> {
    if !matches!(flags, 0x20 | 0x8020) { return None; }
    let mut counts = [0; 3];
    for i in 0..3 {
        counts[i] = threads[i].checked_div(group[i])?;
        if threads[i] % group[i] != 0 || counts[i] > 65535 { return None; }
    }
    Some(counts)
}

unsafe extern "system" fn dispatch_threads(shim: &ListShim, x: u32, y: u32, z: u32, flags: u32) {
    let group = shim.group;
    let Some([gx, gy, gz]) = dispatch_groups([x, y, z], group, flags) else {
        static UNSUPPORTED: AtomicUsize = AtomicUsize::new(0);
        if UNSUPPORTED.fetch_add(1, Ordering::Relaxed) < 8 {
            record("d3d12_xs", "DispatchThreads",
                &format!("unsupported threads=({x},{y},{z}) group={group:?} flags={flags:#x}"));
        }
        return;
    };
    // Use slot 14's selected host path so capture sees the converted dispatch.
    let dispatch: unsafe extern "system" fn(*mut c_void, u32, u32, u32) =
        core::mem::transmute(XBOX_VT[14].load(Ordering::Relaxed));
    dispatch(shim.real, gx, gy, gz);
}

static SHIM_VT: [AtomicUsize; N] = [const { AtomicUsize::new(0) }; N];
static SHIM_BUILT: std::sync::Once = std::sync::Once::new();

/// Build one 16-byte thunk per slot:
///   mov rcx,[rcx+38h] ; mov rax,&XBOX_VT[slot] ; jmp qword ptr [rax]
unsafe fn build_shim_thunks() {
    extern "system" {
        fn VirtualAlloc(addr: *mut c_void, size: usize, ty: u32, protect: u32) -> *mut c_void;
    }
    let code = VirtualAlloc(core::ptr::null_mut(), N * 16, 0x3000, 0x40) as *mut u8;
    assert!(!code.is_null(), "d3d12_xs: cannot allocate command-list shim thunks");
    for i in 0..N {
        let p = code.add(i * 16);
        let mut b = [0u8; 16];
        b[0..4].copy_from_slice(&[0x48, 0x8B, 0x49, SHIM_REAL_OFF as u8]);
        b[4] = 0x48;
        b[5] = 0xB8;
        b[6..14].copy_from_slice(&(XBOX_VT.as_ptr().add(i) as u64).to_le_bytes());
        b[14] = 0xFF;
        b[15] = 0x20;
        core::ptr::copy_nonoverlapping(b.as_ptr(), p, 16);
        let entry = match i {
            10 => shim_reset as usize,
            11 => shim_clear as usize,
            25 => shim_set_pso as usize,
            26 => shim_barrier as *const () as usize,
            80 => route80 as usize,
            90 => route90 as *const () as usize,
            _ => p as usize,
        };
        SHIM_VT[i].store(entry, Ordering::Relaxed);
    }
}

/// Wrap a freshly created real command list in a shim object for the title.
pub fn wrap(real_list: *mut c_void, initial_pso: *mut c_void) -> *mut c_void {
    if real_list.is_null() {
        return real_list;
    }
    crate::texture_state::TRACKER.lock().unwrap().reset(real_list as usize);
    apply(real_list); // builds the Xbox-layout table on first use
    unsafe {
        SHIM_BUILT.call_once(|| build_shim_thunks());
        let layout = std::alloc::Layout::new::<ListShim>();
        let p = std::alloc::alloc_zeroed(layout).cast::<ListShim>();
        assert!(!p.is_null(), "d3d12_xs: cannot allocate command-list shim");
        (*p).vtable = SHIM_VT.as_ptr();
        (*p).real = real_list;
        (*p).remember_pipeline(initial_pso);
        static N_WRAPPED: AtomicUsize = AtomicUsize::new(0);
        if N_WRAPPED.fetch_add(1, Ordering::Relaxed) < 3 {
            record("d3d12_xs", "cmdlist.shim", &format!("real {real_list:p} wrapped as shim {p:p}"));
        }
        p as *mut c_void
    }
}

/// If `p` is one of our shims return the real command list, else `p` unchanged.
pub fn unwrap(p: *mut c_void) -> *mut c_void {
    if p.is_null() {
        return p;
    }
    unsafe {
        if *(p as *const usize) == SHIM_VT.as_ptr() as usize {
            (*p.cast::<ListShim>()).real
        } else {
            p
        }
    }
}

/// Number of calls to diagnostic-only command-list bindings.
pub fn inserted_hits() -> usize {
    INSERTED_HITS.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pix_translates_packet_and_timing_annotations() {
        #[repr(C)]
        struct Sink {
            vt: *const usize,
            events: Vec<(u8, u32, Vec<u8>)>,
        }
        unsafe extern "system" fn begin(s: *mut Sink, tag: u32, p: *const u8, n: u32) {
            unsafe { (*s).events.push((1, tag, core::slice::from_raw_parts(p, n as usize).to_vec())) };
        }
        unsafe extern "system" fn marker(s: *mut Sink, tag: u32, p: *const u8, n: u32) {
            unsafe { (*s).events.push((2, tag, core::slice::from_raw_parts(p, n as usize).to_vec())) };
        }
        unsafe extern "system" fn end(s: *mut Sink) {
            unsafe { (*s).events.push((0, 0, Vec::new())) };
        }
        let mut vt = [0; PC_LAST_SLOT + 1];
        vt[56] = marker as usize;
        vt[57] = begin as usize;
        vt[58] = end as usize;
        let mut sink = Sink { vt: vt.as_ptr(), events: Vec::new() };
        let this = (&mut sink as *mut Sink).cast();
        let packet = 0xf3083u64.to_le_bytes();
        unsafe {
            pix_packet(this, 1, packet.as_ptr(), packet.len() as u32);
            pix_packet(this, 2, packet.as_ptr(), packet.len() as u32);
            pix_packet(this, 0, core::ptr::null(), 0);
            pix_timing(this, 1);
            pix_timing(this, 0);
        }
        assert_eq!(sink.events, vec![
            (1, 6345127, packet.to_vec()), (2, 6345127, packet.to_vec()), (0, 0, vec![]),
            (1, 1, b"Xbox PIX timing event\0".to_vec()), (0, 0, vec![]),
        ]);
    }

    #[test]
    fn thread_dimensions_require_whole_groups_in_every_axis() {
        let threads = [32, 12, 10];
        for flags in [0x20, 0x8020] {
            assert_eq!(dispatch_groups(threads, [8, 4, 2], flags), Some([4, 3, 5]));
            assert_eq!(dispatch_groups(threads, [8, 5, 2], flags), None);
        }
        assert_eq!(dispatch_groups(threads, [0, 0, 0], 0x20), None);
        assert_eq!(dispatch_groups(threads, [1; 3], 0x21), None);
        assert_eq!(dispatch_groups([65535, 1, 1], [1; 3], 0x20), Some([65535, 1, 1]));
        assert_eq!(dispatch_groups([65536, 1, 1], [1; 3], 0x20), None);
    }

    #[test]
    fn slot146_records_only_the_verified_global_uav_barrier() {
        #[repr(C)]
        struct Sink {
            methods: *const usize,
            calls: Vec<(u32, [u64; 4])>,
        }
        unsafe extern "system" fn capture(this: *mut Sink, count: u32, barrier: *const [u64; 4]) {
            unsafe { (*this).calls.push((count, *barrier)) };
        }
        let mut methods = [0; PC_LAST_SLOT + 1];
        methods[26] = capture as usize;
        let mut sink = Sink { methods: methods.as_ptr(), calls: Vec::new() };
        let list = (&mut sink as *mut Sink).cast();
        let mut group = BarrierGroup { kind: 0, count: 1, barriers: &[0x80, 0x80, 0x10, 0x10] };
        unsafe {
            compute_uav_barrier(list, 1, &group);
            compute_uav_barrier(list, 0, core::ptr::null());
            compute_uav_barrier(list, 2, &group);
            group.kind = 1;
            compute_uav_barrier(list, 1, &group);
            group.kind = 0;
            group.barriers = &[0x80, 0x80, 0x10, 0x20];
            compute_uav_barrier(list, 1, &group);
        }
        // This test executable has no GXDK edition tag.
        assert_eq!(unsafe { select_command(capture as *const () as usize, 146) }, p146 as *const () as usize);
        // Type UAV, flags NONE, resource NULL, zeroed union padding.
        assert_eq!(sink.calls, [(1, [2, 0, 0, 0])]);
    }

    #[test]
    fn discard_resolves_to_the_host_method_after_probe_installation() {
        // Give every host method a distinct token. Inspect the finished table
        // without calling a driver or modifying the process-wide dispatch table.
        let pc = std::array::from_fn(|i| i + 1);
        let xbox = std::array::from_fn(|_| AtomicUsize::new(0));
        install_probes(&xbox, &pc);
        assert_eq!(xbox[55].load(Ordering::Relaxed), pc[51]);
    }
}
