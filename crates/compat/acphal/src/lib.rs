//! `AcpHal.dll` compatibility — Xbox Audio Processor (ACP/APU / SHAPE) memory.
//!
//! `FXboxCommonGDKPlatformMemory::CreateApuHeap` fatally errors if `ApuCreateHeap`
//! fails, so these must succeed. We back the "APU heap" with a single host
//! `VirtualAlloc` region and bump-allocate from it (signatures recovered from the
//! call sites in the EXE):
//!   HRESULT ApuCreateHeap(uint32 sizeBytes, uint32 flags)
//!   HRESULT ApuAlloc(void** ppBlock, uint32* pApuAddr, uint32 size, uint32 align)
//!   HRESULT ApuHeapGetState(struct{u32 free; u32 used; ...}* out, uint32 idx)
//! Audio DSP (SHAPE) is not emulated; this only satisfies the memory contract so
//! init proceeds. The returned APU address is modeled as a 32-bit offset.

use core::ffi::c_void;
use std::sync::Mutex;
use compat_common::{record, S_OK};

const M: &str = "acphal";

extern "system" {
    fn VirtualAlloc(addr: *mut c_void, size: usize, typ: u32, protect: u32) -> *mut c_void;
}
const MEM_COMMIT_RESERVE: u32 = 0x1000 | 0x2000;
const PAGE_READWRITE: u32 = 0x04;

struct Heap {
    base: usize,
    size: usize,
    offset: usize,
}

static HEAP: Mutex<Option<Heap>> = Mutex::new(None);

#[no_mangle]
pub extern "system" fn ApuCreateHeap(size_bytes: u32, flags: u32) -> i32 {
    let mut g = HEAP.lock().unwrap();
    if g.is_some() {
        record(M, "ApuCreateHeap", "already created -> S_OK");
        return S_OK;
    }
    // Round up and reserve+commit the APU heap from host memory.
    let size = (size_bytes as usize).max(0x1000);
    let base = unsafe { VirtualAlloc(core::ptr::null_mut(), size, MEM_COMMIT_RESERVE, PAGE_READWRITE) } as usize;
    if base == 0 {
        record(M, "ApuCreateHeap", &format!("VirtualAlloc({size}) FAILED -> E_FAIL"));
        return compat_common::E_FAIL;
    }
    record(M, "ApuCreateHeap", &format!("size={size_bytes} flags={flags:#x} -> heap @ {base:#x} S_OK"));
    *g = Some(Heap { base, size, offset: 0 });
    S_OK
}

#[no_mangle]
/// `ApuAlloc(void** ppBlock, APU_ADDRESS* pApuAddr, UINT32 size, UINT32 align)`.
/// `APU_ADDRESS` is a **32-bit** handle (title call sites at EXE+0x6def61/0x6defb5 pass a
/// u32 slot placed 4 bytes before the block-pointer slot), so only 4 bytes may be written.
pub extern "system" fn ApuAlloc(pp_block: *mut *mut c_void, p_phys: *mut u32, size: u32, align: u32) -> i32 {
    let mut g = HEAP.lock().unwrap();
    let heap = match g.as_mut() {
        Some(h) => h,
        None => {
            record(M, "ApuAlloc", "no heap -> E_FAIL");
            return compat_common::E_FAIL;
        }
    };
    let align = (align as usize).max(16);
    let start = (heap.base + heap.offset + (align - 1)) & !(align - 1);
    let end = start + size as usize;
    let ptr = if end <= heap.base + heap.size {
        heap.offset = end - heap.base;
        start
    } else {
        // Heap exhausted: fall back to a standalone committed region.
        let p = unsafe { VirtualAlloc(core::ptr::null_mut(), (size as usize).max(0x1000), MEM_COMMIT_RESERVE, PAGE_READWRITE) } as usize;
        record(M, "ApuAlloc", &format!("heap full; standalone alloc size={size} -> {p:#x}"));
        p
    };
    if !p_phys.is_null() {
        // APU address = offset of the block inside the APU heap (32-bit).
        unsafe { core::ptr::write_unaligned(p_phys, ptr.wrapping_sub(heap.base) as u32) };
    }
    if !pp_block.is_null() {
        unsafe { *pp_block = ptr as *mut c_void };
    }
    S_OK
}

#[no_mangle]
pub extern "system" fn ApuHeapGetState(out: *mut u32, idx: u32) -> i32 {
    let g = HEAP.lock().unwrap();
    let (free, used) = match g.as_ref() {
        Some(h) => ((h.size - h.offset) as u32, h.offset as u32),
        None => (0u32, 0u32),
    };
    // Game reads out[0] and out[1] (offsets 0 and 4). Write exactly those.
    if !out.is_null() {
        unsafe {
            *out = free;
            *out.add(1) = used;
        }
    }
    let _ = idx;
    S_OK
}

#[no_mangle]
pub extern "system" fn ApuFree(_block: *mut c_void) -> i32 {
    // Bump allocator: individual frees are no-ops (APU heap freed at teardown).
    S_OK
}

// ---- IAcpHal -----------------------------------------------------------------------------
//
// `AcpHalCreate(IAcpHal** ppAcpHal)` (single out-param: title call site EXE+0x6df048 loads
// only rcx). The title then drives the object through its vtable (slot 0 = Connect(0x1000,
// 0x1000), slot 2 = SubmitCommand-style call, ...). There is no SHAPE/XMA hardware on the
// host, so this is an *inert device*: every method is logged (first few per slot) and
// returns S_OK, but no command is ever executed and no completion message is produced.
// XMA audio therefore does not play — tracked in docs/TECHNICAL_DEBT.md.

