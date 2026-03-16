/// Tetris game — runs in the VGA text mode console.
///
/// Controls: arrow keys to move/rotate, space to hard drop. Clear lines to score.
/// Features all 7 standard tetrominoes (I, O, T, S, Z, J, L) encoded as 4x4
/// bitmaps with 4 rotation states each, wall-kick rotation, ghost piece preview,
/// 7-bag randomizer, NES-style scoring, and level progression.
///
/// Uses `crate::timer` for gravity timing and `crate::keyboard::KeyEvent` for input.
/// Renders directly to VGA text-mode memory at 0xB8000.

use crate::{keyboard::KeyEvent, timer};
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

/// VGA text mode screen width in columns.
const VGA_W: usize = 80;
/// VGA text mode screen height in rows.
const VGA_H: usize = 25;
/// Tetris playfield width in cells.
const BOARD_W: usize = 10;
/// Tetris playfield height in cells.
const BOARD_H: usize = 20;
/// VGA column where the board's left edge starts.
const BOARD_X: usize = 25;
/// VGA row where the board's top edge starts.
const BOARD_Y: usize = 2;
/// Timer ticks between gravity drops at level 0 (~500ms at 100 Hz PIT).
const BASE_DROP: u64 = 50;
/// Ticks subtracted from drop interval per level (floor clamped to 5).
const DROP_ACCEL: u64 = 4;
/// Number of cleared lines required to advance one level.
const LINES_PER_LVL: usize = 10;

/// Input: 0=none 1=left 2=right 3=rotate 4=soft-drop 5=hard-drop 6=quit
static INPUT_CMD: AtomicU8 = AtomicU8::new(0);
static GAME_RUNNING: AtomicBool = AtomicBool::new(false);

/// Tetromino types.
#[derive(Clone, Copy, PartialEq)]
#[repr(u8)]
enum Kind { I=0, O=1, T=2, S=3, Z=4, J=5, L=6 }
const ALL_KINDS: [Kind; 7] = [Kind::I, Kind::O, Kind::T, Kind::S, Kind::Z, Kind::J, Kind::L];

/// VGA color attribute per piece kind (I=cyan, O=yellow, T=pink, S=green, Z=red, J=blue, L=brown).
const COLORS: [u8; 7] = [0x0B, 0x0E, 0x0D, 0x0A, 0x0C, 0x09, 0x06];

/// 4x4 bitmaps for each piece in 4 rotation states (u16, MSB = top-left).
const SHAPES: [[u16; 4]; 7] = [
    [0b0000_1111_0000_0000, 0b0010_0010_0010_0010, // I
     0b0000_0000_1111_0000, 0b0100_0100_0100_0100],
    [0b0000_0110_0110_0000, 0b0000_0110_0110_0000, // O
     0b0000_0110_0110_0000, 0b0000_0110_0110_0000],
    [0b0000_0111_0010_0000, 0b0010_0110_0010_0000, // T
     0b0010_0111_0000_0000, 0b0010_0011_0010_0000],
    [0b0000_0011_0110_0000, 0b0010_0011_0001_0000, // S
     0b0000_0011_0110_0000, 0b0010_0011_0001_0000],
    [0b0000_0110_0011_0000, 0b0001_0011_0010_0000, // Z
     0b0000_0110_0011_0000, 0b0001_0011_0010_0000],
    [0b0000_0111_0001_0000, 0b0010_0010_0110_0000, // J
     0b0100_0111_0000_0000, 0b0011_0010_0010_0000],
    [0b0000_0111_0100_0000, 0b0110_0010_0010_0000, // L
     0b0001_0111_0000_0000, 0b0010_0010_0011_0000],
];

/// Check if cell (row, col) is set in a 4x4 bitmap.
fn bget(bmp: u16, r: usize, c: usize) -> bool { bmp & (1 << (15 - (r * 4 + c))) != 0 }

/// A falling piece on the board.
#[derive(Clone, Copy)]
struct Piece { kind: Kind, rot: u8, x: i16, y: i16 }

impl Piece {
    /// Spawn a new piece at the top-center of the board.
    fn spawn(kind: Kind) -> Self { Self { kind, rot: 0, x: (BOARD_W as i16 / 2) - 2, y: -1 } }
    /// Current 4x4 bitmap.
    fn bmp(&self) -> u16 { SHAPES[self.kind as usize][self.rot as usize % 4] }
    /// Color attribute.
    fn color(&self) -> u8 { COLORS[self.kind as usize] }
}

/// Rotate the piece clockwise; returns new rotation index.
fn rotate_piece(p: &Piece) -> u8 { (p.rot + 1) % 4 }

/// Compute new position after moving by (dx, dy).
fn move_piece(p: &Piece, dx: i16, dy: i16) -> (i16, i16) { (p.x + dx, p.y + dy) }

