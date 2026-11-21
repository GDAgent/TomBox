//! `XCRDAPI.dll` compatibility shim — Xbox DirectStorage entry `DStorageGetFactory`.
//!
//! The Xbox IoStore backend (`FXSXFileIoStoreImpl`) calls
//! `DStorageGetFactory(IID_IDStorageFactory_Xbox, &factory)` and dereferences the
//! result. We forward to the real PC DirectStorage (`dstorage.dll`, staged beside
//! the EXE). The Xbox factory IID `{fb3b54ac-bf82-463a-b777-73195837864d}` differs
//! from the PC IID `{6924ea0c-c3cd-4826-b10a-f64f4ed927c1}`, so we substitute the
//! PC IID when asking the real DirectStorage, then hand the PC factory back. This
//! works iff the two `IDStorageFactory` vtables are layout-compatible — if a later
//! factory method mismatches, the crash site will show which method and we build a
//! proper translating shim for it.

use core::ffi::c_void;
use core::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Once;
use compat_common::{record, E_NOTIMPL};

const M: &str = "xcrdapi";

type HMODULE = *mut c_void;
extern "system" {
    fn LoadLibraryW(p: *const u16) -> HMODULE;
    fn GetProcAddress(h: HMODULE, n: *const u8) -> *mut c_void;
    fn VirtualProtect(addr: *mut c_void, size: usize, new_protect: u32, old_protect: *mut u32) -> i32;
    fn CreateFileW(name: *const u16, access: u32, share: u32, sa: *mut c_void, disp: u32, flags: u32, tmpl: *mut c_void) -> isize;
    fn ReadFile(h: isize, buf: *mut u8, n: u32, read: *mut u32, ov: *mut u8) -> i32;
    fn GetLastError() -> u32;
}
const PAGE_READWRITE: u32 = 0x04;
const GENERIC_READ: u32 = 0x8000_0000;
const FILE_SHARE_READ: u32 = 0x1;
const OPEN_EXISTING: u32 = 3;
const INVALID_HANDLE: isize = -1;

/// Map our returned IDStorageFile* -> an OS file handle we open alongside it, so we
/// can service DirectStorage reads ourselves (DStorageCore's own file read path
/// faults on the Xbox-origin requests even once correctly translated).
static FILE_MAP: std::sync::Mutex<Vec<(usize, isize)>> = std::sync::Mutex::new(Vec::new());

fn map_file(file_ptr: usize, os: isize) {
    if let Ok(mut m) = FILE_MAP.lock() { m.push((file_ptr, os)); }
}
fn lookup_file(file_ptr: usize) -> Option<isize> {
    FILE_MAP.lock().ok()?.iter().find(|(p, _)| *p == file_ptr).map(|(_, h)| *h)
}

/// PC `IID_IDStorageFactory` = {6924ea0c-c3cd-4826-b10a-f64f4ed927c1}, LE layout.
static PC_IID_IDSTORAGEFACTORY: [u8; 16] = [
    0x0c, 0xea, 0x24, 0x69, // Data1 = 0x6924EA0C
    0xcd, 0xc3, // Data2 = 0xC3CD
    0x26, 0x48, // Data3 = 0x4826
    0xb1, 0x0a, 0xf6, 0x4f, 0x4e, 0xd9, 0x27, 0xc1, // Data4
];

/// PC `IID_IDStorageFile` = {5de95e7b-955a-4868-a73c-243b29f4b8da}, LE layout.
static PC_IID_IDSTORAGEFILE: [u8; 16] = [
    0x7b, 0x5e, 0xe9, 0x5d, // Data1 = 0x5DE95E7B
    0x5a, 0x95, // Data2 = 0x955A
    0x68, 0x48, // Data3 = 0x4868
    0xa7, 0x3c, 0x24, 0x3b, 0x29, 0xf4, 0xb8, 0xda, // Data4
];

