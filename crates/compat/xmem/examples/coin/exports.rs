//! Private C ABI for the graphical Coin Run example. The caller owns the game
//! handle and borrows the tile array until the next move or destruction.
mod game;
use game::Game;

#[no_mangle]
pub extern "C" fn coin_new() -> *mut Game {
    Game::new().map_or(std::ptr::null_mut(), |g| Box::into_raw(Box::new(g)))
}

#[no_mangle]
pub unsafe extern "C" fn coin_delete(game: *mut Game) {
    drop(Box::from_raw(game));
}

#[no_mangle]
pub extern "C" fn coin_move(game: &mut Game, key: u8) -> bool {
    game.step(key)
}

#[repr(C)]
pub struct Board {
    tiles: *const u8,
    width: usize,
    height: usize,
    player: usize,
    coins: usize,
}

#[no_mangle]
pub extern "C" fn coin_board(game: &Game, out: &mut Board) {
    *out = Board {
        tiles: game.tiles().as_ptr(),
        width: game::WIDTH,
        height: game.tiles().len() / game::WIDTH,
        player: game.player,
        coins: game.coins,
    };
}
