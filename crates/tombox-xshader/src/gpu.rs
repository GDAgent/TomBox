//! Minimal Direct3D 12 compute runner for translator conformance tests (Windows only).
//!
//! Runs one compiled compute shader (DXIL with an embedded root signature) over a buffer of
//! `u32`s and reads the buffer back. The root signature is either a single root UAV at `u0`
//! ([`Gpu::run`]) or SRV / sampler / UAV descriptor tables followed by that UAV
//! ([`Gpu::run_bound`], which also creates and uploads the textures and buffers behind the
//! tables and reads UAVs back). Used to compare the HLSL the translator emits for an RDNA2
//! instruction with a reference model of that instruction, on the host's real GPU and driver.
//!
//! COM is called through raw vtables, as elsewhere in this crate. Slot numbers follow the
//! method order of the `d3d12.h` interfaces (IUnknown 0-2, ID3D12Object 3-6, then the
//! interface's own methods); the device slots 11, 16 and 27 are also used by `xpso` and
//! `d3d12_xs`. Methods returning a struct (the descriptor-heap start handles) take a hidden
//! return pointer, as in the `_WIN32` C declarations of `d3d12.h`.

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
const IID_HEAP: Guid = Guid(0x8efb471d, 0x616c, 0x4f49, [0x90, 0xf7, 0x12, 0x7b, 0xb7, 0x63, 0xfa, 0x51]);
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
const DEV_CHECK_FEATURE: usize = 13;
const DEV_CREATE_HEAP: usize = 14;
const DEV_HANDLE_INCREMENT: usize = 15;
const DEV_CREATE_ROOTSIG: usize = 16;
const DEV_CREATE_SRV: usize = 18;
const DEV_CREATE_UAV: usize = 19;
const DEV_CREATE_SAMPLER: usize = 22;
const DEV_CREATE_COMMITTED: usize = 27;
const DEV_CREATE_FENCE: usize = 36;
const DEV_REMOVED_REASON: usize = 37;
const DEV_COPYABLE_FOOTPRINTS: usize = 38;
// ID3D12GraphicsCommandList
const CL_CLOSE: usize = 9;
const CL_RESET: usize = 10;
const CL_DISPATCH: usize = 14;
const CL_COPY_BUFFER: usize = 15;
const CL_COPY_TEXTURE: usize = 16;
const CL_BARRIER: usize = 26;
const CL_SET_HEAPS: usize = 28;
const CL_SET_COMPUTE_ROOTSIG: usize = 29;
const CL_SET_COMPUTE_TABLE: usize = 31;
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
// ID3D12DescriptorHeap
const H_CPU_START: usize = 9;
const H_GPU_START: usize = 10;