/// PC `IID_IDStorageQueue` = {cfdbd83f-9e06-4fda-8ea5-69042137f49b}, LE layout.
static PC_IID_IDSTORAGEQUEUE: [u8; 16] = [
    0x3f, 0xd8, 0xdb, 0xcf, // Data1 = 0xCFDBD83F
    0x06, 0x9e, // Data2 = 0x9E06
    0xda, 0x4f, // Data3 = 0x4FDA
    0x8e, 0xa5, 0x69, 0x04, 0x21, 0x37, 0xf4, 0x9b, // Data4
];

/// PC `IID_IDStorageStatusArray` = {82397587-7cd5-453b-a02e-31379bd64656}, LE.
static PC_IID_IDSTORAGESTATUSARRAY: [u8; 16] = [
    0x87, 0x75, 0x39, 0x82, // Data1 = 0x82397587
    0xd5, 0x7c, // Data2 = 0x7CD5
    0x3b, 0x45, // Data3 = 0x453B
    0xa0, 0x2e, 0x31, 0x37, 0x9b, 0xd6, 0x46, 0x56, // Data4
];

static ORIG_OPENFILE: AtomicUsize = AtomicUsize::new(0);
static ORIG_CREATEQUEUE: AtomicUsize = AtomicUsize::new(0);
static ORIG_CREATESTATUS: AtomicUsize = AtomicUsize::new(0);

/// Factory slot 5 = CreateStatusArray(this, UINT32 capacity, const CHAR* name,
/// REFIID riid, void** ppv). Substitute the PC IDStorageStatusArray IID so the
/// completion-tracking status array isn't null (EnqueueStatus fail-fasts otherwise).
unsafe extern "system" fn create_status_array_shim(
    this: *mut c_void, capacity: u32, name: *const u8, _riid: *const c_void, ppv: *mut *mut c_void,
) -> i32 {
    let orig: unsafe extern "system" fn(*mut c_void, u32, *const u8, *const c_void, *mut *mut c_void) -> i32 =
        unsafe { core::mem::transmute(ORIG_CREATESTATUS.load(Ordering::Relaxed)) };
    let hr = unsafe { orig(this, capacity, name, PC_IID_IDSTORAGESTATUSARRAY.as_ptr() as *const c_void, ppv) };
    let out = if ppv.is_null() { core::ptr::null_mut() } else { unsafe { *ppv } };
    record(M, "CreateStatusArray", &format!("cap={capacity} (PC IID) -> hr={hr:#010x} arr={out:p}"));
    hr
}
static ENQUEUE_DUMPS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
static ORIG_ENQUEUE: AtomicUsize = AtomicUsize::new(0);
static LAST_OPENED_FILE: AtomicUsize = AtomicUsize::new(0);