/// The playfield grid. Each cell is 0 (empty) or a VGA color attribute (occupied).
struct Board { cells: [[u8; BOARD_W]; BOARD_H] }

impl Board {
    /// Create an empty board.
    fn new() -> Self { Self { cells: [[0u8; BOARD_W]; BOARD_H] } }

    /// Check whether a piece at (px, py) with given rotation collides with walls or locked cells.
    fn check_collision(&self, kind: Kind, rot: u8, px: i16, py: i16) -> bool {
        let bmp = SHAPES[kind as usize][rot as usize % 4];
        for r in 0..4 { for c in 0..4 {
            if !bget(bmp, r, c) { continue; }
            let bx = px + c as i16;
            let by = py + r as i16;
            if bx < 0 || bx >= BOARD_W as i16 || by >= BOARD_H as i16 { return true; }
            if by < 0 { continue; } // above ceiling is OK (spawn zone)
            if self.cells[by as usize][bx as usize] != 0 { return true; }
        }}
        false
    }

    /// Lock the piece into the board grid.
    fn lock_piece(&mut self, p: &Piece) {
        let (bmp, color) = (p.bmp(), p.color());
        for r in 0..4 { for c in 0..4 {
            if !bget(bmp, r, c) { continue; }
            let (bx, by) = (p.x + c as i16, p.y + r as i16);
            if bx >= 0 && bx < BOARD_W as i16 && by >= 0 && by < BOARD_H as i16 {
                self.cells[by as usize][bx as usize] = color;
            }
        }}
    }

    /// Clear completed lines; returns number of lines cleared.
    fn clear_lines(&mut self) -> usize {
        let mut cleared = 0usize;
        let mut dst = BOARD_H;
        for src in (0..BOARD_H).rev() {
            if self.cells[src].iter().all(|&c| c != 0) { cleared += 1; }
            else { dst -= 1; if dst != src { self.cells[dst] = self.cells[src]; } }
        }
        for r in 0..dst { self.cells[r] = [0u8; BOARD_W]; }
        cleared
    }
}

/// Points awarded for clearing 1..4 lines (NES-style scoring).
fn line_score(n: usize, level: usize) -> usize {
    (match n { 1=>100, 2=>300, 3=>500, 4=>800, _=>0 }) * (level + 1)
}

/// Simple LCG PRNG for the random bag.
struct Rng(u32);
impl Rng {
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(1103515245).wrapping_add(12345); self.0 >> 16
    }
    /// Fisher-Yates shuffle on a 7-piece bag.
    fn shuffle(&mut self, bag: &mut [Kind; 7]) {
        *bag = ALL_KINDS;
        for i in (1..7).rev() { let j = self.next() as usize % (i + 1); bag.swap(i, j); }
    }
}

/// Handle a key event during the tetris game.
pub fn handle_input(event: KeyEvent) {
    if !GAME_RUNNING.load(Ordering::SeqCst) { return; }
    let cmd = match event {
        KeyEvent::ArrowLeft  => 1,
        KeyEvent::ArrowRight => 2,
        KeyEvent::ArrowUp    => 3,
        KeyEvent::ArrowDown  => 4,
        KeyEvent::Char(' ')  => 5,
        KeyEvent::Char('q')  => 6,
        _ => return,
    };
    INPUT_CMD.store(cmd, Ordering::SeqCst);
}

/// Whether the game is currently running.
pub fn is_running() -> bool { GAME_RUNNING.load(Ordering::SeqCst) }

// ─── VGA helpers ─────────────────────────────────────────────────────────────

/// Write a character + attribute to the VGA text buffer at 0xB8000.
fn put(x: usize, y: usize, ch: u8, attr: u8) {
    if x >= VGA_W || y >= VGA_H { return; }
    let off = (y * VGA_W + x) * 2;
    unsafe { let v = 0xB8000 as *mut u8; v.add(off).write_volatile(ch); v.add(off+1).write_volatile(attr); }
}

/// Write a byte string at (x, y).
fn puts(x: usize, y: usize, s: &[u8], a: u8) { for (i, &b) in s.iter().enumerate() { put(x+i, y, b, a); } }

/// Render a decimal number at (x, y).
fn put_num(x: usize, y: usize, val: usize) {
    let mut buf = [b' '; 10]; let mut n = val; let mut i = 10;
    if n == 0 { i -= 1; buf[i] = b'0'; }
    else { while n > 0 && i > 0 { i -= 1; buf[i] = b'0' + (n % 10) as u8; n /= 10; } }
    puts(x, y, &buf[i..], 0x0F);
}

