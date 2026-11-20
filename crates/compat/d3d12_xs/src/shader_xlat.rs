//! Runtime shader translation for PSO creation.
//!
//! Xbox shader blob (DXBC wrapper → XDAT → zlib → FSSM → RDNA2) → HLSL (tombox-xshader)
//! → DXC (`dxcompiler.dll`, SM6 DXIL; FXC/`D3DCompile` SM5 DXBC as fallback) → cached bytecode.
//! Each unique shader is translated once per process (keyed by an FNV-1a hash of its
//! bytes). Failures are cached too and reported with the reason; failing shaders are
//! dumped to `%TEMP%\tombox_shaders` so coverage can be extended offline with `xshader`.

use compat_common::record;
use core::ffi::c_void;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tombox_xshader::hlsl::InputInterp;

#[derive(Clone)]
pub struct CompiledShader {
    pub code: Arc<Vec<u8>>,
    pub portable_compute_group: Option<[u32; 3]>,
    /// Pixel shaders: input interpolation modifiers, for translating the stage feeding it.
    ps_input_interp: Arc<InputInterp>,
}
type Cached = Result<CompiledShader, Arc<String>>;
static CACHE: OnceLock<Mutex<HashMap<u64, Cached>>> = OnceLock::new();
static OK: AtomicUsize = AtomicUsize::new(0);
static FAIL: AtomicUsize = AtomicUsize::new(0);
static DUMPED: AtomicUsize = AtomicUsize::new(0);
/// Translated code pointer -> (hash, original Xbox container); kept only for the diagnostic
/// draw trace (`rtcap`).
static ORIGIN: OnceLock<Mutex<HashMap<usize, (u64, Arc<Vec<u8>>)>>> = OnceLock::new();

pub fn origin(code: usize) -> Option<(u64, Arc<Vec<u8>>)> {
    ORIGIN.get()?.lock().unwrap().get(&code).cloned()
}

fn fnv1a(b: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &x in b {
        h ^= x as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

type D3DCompileFn = unsafe extern "system" fn(
    *const c_void, usize, *const u8, *const c_void, *mut c_void, *const u8, *const u8, u32, u32,
    *mut *mut c_void, *mut *mut c_void,
) -> i32;

fn d3dcompile() -> Option<D3DCompileFn> {
    static F: OnceLock<usize> = OnceLock::new();
    let p = *F.get_or_init(|| unsafe {
        extern "system" {
            fn LoadLibraryA(name: *const u8) -> *mut c_void;
            fn GetProcAddress(h: *mut c_void, name: *const u8) -> *mut c_void;
        }
        let h = LoadLibraryA(b"d3dcompiler_47.dll\0".as_ptr());
        if h.is_null() {
            return 0;
        }
        GetProcAddress(h, b"D3DCompile\0".as_ptr()) as usize
    });
    if p == 0 { None } else { Some(unsafe { core::mem::transmute::<usize, D3DCompileFn>(p) }) }
}

/// Read an ID3DBlob's bytes and release it.
unsafe fn take_blob(blob: *mut c_void) -> Vec<u8> {
    if blob.is_null() {
        return Vec::new();
    }
    let vt = *(blob as *const *const usize);
    let get_ptr: unsafe extern "system" fn(*mut c_void) -> *const u8 = core::mem::transmute(*vt.add(3));
    let get_size: unsafe extern "system" fn(*mut c_void) -> usize = core::mem::transmute(*vt.add(4));
    let release: unsafe extern "system" fn(*mut c_void) -> u32 = core::mem::transmute(*vt.add(2));
    let (p, n) = (get_ptr(blob), get_size(blob));
    let v = if p.is_null() { Vec::new() } else { core::slice::from_raw_parts(p, n).to_vec() };
    release(blob);
    v
}

fn compile(hlsl: &str, profile: &str) -> Result<Vec<u8>, String> {
    let f = d3dcompile().ok_or("d3dcompiler_47.dll / D3DCompile unavailable")?;
    let target = format!("{profile}\0");
    let mut code: *mut c_void = core::ptr::null_mut();
    let mut errs: *mut c_void = core::ptr::null_mut();
    let hr = unsafe {
        f(
            hlsl.as_ptr() as *const c_void,
            hlsl.len(),
            b"tombox\0".as_ptr(),
            core::ptr::null(),
            core::ptr::null_mut(),
            b"main\0".as_ptr(),
            target.as_ptr(),
            0,
            0,
            &mut code,
            &mut errs,
        )
    };
    let err_text = String::from_utf8_lossy(&unsafe { take_blob(errs) }).into_owned();
    let bytes = unsafe { take_blob(code) };
    if hr < 0 || bytes.is_empty() {
        return Err(format!("D3DCompile hr={hr:#010x}: {}", err_text.trim()));
    }
    Ok(bytes)
}

fn dump_failure(hash: u64, blob: &[u8], hlsl: Option<&str>, err: &str) {
    if DUMPED.fetch_add(1, Ordering::Relaxed) >= 400 {
        return;
    }
    let dir = std::env::temp_dir().join("tombox_shaders");
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(dir.join(format!("{hash:016x}.dxbc")), blob);
    let _ = std::fs::write(dir.join(format!("{hash:016x}.err.txt")), err);
    if let Some(h) = hlsl {
        let _ = std::fs::write(dir.join(format!("{hash:016x}.hlsl")), h);
    }
}

/// On-disk cache of compiled shaders, keyed by the *generated HLSL* (plus profile): a
/// translator change that alters a shader's HLSL misses the cache, anything else — including
/// rebuilding this DLL — reuses the compiled bytecode. Compiling is the expensive part
/// (seconds for the large dispatcher-style compute shaders; translation takes milliseconds).
fn disk_cache_dir() -> Option<&'static std::path::PathBuf> {
    static DIR: OnceLock<Option<std::path::PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join("tombox_shader_cache").join("by_hlsl");
        std::fs::create_dir_all(&dir).ok()?;
        Some(dir)
    })
    .as_ref()
}

