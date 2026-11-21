//! Opt-in for a title helper that creates a zeroed committed resource, immediately
//! maps it, clears the entire allocation to zero and unmaps before exposing it.
//! A scratch allocation is correct only for this sequence. Both return addresses
//! are checked against the configured executable build.
use super::device_wrap::Guid;
use core::{ffi::c_void, mem, ptr};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex, OnceLock};

extern "system" {
    fn LoadLibraryExA(name: *const u8, file: *mut c_void, flags: u32) -> *mut c_void;
    fn GetModuleHandleW(name: *const u16) -> *const u8;
    fn GetCurrentThreadId() -> u32;
    fn RtlCaptureStackBackTrace(skip: u32, count: u32, frames: *mut usize, hash: *mut u32) -> u16;
    fn VirtualProtect(addr: *mut c_void, size: usize, protect: u32, old: *mut u32) -> i32;
}

type CreateBlob = unsafe extern "system" fn(usize, *mut *mut c_void) -> i32;

// Keep the optional scratch allocator out of the DLL's load-time dependencies.
static CREATE_BLOB: LazyLock<Option<CreateBlob>> = LazyLock::new(|| unsafe {
    const LOAD_LIBRARY_SEARCH_SYSTEM32: u32 = 0x800;
    let module = LoadLibraryExA(
        b"d3dcompiler_47.dll\0".as_ptr(),
        ptr::null_mut(),
        LOAD_LIBRARY_SEARCH_SYSTEM32,
    );
    if module.is_null() {
        return None;
    }
    let proc = crate::GetProcAddress(module, b"D3DCreateBlob\0".as_ptr());
    if proc.is_null() {
        None
    } else {
        Some(mem::transmute::<*mut c_void, CreateBlob>(proc))
    }
});

pub(crate) struct Sites {
    pub create: usize,
    map: usize,
}

static SITES: OnceLock<Option<Sites>> = OnceLock::new();

fn parse_sites(text: &str, stamp: u32, base: usize, image_size: usize) -> Option<Sites> {
    let mut fields = text.split(':');
    let mut hex = || {
        let field = fields.next()?;
        usize::from_str_radix(field.strip_prefix("0x").unwrap_or(field), 16).ok()
    };
    let (build, create, map) = (hex()?, hex()?, hex()?);
    if fields.next().is_some()
        || build != stamp as usize
        || create == map
        || !(1..image_size).contains(&create)
        || !(1..image_size).contains(&map)
    {
        return None;
    }
    Some(Sites {
        create: base.checked_add(create)?,
        map: base.checked_add(map)?,
    })
}

pub(crate) fn sites() -> Option<&'static Sites> {
    SITES
        .get_or_init(|| {
            let text = compat_common::cfg("zero_init_map")?;
            // SAFETY: the loader has mapped and validated the current executable's PE headers.
            let result = unsafe {
                let base = GetModuleHandleW(ptr::null());
                if base.is_null() {
                    return None;
                }
                let pe = base.add(base.add(0x3c).cast::<u32>().read_unaligned() as usize);
                let stamp = pe.add(8).cast::<u32>().read_unaligned();
                let size = pe.add(24 + 56).cast::<u32>().read_unaligned() as usize;
                parse_sites(&text, stamp, base as usize, size)
            };
            if result.is_none() {
                compat_common::record(
                    "d3d12_xs",
                    "zero_init_map",
                    "invalid call sites or executable build mismatch",
                );
            }
            result
        })
        .as_ref()
}

// Inline into each intercepted COM entry point, so skipping its frame yields the title caller.
#[inline(always)]
pub(crate) fn caller() -> usize {
    let mut pc = 0;
    unsafe { RtlCaptureStackBackTrace(1, 1, &mut pc, ptr::null_mut()) };
    pc
}

pub(crate) const CREATE_NOT_ZEROED: u32 = 0x1000;

pub(crate) fn gpu_only(heap_type: u32, cpu_page_property: u32) -> bool {
    heap_type == 1 || (heap_type == 4 && cpu_page_property == 1)
}

// Private to this DLL. The resource owns the eligibility record and any active scratch blob.
const READY: Guid = Guid {
    d1: 0x1055a480,
    d2: 0xfac1,
    d3: 0x4c54,
    d4: [0x84, 0x0d, 0x6c, 0x4f, 0xb8, 0x12, 0x9f, 0x2b],
};
const MAPPED: Guid = Guid {
    d1: 0xc903e67c,
    d2: 0x9df6,
    d3: 0x4d63,
    d4: [0xba, 0x6b, 0x31, 0xd4, 0xcf, 0xf2, 0x27, 0xf9],
};
const SLOT_MAP: usize = 8;
const SLOT_UNMAP: usize = 9;
type MapFn = unsafe extern "system" fn(*mut c_void, u32, *const c_void, *mut *mut c_void) -> i32;
type UnmapFn = unsafe extern "system" fn(*mut c_void, u32, *const c_void);
type Originals = (MapFn, UnmapFn);
static ORIGINALS: LazyLock<Mutex<HashMap<usize, Originals>>> = LazyLock::new(Default::default);

