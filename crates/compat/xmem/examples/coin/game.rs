//! Shared rules and XMem-backed playfield for both homebrew frontends.
use std::ptr::NonNull;

#[link(name = "xmem", kind = "raw-dylib")]
extern "system" {
    fn XMemAlloc(size: usize, attributes: u64) -> *mut u8;
    fn XMemFree(address: *mut u8, attributes: u64);
}

pub const WIDTH: usize = 4;
const MAP: &[u8; 12] = b"...*..#.*...";
const ATTRIBUTES: u64 = 0x0480009e;

pub struct Game {
    tiles: NonNull<[u8; MAP.len()]>,
    pub player: usize,
    pub coins: usize,
}

impl Game {
    pub fn new() -> Option<Self> {
        // SAFETY: allocate enough writable space and initialize the entire array.
        let tiles =
            NonNull::new(unsafe { XMemAlloc(MAP.len(), ATTRIBUTES) }.cast::<[u8; MAP.len()]>())?;
        let mut game = Self {
            tiles,
            player: 0,
            coins: 0,
        };
        game.restart();
        Some(game)
    }

    fn restart(&mut self) {
        // SAFETY: the allocation holds a whole board, including on first initialization.
        unsafe { self.tiles.as_ptr().write(*MAP) };
        self.player = 0;
        self.coins = MAP.iter().filter(|&&tile| tile == b'*').count();
    }

    pub fn tiles(&self) -> &[u8] {
        // SAFETY: Game owns this initialized allocation until drop.
        unsafe { self.tiles.as_ref() }
    }

    pub fn step(&mut self, key: u8) -> bool {
        let p = self.player;
        let next = match key.to_ascii_lowercase() {
            b'r' => {
                self.restart();
                return true;
            }
            b'w' if p >= WIDTH => p - WIDTH,
            b's' if p + WIDTH < MAP.len() => p + WIDTH,
            b'a' if p % WIDTH > 0 => p - 1,
            b'd' if p % WIDTH < WIDTH - 1 => p + 1,
            _ => return false,
        };
        if self.coins == 0 || self.tiles()[next] == b'#' {
            return false;
        }
        self.player = next;
        if self.tiles()[next] == b'*' {
            // SAFETY: only the mutably borrowed Game can write the board.
            unsafe { self.tiles.as_mut()[next] = b'.' };
            self.coins -= 1;
        }
        true
    }
}

impl Drop for Game {
    fn drop(&mut self) {
        // SAFETY: return the allocation once with its original attributes.
        unsafe { XMemFree(self.tiles.as_ptr().cast(), ATTRIBUTES) };
    }
}
