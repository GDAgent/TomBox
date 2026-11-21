use super::*;
use std::hint::black_box;

const IID_DEVICE: Guid = Guid {
    d1: 0x189819f1,
    d2: 0x1db6,
    d3: 0x4b57,
    d4: [0xbe, 0x54, 0x18, 0x21, 0x33, 0x9b, 0x85, 0xf7],
};
const IID_RESOURCE: Guid = Guid {
    d1: 0x696442be,
    d2: 0xa72e,
    d3: 0x4059,
    d4: [0xbc, 0x79, 0x5b, 0x5c, 0x98, 0x04, 0x0f, 0xad],
};

#[test]
fn require_exact_build_and_valid_distinct_sites() {
    let sites = parse_sites("123:0x20:40", 0x123, 0x1000, 0x100).unwrap();
    assert_eq!((sites.create, sites.map), (0x1020, 0x1040));
    for invalid in [
        "124:20:40",
        "123:20:20",
        "123:20:100",
        "123:0:40",
        "123:20",
        "123:20:40:60",
    ] {
        assert!(parse_sites(invalid, 0x123, 0x1000, 0x100).is_none());
    }
}

#[test]
fn only_non_cpu_visible_heaps_are_eligible() {
    assert!(gpu_only(1, 0));
    assert!(gpu_only(4, 1));
    assert!(!gpu_only(2, 0));
    assert!(!gpu_only(3, 0));
    assert!(!gpu_only(4, 3));
}

type CreateFn = unsafe extern "system" fn(
    *mut c_void,
    *const u32,
    u32,
    *const u8,
    u32,
    *const c_void,
    *const Guid,
    *mut *mut c_void,
) -> i32;

// Discover the two return addresses of the synthetic title helper under ASLR.
unsafe extern "system" fn create_pc(
    _: *mut c_void,
    _: *const u32,
    _: u32,
    _: *const u8,
    _: u32,
    _: *const c_void,
    _: *const Guid,
    out: *mut *mut c_void,
) -> i32 {
    *out = caller() as *mut c_void;
    0
}

unsafe extern "system" fn map_pc(
    _: *mut c_void,
    _: u32,
    _: *const c_void,
    out: *mut *mut c_void,
) -> i32 {
    *out = caller() as *mut c_void;
    0
}

#[inline(never)]
unsafe fn title_create(
    f: CreateFn,
    device: *mut c_void,
    heap: &[u32; 5],
    desc: &[u8; 56],
    out: &mut *mut c_void,
) -> i32 {
    black_box(black_box(f)(
        device,
        heap.as_ptr(),
        0,
        desc.as_ptr(),
        4,
        ptr::null(),
        &IID_RESOURCE,
        out,
    ))
}

#[inline(never)]
unsafe fn title_map(f: MapFn, res: *mut c_void, out: &mut *mut c_void) -> i32 {
    black_box(black_box(f)(res, 0, ptr::null(), out))
}