/// Render the complete frame: board, active piece, score panel, and next piece preview.
fn render(board: &Board, piece: &Piece, score: usize, level: usize, lines: usize, next: Kind) {
    // Clear screen
    unsafe { let v = 0xB8000 as *mut u8;
        for i in 0..(VGA_W * VGA_H) { v.add(i*2).write_volatile(b' '); v.add(i*2+1).write_volatile(0x00); }
    }
    puts(BOARD_X, 0, b"TETRIS - MerlionOS", 0x0E);
    // Board border (each cell = 2 VGA columns for square look)
    let (lw, rw) = (BOARD_X - 1, BOARD_X + BOARD_W * 2);
    for r in 0..BOARD_H { let vy = BOARD_Y + r; put(lw, vy, b'|', 0x08); put(rw, vy, b'|', 0x08); }
    put(lw, BOARD_Y + BOARD_H, b'+', 0x08); put(rw, BOARD_Y + BOARD_H, b'+', 0x08);
    for c in 0..BOARD_W*2 { put(BOARD_X + c, BOARD_Y + BOARD_H, b'-', 0x08); }
    // Locked cells
    for r in 0..BOARD_H { for c in 0..BOARD_W {
        let col = board.cells[r][c];
        if col != 0 { let vx = BOARD_X + c*2; put(vx, BOARD_Y+r, b'[', col); put(vx+1, BOARD_Y+r, b']', col); }
    }}
    // Ghost piece (drop shadow) — shows where the piece will land
    let ghost_y = {
        let mut gy = piece.y;
        while !board.check_collision(piece.kind, piece.rot, piece.x, gy + 1) { gy += 1; }
        gy
    };
    if ghost_y != piece.y {
        let gbmp = piece.bmp();
        for r in 0..4 { for c in 0..4 { if bget(gbmp, r, c) {
            let (bx, by) = (piece.x + c as i16, ghost_y + r as i16);
            if bx >= 0 && bx < BOARD_W as i16 && by >= 0 && by < BOARD_H as i16 {
                let vx = BOARD_X + bx as usize * 2;
                put(vx, BOARD_Y + by as usize, b'.', 0x08);
                put(vx+1, BOARD_Y + by as usize, b'.', 0x08);
            }
        }}}
    }
    // Active piece
    let (bmp, pc) = (piece.bmp(), piece.color());
    for r in 0..4 { for c in 0..4 { if bget(bmp, r, c) {
        let (bx, by) = (piece.x + c as i16, piece.y + r as i16);
        if bx >= 0 && bx < BOARD_W as i16 && by >= 0 && by < BOARD_H as i16 {
            let vx = BOARD_X + bx as usize * 2;
            put(vx, BOARD_Y + by as usize, b'[', pc); put(vx+1, BOARD_Y + by as usize, b']', pc);
        }
    }}}
    // Score / Level / Lines panel
    let px = BOARD_X + BOARD_W * 2 + 3;
    puts(px, BOARD_Y,   b"SCORE", 0x0F); put_num(px, BOARD_Y+1, score);
    puts(px, BOARD_Y+3, b"LEVEL", 0x0F); put_num(px, BOARD_Y+4, level);
    puts(px, BOARD_Y+6, b"LINES", 0x0F); put_num(px, BOARD_Y+7, lines);
    // Next piece preview
    puts(px, BOARD_Y+9, b"NEXT", 0x0F);
    let (nb, nc) = (SHAPES[next as usize][0], COLORS[next as usize]);
    for r in 0..4 { for c in 0..4 { if bget(nb, r, c) {
        put(px+c*2, BOARD_Y+10+r, b'[', nc); put(px+c*2+1, BOARD_Y+10+r, b']', nc);
    }}}
    // Controls
    let hy = BOARD_Y + 16;
    puts(px, hy,   b"<-/-> Move",    0x07); puts(px, hy+1, b"  ^   Rotate",  0x07);
    puts(px, hy+2, b"  v   Soft drop",0x07); puts(px, hy+3, b"SPACE Hard drop",0x07);
    puts(px, hy+4, b"  Q   Quit",     0x07);
}

/// Display the game-over overlay centered on the board.
fn render_game_over(score: usize) {
    let (cx, cy, a) = (BOARD_X + BOARD_W - 7, BOARD_Y + BOARD_H / 2 - 1, 0x4F);
    puts(cx, cy,   b"                ", a);
    puts(cx, cy+1, b"   GAME  OVER   ", a);
    puts(cx, cy+2, b"                ", a);
    let mut sb = [b' '; 16];
    let pfx = b"  Score: ";
    for (i, &b) in pfx.iter().enumerate() { sb[i] = b; }
    let (mut n, mut p) = (score, 14usize);
    if n == 0 { sb[p] = b'0'; }
    else { while n > 0 && p >= pfx.len() { sb[p] = b'0' + (n % 10) as u8; n /= 10; p -= 1; } }
    puts(cx, cy+3, &sb, a);
    puts(cx, cy+4, b" Press any key  ", 0x07);
}