unsafe fn table(obj: *mut c_void) -> *mut usize {
    *obj.cast::<*mut usize>()
}

unsafe fn vslot(obj: *mut c_void, slot: usize) -> usize {
    *table(obj).add(slot)
}

unsafe fn originals(res: *mut c_void) -> Originals {
    ORIGINALS.lock().unwrap()[&(table(res) as usize)]
}

unsafe fn patch(res: *mut c_void) -> bool {
    let vt = table(res);
    let mut tables = ORIGINALS.lock().unwrap();
    if tables.contains_key(&(vt as usize)) {
        return true;
    }
    let slots = vt.add(SLOT_MAP);
    let mut old = 0;
    let len = 2 * mem::size_of::<usize>();
    if VirtualProtect(slots.cast(), len, 4, &mut old) == 0 {
        return false;
    }
    tables.insert(
        vt as usize,
        (mem::transmute(*slots), mem::transmute(*slots.add(1))),
    );
    AtomicUsize::from_ptr(slots).store(map as *const () as usize, Ordering::Release);
    AtomicUsize::from_ptr(slots.add(1)).store(unmap as *const () as usize, Ordering::Release);
    let mut unused = 0;
    VirtualProtect(slots.cast(), len, old, &mut unused);
    true
}

unsafe fn get_private<T>(res: *mut c_void, key: &Guid, value: &mut T) -> bool {
    let f: unsafe extern "system" fn(*mut c_void, &Guid, *mut u32, *mut c_void) -> i32 =
        mem::transmute(vslot(res, 3));
    let mut n = mem::size_of::<T>() as u32;
    f(res, key, &mut n, ptr::from_mut(value).cast()) >= 0 && n == mem::size_of::<T>() as u32
}

unsafe fn set_data(res: *mut c_void, key: &Guid, len: u32, data: *const c_void) -> i32 {
    let f: unsafe extern "system" fn(*mut c_void, &Guid, u32, *const c_void) -> i32 =
        mem::transmute(vslot(res, 4));
    f(res, key, len, data)
}

unsafe fn release(obj: *mut c_void) -> u32 {
    let f: unsafe extern "system" fn(*mut c_void) -> u32 = mem::transmute(vslot(obj, 2));
    f(obj)
}

unsafe fn blob_ptr(blob: *mut c_void) -> *mut u8 {
    let f: unsafe extern "system" fn(*mut c_void) -> *mut u8 = mem::transmute(vslot(blob, 3));
    f(blob)
}

pub(crate) unsafe fn arm(res: *mut c_void, bytes: usize) {
    if res.is_null() || !patch(res) {
        return;
    }
    let ready = [bytes, GetCurrentThreadId() as usize];
    set_data(
        res,
        &READY,
        mem::size_of_val(&ready) as u32,
        ready.as_ptr().cast(),
    );
}

unsafe extern "system" fn map(
    res: *mut c_void,
    sub: u32,
    range: *const c_void,
    out: *mut *mut c_void,
) -> i32 {
    let (native, _) = originals(res);
    let mut ready = [0usize; 2];
    if !get_private(res, &READY, &mut ready) {
        return native(res, sub, range, out);
    }
    // Only the creating thread may use the marker. Any other first Map retires it.
    let pc = caller();
    if set_data(res, &READY, 0, ptr::null()) < 0
        || GetCurrentThreadId() as usize != ready[1]
        || sub != 0
        || !range.is_null()
        || out.is_null()
        || !sites().is_some_and(|sites| sites.map == pc)
    {
        return native(res, sub, range, out);
    }
    *out = ptr::null_mut();
    let mut blob: *mut c_void = ptr::null_mut();
    let Some(create_blob) = *CREATE_BLOB else {
        return compat_common::E_NOTIMPL;
    };
    let hr = create_blob(ready[0], &mut blob);
    if hr < 0 {
        return hr;
    }
    let attach: unsafe extern "system" fn(*mut c_void, &Guid, *mut c_void) -> i32 =
        mem::transmute(vslot(res, 5));
    let hr = attach(res, &MAPPED, blob);
    if hr >= 0 {
        let bytes = blob_ptr(blob);
        bytes.write_bytes(0, ready[0]);
        *out = bytes.cast();
    }
    release(blob); // the resource holds its own reference until Unmap or destruction
    hr
}

unsafe extern "system" fn unmap(res: *mut c_void, sub: u32, written: *const c_void) {
    let mut blob: *mut c_void = ptr::null_mut();
    if sub == 0 && get_private(res, &MAPPED, &mut blob) && !blob.is_null() {
        let size: unsafe extern "system" fn(*mut c_void) -> usize = mem::transmute(vslot(blob, 4));
        if core::slice::from_raw_parts(blob_ptr(blob), size(blob))
            .iter()
            .any(|&b| b != 0)
        {
            compat_common::record(
                "d3d12_xs",
                "zero_init_map",
                "configured helper wrote nonzero bytes; no GPU transfer was performed",
            );
        }
        set_data(res, &MAPPED, 0, ptr::null());
        release(blob); // GetPrivateData added a reference
    } else {
        originals(res).1(res, sub, written);
    }
}

#[cfg(test)]
mod tests;
