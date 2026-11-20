//! Minimal Direct3D 12 compute runner for translator conformance tests (Windows only).
//!
//! Runs one compiled compute shader (DXIL with an embedded root signature consisting of a
//! single root UAV at `u0`) over a buffer of `u32`s and reads the buffer back. Used to compare
//! the HLSL the translator emits for an RDNA2 instruction with a reference model of that
//! instruction, on the host's real GPU and driver.
//!
//! COM is called through raw vtables, as elsewhere in this crate. Slot numbers follow the
//! method order of the `d3d12.h` interfaces (IUnknown 0-2, ID3D12Object 3-6, then the
//! interface's own methods); the device slots 11, 16 and 27 are also used by `xpso` and
//! `d3d12_xs`.

use std::ffi::c_void;
use std::ptr::null_mut;

#[repr(C)]
struct Guid(u32, u16, u16, [u8; 8]);
const IID_DEVICE: Guid = Guid(0x189819f1, 0x1db6, 0x4b57, [0xbe, 0x54, 0x18, 0x21, 0x33, 0x9b, 0x85, 0xf7]);
const IID_QUEUE: Guid = Guid(0x0ec870a6, 0x5d7e, 0x4c22, [0x8c, 0xfc, 0x5b, 0xaa, 0xe0, 0x76, 0x16, 0xed]);
const IID_ALLOCATOR: Guid = Guid(0x6102dee4, 0xaf59, 0x4b09, [0xb9, 0x99, 0xb4, 0x4d, 0x73, 0xf0, 0x9b, 0x24]);
const IID_LIST: Guid = Guid(0x5b160d0f, 0xac1b, 0x4185, [0x8b, 0xa8, 0xb3, 0xae, 0x42, 0xa5, 0xa4, 0x55]);
const IID_FENCE: Guid = Guid(0x0a753dcf, 0xc4d8, 0x4b91, [0xad, 0xf6, 0xbe, 0x5a, 0x60, 0xd9, 0x5a, 0x76]);
const IID_RESOURCE: Guid = Guid(0x696442be, 0xa72e, 0x4059, [0xbc, 0x79, 0x5b, 0x5c, 0x98, 0x04, 0x0f, 0xad]);
const IID_ROOTSIG: Guid = Guid(0xc54a6b66, 0x72df, 0x4ee8, [0x8b, 0xe5, 0xa9, 0x46, 0xa1, 0x42, 0x92, 0x14]);
const IID_PSO: Guid = Guid(0x765a30f3, 0xf624, 0x4c6f, [0xa8, 0x28, 0xac, 0xe9, 0x48, 0x62, 0x24, 0x45]);

extern "system" {
    fn LoadLibraryA(n: *const u8) -> *mut c_void;
    fn GetProcAddress(h: *mut c_void, n: *const u8) -> *mut c_void;
}

// ID3D12Device
const DEV_CREATE_QUEUE: usize = 8;
const DEV_CREATE_ALLOCATOR: usize = 9;
const DEV_CREATE_COMPUTE_PSO: usize = 11;
const DEV_CREATE_LIST: usize = 12;
const DEV_CREATE_ROOTSIG: usize = 16;
const DEV_CREATE_COMMITTED: usize = 27;
const DEV_CREATE_FENCE: usize = 36;
const DEV_REMOVED_REASON: usize = 37;
// ID3D12GraphicsCommandList
const CL_CLOSE: usize = 9;
const CL_RESET: usize = 10;
const CL_DISPATCH: usize = 14;
const CL_COPY_BUFFER: usize = 15;
const CL_BARRIER: usize = 26;
const CL_SET_COMPUTE_ROOTSIG: usize = 29;
const CL_SET_COMPUTE_ROOT_UAV: usize = 41;
// ID3D12CommandQueue
const Q_EXECUTE: usize = 10;
const Q_SIGNAL: usize = 14;
// ID3D12Fence
const F_COMPLETED: usize = 8;
const F_SET_EVENT: usize = 9;
// ID3D12Resource
const R_MAP: usize = 8;
const R_UNMAP: usize = 9;
const R_GPUVA: usize = 11;
// ID3D12CommandAllocator
const A_RESET: usize = 8;

