//! Run cache compute fixtures on a Windows D3D12 device.
//! The launcher supplies a DXBC envelope with the measured guest register ABI.
#[cfg(windows)]
#[path = "ps09_compute/classify.rs"]
mod classify;

/// The PSO 205 fixture exercises both halves of its first copy loop. The
/// table indices and typed views below mirror the original scalar loads.
#[cfg(windows)]
fn run_mixed(source: &str, predicate: bool) -> Result<Vec<u8>, String> {
    use tuxbox_xshader::{
        dxc,
        gpu::{Binding, BufferData, Gpu},
    };

    fn buffer(format: u32, bytes: Vec<u8>) -> Binding {
        Binding::Buffer(BufferData {
            data: bytes,
            format,
            format_bytes: 16,
            stride: 0,
        })
    }
    fn words(values: &[u32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }
    let (decl, entry) = source
        .split_once("[numthreads(")
        .ok_or("missing compute entry")?;
    let root = "DescriptorTable(SRV(t0)), DescriptorTable(Sampler(s0)), DescriptorTable(UAV(u0,numDescriptors=12)), UAV(u0,space=1), RootConstants(num32BitConstants=8,b0)";
    let hlsl = format!("{decl}[RootSignature(\"{root}\")]\n[numthreads({entry}");
    let dxil = dxc::compile(&hlsl, "cs_6_0")?;
    let mut views = vec![Binding::Null; 12];
    const FLOAT4: u32 = 2;
    const UINT4: u32 = 3;
    const SINT4: u32 = 4;
    // Lane zero reads a command-list pointer at element 8. Its one command
    // at 1048 copies the low half from u5[0] to u6[3]. The other lanes have
    // empty lists. The high half at 1041 is always nonzero to detect a
    // mistaken 32-bit compare when the command count's low half is zero.
    let mut commands = vec![0u32; 1064 * 4];
    for (element, command) in [
        (8, 1040),
        (1040, 1),
        (1041, if predicate { 0x10001 } else { 0x10000 }),
        (1048, 2),
        (1049, 0x100),
        (1050, 0x300),
    ] {
        commands[element * 4] = command;
    }
    views[3] = buffer(SINT4, words(&commands));
    let mut params = vec![0u32; 32 * 4];
    params[0] = 0x3f805678;
    views[5] = buffer(FLOAT4, words(&params));
    views[6] = buffer(UINT4, vec![0x99; 16 * 32]);
    // Source half bit patterns are in the low halves of the four input words.
    views[7] = buffer(
        FLOAT4,
        words(&[0x3f803c00, 0x3f80c000, 0x3f804200, 0x3f80c400]),
    );
    views[8] = buffer(
        FLOAT4,
        words(&[0x3f804500, 0x3f80c600, 0x3f804700, 0x3f80c800]),
    );
    views[9] = buffer(
        FLOAT4,
        words(&[0x3f800004, 0x3f800004, 0x3f800001, 0x3f800001]),
    );
    views[11] = buffer(FLOAT4, vec![0x55; 16 * 32]);
    let mut guard = vec![0xfee1a11d; 8];
    Gpu::new()?.run_bound_with_constants(&dxil, &mut guard, 1, &[], &[], &mut views, &[0; 8])?;
    if guard != [0xfee1a11d; 8] {
        return Err("unexpected root UAV write".into());
    }
    let Binding::Buffer(output) = &views[11] else {
        return Err("missing output".into());
    };
    let Binding::Buffer(dispatch_output) = &views[6] else {
        return Err("missing command result".into());
    };
    Ok([output.data.as_slice(), dispatch_output.data.as_slice()].concat())
}

#[cfg(windows)]
fn run_filter(
    source: &str,
    packed: bool,
    flat: bool,
    direct_neighbors: bool,
) -> Result<Vec<u8>, String> {
    use tuxbox_xshader::{
        dxc,
        gpu::{Binding, BufferData, Gpu, TexDim, Texture},
    };
    fn texture(format: u32, data: Vec<u32>) -> Binding {
        Binding::Texture(Texture {
            dim: TexDim::D2,
            format,
            texel_bytes: 4,
            width: 32,
            height: 32,
            depth: 1,
            mips: 1,
            data: data.iter().flat_map(|w| w.to_le_bytes()).collect(),
        })
    }
    let (decl, entry) = source
        .split_once("[numthreads(")
        .ok_or("missing compute entry")?;
    let root = "DescriptorTable(SRV(t0,numDescriptors=61)), DescriptorTable(Sampler(s0)), DescriptorTable(UAV(u0,numDescriptors=49)), UAV(u0,space=1), RootConstants(num32BitConstants=36,b5)";
    let dxil = dxc::compile(
        &format!("{decl}[RootSignature(\"{root}\")]\n[numthreads({entry}"),
        "cs_6_0",
    )?;
    let mut srv = vec![Binding::Null; 61];
    let mut uav = vec![Binding::Null; 49];
    let mut constants = [0u32; 36];
    constants[20] = 4;
    constants[34] = u32::from(packed);
    // Input and output use tile zero. The depth lookup starts at (8,8)
    // so every sample, including the neighborhood, is in bounds.
    let params = [1u32 | 1 << 14, 1u32 | 1 << 16, 0, 0];
    uav[48] = Binding::Buffer(BufferData {
        data: params.iter().flat_map(|w| w.to_le_bytes()).collect(),
        format: 0,
        format_bytes: 0,
        stride: 16,
    });
    srv[60] = texture(
        41,
        (0..1024)
            .map(|i| {
                let exp = if flat {
                    0
                } else {
                    (i % 32 + 3 * (i / 32)) % 17
                };
                ((127 + exp) as u32) << 23
            })
            .collect(),
    );
    // An indirect tile (256) reads u43. Direct tiles encode opacity in
    // four or eight bits, and the half-float predicate skips their fetch.
    let full_opacity = if packed { 15 } else { 255 };
    let mut tiles = vec![if direct_neighbors { full_opacity } else { 256 }; 1024];
    tiles[33] = 256;
    uav[38] = texture(43, tiles);
    uav[43] = texture(
        42,
        (0..1024u32)
            .map(|i| {
                let x = i % 32;
                let y = i / 32;
                if packed {
                    ((2 * x + y) & 15) | (((2 * x + y + 1) & 15) << 4)
                } else {
                    3 * x + 2 * y + 17
                }
            })
            .collect(),
    );
    uav[44] = texture(42, vec![0x95959595; 1024]);
    let mut guards = [0x5555aaaa; 8];
    Gpu::new()?.run_bound_with_constants(&dxil, &mut guards, 1, &srv, &[], &mut uav, &constants)?;
    let Binding::Texture(result) = &uav[44] else {
        return Err("missing filter output".into());
    };
    Ok(result.data.clone())
}

#[cfg(windows)]
fn run() -> Result<(), String> {
    use std::{env, fs, path::PathBuf};
    use tuxbox_xshader::{dxc, extract, gpu::Gpu, hlsl};

    let mut args = env::args_os().skip(1).map(PathBuf::from);
    let first = args.next().ok_or("expected shader and output paths")?;
    let mixed = first.as_os_str() == "--mixed" || first.as_os_str() == "--mixed-zero";
    let predicate = first.as_os_str() == "--mixed";
    let mode = first.to_string_lossy().to_string();
    let filter = mode.starts_with("--filter");
    let classifier = mode == "--classify";
    let shader = if mixed || filter || classifier {
        args.next().ok_or("expected mixed shader path")?
    } else {
        first
    };
    let output = args
        .next()
        .filter(|_| args.next().is_none())
        .ok_or("expected one output path")?;
    let bytes = fs::read(shader).map_err(|e| e.to_string())?;
    let program = extract(&bytes)?;
    let translated = hlsl::translate(&program)?;
    if classifier {
        return fs::write(output, classify::run(&translated.hlsl)?).map_err(|e| e.to_string());
    }
    if filter {
        return fs::write(
            output,
            run_filter(
                &translated.hlsl,
                mode.contains("packed"),
                mode.contains("flat"),
                mode.contains("direct"),
            )?,
        )
        .map_err(|e| e.to_string());
    }
    if mixed {
        return fs::write(output, run_mixed(&translated.hlsl, predicate)?)
            .map_err(|e| e.to_string());
    }
    let (decl, entry) = translated
        .hlsl
        .split_once("[numthreads(")
        .ok_or("missing compute entry")?;
    // D3D12 test adapter binds the single, four-byte-addressed output as a root
    // UAV. The original compute body and group dimensions come from the cache.
    let source = format!("{decl}[RootSignature(\"UAV(u0)\")]\n[numthreads({entry}");
    let dxil = dxc::compile(&source, "cs_6_0")?;
    let mut data: Vec<u32> = (0..128).map(|i| 0xfeed_0000 | i).collect();
    Gpu::new()?.run(&dxil, &mut data, 3)?;
    let bytes: Vec<_> = data.iter().flat_map(|v| v.to_le_bytes()).collect();
    fs::write(output, bytes).map_err(|e| e.to_string())
}

fn main() -> std::process::ExitCode {
    #[cfg(windows)]
    let result = run();
    #[cfg(not(windows))]
    let result: Result<(), String> = Err("ps09_compute needs a D3D12 device".into());
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(reason) => {
            eprintln!("PS09 compute probe: {reason}");
            std::process::ExitCode::FAILURE
        }
    }
}