fn translate_uncached(blob: &[u8], hash: u64, feeds_ps: Option<&InputInterp>) -> Result<CompiledShader, String> {
    let ex = tombox_xshader::extract(blob).map_err(|e| {
        dump_failure(hash, blob, None, &e);
        format!("extract: {e}")
    })?;
    let mut t = tombox_xshader::hlsl::translate_linked(&ex, feeds_ps).map_err(|e| {
        dump_failure(hash, blob, None, &e);
        format!("translate: {e}")
    })?;
    // Development aid: `%TEMP%	ombox_shader_override<hash>.hlsl` replaces the generated
    // HLSL for that shader (to instrument a translation while debugging). Logged every time.
    if let Ok(h) = std::fs::read_to_string(std::env::temp_dir().join("tombox_shader_override").join(format!("{hash:016x}.hlsl"))) {
        record("d3d12_xs", "shader_xlat", &format!("OVERRIDE: hash={hash:016x} uses hand-edited HLSL ({} bytes)", h.len()));
        t.hlsl = h;
        t.portable_compute_group = None;
    }
    // Compiler: DXC (SM6 DXIL) when dxcompiler.dll + dxil.dll are available, otherwise FXC
    // (SM5 DXBC). FXC needs minutes for some of the generated compute shaders; DXC about a
    // second. `TOMBOX_SHADER_COMPILER=fxc` forces the old path.
    let use_dxc = tombox_xshader::dxc::available()
        && std::env::var("TOMBOX_SHADER_COMPILER").map(|v| v != "fxc").unwrap_or(true);
    static ANNOUNCED: std::sync::Once = std::sync::Once::new();
    ANNOUNCED.call_once(|| {
        record("d3d12_xs", "shader_xlat", if use_dxc { "compiler: DXC (shader model 6, DXIL)" } else { "compiler: FXC (shader model 5, DXBC) - DXC not available or disabled" });
    });
    let profile = if use_dxc { t.dxc_profile() } else { t.profile.to_string() };
    let _ = disk_cache_dir(); // creates the cache directory
    let ps_input_interp = Arc::new(std::mem::take(&mut t.ps_input_interp));
    let compiled = |code| CompiledShader { code: Arc::new(code), portable_compute_group: t.portable_compute_group, ps_input_interp: ps_input_interp.clone() };
    let cached = Some(tombox_xshader::dxc::cache_path(&t.hlsl, &profile));
    if let Some(p) = &cached {
        if let Ok(v) = std::fs::read(p) {
            if !v.is_empty() {
                return Ok(compiled(v));
            }
        }
    }
    // Leave the input on disk while it is being compiled: if FXC hangs or the title's
    // watchdog fires first, the file that remains identifies the shader.
    let dir = std::env::temp_dir().join("tombox_shaders");
    let _ = std::fs::create_dir_all(&dir);
    let inflight = dir.join(format!("inflight_{hash:016x}.dxbc"));
    let _ = std::fs::write(&inflight, blob);
    let t0 = std::time::Instant::now();
    let r = if use_dxc { tombox_xshader::dxc::compile(&t.hlsl, &profile) } else { compile(&t.hlsl, &profile) };
    let _ = std::fs::remove_file(&inflight);
    let ms = t0.elapsed().as_millis();
    if ms > 1500 {
        record("d3d12_xs", "shader_xlat", &format!("SLOW: hash={hash:016x} took {ms} ms to compile ({} bytes in, {} HLSL lines)", blob.len(), t.hlsl.lines().count()));
        if ms > 10_000 {
            let _ = std::fs::write(dir.join(format!("slow_{hash:016x}.dxbc")), blob);
        }
    }
    match r {
        Ok(v) => {
            if let Some(p) = &cached {
                let _ = std::fs::write(p, &v);
            }
            Ok(compiled(v))
        }
        Err(e) => {
            dump_failure(hash, blob, Some(&t.hlsl), &e);
            Err(e)
        }
    }
}