const HEAP_DEFAULT: u32 = 1;
const HEAP_UPLOAD: u32 = 2;
const HEAP_READBACK: u32 = 3;
const STATE_COMMON: u32 = 0;
const STATE_UAV: u32 = 0x8;
const STATE_NON_PIXEL_SHADER_RESOURCE: u32 = 0x40;
const STATE_COPY_DEST: u32 = 0x400;
const STATE_COPY_SOURCE: u32 = 0x800;
const STATE_GENERIC_READ: u32 = 0xac3;
const HEAP_TYPE_CBV_SRV_UAV: u32 = 0;
const HEAP_TYPE_SAMPLER: u32 = 1;
const DXGI_R32_TYPELESS: u32 = 39;
const DXGI_R32_FLOAT: u32 = 41;

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

    /// Highest shader model the device supports, as `0xMm` (e.g. 0x66 for 6.6).
    pub fn shader_model(&self) -> u32 {
        // D3D12_FEATURE_SHADER_MODEL (7): in = highest model the caller knows, out = supported.
        // Walk down from 6.8: a device older than the requested model's runtime fails the call.
        for want in (0x60..=0x68u32).rev() {
            let mut sm = want;
            let f: unsafe extern "system" fn(*mut c_void, u32, *mut u32, u32) -> i32 = unsafe { slot(self.dev, DEV_CHECK_FEATURE) };
            if unsafe { f(self.dev, 7, &mut sm, 4) } >= 0 {
                return sm;
            }
        }
        0
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

    unsafe fn texture(&self, t: &Texture, uav: bool) -> Result<Owned, String> {
        let hp = HeapProps { ty: HEAP_DEFAULT, cpu_page: 0, pool: 0, creation_mask: 0, visible_mask: 0 };
        let rd = t.resource_desc(uav);
        let mut r = null_mut();
        let f: unsafe extern "system" fn(*mut c_void, *const HeapProps, u32, *const ResourceDesc, u32, *const c_void, *const Guid, *mut *mut c_void) -> i32 =
            slot(self.dev, DEV_CREATE_COMMITTED);
        check(f(self.dev, &hp, 0, &rd, STATE_COPY_DEST, std::ptr::null(), &IID_RESOURCE, &mut r), "CreateCommittedResource(texture)")?;
        Ok(Owned(r))
    }

    /// Placed footprints of every subresource of `t`, and the total staging size.
    unsafe fn footprints(&self, t: &Texture) -> (Vec<Footprint>, u64) {
        let rd = t.resource_desc(false);
        let n = t.subresources();
        let mut fp: Vec<Footprint> = (0..n).map(|_| Footprint::default()).collect();
        let mut total = 0u64;
        let f: unsafe extern "system" fn(*mut c_void, *const ResourceDesc, u32, u32, u64, *mut Footprint, *mut u32, *mut u64, *mut u64) =
            slot(self.dev, DEV_COPYABLE_FOOTPRINTS);
        f(self.dev, &rd, 0, n, 0, fp.as_mut_ptr(), null_mut(), null_mut(), &mut total);
        (fp, total)
    }

    unsafe fn heap(&self, ty: u32, n: u32) -> Result<(Owned, u64, u64, u64), String> {
        // D3D12_DESCRIPTOR_HEAP_DESC { Type, NumDescriptors, Flags (shader visible), NodeMask }
        let desc = [ty, n.max(1), 1, 0];
        let mut h = null_mut();
        let f: unsafe extern "system" fn(*mut c_void, *const u32, *const Guid, *mut *mut c_void) -> i32 = slot(self.dev, DEV_CREATE_HEAP);
        check(f(self.dev, desc.as_ptr(), &IID_HEAP, &mut h), "CreateDescriptorHeap")?;
        let h = Owned(h);
        let f: unsafe extern "system" fn(*mut c_void, u32) -> u32 = slot(self.dev, DEV_HANDLE_INCREMENT);
        let inc = f(self.dev, ty) as u64;
        // The handle getters return their struct through a hidden pointer (see d3d12.h, _WIN32).
        let mut cpu = 0u64;
        let f: unsafe extern "system" fn(*mut c_void, *mut u64) -> *mut u64 = slot(h.0, H_CPU_START);
        f(h.0, &mut cpu);
        let mut gpu = 0u64;
        let f: unsafe extern "system" fn(*mut c_void, *mut u64) -> *mut u64 = slot(h.0, H_GPU_START);
        f(h.0, &mut gpu);
        Ok((h, inc, cpu, gpu))
    }

    unsafe fn create_pso(&self, dxil: &[u8]) -> Result<(Owned, Owned), String> {
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
        Ok((rs, Owned(pso)))
    }

    unsafe fn write_mapped(r: *mut c_void, bytes: &[u8]) -> Result<(), String> {
        let map: unsafe extern "system" fn(*mut c_void, u32, *const c_void, *mut *mut c_void) -> i32 = slot(r, R_MAP);
        let unmap: unsafe extern "system" fn(*mut c_void, u32, *const c_void) = slot(r, R_UNMAP);
        let mut p = null_mut();
        check(map(r, 0, std::ptr::null(), &mut p), "Map(upload)")?;
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p as *mut u8, bytes.len());
        unmap(r, 0, std::ptr::null());
        Ok(())
    }

    unsafe fn read_mapped(r: *mut c_void, out: &mut [u8]) -> Result<(), String> {
        let map: unsafe extern "system" fn(*mut c_void, u32, *const c_void, *mut *mut c_void) -> i32 = slot(r, R_MAP);
        let unmap: unsafe extern "system" fn(*mut c_void, u32, *const c_void) = slot(r, R_UNMAP);
        let mut p = null_mut();
        check(map(r, 0, std::ptr::null(), &mut p), "Map(readback)")?;
        std::ptr::copy_nonoverlapping(p as *const u8, out.as_mut_ptr(), out.len());
        unmap(r, 0, std::ptr::null());
        Ok(())
    }

    unsafe fn copy_texture(&self, dst: &CopyLoc, src: &CopyLoc) {
        let f: unsafe extern "system" fn(*mut c_void, *const CopyLoc, u32, u32, u32, *const CopyLoc, *const c_void) = slot(self.list, CL_COPY_TEXTURE);
        f(self.list, dst, 0, 0, 0, src, std::ptr::null());
    }

    unsafe fn submit_and_wait(&self) -> Result<(), String> {
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
        Ok(())
    }

    /// Run `dxil` (a compute shader whose root signature is one root UAV) as `groups`
    /// thread groups over `data`, then copy the buffer back into `data`.
    ///
    /// Root UAVs are not bounds-checked: `data` must cover every address any thread of
    /// every group touches, or the GPU faults (device removed).
    pub fn run(&self, dxil: &[u8], data: &mut [u32], groups: u32) -> Result<(), String> {
        self.run_bound(dxil, data, groups, &[], &[], &mut [])
    }

    /// Like [`Gpu::run`], for a shader whose root signature is
    /// `DescriptorTable(SRV(t0..)), DescriptorTable(Sampler(s0..)), DescriptorTable(UAV(u0..)),
    /// UAV(u0, space1)` (the layout of `hlsl::translate_mem_probe`): `srv`, `samplers` and
    /// `uav` fill the tables from slot 0. Every table gets at least one descriptor; slots the
    /// caller leaves out, and [`Binding::Null`] slots, hold null descriptors, so a table must
    /// be passed in full up to the highest slot the shader reads. After the dispatch the
    /// contents of every UAV binding are read back into it.
    pub fn run_bound(
        &self,
        dxil: &[u8],
        data: &mut [u32],
        groups: u32,
        srv: &[Binding],
        samplers: &[Sampler],
        uav: &mut [Binding],
    ) -> Result<(), String> {
        let descriptor_tables = !(srv.is_empty() && samplers.is_empty() && uav.is_empty());
        unsafe {
            let bytes = (data.len() * 4).max(4) as u64;
            let (rs, pso) = self.create_pso(dxil)?;

            let gpu_buf = self.buffer(HEAP_DEFAULT, bytes, STATE_COMMON, true)?;
            let up = self.buffer(HEAP_UPLOAD, bytes, STATE_GENERIC_READ, false)?;
            let rb = self.buffer(HEAP_READBACK, bytes, STATE_COPY_DEST, false)?;
            Self::write_mapped(up.0, std::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() * 4))?;

            let reset: unsafe extern "system" fn(*mut c_void) -> i32 = slot(self.alloc, A_RESET);
            check(reset(self.alloc), "CommandAllocator::Reset")?;
            let reset: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void) -> i32 = slot(self.list, CL_RESET);
            check(reset(self.list, self.alloc, pso.0), "CommandList::Reset")?;
            let copy: unsafe extern "system" fn(*mut c_void, *mut c_void, u64, *mut c_void, u64, u64) = slot(self.list, CL_COPY_BUFFER);
            self.barrier(gpu_buf.0, STATE_COMMON, STATE_COPY_DEST);
            copy(self.list, gpu_buf.0, 0, up.0, 0, bytes);
            self.barrier(gpu_buf.0, STATE_COPY_DEST, STATE_UAV);

            // Resources, their staging buffers, and the descriptor heaps (all kept alive until
            // the GPU is done).
            let mut keep: Vec<Owned> = Vec::new();
            let mut uav_res: Vec<(usize, *mut c_void)> = Vec::new();
            let f: unsafe extern "system" fn(*mut c_void, *mut c_void) = slot(self.list, CL_SET_COMPUTE_ROOTSIG);
            f(self.list, rs.0);
            if descriptor_tables {
                let nsrv = srv.len().max(1) as u32;
                let nuav = uav.len().max(1) as u32;
                let (view_heap, inc, cpu, gpu) = self.heap(HEAP_TYPE_CBV_SRV_UAV, nsrv + nuav)?;
                let (samp_heap, sinc, scpu, sgpu) = self.heap(HEAP_TYPE_SAMPLER, samplers.len().max(1) as u32)?;
                for k in 0..nsrv as usize {
                    let r = self.upload_binding(srv.get(k).unwrap_or(&Binding::Null), false, &mut keep)?;
                    let desc = srv.get(k).unwrap_or(&Binding::Null).srv_desc();
                    let f: unsafe extern "system" fn(*mut c_void, *mut c_void, *const ViewDesc, u64) = slot(self.dev, DEV_CREATE_SRV);
                    f(self.dev, r, &desc, cpu + k as u64 * inc);
                }
                for k in 0..nuav as usize {
                    let b = uav.get(k).unwrap_or(&Binding::Null);
                    let r = self.upload_binding(b, true, &mut keep)?;
                    if !r.is_null() {
                        uav_res.push((k, r));
                    }
                    let desc = b.uav_desc();
                    let f: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void, *const ViewDesc, u64) = slot(self.dev, DEV_CREATE_UAV);
                    f(self.dev, r, null_mut(), &desc, cpu + (nsrv as u64 + k as u64) * inc);
                }
                let default_sampler = Sampler::point_clamp();
                for k in 0..samplers.len().max(1) {
                    let s = samplers.get(k).unwrap_or(&default_sampler);
                    let f: unsafe extern "system" fn(*mut c_void, *const Sampler, u64) = slot(self.dev, DEV_CREATE_SAMPLER);
                    f(self.dev, s, scpu + k as u64 * sinc);
                }
                let heaps = [view_heap.0, samp_heap.0];
                let f: unsafe extern "system" fn(*mut c_void, u32, *const *mut c_void) = slot(self.list, CL_SET_HEAPS);
                f(self.list, 2, heaps.as_ptr());
                let f: unsafe extern "system" fn(*mut c_void, u32, u64) = slot(self.list, CL_SET_COMPUTE_TABLE);
                f(self.list, 0, gpu);
                f(self.list, 1, sgpu);
                f(self.list, 2, gpu + nsrv as u64 * inc);
                keep.push(view_heap);
                keep.push(samp_heap);
            }
            let va: unsafe extern "system" fn(*mut c_void) -> u64 = slot(gpu_buf.0, R_GPUVA);
            let f: unsafe extern "system" fn(*mut c_void, u32, u64) = slot(self.list, CL_SET_COMPUTE_ROOT_UAV);
            f(self.list, if descriptor_tables { 3 } else { 0 }, va(gpu_buf.0));
            let f: unsafe extern "system" fn(*mut c_void, u32, u32, u32) = slot(self.list, CL_DISPATCH);
            f(self.list, groups, 1, 1);
            self.barrier(gpu_buf.0, STATE_UAV, STATE_COPY_SOURCE);
            copy(self.list, rb.0, 0, gpu_buf.0, 0, bytes);

            // UAV contents back to staging buffers.
            let mut readbacks: Vec<(usize, Owned, Vec<Footprint>)> = Vec::new();
            for &(k, r) in &uav_res {
                self.barrier(r, STATE_UAV, STATE_COPY_SOURCE);
                match &uav[k] {
                    Binding::Texture(t) => {
                        let (fp, total) = self.footprints(t);
                        let st = self.buffer(HEAP_READBACK, total, STATE_COPY_DEST, false)?;
                        for (i, p) in fp.iter().enumerate() {
                            let dst = CopyLoc { resource: st.0, ty: 1, footprint: *p };
                            let src = CopyLoc::subresource(r, i as u32);
                            self.copy_texture(&dst, &src);
                        }
                        readbacks.push((k, st, fp));
                    }
                    Binding::Buffer(b) => {
                        let st = self.buffer(HEAP_READBACK, b.data.len() as u64, STATE_COPY_DEST, false)?;
                        copy(self.list, st.0, 0, r, 0, b.data.len() as u64);
                        readbacks.push((k, st, Vec::new()));
                    }
                    Binding::Null => {}
                }
            }
            self.submit_and_wait()?;

            Self::read_mapped(rb.0, std::slice::from_raw_parts_mut(data.as_mut_ptr() as *mut u8, data.len() * 4))?;
            for (k, st, fp) in readbacks {
                match &mut uav[k] {
                    Binding::Texture(t) => {
                        let total = fp.last().map(|p| p.offset + p.row_pitch as u64 * p.height as u64 * p.depth as u64).unwrap_or(0);
                        let mut staged = vec![0u8; total as usize];
                        Self::read_mapped(st.0, &mut staged)?;
                        t.data = t.unpack(&fp, &staged);
                    }
                    Binding::Buffer(b) => Self::read_mapped(st.0, &mut b.data)?,
                    Binding::Null => {}
                }
            }
            drop(keep);
            Ok(())
        }
    }

    /// Create the resource behind a binding and record its upload (null for
    /// [`Binding::Null`]). The resource ends in the shader-resource or UAV state.
    unsafe fn upload_binding(&self, b: &Binding, uav: bool, keep: &mut Vec<Owned>) -> Result<*mut c_void, String> {
        let after = if uav { STATE_UAV } else { STATE_NON_PIXEL_SHADER_RESOURCE };
        match b {
            Binding::Null => Ok(null_mut()),
            Binding::Texture(t) => {
                if t.data.len() != t.byte_len() {
                    return Err(format!("texture data is {} bytes, layout needs {}", t.data.len(), t.byte_len()));
                }
                let tex = self.texture(t, uav)?;
                let (fp, total) = self.footprints(t);
                let up = self.buffer(HEAP_UPLOAD, total, STATE_GENERIC_READ, false)?;
                Self::write_mapped(up.0, &t.pack(&fp, total as usize))?;
                for (i, p) in fp.iter().enumerate() {
                    let dst = CopyLoc::subresource(tex.0, i as u32);
                    let src = CopyLoc { resource: up.0, ty: 1, footprint: *p };
                    self.copy_texture(&dst, &src);
                }
                self.barrier(tex.0, STATE_COPY_DEST, after);
                let r = tex.0;
                keep.push(tex);
                keep.push(up);
                Ok(r)
            }
            Binding::Buffer(bf) => {
                let n = (bf.data.len() as u64).max(4);
                let buf = self.buffer(HEAP_DEFAULT, n, STATE_COMMON, uav)?;
                let up = self.buffer(HEAP_UPLOAD, n, STATE_GENERIC_READ, false)?;
                Self::write_mapped(up.0, &bf.data)?;
                self.barrier(buf.0, STATE_COMMON, STATE_COPY_DEST);
                let copy: unsafe extern "system" fn(*mut c_void, *mut c_void, u64, *mut c_void, u64, u64) = slot(self.list, CL_COPY_BUFFER);
                copy(self.list, buf.0, 0, up.0, 0, bf.data.len() as u64);
                self.barrier(buf.0, STATE_COPY_DEST, after);
                let r = buf.0;
                keep.push(buf);
                keep.push(up);
                Ok(r)
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// Resources for `run_bound`

/// Texture shape. Each maps to one D3D12 resource dimension and SRV/UAV view dimension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TexDim {
    D1,
    D1Array,
    D2,
    D2Array,
    D3,
    /// Six faces (array slices 0..6 in D3D order +X, -X, +Y, -Y, +Z, -Z).
    Cube,
}

/// A texture with initial contents. `data` holds every subresource in D3D12 order
/// (array slice major, then mip; a 3D mip holds all its depth slices), each tightly packed:
/// rows of `width * texel_bytes` bytes. Block-compressed formats are not supported.
#[derive(Clone, Debug)]
pub struct Texture {
    pub dim: TexDim,
    /// DXGI_FORMAT.
    pub format: u32,
    pub texel_bytes: u32,
    pub width: u32,
    pub height: u32,
    /// Depth (3D) or array size (arrays; 6 for a cube). 1 otherwise.
    pub depth: u32,
    pub mips: u32,
    pub data: Vec<u8>,
}

impl Texture {
    fn layers(&self) -> u32 {
        match self.dim {
            TexDim::D3 => 1,
            _ => self.depth,
        }
    }
    fn subresources(&self) -> u32 {
        self.layers() * self.mips
    }
    /// (width, height, depth) of mip `m`.
    pub fn mip_size(&self, m: u32) -> (u32, u32, u32) {
        let h = if matches!(self.dim, TexDim::D1 | TexDim::D1Array) { 1 } else { (self.height >> m).max(1) };
        let d = if self.dim == TexDim::D3 { (self.depth >> m).max(1) } else { 1 };
        ((self.width >> m).max(1), h, d)
    }
    /// Byte offset of (layer, mip) in `data`.
    pub fn sub_offset(&self, layer: u32, mip: u32) -> usize {
        let mut off = 0usize;
        for l in 0..self.layers() {
            for m in 0..self.mips {
                if (l, m) == (layer, mip) {
                    return off;
                }
                let (w, h, d) = self.mip_size(m);
                off += (w * h * d * self.texel_bytes) as usize;
            }
        }
        off
    }
    pub fn byte_len(&self) -> usize {
        self.sub_offset(self.layers(), 0)
    }
    fn resource_desc(&self, uav: bool) -> ResourceDesc {
        let dimension = match self.dim {
            TexDim::D1 | TexDim::D1Array => 2,
            TexDim::D3 => 4,
            _ => 3,
        };
        ResourceDesc {
            dimension,
            alignment: 0,
            width: self.width as u64,
            height: if dimension == 2 { 1 } else { self.height },
            depth: self.depth as u16,
            mips: self.mips as u16,
            format: self.format,
            sample_count: 1,
            sample_quality: 0,
            layout: 0,
            flags: if uav { 0x4 } else { 0 },
        }
    }
    /// Tightly packed `data` -> staging buffer laid out by `fp`.
    fn pack(&self, fp: &[Footprint], total: usize) -> Vec<u8> {
        let mut out = vec![0u8; total];
        let mut src = 0usize;
        for p in fp {
            let row = (p.width * self.texel_bytes) as usize;
            for z in 0..p.depth as usize {
                for y in 0..p.height as usize {
                    let dst = p.offset as usize + (z * p.height as usize + y) * p.row_pitch as usize;
                    out[dst..dst + row].copy_from_slice(&self.data[src..src + row]);
                    src += row;
                }
            }
        }
        out
    }
    /// Staging buffer laid out by `fp` -> tightly packed bytes.
    fn unpack(&self, fp: &[Footprint], staged: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.byte_len());
        for p in fp {
            let row = (p.width * self.texel_bytes) as usize;
            for z in 0..p.depth as usize {
                for y in 0..p.height as usize {
                    let s = p.offset as usize + (z * p.height as usize + y) * p.row_pitch as usize;
                    out.extend_from_slice(&staged[s..s + row]);
                }
            }
        }
        out
    }
}

/// A buffer viewed as typed (`format` != 0: Buffer<T> / RWBuffer<T>), structured (`stride`
/// != 0) or raw (both 0: ByteAddressBuffer).
#[derive(Clone, Debug)]
pub struct BufferData {
    pub data: Vec<u8>,
    pub format: u32,
    pub format_bytes: u32,
    pub stride: u32,
}

/// What a descriptor-table slot holds.
#[derive(Clone, Debug)]
pub enum Binding {
    Null,
    Texture(Texture),
    Buffer(BufferData),
}

impl Binding {
    fn buffer_view(b: &BufferData) -> (u32, u32, u32, u32) {
        // (format, element count, structure stride, flags)
        if b.format != 0 {
            (b.format, b.data.len() as u32 / b.format_bytes.max(1), 0, 0)
        } else if b.stride != 0 {
            (0, b.data.len() as u32 / b.stride, b.stride, 0)
        } else {
            (DXGI_R32_TYPELESS, b.data.len() as u32 / 4, 0, 1)
        }
    }

    fn srv_desc(&self) -> ViewDesc {
        let mut d = ViewDesc { format: DXGI_R32_FLOAT, dim: 4, mapping: 0x1688, pad: 0, u: [0; 8] };
        match self {
            // Null descriptor: a 2D view of nothing.
            Binding::Null => d.u[1] = u32::MAX,
            Binding::Texture(t) => {
                d.format = t.format;
                // { MostDetailedMip, MipLevels (all), then dimension-specific fields }
                d.u[1] = u32::MAX;
                match t.dim {
                    TexDim::D1 => d.dim = 2,
                    TexDim::D1Array => {
                        d.dim = 3;
                        d.u[3] = t.depth;
                    }
                    TexDim::D2 => d.dim = 4,
                    TexDim::D2Array => {
                        d.dim = 5;
                        d.u[3] = t.depth;
                    }
                    TexDim::D3 => d.dim = 8,
                    TexDim::Cube => d.dim = 9,
                }
            }
            Binding::Buffer(b) => {
                let (fmt, n, stride, flags) = Self::buffer_view(b);
                d.format = fmt;
                d.dim = 1;
                // { FirstElement (u64), NumElements, StructureByteStride, Flags }
                d.u[2] = n;
                d.u[3] = stride;
                d.u[4] = flags;
            }
        }
        d
    }

    fn uav_desc(&self) -> ViewDesc {
        // D3D12_UNORDERED_ACCESS_VIEW_DESC has no component mapping: its union starts at
        // byte 8, where `mapping` sits in this layout. Fields below are shifted accordingly.
        let mut d = ViewDesc { format: DXGI_R32_FLOAT, dim: 4, mapping: 0, pad: 0, u: [0; 8] };
        match self {
            Binding::Null => {}
            Binding::Texture(t) => {
                d.format = t.format;
                // Mip 0, every slice.
                match t.dim {
                    TexDim::D1 => d.dim = 2,
                    TexDim::D1Array => {
                        d.dim = 3;
                        d.u[0] = t.depth; // { MipSlice, FirstArraySlice, ArraySize }
                    }
                    TexDim::D2 => d.dim = 4,
                    TexDim::D2Array | TexDim::Cube => {
                        d.dim = 5;
                        d.u[0] = t.depth; // { MipSlice, FirstArraySlice, ArraySize, PlaneSlice }
                    }
                    TexDim::D3 => {
                        d.dim = 8;
                        d.u[0] = u32::MAX; // { MipSlice, FirstWSlice, WSize (all) }
                    }
                }
            }
            Binding::Buffer(b) => {
                let (fmt, n, stride, flags) = Self::buffer_view(b);
                d.format = fmt;
                d.dim = 1;
                // { FirstElement (u64), NumElements, StructureByteStride, CounterOffset (u64), Flags }
                d.u[0] = n;
                d.u[1] = stride;
                d.u[4] = flags;
            }
        }
        d
    }
}

/// D3D12_SAMPLER_DESC.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Sampler {
    /// D3D12_FILTER (0 = point, 0x15 = trilinear, 0x80 bit = comparison).
    pub filter: u32,
    /// D3D12_TEXTURE_ADDRESS_MODE for u, v, w (1 wrap, 2 mirror, 3 clamp, 4 border).
    pub address: [u32; 3],
    pub mip_lod_bias: f32,
    pub max_anisotropy: u32,
    /// D3D12_COMPARISON_FUNC (1 never .. 8 always).
    pub comparison: u32,
    pub border: [f32; 4],
    pub min_lod: f32,
    pub max_lod: f32,
}

