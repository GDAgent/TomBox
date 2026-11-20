//! Identify the caller's GXDK edition for command-list slots with conflicting ABIs.
//! A DLL can issue commands on a list created by another module.

use core::ffi::c_void;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Edition {
    Gxdk250405,
    AprilUpdate2,
    October,
    Unknown,
    Unresolved,
}

/// GXDK stores NUL-delimited ASCII fields. Sections can span pages and a tag
/// can straddle two reads. Conflicting edition tags cannot select a safe ABI.
fn from_xbld(size: usize, mut read: impl FnMut(usize, &mut [u8]) -> bool) -> Edition {
    const PREFIX: &[u8] = b"_xbld_edition_sdktype=GXDK,";
    let mut field = [0; PREFIX.len() + 6];
    let mut length = 0usize;
    let mut edition = None;
    let mut chunk = [0; 512];
    for start in (0..size).step_by(chunk.len()) {
        let count = (size - start).min(chunk.len());
        let bytes = &mut chunk[..count];
        if !read(start, bytes) {
            return Edition::Unresolved;
        }
        for &byte in bytes.iter() {
            if byte != 0 {
                if let Some(dest) = field.get_mut(length) {
                    *dest = byte;
                }
                length += 1;
                continue;
            }
            if length >= PREFIX.len() && field.starts_with(PREFIX) {
                let tag = match field.get(PREFIX.len()..length) {
                    Some(b"250405") => Edition::Gxdk250405,
                    Some(b"260402") => Edition::AprilUpdate2,
                    Some(b"261000") => Edition::October,
                    _ => Edition::Unknown,
                };
                if edition.is_some_and(|previous| previous != tag) {
                    return Edition::Unresolved;
                }
                edition = Some(tag);
            }
            length = 0;
        }
    }
    edition.unwrap_or(Edition::Unknown)
}

#[link(name = "ntdll")]
extern "system" {
    fn RtlPcToFileHeader(pc: usize, base: *mut *const u8) -> *const u8;
    fn RtlImageNtHeader(base: *const u8) -> *const u8;
}

extern "system" {
    fn ReadProcessMemory(
        process: isize,
        address: *const u8,
        buffer: *mut c_void,
        len: usize,
        read: *mut usize,
    ) -> i32;
}

/// The caller is executing inside the image, so its loader-owned headers stay
/// mapped across this call. Read only the small .xbld section. Other sections
/// of a loaded image can be discardable or inaccessible.
pub(crate) fn at_address(pc: usize) -> Edition {
    let mut base = core::ptr::null();
    unsafe {
        if RtlPcToFileHeader(pc, &mut base).is_null() {
            return Edition::Unknown;
        }
        let nt = RtlImageNtHeader(base);
        if nt.is_null() {
            return Edition::Unknown;
        }
        let read16 = |p: *const u8| p.cast::<u16>().read_unaligned() as usize;
        let read32 = |p: *const u8| p.cast::<u32>().read_unaligned() as usize;
        let optional = nt.add(24);
        let optional_size = read16(nt.add(20));
        if optional_size < 64 || read16(optional) != 0x20b {
            return Edition::Unknown;
        }
        let count = read16(nt.add(6));
        let image_size = read32(optional.add(56));
        let header_size = read32(optional.add(60));
        let sections = optional.add(optional_size);
        if sections as usize - base as usize + count * 40 > header_size || header_size > image_size
        {
            return Edition::Unknown;
        }
        for i in 0..count {
            let entry = sections.add(i * 40);
            if entry.cast::<[u8; 8]>().read_unaligned() != *b".xbld\0\0\0" {
                continue;
            }
            let size = read32(entry.add(8));
            let rva = read32(entry.add(12));
            if rva + size > image_size {
                return Edition::Unresolved;
            }
            return from_xbld(size, |offset, bytes| {
                let mut read = 0;
                ReadProcessMemory(
                    -1,
                    base.add(rva + offset),
                    bytes.as_mut_ptr().cast(),
                    bytes.len(),
                    &mut read,
                ) != 0
                    && read == bytes.len()
            });
        }
    }
    Edition::Unknown
}

pub(crate) fn caller_location(caller: usize) -> String {
    extern "system" {
        fn GetModuleHandleExW(flags: u32, address: usize, module: *mut usize) -> i32;
        fn GetModuleFileNameW(module: usize, path: *mut u16, size: u32) -> u32;
    }
    let mut base = 0;
    let mut path = [0u16; 512];
    // FROM_ADDRESS | UNCHANGED_REFCOUNT. The caller is still on the stack.
    if unsafe { GetModuleHandleExW(6, caller, &mut base) } == 0 { return format!("{caller:#x}"); }
    let n = unsafe { GetModuleFileNameW(base, path.as_mut_ptr(), path.len() as u32) } as usize;
    if n == 0 || n == path.len() { return format!("{caller:#x}"); }
    let name = String::from_utf16_lossy(&path[..n]);
    format!("{}+{:#x}", name.rsplit(['/', '\\']).next().unwrap_or("?"), caller - base)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(bytes: &[u8]) -> Edition {
        from_xbld(bytes.len(), |start, chunk| {
            chunk.copy_from_slice(&bytes[start..start + chunk.len()]);
            true
        })
    }

    #[test]
    fn only_exact_observed_edition_fields_select_a_layout() {
        assert!(parse(b"_xbld_edition_sdktype=GXDK,250405\0") == Edition::Gxdk250405);
        assert!(parse(b"tag\0_xbld_edition_sdktype=GXDK,260402\0") == Edition::AprilUpdate2);
        assert!(parse(b"_xbld_edition_sdktype=GXDK,261000\0") == Edition::October);
        assert!(parse(b"_xbld_edition_sdktype=GXDK,2610001\0") == Edition::Unknown);
        assert!(parse(b"prefix_xbld_edition_sdktype=GXDK,260402\0") == Edition::Unknown);
    }

    #[test]
    fn edition_crosses_a_chunk_boundary_and_conflicts_do_not_select_a_layout() {
        let mut contents = vec![0; 1530];
        contents.extend_from_slice(b"_xbld_edition_sdktype=GXDK,260402\0");
        assert!(parse(&contents) == Edition::AprilUpdate2);
        contents.extend_from_slice(b"_xbld_edition_sdktype=GXDK,261000\0");
        assert!(parse(&contents) == Edition::Unresolved);
        assert!(from_xbld(200, |_, _| false) == Edition::Unresolved);
    }
}
