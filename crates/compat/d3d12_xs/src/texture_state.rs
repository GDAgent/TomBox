//! Explicit transitions for CPU-mirrored placed textures. A recording may be
//! reset without execution, submitted later, or submitted more than once.
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

pub(crate) static TRACKER: LazyLock<Mutex<Tracker>> = LazyLock::new(Default::default);

#[derive(Clone, Copy)]
struct Submitted {
    queue: usize,       // zero until the first explicit transition
    state: Option<u32>, // None between BEGIN_ONLY and END_ONLY
}

struct Texture {
    generation: u64,
    simultaneous: bool,
    subs: Vec<Submitted>,
}

#[derive(Clone, Copy)]
struct Transition {
    resource: usize,
    generation: u64,
    sub: u32,
    after: Option<u32>,
}

#[derive(Default)]
pub(crate) struct Tracker {
    generation: u64,
    textures: HashMap<usize, Texture>,
    lists: HashMap<usize, Vec<Transition>>,
}

impl Tracker {
    pub fn register(&mut self, resource: usize, count: usize, initial: u32, simultaneous: bool) {
        self.generation += 1;
        self.textures.insert(
            resource,
            Texture {
                generation: self.generation,
                simultaneous,
                subs: vec![
                    Submitted {
                        queue: 0,
                        state: Some(initial)
                    };
                    count
                ],
            },
        );
    }

    pub fn forget(&mut self, resource: usize) {
        self.textures.remove(&resource);
    }

    pub fn reset(&mut self, list: usize) {
        self.lists.remove(&list);
    }

    pub fn record(&mut self, list: usize, resource: usize, sub: u32, after: Option<u32>) {
        if let Some(texture) = self.textures.get(&resource) {
            self.lists.entry(list).or_default().push(Transition {
                resource,
                generation: texture.generation,
                sub,
                after,
            });
        }
    }

    pub fn submit(&mut self, queue: usize, list: usize) {
        let Some(changes) = self.lists.get(&list) else {
            return;
        };
        for change in changes {
            let Some(texture) = self.textures.get_mut(&change.resource) else {
                continue;
            };
            if texture.generation != change.generation {
                continue; // The resource pointer has been recycled.
            }
            let state = Submitted {
                queue,
                state: change
                    .after
                    .map(|after| if texture.simultaneous { 0 } else { after }),
            };
            if change.sub == u32::MAX {
                texture.subs.fill(state);
            } else if let Some(sub) = texture.subs.get_mut(change.sub as usize) {
                *sub = state;
            }
        }
    }

    pub fn upload_state(&self, resource: usize, sub: u32, queue: usize) -> Option<u32> {
        let state = self.textures.get(&resource)?.subs.get(sub as usize)?;
        // A different queue needs a GPU fence handoff. Do not infer one from CPU call order.
        if state.queue != 0 && state.queue != queue {
            return None;
        }
        state.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const QUEUE: usize = 1;
    const TEXTURE: usize = 42;
    const READ: u32 = 0x80;
    const COPY_SOURCE: u32 = 0x800;
    const COPY_DEST: u32 = 0x400;

    #[test]
    fn split_barriers_block_only_pending_subresources_until_submitted_end() {
        let mut t = Tracker::default();
        t.register(TEXTURE, 2, READ, false);
        t.record(10, TEXTURE, 1, None);
        t.record(11, TEXTURE, 1, Some(COPY_SOURCE));
        t.submit(QUEUE, 10);
        assert_eq!(t.upload_state(TEXTURE, 1, QUEUE), None);
        assert_eq!(t.upload_state(TEXTURE, 0, QUEUE), Some(READ));
        t.submit(QUEUE, 11);
        assert_eq!(t.upload_state(TEXTURE, 1, QUEUE), Some(COPY_SOURCE));
        assert_eq!(t.upload_state(TEXTURE, 1, QUEUE + 1), None);
    }

    #[test]
    fn reset_discards_commands_and_replay_applies_all_subresources() {
        let mut t = Tracker::default();
        t.register(TEXTURE, 2, READ, false);
        t.record(10, TEXTURE, u32::MAX, Some(COPY_SOURCE));
        t.record(11, TEXTURE, 1, Some(COPY_DEST));
        t.submit(QUEUE, 10);
        t.submit(QUEUE, 11);
        assert_eq!(t.upload_state(TEXTURE, 0, QUEUE), Some(COPY_SOURCE));
        assert_eq!(t.upload_state(TEXTURE, 1, QUEUE), Some(COPY_DEST));
        t.reset(11);
        t.submit(QUEUE, 10);
        t.submit(QUEUE, 11);
        assert_eq!(t.upload_state(TEXTURE, 1, QUEUE), Some(COPY_SOURCE));
    }

    #[test]
    fn recycled_resources_ignore_old_lists_and_simultaneous_textures_decay() {
        let mut t = Tracker::default();
        t.register(TEXTURE, 1, READ, false);
        t.record(10, TEXTURE, 0, Some(COPY_DEST));
        t.forget(TEXTURE);
        t.register(TEXTURE, 1, READ, true);
        t.submit(QUEUE, 10);
        assert_eq!(t.upload_state(TEXTURE, 0, QUEUE), Some(READ));
        t.record(11, TEXTURE, 0, Some(COPY_SOURCE));
        t.submit(QUEUE, 11);
        assert_eq!(t.upload_state(TEXTURE, 0, QUEUE), Some(0));
    }
}