impl Sampler {
    pub fn point_clamp() -> Sampler {
        Sampler { filter: 0, address: [3; 3], mip_lod_bias: 0.0, max_anisotropy: 1, comparison: 1, border: [0.0; 4], min_lod: 0.0, max_lod: f32::MAX }
    }
}

/// Shared layout of D3D12_SHADER_RESOURCE_VIEW_DESC (format, dimension, mapping, union at
/// byte 16) and D3D12_UNORDERED_ACCESS_VIEW_DESC (format, dimension, union at byte 8 — here
/// `mapping`, `pad`, `u`). 48 bytes cover both (40 each).
#[repr(C)]
struct ViewDesc {
    format: u32,
    dim: u32,
    mapping: u32,
    pad: u32,
    u: [u32; 8],
}

/// D3D12_PLACED_SUBRESOURCE_FOOTPRINT.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
struct Footprint {
    offset: u64,
    format: u32,
    width: u32,
    height: u32,
    depth: u32,
    row_pitch: u32,
}

/// D3D12_TEXTURE_COPY_LOCATION (type 0 = subresource index, 1 = placed footprint).
#[repr(C)]
struct CopyLoc {
    resource: *mut c_void,
    ty: u32,
    footprint: Footprint,
}

impl CopyLoc {
    fn subresource(r: *mut c_void, index: u32) -> CopyLoc {
        // The union's first 4 bytes are the subresource index.
        CopyLoc { resource: r, ty: 0, footprint: Footprint { offset: index as u64, ..Default::default() } }
    }
}