/// Queue slot 3 = EnqueueRequest(this, const DSTORAGE_REQUEST*).
///
/// The Xbox DSTORAGE_REQUEST orders Destination before Source (reverse of PC), so
/// forwarding it verbatim fail-fasts E_INVALIDARG. Reconstructed Xbox layout:
///   @0x00 Options(=0: File src, Memory dst, no compression)
///   @0x08 Dest.Buffer   @0x18 Dest.Size
///   @0x20 Src.File      @0x28 Src.Offset   @0x30 Src.Size
/// PC layout (dstorage.h):
///   @0x00 Options  @0x08 Src.File @0x10 Src.Offset @0x18 Src.Size(u32)
///   @0x20 Dst.Buffer @0x28 Dst.Size(u32)  @0x48 UncompressedSize  @0x50 Tag  @0x58 Name
unsafe extern "system" fn enqueue_shim(this: *mut c_void, preq: *const u8) {
    if preq.is_null() {
        return;
    }
    let x = unsafe { core::slice::from_raw_parts(preq, 0x40) };
    let rd = |o: usize| u64::from_le_bytes(x[o..o + 8].try_into().unwrap());
    let (options, dst_buf, dst_size, src_file, src_off, src_size) =
        (rd(0x00), rd(0x08), rd(0x18), rd(0x20), rd(0x28), rd(0x30));
    let _ = this;
    // Options byte 0 = CompressionFormat (0=None). If nonzero, Xbox DirectStorage
    // hardware-decompresses during the read; our raw read would leave it compressed.
    let compression = (options & 0xFF) as u8;
    if ENQUEUE_DUMPS.load(Ordering::Relaxed) < 400 {
        record(M, "EnqueueRequest.opt",
            &format!("options={options:#x} comp={compression} off={src_off:#x} size={src_size:#x} dst={dst_buf:#x}"));
    }

    // Service the read directly from the OS file we opened in OpenFile. (The real
    // DStorageCore read path faults on these requests; the game's Submit still
    // fires its status/fence on the real queue, so completion signals normally with
    // the data already written here.)
    let os = lookup_file(src_file as usize);
    let n = ENQUEUE_DUMPS.fetch_add(1, Ordering::Relaxed);
    match os {
        Some(h) if dst_buf > 0x10000 && src_size > 0 => {
            let (Ok(read_size), Ok(capacity)) = (u32::try_from(src_size), u32::try_from(dst_size)) else {
                record(M, "EnqueueRequest", "unsupported file or destination size");
                return;
            };
            if capacity == 0 || (compression == 0 && read_size > capacity) {
                record(M, "EnqueueRequest", "file request exceeds destination capacity");
                return;
            }
            let mut ov = [0usize; 4]; // x64 OVERLAPPED, including aligned Offset/OffsetHigh.
            ov[2] = src_off as usize;
            let mut compressed = Vec::new();
            let output = if compression == 0 {
                dst_buf as *mut u8
            } else {
                if compressed.try_reserve_exact(read_size as usize).is_err() {
                    record(M, "EnqueueRequest", "compressed input allocation failed");
                    return;
                }
                compressed.resize(read_size as usize, 0u8);
                compressed.as_mut_ptr()
            };
            let mut got = 0;
            // SAFETY: the handle belongs to the synchronous file map. For a raw read,
            // read_size fits the caller's destination. Compressed input uses its own buffer.
            let ok = unsafe { ReadFile(h, output, read_size, &mut got, ov.as_mut_ptr().cast()) };
            if ok == 0 || got != read_size {
                record(M, "EnqueueRequest", &format!("BAD read off={src_off:#x} size={read_size:#x} got={got:#x}"));
            } else if compression != 0 {
                match miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(&compressed, capacity as usize) {
                    Ok(data) => {
                        // SAFETY: the decoder caps output at capacity and owns a separate buffer.
                        unsafe { core::ptr::copy_nonoverlapping(data.as_ptr(), dst_buf as *mut u8, data.len()) };
                        if n < 6 {
                            record(M, "EnqueueRequest", &format!("INFLATE comp={compression} off={src_off:#x} size={:#x}", data.len()));
                        }
                    }
                    Err(e) => record(M, "EnqueueRequest", &format!("zlib decode failed comp={compression} status={:?}", e.status)),
                }
            }
        }
        _ => {
            if n < 8 {
                record(M, "EnqueueRequest", &format!("UNSERVICED file={src_file:#x} off={src_off:#x} size={src_size:#x} dst={dst_buf:#x} (no os handle)"));
            }
        }
    }
    // no forward: we serviced the read ourselves.
}