const HEAP_DEFAULT: u32 = 1;
const HEAP_UPLOAD: u32 = 2;
const HEAP_READBACK: u32 = 3;
const STATE_COMMON: u32 = 0;
const STATE_UAV: u32 = 0x8;
const STATE_COPY_DEST: u32 = 0x400;
const STATE_COPY_SOURCE: u32 = 0x800;
const STATE_GENERIC_READ: u32 = 0xac3;

#[repr(C)]
struct HeapProps {
    ty: u32,
    cpu_page: u32,
    pool: u32,
    creation_mask: u32,
    visible_mask: u32,
}

#[repr(C)]
struct ResourceDesc {
    dimension: u32,
    alignment: u64,
    width: u64,
    height: u32,
    depth: u16,
    mips: u16,
    format: u32,
    sample_count: u32,
    sample_quality: u32,
    layout: u32,
    flags: u32,
}

#[repr(C)]
struct Barrier {
    ty: u32,
    flags: u32,
    resource: *mut c_void,
    subresource: u32,
    before: u32,
    after: u32,
}

unsafe fn vt(obj: *mut c_void) -> *const usize {
    *(obj as *const *const usize)
}
unsafe fn slot<F: Copy>(obj: *mut c_void, i: usize) -> F {
    std::mem::transmute_copy(&*vt(obj).add(i))
}
unsafe fn release(obj: *mut c_void) {
    if !obj.is_null() {
        let f: unsafe extern "system" fn(*mut c_void) -> u32 = slot(obj, 2);
        f(obj);
    }
}

fn check(hr: i32, what: &str) -> Result<(), String> {
    if hr < 0 {
        Err(format!("{what} failed: hr={hr:#010x}"))
    } else {
        Ok(())
    }
}

/// A D3D12 device with one direct queue, allocator, command list and fence.
pub struct Gpu {
    dev: *mut c_void,
    queue: *mut c_void,
    alloc: *mut c_void,
    list: *mut c_void,
    fence: *mut c_void,
    fence_value: std::cell::Cell<u64>,
}

impl Drop for Gpu {
    fn drop(&mut self) {
        unsafe {
            for o in [self.list, self.alloc, self.fence, self.queue, self.dev] {
                release(o);
            }
        }
    }
}

struct Owned(*mut c_void);
impl Drop for Owned {
    fn drop(&mut self) {
        unsafe { release(self.0) }
    }
}

impl Gpu {
    /// Default adapter, feature level 11.0.
    pub fn new() -> Result<Gpu, String> {
        unsafe {
            let d3d12 = LoadLibraryA(b"d3d12.dll\0".as_ptr());
            if d3d12.is_null() {
                return Err("d3d12.dll not available".into());
            }
            let create = GetProcAddress(d3d12, b"D3D12CreateDevice\0".as_ptr());
            if create.is_null() {
                return Err("D3D12CreateDevice not exported".into());
            }
            let create: unsafe extern "system" fn(*mut c_void, u32, *const Guid, *mut *mut c_void) -> i32 = std::mem::transmute(create);
            let mut g = Gpu { dev: null_mut(), queue: null_mut(), alloc: null_mut(), list: null_mut(), fence: null_mut(), fence_value: 0.into() };
            check(create(null_mut(), 0xb000, &IID_DEVICE, &mut g.dev), "D3D12CreateDevice")?;
            // D3D12_COMMAND_QUEUE_DESC { Type DIRECT, Priority, Flags, NodeMask }
            let qdesc = [0u32; 4];
            let f: unsafe extern "system" fn(*mut c_void, *const u32, *const Guid, *mut *mut c_void) -> i32 = slot(g.dev, DEV_CREATE_QUEUE);
            check(f(g.dev, qdesc.as_ptr(), &IID_QUEUE, &mut g.queue), "CreateCommandQueue")?;
            let f: unsafe extern "system" fn(*mut c_void, u32, *const Guid, *mut *mut c_void) -> i32 = slot(g.dev, DEV_CREATE_ALLOCATOR);
            check(f(g.dev, 0, &IID_ALLOCATOR, &mut g.alloc), "CreateCommandAllocator")?;
            let f: unsafe extern "system" fn(*mut c_void, u32, u32, *mut c_void, *mut c_void, *const Guid, *mut *mut c_void) -> i32 =
                slot(g.dev, DEV_CREATE_LIST);
            check(f(g.dev, 0, 0, g.alloc, null_mut(), &IID_LIST, &mut g.list), "CreateCommandList")?;
            let f: unsafe extern "system" fn(*mut c_void) -> i32 = slot(g.list, CL_CLOSE);
            check(f(g.list), "Close")?;
            let f: unsafe extern "system" fn(*mut c_void, u64, u32, *const Guid, *mut *mut c_void) -> i32 = slot(g.dev, DEV_CREATE_FENCE);
            check(f(g.dev, 0, 0, &IID_FENCE, &mut g.fence), "CreateFence")?;
            Ok(g)
        }
    }

