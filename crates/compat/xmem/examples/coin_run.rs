//! Synthetic x64 Windows title exercising TomBox's xmem.dll.
use std::io::{self, BufRead, Write};
use std::process::ExitCode;

#[path = "coin/game.rs"]
mod game;
use game::{Game, WIDTH};

fn play(board: &mut Game) -> io::Result<bool> {
    let mut lines = io::stdin().lock().lines();
    println!("Collect both coins (*). Move with WASD, then Enter. R restarts. Q quits.");
    loop {
        for (y, row) in board.tiles().chunks_exact(WIDTH).enumerate() {
            for (x, &tile) in row.iter().enumerate() {
                print!(
                    "{}",
                    if y * WIDTH + x == board.player {
                        '@'
                    } else {
                        char::from(tile)
                    }
                );
            }
            println!();
        }
        if board.coins == 0 {
            println!("Both coins collected!");
            return Ok(true);
        }
        io::stdout().flush()?;
        let Some(line) = lines.next().transpose()? else {
            return Ok(false);
        };
        for key in line.bytes() {
            if key.eq_ignore_ascii_case(&b'q') {
                return Ok(false);
            }
            board.step(key);
            if board.coins == 0 {
                break;
            }
        }
    }
}

fn main() -> ExitCode {
    let Some(mut board) = Game::new() else {
        eprintln!("XMemAlloc failed");
        return ExitCode::FAILURE;
    };
    match play(&mut board) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(err) => {
            eprintln!("Game I/O failed: {err}");
            ExitCode::FAILURE
        }
    }
}
