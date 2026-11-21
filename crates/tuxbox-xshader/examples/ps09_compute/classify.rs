//! Exercise the five bindings in compute PSO 439 using blank, high-word-only,
//! distinct material, sparse, and clipped visibility tiles.
use tuxbox_xshader::{
    dxc,
    gpu::{Binding, BufferData, Gpu, TexDim, Texture},
};

fn bytes(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|word| word.to_le_bytes()).collect()
}

fn buffer(stride: u32, values: &[u32]) -> Binding {
    Binding::Buffer(BufferData {
        data: bytes(values),
        format: 0,
        format_bytes: 0,
        stride,
    })
}

pub(super) fn run(source: &str) -> Result<Vec<u8>, String> {
    let (decl, entry) = source
        .split_once("[numthreads(")
        .ok_or("missing compute entry")?;
    let root = "DescriptorTable(SRV(t0,numDescriptors=238)), DescriptorTable(Sampler(s0)), DescriptorTable(UAV(u0,numDescriptors=25)), UAV(u0,space=1), RootConstants(num32BitConstants=4,b17)";
    let dxil = dxc::compile(
        &format!("{decl}[RootSignature(\"{root}\")]\n[numthreads({entry}"),
        "cs_6_0",
    )?;
    let device = Gpu::new()?;
    let mut srv = vec![Binding::Null; 238];
    let mut materials = vec![0u32; 33 * 84];
    materials[84 + 0x138 / 4] = 0x88; // low half of flag word → bin bit 0
    materials[2 * 84 + 0x8c / 4] = 0x5500; // second byte → bin bit 2
    materials[32 * 84 + 0x58 / 4] = 1; // index into t72
    let mut geometry = vec![0; 2 * 44];
    geometry[44 + 1] = 0x44; // nonzero geometry field → bin bit 1
    srv[71] = buffer(336, &materials);
    srv[72] = buffer(176, &geometry);
    let mut all = Vec::new();
    for mode in 0..5 {
        let mut pixels = [0u32; 128];
        for (lane, pixel) in pixels.chunks_exact_mut(2).enumerate() {
            let x = lane % 8;
            match mode {
                1 => pixel[1] = 1,
                2 => pixel[0] = (if x < 4 { 1 } else { 2 }) << 27,
                3 if lane == 63 => pixel[1] = 1,
                4 if lane == 18 || lane == 20 => pixel[0] = 1 << 27,
                4 if lane == 27 || x > 4 || lane >= 32 => pixel[0] = 2 << 27,
                _ => {}
            }
        }
        srv[237] = Binding::Texture(Texture {
            dim: TexDim::D2,
            format: 17,
            texel_bytes: 8,
            width: 8,
            height: 8,
            depth: 1,
            mips: 1,
            data: bytes(&pixels),
        });
        let mut words = [0x77777777; 128];
        words[..8].fill(8);
        let mut uav = vec![Binding::Null; 25];
        uav[24] = buffer(4, &words);
        let constants = if mode == 4 {
            [5, 4, 16, 0]
        } else {
            [8, 8, 16, 0]
        };
        let mut guards = [0xfeedcafe; 8];
        device.run_bound_with_constants(&dxil, &mut guards, 1, &srv, &[], &mut uav, &constants)?;
        if guards != [0xfeedcafe; 8] {
            return Err("unexpected root UAV write".into());
        }
        let Binding::Buffer(result) = &uav[24] else {
            return Err("missing classifier output".into());
        };
        all.extend_from_slice(&result.data);
    }
    Ok(all)
}