#[test]
fn zero_init_helper_on_real_device() {
    extern "system" {
        fn LoadLibraryA(n: *const u8) -> *mut c_void;
        fn GetProcAddress(h: *mut c_void, n: *const u8) -> *mut c_void;
    }
    unsafe {
        let d3d12 = LoadLibraryA(b"d3d12.dll\0".as_ptr());
        let create = if d3d12.is_null() {
            core::ptr::null_mut()
        } else {
            GetProcAddress(d3d12, b"D3D12CreateDevice\0".as_ptr())
        };
        if create.is_null() {
            eprintln!("skipped: no d3d12.dll");
            return;
        }
        let create: unsafe extern "system" fn(
            *mut c_void,
            u32,
            *const Guid,
            *mut *mut c_void,
        ) -> i32 = core::mem::transmute(create);
        let mut dev = core::ptr::null_mut();
        if create(core::ptr::null_mut(), 0xb000, &IID_DEVICE, &mut dev) < 0 {
            eprintln!("skipped: no D3D12 device");
            return;
        }
        // DEFAULT heap, RGBA8 1904x1041 render target (the shape seen in the title).
        let heap = [1u32, 0, 0, 1, 1];
        let mut desc = [0u8; 0x38];
        desc[0..4].copy_from_slice(&3u32.to_le_bytes());
        desc[0x10..0x18].copy_from_slice(&1904u64.to_le_bytes());
        desc[0x18..0x1c].copy_from_slice(&1041u32.to_le_bytes());
        desc[0x1c..0x1e].copy_from_slice(&1u16.to_le_bytes());
        desc[0x1e..0x20].copy_from_slice(&1u16.to_le_bytes());
        desc[0x20..0x24].copy_from_slice(&28u32.to_le_bytes());
        desc[0x24..0x28].copy_from_slice(&1u32.to_le_bytes());
        desc[0x30..0x34].copy_from_slice(&1u32.to_le_bytes()); // ALLOW_RENDER_TARGET

        let wrapper = crate::device_wrap::wrap_device(dev);
        let create: CreateFn = mem::transmute(vslot(wrapper, 27));
        let mut p = ptr::null_mut();
        let mut res = ptr::null_mut();
        title_create(create_pc, wrapper, &heap, &desc, &mut res);
        title_map(map_pc, res, &mut p);
        assert!(SITES
            .set(Some(Sites {
                create: res as usize,
                map: p as usize
            }))
            .is_ok());

        for abandon in [false, true] {
            assert_eq!(title_create(create, wrapper, &heap, &desc, &mut res), 0);
            let map: MapFn = mem::transmute(vslot(res, SLOT_MAP));
            assert_eq!(title_map(map, res, &mut p), 0);
            let mut blob: *mut c_void = ptr::null_mut();
            assert!(get_private(res, &MAPPED, &mut blob));
            let size: unsafe extern "system" fn(*mut c_void) -> usize =
                mem::transmute(vslot(blob, 4));
            assert_eq!(size(blob), 0x870000);
            p.cast::<u8>().write_bytes(0, size(blob));
            if !abandon {
                let unmap: UnmapFn = mem::transmute(vslot(res, SLOT_UNMAP));
                unmap(res, 0, ptr::null());
                assert!(title_map(map, res, &mut p) < 0, "one shot only");
            }
            release(res);
            assert_eq!(
                release(blob),
                0,
                "resource destruction releases a persistent mapping"
            );
        }

        // Same descriptor, wrong creation site. Even the authorized Map must fail.
        assert_eq!(
            create(
                wrapper,
                heap.as_ptr(),
                0,
                desc.as_ptr(),
                4,
                ptr::null(),
                &IID_RESOURCE,
                &mut res
            ),
            0
        );
        let map: MapFn = mem::transmute(vslot(res, SLOT_MAP));
        assert!(title_map(map, res, &mut p) < 0);
        release(res);

        // Correct creation site, wrong Map site. No later retry can claim freshness.
        assert_eq!(title_create(create, wrapper, &heap, &desc, &mut res), 0);
        let map: MapFn = mem::transmute(vslot(res, SLOT_MAP));
        assert!(map(res, 0, ptr::null(), &mut p) < 0);
        assert!(title_map(map, res, &mut p) < 0);
        release(res);
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
        assert!(
            create(
                wrapper,
                up.as_ptr(),
                0,
                bdesc.as_ptr(),
                0xac3, /* GENERIC_READ */
                core::ptr::null(),
                &IID_RESOURCE,
                &mut buf
            ) >= 0
        );
        let map_fn: MapFn = mem::transmute(vslot(buf, SLOT_MAP));
        assert_eq!(map_fn(buf, 0, core::ptr::null(), &mut p), 0);
        p.cast::<u32>().write(0xdeadbeef);
        let unmap_fn: UnmapFn = mem::transmute(vslot(buf, SLOT_UNMAP));
        unmap_fn(buf, 0, core::ptr::null());

        assert_eq!(map_fn(buf, 0, ptr::null(), &mut p), 0);
        assert_eq!(p.cast::<u32>().read(), 0xdeadbeef);
        unmap_fn(buf, 0, ptr::null());
        release(buf);
        release(wrapper);
    }
}