const HAL_SLOTS: usize = 32;
static HAL_VTBL: [std::sync::atomic::AtomicUsize; HAL_SLOTS] =
    [const { std::sync::atomic::AtomicUsize::new(0) }; HAL_SLOTS];
static HAL_OBJ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn hal_log(slot: usize, a: u64, b: u64, c: u64) {
    static PER: [std::sync::atomic::AtomicUsize; HAL_SLOTS] =
        [const { std::sync::atomic::AtomicUsize::new(0) }; HAL_SLOTS];
    if PER[slot].fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 3 {
        record(M, "IAcpHal", &format!("slot {slot} (a={a:#x} b={b:#x} c={c:#x}) -> S_OK (inert, no SHAPE DSP)"));
    }
}

macro_rules! hal_methods {
    ($(($name:ident, $slot:literal)),* $(,)?) => {
        $(
            unsafe extern "system" fn $name(_this: *mut c_void, a: u64, b: u64, c: u64) -> i32 {
                hal_log($slot, a, b, c);
                S_OK
            }
        )*
        fn hal_install() {
            $( HAL_VTBL[$slot].store($name as usize, std::sync::atomic::Ordering::Relaxed); )*
        }
    };
}
hal_methods!(
    (h0,0),(h1,1),(h2,2),(h3,3),(h4,4),(h5,5),(h6,6),(h7,7),(h8,8),(h9,9),(h10,10),(h11,11),(h12,12),
    (h13,13),(h14,14),(h15,15),(h16,16),(h17,17),(h18,18),(h19,19),(h20,20),(h21,21),(h22,22),(h23,23),
    (h24,24),(h25,25),(h26,26),(h27,27),(h28,28),(h29,29),(h30,30),(h31,31),
);

#[no_mangle]
pub extern "system" fn AcpHalCreate(out: *mut *mut c_void) -> i32 {
    if out.is_null() {
        return compat_common::E_FAIL;
    }
    use std::sync::atomic::Ordering;
    let mut obj = HAL_OBJ.load(Ordering::Relaxed);
    if obj == 0 {
        hal_install();
        // Object = one pointer-sized vtable slot (leaked; process-lifetime singleton).
        let b: &'static mut usize = Box::leak(Box::new(HAL_VTBL.as_ptr() as usize));
        obj = b as *mut usize as usize;
        HAL_OBJ.store(obj, Ordering::Relaxed);
    }
    unsafe { *out = obj as *mut c_void };
    record(M, "AcpHalCreate", &format!("-> inert IAcpHal @ {obj:#x}"));
    S_OK
}

/// `AcpHalAllocateShapeContexts(SHAPE_CONTEXTS*)`.
///
/// Layout (GDK `SHAPE_CONTEXTS`, confirmed against the title's XMA init at EXE+0x6deee6:
/// it sets the u32 at +0x10 to 0x200 and afterwards zeroes 0x8000 bytes at the pointer
/// stored at +0x38, i.e. 0x200 × 64-byte SHAPE_XMA_CONTEXT):
///   +0x00 numSrc, +0x04 numEqComp, +0x08 numFiltVol, +0x0C numDma, +0x10 numXma, +0x14 numPcm
///   +0x18 srcArray, +0x20 eqCompArray, +0x28 filtVolArray, +0x30 dmaArray,
///   +0x38 xmaArray, +0x40 pcmArray
/// We allocate the XMA context array (the only one this title requests). There is no
/// SHAPE DSP behind it: contexts are plain memory and nothing decodes XMA yet (audio is
/// tracked separately in docs/TECHNICAL_DEBT.md). Other context kinds are not allocated
/// because their element sizes are unverified — requesting them fails explicitly.
#[no_mangle]
pub extern "system" fn AcpHalAllocateShapeContexts(ctx: *mut u8, _b: u32, _c: *mut c_void) -> i32 {
    if ctx.is_null() {
        return compat_common::E_FAIL;
    }
    let rd = |o: usize| unsafe { core::ptr::read_unaligned(ctx.add(o) as *const u32) };
    let (src, eq, fv, dma, xma, pcm) = (rd(0), rd(4), rd(8), rd(0xc), rd(0x10), rd(0x14));
    if src != 0 || eq != 0 || fv != 0 || dma != 0 || pcm != 0 {
        record(M, "AcpHalAllocateShapeContexts",
            &format!("unsupported context request src={src} eq={eq} filtvol={fv} dma={dma} pcm={pcm} -> E_FAIL"));
        return compat_common::E_FAIL;
    }
    const SHAPE_XMA_CONTEXT_SIZE: usize = 64;
    let bytes = (xma as usize * SHAPE_XMA_CONTEXT_SIZE).max(0x1000);
    let p = unsafe { VirtualAlloc(core::ptr::null_mut(), bytes, MEM_COMMIT_RESERVE, PAGE_READWRITE) };
    if p.is_null() {
        return compat_common::E_FAIL;
    }
    unsafe { core::ptr::write_unaligned(ctx.add(0x38) as *mut *mut c_void, p) };
    record(M, "AcpHalAllocateShapeContexts",
        &format!("numXma={xma} -> xmaContextArray={p:p} ({bytes} bytes; no SHAPE DSP attached)"));
    S_OK
}
