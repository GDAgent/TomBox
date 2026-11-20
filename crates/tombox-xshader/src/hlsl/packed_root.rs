//! Some Scarlett compute shaders load root descriptors and inline constants from one
//! argument block pointed to by s[0:1]. Track the origin of the descriptor words so
//! MUBUF operations can use desktop root SRVs/UAVs without exposing a GPU address.
use super::*;
use crate::meta::RootParamKind;

#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
struct Binding {
    slot: u32,
    uav: bool,
}

impl Binding {
    fn register(self) -> String {
        format!("{}{}", if self.uav { "u" } else { "t" }, self.slot)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Word {
    Data,
    Immediate(u32),
    PointerLo,
    PointerHi,
    AddressLo(Binding),
    AddressHi(Binding, u32), // descriptor stride in bytes
    Unavailable,
}

impl Word {
    fn opaque(self) -> bool {
        !matches!(self, Self::Data | Self::Immediate(_))
    }
}

#[derive(Clone, Copy)]
enum Argument {
    Descriptor(Word),
    Constant(u32, u32), // shader register and dword offset
}

#[derive(Default)]
pub(super) struct PackedRoot {
    loads: BTreeMap<usize, Vec<Option<(u32, u32)>>>,
    buffers: BTreeMap<usize, (Binding, u32)>,
}

type State = [Word; 106];

fn value(state: &State, op: Opnd) -> Word {
    match op {
        Opnd::Sgpr(n) => state.get(n as usize).copied().unwrap_or(Word::Unavailable),
        Opnd::ConstInt(i) => Word::Immediate(i as u32),
        Opnd::Literal(w) => Word::Immediate(w),
        _ => Word::Data,
    }
}

fn merge(state: &mut State, other: &State) {
    for (a, &b) in state.iter_mut().zip(other) {
        if *a != b {
            *a = if a.opaque() || b.opaque() {
                Word::Unavailable
            } else {
                Word::Data
            };
        }
    }
}

impl PackedRoot {
    pub(super) fn analyze(ex: &ShaderExtract, insns: &[Insn]) -> Result<Option<Self>, String> {
        let Some(root) = &ex.root_sig else { return Ok(None) };
        // This executable layout is corroborated by PSV thread dimensions and the
        // compiler prologue which keeps the low eight bits of the argument pointer’s high word.
        if ex.stage != Stage::Compute
            || root.flags != 0x0300_0000
            || ex.psv_threads.is_none()
            || !insns
                .iter()
                .find(|i| !matches!(i.mnemonic.as_str(), "v_nop" | "s_nop" | "s_inst_prefetch"))
                .is_some_and(|i| i.mnemonic == "s_and_b32" && i.src == [Opnd::Sgpr(1), Opnd::Literal(255)])
        {
            return Ok(None);
        }
        let mut args = Vec::new();
        for (i, p) in root.params.iter().enumerate() {
            match p.kind {
                RootParamKind::Descriptor {
                    kind: k @ (3 | 4),
                    register,
                    space: 0,
                } => {
                    let b = Binding {
                        slot: register,
                        uav: k == 4,
                    };
                    args.extend([
                        Argument::Descriptor(Word::AddressLo(b)),
                        Argument::Descriptor(Word::AddressHi(b, 0)),
                    ]);
                }
                RootParamKind::Constants {
                    register,
                    space: 0,
                    num32,
                } if i + 1 == root.params.len() && (1..=64).contains(&num32) => {
                    args.extend((0..num32).map(|k| Argument::Constant(register, k)));
                }
                _ => return Err("unsupported packed compute root parameter".into()),
            }
            if args.len() > 64 {
                return Err("packed root arguments exceed 64 dwords".into());
            }
        }
        let mut plan = Self::default();
        let mut state = [Word::Unavailable; 106];
        state[4] = Word::Data; // Group ID X in the observed one-dimensional dispatch layout.
        state[0] = Word::PointerLo;
        state[1] = Word::PointerHi;
        let mut joins = BTreeMap::<usize, State>::new();
        let mut falls = true;
        for (idx, ins) in insns.iter().enumerate() {
            if let Some(incoming) = joins.remove(&ins.offset) {
                if falls {
                    merge(&mut state, &incoming);
                } else {
                    state = incoming;
                }
                falls = true;
            }
            if !falls {
                // The common HLSL emitter visits unreachable blocks too.
                return Err("unreachable block in packed compute shader".into());
            }
            if ins.enc == Enc::Sopp && Emitter::branch_cond(ins.op).is_some() {
                let target = ins.offset as i64 + 4 + 4 * ins.aux.imm;
                if target <= ins.offset as i64 {
                    return Err("backward branch in packed compute shader".into());
                }
                joins
                    .entry(target as usize)
                    .and_modify(|prior| merge(prior, &state))
                    .or_insert(state);
                falls = ins.op != 2;
            } else if ins.mnemonic == "s_endpgm" {
                falls = false;
            }
            plan.step(idx, ins, &args, &mut state)
                .map_err(|e| format!("packed roots at {:#x} ({}): {e}", ins.offset, ins.mnemonic))?;
        }
        if !joins.is_empty() {
            return Err("branch target outside packed shader".into());
        }
        Ok(Some(plan))
    }

    fn step(&mut self, idx: usize, ins: &Insn, args: &[Argument], state: &mut State) -> Result<(), &'static str> {
        let m = ins.mnemonic.as_str();
        if m == "v_nop" {
            return Ok(());
        }
        let s = &ins.src;
        let dst = match ins.dst.first() {
            Some(Opnd::Sgpr(n)) => Some(*n as usize),
            _ => None,
        };
        match ins.enc {
            Enc::Smem => {
                let Some(Opnd::Sgpr(base)) = s.first() else {
                    return Err("bad scalar load base");
                };
                if ins.op > 4
                    || state.get(*base as usize..*base as usize + 2) != Some(&[Word::PointerLo, Word::PointerHi])
                    || ins.raw.get(1).ok_or("truncated scalar load")? >> 25 != 0x7d
                    || ins.aux.imm < 0
                    || ins.aux.imm % 4 != 0
                {
                    return Err("unsupported root argument load");
                }
                let first = ins.aux.imm as usize / 4;
                if first >= args.len() {
                    return Err("root argument offset outside signature");
                }
                let d = dst.ok_or("bad scalar load destination")?;
                let words = state
                    .get_mut(d..d + (1 << ins.op))
                    .ok_or("scalar load beyond register file")?;
                let mut loads = Vec::new();
                for (k, word) in words.iter_mut().enumerate() {
                    let arg = args.get(first + k);
                    *word = match arg {
                        Some(Argument::Constant(..)) => Word::Data,
                        Some(Argument::Descriptor(w)) => *w,
                        None => Word::Unavailable, // unused tail of a vectorized constant load
                    };
                    loads.push(match arg {
                        Some(Argument::Constant(reg, off)) => Some((*reg, *off)),
                        _ => None,
                    });
                }
                self.loads.insert(idx, loads);
                return Ok(());
            }
            Enc::Mubuf => {
                let Some(Opnd::Sgpr(rs)) = s.get(1) else {
                    return Err("bad MUBUF descriptor");
                };
                let Some(
                    [Word::AddressLo(lo), Word::AddressHi(hi, stride), Word::Immediate(u32::MAX), Word::Immediate(0x1101_4fac)],
                ) = state.get(*rs as usize..*rs as usize + 4)
                else {
                    return Err("unresolved or unsupported buffer descriptor");
                };
                if lo != hi
                    || ins.raw[0] & (1 << 16) != 0
                    || ins.raw.get(1).ok_or("truncated buffer instruction")? & (1 << 23) != 0
                    || !matches!(s.get(2), Some(Opnd::ConstInt(0)))
                    || ins.aux.mask & 3 == 3
                {
                    return Err("unsupported root buffer addressing");
                }
                if !(m.starts_with("buffer_load_dword") || (lo.uav && m.starts_with("buffer_store_dword"))) {
                    return Err("unsupported root buffer operation");
                }
                self.buffers.insert(idx, (*lo, *stride));
                return Ok(());
            }
            Enc::Mtbuf | Enc::Flat | Enc::Mimg => return Err("unsupported packed resource access"),
            _ => {}
        }
        // Only single-word ALU operands have provenance here. SCC can depend on
        // the true pointer high bits after masking, and VOP3 carry instructions
        // can write a second scalar destination that the decoder leaves implicit.
        if m.split('_')
            .any(|part| matches!(part, "b64" | "f64" | "u64" | "i64" | "co"))
            || s.contains(&Opnd::Scc)
        {
            return Err("implicit register pair, carry, or SCC in packed shader");
        }
        match ins.enc {
            Enc::Sop1
                if !matches!(
                    m,
                    "s_mov_b32"
                        | "s_bitset1_b32"
                        | "s_not_b32"
                        | "s_brev_b32"
                        | "s_bcnt1_i32_b32"
                        | "s_ff1_i32_b32"
                        | "s_abs_i32"
                ) =>
            {
                return Err("unsupported scalar operation on packed roots")
            }
            Enc::Sop2 if matches!(m, "s_cselect_b32" | "s_addc_u32" | "s_subb_u32") => {
                return Err("SCC may depend on a root address")
            }
            Enc::Sopk => return Err("unsupported implicit scalar operand on packed roots"),
            Enc::Sopp
                if !matches!(
                    m,
                    "s_branch"
                        | "s_cbranch_execz"
                        | "s_cbranch_execnz"
                        | "s_endpgm"
                        | "s_nop"
                        | "s_waitcnt"
                        | "s_inst_prefetch"
                ) =>
            {
                return Err("unsupported packed control flow")
            }
            _ => {}
        }
        if let Some(d) = dst {
            let source = s.first().map(|o| value(state, *o));
            let rhs = s.get(1).map(|o| value(state, *o));
            let special = match (m, source, rhs) {
                ("s_mov_b32", v, _) => v,
                ("s_and_b32", Some(Word::PointerHi), Some(Word::Immediate(255))) => Some(Word::PointerHi),
                ("s_bitset1_b32", Some(Word::Immediate(bit @ 16..=29)), _) => match state[d] {
                    Word::AddressHi(b, old) => Some(Word::AddressHi(b, old | (1 << (bit - 16)))),
                    _ => None,
                },
                ("s_or_b32", Some(Word::AddressHi(b, old)), Some(Word::Immediate(bits)))
                    if bits & !0x3fff0000 == 0 =>
                {
                    Some(Word::AddressHi(b, old | (bits >> 16)))
                }
                _ => None,
            };
            if m == "s_bitset1_b32" && special.is_none() && state[d].opaque() {
                return Err("unsupported mutation of a root address");
            }
            if let Some(w) = special {
                state[d] = w;
                return Ok(());
            }
        }
        // Unused trailing VOP3 source fields encode s0. Only inspect actual operands.
        let sources = if ins.enc == Enc::Vop3
            && (ins.op < 0x100
                || matches!(
                    m,
                    "v_add_f32" | "v_sub_f32" | "v_mul_legacy_f32" | "v_mul_hi_u32" | "v_mul_lo_u32"
                )) {
            &s[..2.min(s.len())]
        } else {
            s.as_slice()
        };
        if sources.iter().any(|o| value(state, *o).opaque()) {
            return Err("root address or padding used as shader data");
        }
        if let Some(d) = dst {
            state[d] = Word::Data;
        }
        Ok(())
    }

    pub(super) fn declarations(&self, h: &mut String) {
        for binding in self.buffers.values().map(|(b, _)| *b).collect::<BTreeSet<_>>() {
            let reg = binding.register();
            let rw = if binding.uav { "RW" } else { "" };
            writeln!(h, "{rw}ByteAddressBuffer root_{reg} : register({reg});").unwrap();
        }
    }
}

impl Emitter<'_> {
    pub(super) fn resolve_packed(&mut self) -> Result<bool, String> {
        self.packed = PackedRoot::analyze(self.ex, &self.insns)?;
        let Some(ref plan) = self.packed else { return Ok(false) };
        for &(reg, off) in plan.loads.values().flatten().flatten() {
            let size = self.cb_size.entry(reg).or_default();
            *size = (*size).max(off + 1);
        }
        Ok(true)
    }