/// Spawn the next piece from the bag, reshuffling when exhausted.
fn next_from_bag(bag: &mut [Kind; 7], idx: &mut usize, rng: &mut Rng) -> Kind {
    if *idx >= 7 { rng.shuffle(bag); *idx = 0; }
    let k = bag[*idx]; *idx += 1; k
}

/// Lock current piece, clear lines, update score, spawn next. Returns false on game-over.
fn lock_and_spawn(
    board: &mut Board, current: &mut Piece, next: &mut Kind,
    score: &mut usize, level: &mut usize, total: &mut usize,
    bag: &mut [Kind; 7], idx: &mut usize, rng: &mut Rng,
) -> bool {
    board.lock_piece(current);
    let cl = board.clear_lines();
    if cl > 0 { *total += cl; *score += line_score(cl, *level); *level = *total / LINES_PER_LVL; }
    *current = Piece::spawn(*next);
    *next = next_from_bag(bag, idx, rng);
    !board.check_collision(current.kind, current.rot, current.x, current.y)
}

/// Run the Tetris game (blocks until game over or 'q' pressed).
pub fn run() {
    GAME_RUNNING.store(true, Ordering::SeqCst);
    INPUT_CMD.store(0, Ordering::SeqCst);

    let mut board = Board::new();
    let mut rng = Rng(timer::ticks() as u32 ^ 0xDEAD_BEEF);
    let mut bag = ALL_KINDS;
    rng.shuffle(&mut bag);
    let mut idx: usize = 0;

    let mut current = Piece::spawn(next_from_bag(&mut bag, &mut idx, &mut rng));
    let mut next = next_from_bag(&mut bag, &mut idx, &mut rng);
    let (mut score, mut level, mut total_lines) = (0usize, 0usize, 0usize);
    let mut last_drop = timer::ticks();

    render(&board, &current, score, level, total_lines, next);

    while GAME_RUNNING.load(Ordering::SeqCst) {
        let drop_iv = BASE_DROP.saturating_sub(level as u64 * DROP_ACCEL).max(5);
        let cmd = INPUT_CMD.swap(0, Ordering::SeqCst);
        let mut dirty = false;

        match cmd {
            1 => { // left
                let (nx, ny) = move_piece(&current, -1, 0);
                if !board.check_collision(current.kind, current.rot, nx, ny) {
                    current.x = nx; current.y = ny; dirty = true;
                }
            }
            2 => { // right
                let (nx, ny) = move_piece(&current, 1, 0);
                if !board.check_collision(current.kind, current.rot, nx, ny) {
                    current.x = nx; current.y = ny; dirty = true;
                }
            }
            3 => { // rotate CW with simple wall kick
                let nr = rotate_piece(&current);
                if !board.check_collision(current.kind, nr, current.x, current.y) {
                    current.rot = nr; dirty = true;
                } else if !board.check_collision(current.kind, nr, current.x-1, current.y) {
                    current.rot = nr; current.x -= 1; dirty = true;
                } else if !board.check_collision(current.kind, nr, current.x+1, current.y) {
                    current.rot = nr; current.x += 1; dirty = true;
                }
            }
            4 => { // soft drop
                let (nx, ny) = move_piece(&current, 0, 1);
                if !board.check_collision(current.kind, current.rot, nx, ny) {
                    current.y = ny; score += 1; last_drop = timer::ticks(); dirty = true;
                }
            }
            5 => { // hard drop
                let mut d = 0usize;
                while !board.check_collision(current.kind, current.rot, current.x, current.y + d as i16 + 1) { d += 1; }
                score += d * 2; current.y += d as i16;
                if !lock_and_spawn(&mut board, &mut current, &mut next, &mut score, &mut level, &mut total_lines, &mut bag, &mut idx, &mut rng) { break; }
                last_drop = timer::ticks(); dirty = true;
            }
            6 => { GAME_RUNNING.store(false, Ordering::SeqCst); return; }
            _ => {}
        }

        // Timer-based gravity
        let now = timer::ticks();
        if now.saturating_sub(last_drop) >= drop_iv {
            last_drop = now;
            let (_, ny) = move_piece(&current, 0, 1);
            if !board.check_collision(current.kind, current.rot, current.x, ny) {
                current.y = ny; dirty = true;
            } else {
                if !lock_and_spawn(&mut board, &mut current, &mut next, &mut score, &mut level, &mut total_lines, &mut bag, &mut idx, &mut rng) { break; }
                dirty = true;
            }
        }

        if dirty { render(&board, &current, score, level, total_lines, next); }
        x86_64::instructions::hlt();
    }

    GAME_RUNNING.store(false, Ordering::SeqCst);
    render(&board, &current, score, level, total_lines, next);
    render_game_over(score);
}
