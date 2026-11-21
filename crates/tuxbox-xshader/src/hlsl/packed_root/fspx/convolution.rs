//! The audited stride-two pass has one tensor UAV and one read-only weight
//! buffer. Every bias/weight scalar load is a fixed offset into t18.
use super::*;
use crate::meta::RootRange;

pub(super) fn bindings(
    ex: &ShaderExtract,
    insns: &[Insn],
    uav: &RootRange,
    srv: &RootRange,
) -> Result<PackedRoot, String> {
    let resources = [(2, 0), (4, 18), (7, 11)];
    if uav.base_register != 0
        || uav.num != 13
        || srv.base_register != 0
        || srv.num != 21
        || !matches!(srv.offset_in_table, 0 | u32::MAX)
        || ex.resources.len() != 3
        || !ex.resources.iter().zip(resources).all(|(r, (ty, reg))| {
            r.res_type == ty && r.space == 0 && r.lower == reg && r.upper == reg
        })
    {
        return Err("unsupported FSPX convolution resources".into());
    }
    let mut plan = PackedRoot {
        abi: RootAbi::RegisterWave32,
        ..PackedRoot::default()
    };
    let weights = Binding {
        slot: 18,
        uav: false,
    };
    let features = Binding {
        slot: 11,
        uav: true,
    };
    for (idx, ins) in insns.iter().enumerate() {
        match (ins.enc, ins.mnemonic.as_str(), ins.offset) {
            (Enc::Smem, "s_buffer_load_dwordx2", 0x38) => {
                plan.loads.insert(idx, vec![Some((0, 16)), Some((0, 17))]);
            }
            (Enc::Smem, "s_load_dwordx4", 0x7c | 0x108) => {
                plan.loads.insert(idx, vec![None; 4]);
            }
            (Enc::Smem, "s_buffer_load_dwordx16", _) => {
                let offset = u32::try_from(ins.aux.imm).map_err(|_| "invalid weight offset")?;
                plan.raw_scalar.insert(idx, (weights, offset, 16));
            }
            (Enc::Mubuf, "buffer_load_dwordx4" | "buffer_store_dwordx4", _) => {
                plan.buffers.insert(idx, (features, 0));
            }
            (Enc::Smem | Enc::Mubuf | Enc::Mimg, _, _) => {
                return Err("unexpected FSPX convolution access".into())
            }
            _ => {}
        }
    }
    Ok(plan)
}