    unsafe fn buffer(&self, heap: u32, bytes: u64, state: u32, uav: bool) -> Result<Owned, String> {
        let hp = HeapProps { ty: heap, cpu_page: 0, pool: 0, creation_mask: 0, visible_mask: 0 };
        let rd = ResourceDesc {
            dimension: 1,
            alignment: 0,
            width: bytes,
            height: 1,
            depth: 1,
            mips: 1,
            format: 0,
            sample_count: 1,
            sample_quality: 0,
            layout: 1,
            flags: if uav { 0x4 } else { 0 },
        };
        let mut r = null_mut();
        let f: unsafe extern "system" fn(*mut c_void, *const HeapProps, u32, *const ResourceDesc, u32, *const c_void, *const Guid, *mut *mut c_void) -> i32 =
            slot(self.dev, DEV_CREATE_COMMITTED);
        check(f(self.dev, &hp, 0, &rd, state, std::ptr::null(), &IID_RESOURCE, &mut r), "CreateCommittedResource")?;
        Ok(Owned(r))
    }

    unsafe fn barrier(&self, r: *mut c_void, before: u32, after: u32) {
        let b = Barrier { ty: 0, flags: 0, resource: r, subresource: 0xffff_ffff, before, after };
        let f: unsafe extern "system" fn(*mut c_void, u32, *const Barrier) = slot(self.list, CL_BARRIER);
        f(self.list, 1, &b);
    }

