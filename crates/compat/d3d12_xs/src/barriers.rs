//! Legacy x64 D3D12 resource barriers. Only transitions have resource states.
use core::ffi::c_void;
use core::mem::MaybeUninit;
use std::borrow::Cow;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone, Copy)]
#[repr(C)]
pub(crate) struct Transition {
    pub resource: *mut c_void,
    pub subresource: u32,
    pub before: u32,
    pub after: u32,
}

#[derive(Clone, Copy)]
#[repr(C)]
union Payload {
    transition: Transition,
    // Preserve inactive union bytes and padding when copying aliasing or UAV barriers.
    opaque: [MaybeUninit<u64>; 3],
}

#[derive(Clone, Copy)]
#[repr(C)]
pub(crate) struct Barrier {
    kind: u32,
    flags: u32,
    payload: Payload,
}

const _: () = assert!(size_of::<Barrier>() == 32 && align_of::<Barrier>() == 8);

impl Barrier {
    pub fn transition(&self) -> Option<Transition> {
        // D3D12_RESOURCE_BARRIER_TYPE_TRANSITION selects the active union member.
        (self.kind == 0).then(|| unsafe { self.payload.transition })
    }

    pub fn completed_state(&self, transition: Transition) -> Option<u32> {
        const BEGIN_ONLY: u32 = 1;
        (self.flags != BEGIN_ONLY).then_some(transition.after)
    }
}

// The host and both trackers consume this same batch. Allocate at the first change.
pub(crate) unsafe fn translate(input: &[Barrier]) -> Cow<'_, [Barrier]> {
    let mut output: Option<Vec<Barrier>> = None;
    for (i, &barrier) in input.iter().enumerate() {
        let mut host = barrier;
        if let Some(mut t) = barrier.transition() {
            if (t.before | t.after) & crate::device_wrap::BUFFER_ONLY_STATES != 0
                && !t.resource.is_null()
                && !crate::device_wrap::is_buffer(t.resource)
            {
                let batch = output.get_or_insert_with(|| {
                    let mut batch = Vec::with_capacity(input.len());
                    batch.extend_from_slice(&input[..i]);
                    batch
                });
                let before = crate::device_wrap::texture_state(t.before);
                let after = crate::device_wrap::texture_state(t.after);
                static LOGGED: AtomicUsize = AtomicUsize::new(0);
                if LOGGED.fetch_add(1, Ordering::Relaxed) < 4 {
                    compat_common::record(
                        "d3d12_xs",
                        "ResourceBarrier",
                        &format!(
                            "texture {:p}: {:#x}->{:#x} becomes {before:#x}->{after:#x}",
                            t.resource, t.before, t.after
                        ),
                    );
                }
                if before != after {
                    t.before = before;
                    t.after = after;
                    host.payload = Payload { transition: t };
                    batch.push(host);
                }
                continue;
            }
        }
        if let Some(batch) = &mut output {
            batch.push(host);
        }
    }
    match output {
        Some(batch) => Cow::Owned(batch),
        None => Cow::Borrowed(input),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[repr(C)]
    struct Resource {
        vtable: *const usize,
        dimension: u32,
    }

    unsafe extern "system" fn get_desc(res: *const Resource, desc: *mut u64) -> *mut u64 {
        desc.write(u64::from((*res).dimension));
        desc
    }

    fn transition(resource: &Resource, flags: u32, before: u32, after: u32) -> Barrier {
        let t = Transition {
            resource: core::ptr::from_ref(resource).cast_mut().cast(),
            subresource: 7,
            before,
            after,
        };
        Barrier {
            kind: 0,
            flags,
            payload: Payload { transition: t },
        }
    }

    #[test]
    fn mixed_batches_preserve_order_and_split_flags() {
        let mut vt = [0; 11];
        vt[10] = get_desc as *const () as usize;
        let texture = Resource {
            vtable: vt.as_ptr(),
            dimension: 3,
        };
        let buffer = Resource {
            vtable: vt.as_ptr(),
            dimension: 1,
        };
        let opaque = |kind| Barrier {
            kind,
            flags: 0,
            payload: Payload {
                opaque: [MaybeUninit::new(10 + kind as u64); 3],
            },
        };
        let batch = [
            opaque(2),
            transition(&texture, 1, 0x81, 0x80),
            transition(&texture, 2, 0x81, 0x80),
            transition(&texture, 1, 4, 0x8c3),
            opaque(1),
            transition(&buffer, 0, 0x81, 0x80),
            transition(&texture, 2, 4, 0x8c3),
        ];
        let out = unsafe { translate(&batch) };
        assert_eq!(
            out.iter().map(|b| (b.kind, b.flags)).collect::<Vec<_>>(),
            [(2, 0), (0, 1), (1, 0), (0, 0), (0, 2)]
        );
        for (i, marker) in [(0, 12), (2, 11)] {
            // These opaque fixture records have every payload byte initialized.
            assert_eq!(
                unsafe { out[i].payload.opaque.map(|v| v.assume_init()) },
                [marker; 3]
            );
        }
        for i in [1, 4] {
            let t = out[i].transition().unwrap();
            assert_eq!((t.before, t.after, t.subresource), (4, 0x8c0, 7));
            assert_eq!(
                out[i].completed_state(t),
                if i == 1 { None } else { Some(0x8c0) }
            );
        }
        let t = out[3].transition().unwrap();
        assert_eq!((t.before, t.after), (0x81, 0x80));
        assert_eq!(batch[3].transition().unwrap().after, 0x8c3);
        assert!(unsafe { translate(&batch[1..3]) }.is_empty());
        assert!(matches!(
            unsafe { translate(&batch[4..6]) },
            Cow::Borrowed(_)
        ));
    }
}