/// Factory slot 3 = CreateQueue(this, const DSTORAGE_QUEUE_DESC*, REFIID, void** ppv).
/// Substitute the PC IDStorageQueue IID so the async IoStore read queue isn't null.
unsafe extern "system" fn create_queue_shim(
    this: *mut c_void, pdesc: *const c_void, _riid: *const c_void, ppv: *mut *mut c_void,
) -> i32 {
    let orig: unsafe extern "system" fn(*mut c_void, *const c_void, *const c_void, *mut *mut c_void) -> i32 =
        unsafe { core::mem::transmute(ORIG_CREATEQUEUE.load(Ordering::Relaxed)) };
    // The PC DSTORAGE_QUEUE_DESC is { SourceType u64; Capacity u16; Priority i8; Name char*;
    // ID3D12Device* Device } (0x20 bytes). The title's Xbox desc carries only the first
    // three members (observed: SourceType 0, Capacity 0x2000, Priority 0/2, Name null); the
    // qword where PC expects `Device` is whatever follows on the title's stack. Passing the
    // desc through made DirectStorage treat stack garbage as a device (queue creation then
    // failed with a nonsense HRESULT whenever that garbage was non-zero). Build a PC desc
    // from the first 0x18 bytes with no device: these are file-to-memory queues.
    let mut pc_desc = [0u8; 0x20];
    let desc_ptr = if pdesc.is_null() {
        pdesc
    } else {
        unsafe { core::ptr::copy_nonoverlapping(pdesc as *const u8, pc_desc.as_mut_ptr(), 0x18) };
        pc_desc.as_ptr() as *const c_void
    };
    let hr = unsafe { orig(this, desc_ptr, PC_IID_IDSTORAGEQUEUE.as_ptr() as *const c_void, ppv) };
    let out = if ppv.is_null() { core::ptr::null_mut() } else { unsafe { *ppv } };
    // Patch the new queue's vtable slot 3 (EnqueueRequest) once to dump requests.
    if hr == 0 && !out.is_null() {
        static Q_PATCHED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
        if !Q_PATCHED.swap(true, Ordering::Relaxed) {
            unsafe {
                let qvt = *(out as *const *mut usize);
                let slot = qvt.add(3);
                ORIG_ENQUEUE.store(*slot, Ordering::Relaxed);
                let mut old: u32 = 0;
                if VirtualProtect(slot as *mut c_void, 8, PAGE_READWRITE, &mut old) != 0 {
                    *slot = enqueue_shim as usize;
                    let mut tmp: u32 = 0;
                    VirtualProtect(slot as *mut c_void, 8, old, &mut tmp);
                    record(M, "patch_queue", &format!("queue vtable {qvt:p} slot 3 EnqueueRequest -> Xbox->PC translation shim"));
                }
            }
        }
    }
    record(M, "CreateQueue", &format!("(PC queue IID) -> hr={hr:#010x} queue={out:p}"));
    hr
}

/// Factory slot 4 = OpenFile(this, PCWSTR path, REFIID, void** ppv). The Xbox
/// IoStore passes the Xbox IDStorageFile IID, which PC OpenFile rejects; swap it
/// for the PC IID so the real DirectStorage hands back an IDStorageFile.
unsafe extern "system" fn open_file_shim(
    this: *mut c_void, path: *const u16, _riid: *const c_void, ppv: *mut *mut c_void,
) -> i32 {
    let orig: unsafe extern "system" fn(*mut c_void, *const u16, *const c_void, *mut *mut c_void) -> i32 =
        unsafe { core::mem::transmute(ORIG_OPENFILE.load(Ordering::Relaxed)) };
    let hr = unsafe { orig(this, path, PC_IID_IDSTORAGEFILE.as_ptr() as *const c_void, ppv) };
    let out = if ppv.is_null() { core::ptr::null_mut() } else { unsafe { *ppv } };
    if hr == 0 && !out.is_null() {
        LAST_OPENED_FILE.store(out as usize, Ordering::Relaxed);
        // Open the same file via Win32 so we can service reads ourselves.
        if !path.is_null() {
            let os = unsafe { CreateFileW(path, GENERIC_READ, FILE_SHARE_READ, core::ptr::null_mut(), OPEN_EXISTING, 0, core::ptr::null_mut()) };
            if os != INVALID_HANDLE {
                map_file(out as usize, os);
            } else {
                record(M, "OpenFile", &format!("CreateFileW failed err={}", unsafe { GetLastError() }));
            }
        }
    }
    record(M, "OpenFile", &format!("(PC file IID) -> hr={hr:#010x} file={out:p}"));
    hr
}

/// Patch the real factory's vtable slot 4 (OpenFile) once.
fn patch_factory(factory: *mut c_void) {
    if factory.is_null() || ORIG_OPENFILE.load(Ordering::Relaxed) != 0 {
        return;
    }
    unsafe {
        let vtbl = *(factory as *const *mut usize);
        // slot 3 = CreateQueue, slot 4 = OpenFile, slot 5 = CreateStatusArray.
        ORIG_CREATEQUEUE.store(*vtbl.add(3), Ordering::Relaxed);
        ORIG_OPENFILE.store(*vtbl.add(4), Ordering::Relaxed);
        ORIG_CREATESTATUS.store(*vtbl.add(5), Ordering::Relaxed);
        let addr = vtbl.add(3) as *mut c_void;
        let mut old: u32 = 0;
        if VirtualProtect(addr, 24, PAGE_READWRITE, &mut old) != 0 {
            *vtbl.add(3) = create_queue_shim as usize;
            *vtbl.add(4) = open_file_shim as usize;
            *vtbl.add(5) = create_status_array_shim as usize;
            let mut tmp: u32 = 0;
            VirtualProtect(addr, 24, old, &mut tmp);
            record(M, "patch_factory", &format!("vtable {vtbl:p} slots 3/4/5 (CreateQueue/OpenFile/CreateStatusArray) -> IID shims"));
        }
    }
}