    /// Run `dxil` (a compute shader whose root signature is one root UAV) as `groups`
    /// thread groups over `data`, then copy the buffer back into `data`.
    ///
    /// Root UAVs are not bounds-checked: `data` must cover every address any thread of
    /// every group touches, or the GPU faults (device removed).
    pub fn run(&self, dxil: &[u8], data: &mut [u32], groups: u32) -> Result<(), String> {
        unsafe {
            let bytes = (data.len() * 4).max(4) as u64;
            let mut rs = null_mut();
            let f: unsafe extern "system" fn(*mut c_void, u32, *const u8, usize, *const Guid, *mut *mut c_void) -> i32 = slot(self.dev, DEV_CREATE_ROOTSIG);
            check(f(self.dev, 0, dxil.as_ptr(), dxil.len(), &IID_ROOTSIG, &mut rs), "CreateRootSignature")?;
            let rs = Owned(rs);
            // D3D12_COMPUTE_PIPELINE_STATE_DESC: root signature, CS bytecode (ptr, len), node
            // mask, cached PSO (ptr, len), flags.
            let mut desc = [0u64; 7];
            desc[0] = rs.0 as u64;
            desc[1] = dxil.as_ptr() as u64;
            desc[2] = dxil.len() as u64;
            let mut pso = null_mut();
            let f: unsafe extern "system" fn(*mut c_void, *const u64, *const Guid, *mut *mut c_void) -> i32 = slot(self.dev, DEV_CREATE_COMPUTE_PSO);
            check(f(self.dev, desc.as_ptr(), &IID_PSO, &mut pso), "CreateComputePipelineState")?;
            let pso = Owned(pso);

            let gpu_buf = self.buffer(HEAP_DEFAULT, bytes, STATE_COMMON, true)?;
            let up = self.buffer(HEAP_UPLOAD, bytes, STATE_GENERIC_READ, false)?;
            let rb = self.buffer(HEAP_READBACK, bytes, STATE_COPY_DEST, false)?;

            let map: unsafe extern "system" fn(*mut c_void, u32, *const c_void, *mut *mut c_void) -> i32 = slot(up.0, R_MAP);
            let unmap: unsafe extern "system" fn(*mut c_void, u32, *const c_void) = slot(up.0, R_UNMAP);
            let mut p = null_mut();
            check(map(up.0, 0, std::ptr::null(), &mut p), "Map(upload)")?;
            std::ptr::copy_nonoverlapping(data.as_ptr(), p as *mut u32, data.len());
            unmap(up.0, 0, std::ptr::null());

            let reset: unsafe extern "system" fn(*mut c_void) -> i32 = slot(self.alloc, A_RESET);
            check(reset(self.alloc), "CommandAllocator::Reset")?;
            let reset: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void) -> i32 = slot(self.list, CL_RESET);
            check(reset(self.list, self.alloc, pso.0), "CommandList::Reset")?;
            let copy: unsafe extern "system" fn(*mut c_void, *mut c_void, u64, *mut c_void, u64, u64) = slot(self.list, CL_COPY_BUFFER);
            self.barrier(gpu_buf.0, STATE_COMMON, STATE_COPY_DEST);
            copy(self.list, gpu_buf.0, 0, up.0, 0, bytes);
            self.barrier(gpu_buf.0, STATE_COPY_DEST, STATE_UAV);
            let f: unsafe extern "system" fn(*mut c_void, *mut c_void) = slot(self.list, CL_SET_COMPUTE_ROOTSIG);
            f(self.list, rs.0);
            let va: unsafe extern "system" fn(*mut c_void) -> u64 = slot(gpu_buf.0, R_GPUVA);
            let f: unsafe extern "system" fn(*mut c_void, u32, u64) = slot(self.list, CL_SET_COMPUTE_ROOT_UAV);
            f(self.list, 0, va(gpu_buf.0));
            let f: unsafe extern "system" fn(*mut c_void, u32, u32, u32) = slot(self.list, CL_DISPATCH);
            f(self.list, groups, 1, 1);
            self.barrier(gpu_buf.0, STATE_UAV, STATE_COPY_SOURCE);
            copy(self.list, rb.0, 0, gpu_buf.0, 0, bytes);
            let close: unsafe extern "system" fn(*mut c_void) -> i32 = slot(self.list, CL_CLOSE);
            check(close(self.list), "CommandList::Close")?;

            let lists = [self.list];
            let f: unsafe extern "system" fn(*mut c_void, u32, *const *mut c_void) = slot(self.queue, Q_EXECUTE);
            f(self.queue, 1, lists.as_ptr());
            let v = self.fence_value.get() + 1;
            self.fence_value.set(v);
            let f: unsafe extern "system" fn(*mut c_void, *mut c_void, u64) -> i32 = slot(self.queue, Q_SIGNAL);
            check(f(self.queue, self.fence, v), "Signal")?;
            // A null event makes SetEventOnCompletion block until the fence reaches `v`.
            let f: unsafe extern "system" fn(*mut c_void, u64, *mut c_void) -> i32 = slot(self.fence, F_SET_EVENT);
            check(f(self.fence, v, null_mut()), "SetEventOnCompletion")?;
            let f: unsafe extern "system" fn(*mut c_void) -> u64 = slot(self.fence, F_COMPLETED);
            if f(self.fence) < v {
                let r: unsafe extern "system" fn(*mut c_void) -> i32 = slot(self.dev, DEV_REMOVED_REASON);
                return Err(format!("GPU did not complete (device removed reason {:#010x})", r(self.dev)));
            }

            let map: unsafe extern "system" fn(*mut c_void, u32, *const c_void, *mut *mut c_void) -> i32 = slot(rb.0, R_MAP);
            let unmap: unsafe extern "system" fn(*mut c_void, u32, *const c_void) = slot(rb.0, R_UNMAP);
            let mut p = null_mut();
            check(map(rb.0, 0, std::ptr::null(), &mut p), "Map(readback)")?;
            std::ptr::copy_nonoverlapping(p as *const u32, data.as_mut_ptr(), data.len());
            unmap(rb.0, 0, std::ptr::null());
            Ok(())
        }
    }
}
