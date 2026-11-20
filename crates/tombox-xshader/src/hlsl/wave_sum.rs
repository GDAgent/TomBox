//! The BVH kernels use two fixed-width integer reduction sequences. Keep their
//! register writes (including the half-wave sum) observable after the reduction.
use super::{Emitter, Enc, Insn, Opnd};

pub(super) const DPP_ADD: u32 = 0x7ff1;
pub(super) const READ_LANE: u32 = 0x7ff2;

pub(super) fn lower(insns: &mut [Insn], first: usize) -> Result<(u32, usize), &'static str> {
    let start = first.checked_sub(2).ok_or("missing integer sum prologue")?;
    let tail = &insns[start..];
    let save = &tail[0];
    let width = match save.mnemonic.as_str() {
        "s_or_saveexec_b32" => 32,
        "s_or_saveexec_b64" => 64,
        _ => return Err("unrecognized integer DPP add sequence"),
    };
    let last_read = if width == 32 { 8 } else { 10 };
    let block = tail.get(..last_read + 2).ok_or("truncated integer sum")?;
    let [Opnd::Sgpr(saved)] = save.dst[..] else {
        return Err("invalid saved EXEC register");
    };
    let [Opnd::Vgpr(acc)] = block[1].dst[..] else {
        return Err("invalid accumulator");
    };
    if save.src != [Opnd::ConstInt(-1)]
        || block[1].mnemonic != "v_cndmask_b32"
        || !matches!(block[1].src[..], [Opnd::ConstInt(0), Opnd::Vgpr(_), Opnd::Sgpr(n)] if n == saved)
    {
        return Err("invalid masked integer sum input");
    }

    // The first three DPP8 shuffles accumulate pairs, quads and octets.
    // DPP16 row_mirror combines the two octets in each 16-lane row.
    let controls = [0xde54c1, 0x936012, 0x000924, 0xff0940];
    for (i, (&ctrl, ins)) in controls.iter().zip(&block[2..6]).enumerate() {
        let selector = if i == 3 { 250 } else { 233 };
        if ins.mnemonic != "v_add_nc_u32"
            || ins.dst != [Opnd::Vgpr(acc)]
            || ins.src != [Opnd::Raw(selector), Opnd::Vgpr(acc)]
            || ins.raw.get(1) != Some(&(ctrl << 8 | u32::from(acc)))
        {
            return Err("unrecognized DPP add shuffle");
        }
    }
    let perm = &block[6];
    let [tmp @ Opnd::Vgpr(_)] = perm.dst[..] else {
        return Err("invalid cross-row destination");
    };
    let vr = Opnd::Vgpr(acc);
    if perm.mnemonic != "v_permlanex16_b32"
        || perm.src != [vr, Opnd::ConstInt(0), Opnd::ConstInt(0)]
        || block[7].mnemonic != "v_add_nc_u32"
        || block[7].dst != [vr]
        || block[7].src != [tmp, vr]
    {
        return Err("unrecognized cross-row add");
    }
    let lane_read = |ins: &Insn, lane| match ins.dst[..] {
        [Opnd::Vgpr(d @ 0..=105)]
            if ins.mnemonic == "v_readlane_b32"
                && ins.src.first() == Some(&vr)
                && ins.src.get(1) == Some(&Opnd::ConstInt(lane))
                && ins.raw.as_slice()
                    == [
                        0xd760_0000 | u32::from(d),
                        ((128 + lane as u32) << 9) | 256 | u32::from(acc),
                    ] =>
        {
            Ok(d)
        }
        _ => Err("invalid integer sum lane read"),
    };
    if width == 64 {
        let high = lane_read(&block[8], 32)?;
        if block[9].mnemonic != "v_add_nc_u32" || block[9].dst != [vr] || block[9].src != [Opnd::Sgpr(high), vr] {
            return Err("missing upper-half add");
        }
    }
    lane_read(&block[last_read], 0)?;
    let restore = &block[last_read + 1];
    if restore.mnemonic != (if width == 32 { "s_mov_b32" } else { "s_mov_b64" })
        || restore.dst != [Opnd::ExecLo]
        || restore.src != [Opnd::Sgpr(saved)]
    {
        return Err("integer sum does not restore EXEC");
    }

    for (step, ins) in insns[first..first + 4].iter_mut().enumerate() {
        ins.op = DPP_ADD;
        ins.src = vec![vr];
        ins.aux.imm = step as i64;
    }
    // READLANE's encoded destination names an SGPR. Mark just the actual
    // broadcasts here so the independent scalar lane-spill pass ignores them.
    for i in (8..=last_read).step_by(2) {
        let ins = &mut insns[start + i];
        let Opnd::Vgpr(dst) = ins.dst[0] else {
            return Err("invalid lane result");
        };
        ins.op = READ_LANE;
        ins.dst[0] = Opnd::Sgpr(dst);
        ins.src.truncate(2);
    }
    Ok((width, start + last_read + 2))
}

/// The branch emitter predicates regions per host invocation. HLSL lane reads
/// require the source invocation to participate, including lane zero when guest
/// EXEC has masked it. A branch around or into the region can violate that.
pub(super) fn check_control_flow(insns: &[Insn], width: u32) -> Result<(), String> {
    for (first, ins) in insns.iter().enumerate() {
        if ins.op != DPP_ADD || ins.aux.imm != 0 {
            continue;
        }
        let start = insns[first - 2].offset as i64;
        let end = insns[first + if width == 32 { 7 } else { 9 }].offset as i64;
        for branch in insns.iter().filter(|i| i.enc == Enc::Sopp) {
            let pc = branch.offset as i64;
            if branch.op == 1 && pc < start {
                return Err(format!("integer wave sum at {start:#x} after an early exit"));
            }
            if Emitter::branch_cond(branch.op).is_some() {
                let target = pc + 4 + 4 * branch.aux.imm;
                if (pc < start && target > start) || (pc > end && target <= end) {
                    return Err(format!(
                        "integer wave sum at {start:#x} may execute in divergent control flow"
                    ));
                }
            }
        }
    }
    Ok(())
}