static REAL: AtomicUsize = AtomicUsize::new(0);
static INIT: Once = Once::new();

fn real_get_factory() -> usize {
    INIT.call_once(|| {
        let name: Vec<u16> = "dstorage.dll\0".encode_utf16().collect();
        let h = unsafe { LoadLibraryW(name.as_ptr()) };
        let p = if h.is_null() {
            0
        } else {
            unsafe { GetProcAddress(h, b"DStorageGetFactory\0".as_ptr()) as usize }
        };
        record(M, "init", &format!("real dstorage.dll DStorageGetFactory = {p:#x} (0 = not found)"));
        REAL.store(p, Ordering::Relaxed);
    });
    REAL.load(Ordering::Relaxed)
}

#[no_mangle]
pub extern "system" fn DStorageGetFactory(_riid: *const c_void, ppv: *mut *mut c_void) -> i32 {
    let p = real_get_factory();
    if p == 0 {
        record(M, "DStorageGetFactory", "[real dstorage.dll not available -> E_NOTIMPL]");
        return E_NOTIMPL;
    }
    // Ask the real DirectStorage for a *PC* IDStorageFactory (ignore the Xbox riid).
    let f: extern "system" fn(*const c_void, *mut *mut c_void) -> i32 = unsafe { core::mem::transmute(p) };
    let hr = f(PC_IID_IDSTORAGEFACTORY.as_ptr() as *const c_void, ppv);
    let out = if ppv.is_null() { core::ptr::null_mut() } else { unsafe { *ppv } };
    if hr == 0 {
        patch_factory(out);
    }
    record(M, "DStorageGetFactory", &format!("forwarded (PC IID) -> hr={hr:#010x} factory={out:p}"));
    hr
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::os::windows::io::AsRawHandle;

    #[test]
    fn positioned_file_reads_match_plain_and_zlib_payloads() {
        let plain = b"TuxBox storage fixture";
        let compressed = miniz_oxide::deflate::compress_to_vec_zlib(plain, 6);
        let path = std::env::temp_dir().join(format!("tuxbox-storage-{}.bin", std::process::id()));
        std::fs::write(&path, [plain.as_slice(), &compressed].concat()).unwrap();
        let file = std::fs::File::open(&path).unwrap();
        let token = file.as_raw_handle() as usize;
        map_file(token, token as isize);
        for (format, offset, length) in [(0, 0, plain.len()), (1, plain.len(), compressed.len())] {
            let mut result = [0xa5u8; 32];
            let mut request = [0u64; 8];
            request[0] = format;
            request[1] = result.as_mut_ptr() as u64;
            request[3] = plain.len() as u64;
            request[4] = token as u64;
            request[5] = offset as u64;
            request[6] = length as u64;
            // SAFETY: all request storage and the mapped file live through the synchronous call.
            unsafe { enqueue_shim(core::ptr::null_mut(), request.as_ptr().cast()) };
            assert_eq!(&result[..plain.len()], plain);
            assert!(result[plain.len()..].iter().all(|&value| value == 0xa5));

            // The destination view can be smaller than its backing allocation.
            result.fill(0xa5);
            request[3] = (plain.len() - 1) as u64;
            unsafe { enqueue_shim(core::ptr::null_mut(), request.as_ptr().cast()) };
            assert_eq!(result, [0xa5; 32]);
        }
        FILE_MAP.lock().unwrap().retain(|(key, _)| *key != token);
        drop(file);
        std::fs::remove_file(path).unwrap();
    }
}