    pub(super) fn emit_packed_load(&mut self, idx: usize, ins: &Insn) -> Result<(), String> {
        let words = self
            .packed
            .as_ref()
            .and_then(|p| p.loads.get(&idx))
            .ok_or("unresolved packed argument load")?
            .clone();
        let d = sreg_num(&ins.dst[0]).ok_or("bad packed load destination")?;
        for (k, w) in words.into_iter().enumerate() {
            let v = w.map_or_else(
                || "0u".to_string(),
                |(reg, off)| format!("cb{reg}[{}].{}", off / 4, XYZW[off as usize % 4]),
            );
            self.ws(&sreg_opnd(d + k as u8), &v)?;
        }
        Ok(())
    }

    pub(super) fn emit_packed_buffer(&mut self, idx: usize, ins: &Insn) -> Result<(), String> {
        let &(binding, stride) = self
            .packed
            .as_ref()
            .and_then(|p| p.buffers.get(&idx))
            .ok_or("unresolved packed buffer")?;
        let n = Self::mubuf_dwords(&ins.mnemonic).ok_or("bad root buffer width")? as u8;
        let Opnd::Vgpr(vd) = ins.dst[0] else {
            return Err("bad root buffer data register".into());
        };
        let t = self.tmp;
        self.tmp += 1;
        let offset = ins.aux.imm;
        let address = match ins.aux.mask & 3 {
            0 => format!("{offset}u"),
            1 => format!("{offset}u + {}", r(&ins.src[0])?),
            2 => format!("{offset}u + {stride}u * {}", r(&ins.src[0])?),
            _ => return Err("unsupported root buffer addressing".into()),
        };
        self.line(&format!("uint t{t}addr = {address};"));
        let resource = format!("root_{}", binding.register());
        let suffix = if n == 1 { String::new() } else { n.to_string() };
        if ins.mnemonic.starts_with("buffer_load") {
            self.line("if (exec_lo & 1u) {");
            self.line(&format!("uint{suffix} t{t}load = {resource}.Load{suffix}(t{t}addr);"));
            for k in 0..n {
                let sw = if n == 1 {
                    String::new()
                } else {
                    format!(".{}", XYZW[k as usize])
                };
                self.ws(
                    &Opnd::Vgpr(vd.checked_add(k).ok_or("root load exceeds VGPRs")?),
                    &format!("t{t}load{sw}"),
                )?;
            }
            self.line("}");
        } else {
            let mut words = Vec::new();
            for k in 0..n {
                let v = Opnd::Vgpr(vd.checked_add(k).ok_or("root store exceeds VGPRs")?);
                self.touch(&v);
                words.push(r(&v)?);
            }
            self.line(&format!(
                "if (exec_lo & 1u) {resource}.Store{suffix}(t{t}addr, uint{suffix}({}));",
                words.join(", ")
            ));
        }
        Ok(())
    }
}