/// Translate an Xbox shader blob to PC bytecode (cached). `ptr/len` come from a
/// D3D12_SHADER_BYTECODE in the title's PSO desc.
pub unsafe fn translate(ptr: *const u8, len: usize) -> Cached {
    translate_linked(ptr, len, None)
}

/// Prefer the ordinary producer when its compiled outputs already line up with the
/// pixel inputs. Reapplying modifiers can move a shared output when other outputs
/// are unused by this pixel shader.
pub unsafe fn translate_producer(ptr: *const u8, len: usize, pixel: &CompiledShader) -> Cached {
    let ordinary = translate(ptr, len);
    if pixel.ps_input_interp.0.is_empty() {
        return ordinary;
    }
    if let Ok(producer) = &ordinary {
        if !tombox_xshader::meta::has_packing_mismatch(&producer.code, &pixel.code) {
            return ordinary;
        }
    }
    translate_linked(ptr, len, Some(&pixel.ps_input_interp))
}

unsafe fn translate_linked(ptr: *const u8, len: usize, feeds_ps: Option<&InputInterp>) -> Cached {
    if ptr.is_null() || len < 32 || len > 0x40_0000 {
        return Err(Arc::new(format!("bad shader blob ({ptr:p}, {len})")));
    }
    let blob = core::slice::from_raw_parts(ptr, len);
    let feeds_ps = feeds_ps.filter(|m| !m.0.is_empty());
    // Cache key: the blob, plus the modifiers when there are any (`hash` alone still names
    // the original shader in logs and dumps).
    let hash = fnv1a(blob);
    let key = match feeds_ps {
        Some(m) => hash ^ fnv1a(format!("{:?}", m.0).as_bytes()).rotate_left(29),
        None => hash,
    };
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(hit) = cache.lock().unwrap().get(&key) {
        return hit.clone();
    }
    // Translation never unwinds across the FFI boundary: a panic becomes an error.
    let res = std::panic::catch_unwind(|| translate_uncached(blob, hash, feeds_ps))
        .unwrap_or_else(|_| Err("translator panicked".to_string()));
    let entry: Cached = match res {
        Ok(v) => {
            let n = OK.fetch_add(1, Ordering::Relaxed) + 1;
            if n <= 8 || n % 50 == 0 {
                record("d3d12_xs", "shader_xlat", &format!("ok #{n} hash={hash:016x} {len}B -> {}B DXBC", v.code.len()));
            }
            if crate::rtcap::enabled() {
                ORIGIN.get_or_init(|| Mutex::new(HashMap::new())).lock().unwrap().insert(v.code.as_ptr() as usize, (hash, Arc::new(blob.to_vec())));
            }
            Ok(v)
        }
        Err(e) => {
            let n = FAIL.fetch_add(1, Ordering::Relaxed) + 1;
            if n <= 40 || n % 50 == 0 {
                let first = e.lines().next().unwrap_or("");
                record("d3d12_xs", "shader_xlat", &format!("FAIL #{n} hash={hash:016x}: {first}"));
            }
            Err(Arc::new(e))
        }
    };
    cache.lock().unwrap().insert(key, entry.clone());
    entry
}

pub fn counts() -> (usize, usize) {
    (OK.load(Ordering::Relaxed), FAIL.load(Ordering::Relaxed))
}
