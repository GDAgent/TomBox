//! Synthetic x64 Windows title exercising TomBox's xmem.dll.
use std::io::{self, BufRead, Write};
use std::process::ExitCode;

#[link(name = "xmem", kind = "raw-dylib")]
extern "system" {
    fn XMemAlloc(size: usize, attributes: u64) -> *mut u8;
    fn XMemFree(address: *mut u8, attributes: u64);
}

const WIDTH: usize = 4;
const MAP: &[u8; 12] = b"...*..#.*...";
const ATTRIBUTES: u64 = 0x0480009e;

fn play(board: &mut [u8]) -> io::Result<bool> {
    let mut player = 0;
    let mut coins = board.iter().filter(|&&tile| tile == b'*').count();
    let mut lines = io::stdin().lock().lines();
    println!("Collect both coins (*). Move with WASD, then Enter. Q quits.");
    loop {
        for (y, row) in board.chunks_exact(WIDTH).enumerate() {
            for (x, &tile) in row.iter().enumerate() {
                print!(
                    "{}",
                    if y * WIDTH + x == player {
                        '@'
                    } else {
                        char::from(tile)
                    }
                );
            }
            println!();
        }
        if coins == 0 {
            println!("Both coins collected!");
            return Ok(true);
        }
        io::stdout().flush()?;
        let Some(line) = lines.next().transpose()? else {
            return Ok(false);
        };
        for key in line.bytes() {
            let next = match key.to_ascii_lowercase() {
                b'w' if player >= WIDTH => player - WIDTH,
                b's' if player + WIDTH < board.len() => player + WIDTH,
                b'a' if player % WIDTH > 0 => player - 1,
                b'd' if player % WIDTH < WIDTH - 1 => player + 1,
                b'q' => return Ok(false),
                _ => continue,
            };
            if board[next] == b'#' {
                continue;
            }
            player = next;
            if board[player] == b'*' {
                board[player] = b'.';
                coins -= 1;
                if coins == 0 {
                    break;
                }
            }
        }
    }
}

fn main() -> ExitCode {
    // SAFETY: request a writable board, initialize all its bytes before borrowing
    // it, then free it with the same attributes after play releases the borrow.
    let memory = unsafe { XMemAlloc(MAP.len(), ATTRIBUTES) };
    if memory.is_null() {
        eprintln!("XMemAlloc failed");
        return ExitCode::FAILURE;
    }
    let result = unsafe {
        memory.copy_from_nonoverlapping(MAP.as_ptr(), MAP.len());
        play(std::slice::from_raw_parts_mut(memory, MAP.len()))
    };
    unsafe { XMemFree(memory, ATTRIBUTES) };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(err) => {
            eprintln!("Game I/O failed: {err}");
            ExitCode::FAILURE
        }
    }
}
