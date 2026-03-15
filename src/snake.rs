/// Snake game — runs in the VGA text mode console.
/// Arrow keys to move, eat food (*) to grow. Hit wall or self = game over.

use crate::{vga, keyboard::KeyEvent, timer};
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

const WIDTH: usize = 78;   // play area (leave 1 col border each side)
const HEIGHT: usize = 23;  // play area (leave 1 row top + 1 bottom)
const MAX_LEN: usize = 200;

// Direction: 0=up, 1=right, 2=down, 3=left
static DIRECTION: AtomicU8 = AtomicU8::new(1); // start moving right
static GAME_RUNNING: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy)]
struct Pos { x: usize, y: usize }

/// Handle arrow key input during game.
pub fn handle_input(event: KeyEvent) {
    if !GAME_RUNNING.load(Ordering::SeqCst) { return; }
    let new_dir = match event {
        KeyEvent::ArrowUp => 0,
        KeyEvent::ArrowRight => 1,
        KeyEvent::ArrowDown => 2,
        KeyEvent::ArrowLeft => 3,
        KeyEvent::Char('q') => { GAME_RUNNING.store(false, Ordering::SeqCst); return; }
        _ => return,
    };
    let cur = DIRECTION.load(Ordering::SeqCst);
    // Prevent 180-degree turns
    if (new_dir + 2) % 4 != cur {
        DIRECTION.store(new_dir, Ordering::SeqCst);
    }
}

/// Run the snake game (blocks until game over or 'q').
pub fn run() {
    GAME_RUNNING.store(true, Ordering::SeqCst);
    DIRECTION.store(1, Ordering::SeqCst);

    let mut snake = [Pos { x: 0, y: 0 }; MAX_LEN];
    let mut len: usize = 3;
    let mut score: usize = 0;

    // Initial snake position (center)
    for i in 0..len {
        snake[i] = Pos { x: WIDTH / 2 - i, y: HEIGHT / 2 };
    }

    // Initial food
    let mut food = Pos { x: WIDTH / 4, y: HEIGHT / 4 };
    let mut rng_state: u32 = timer::ticks() as u32;

    // Draw border + initial state
    draw_border();
    draw_food(food);
    draw_snake(&snake, len);
    draw_score(score);

    let speed = 8; // ticks per move (~80ms at 100Hz)

    while GAME_RUNNING.load(Ordering::SeqCst) {
        let next_tick = timer::ticks() + speed;

        // Move snake
        let dir = DIRECTION.load(Ordering::SeqCst);
        let head = snake[0];
        let new_head = match dir {
            0 => Pos { x: head.x, y: head.y.wrapping_sub(1) },
            1 => Pos { x: head.x + 1, y: head.y },
            2 => Pos { x: head.x, y: head.y + 1 },
            3 => Pos { x: head.x.wrapping_sub(1), y: head.y },
            _ => head,
        };

        // Check wall collision
        if new_head.x == 0 || new_head.x >= WIDTH - 1
            || new_head.y == 0 || new_head.y >= HEIGHT - 1 {
            break; // game over
        }

        // Check self collision
        for i in 0..len {
            if snake[i].x == new_head.x && snake[i].y == new_head.y {
                GAME_RUNNING.store(false, Ordering::SeqCst);
                break;
            }
        }
        if !GAME_RUNNING.load(Ordering::SeqCst) { break; }

        // Erase tail
        let tail = snake[len - 1];
        put_char(tail.x + 1, tail.y + 1, b' ', 0x00);

        // Shift body
        for i in (1..len).rev() {
            snake[i] = snake[i - 1];
        }
        snake[0] = new_head;

        // Check food
        if new_head.x == food.x && new_head.y == food.y {
            score += 10;
            if len < MAX_LEN - 1 { len += 1; }
            // New food position (simple LCG random)
            rng_state = rng_state.wrapping_mul(1103515245).wrapping_add(12345);
            food.x = 1 + ((rng_state >> 16) as usize % (WIDTH - 3));
            rng_state = rng_state.wrapping_mul(1103515245).wrapping_add(12345);
            food.y = 1 + ((rng_state >> 16) as usize % (HEIGHT - 3));
            draw_food(food);
            draw_score(score);
        }

        // Draw head
        put_char(new_head.x + 1, new_head.y + 1, b'O', 0x0A); // green

        // Wait for next tick
        while timer::ticks() < next_tick {
            x86_64::instructions::hlt();
        }
    }

    GAME_RUNNING.store(false, Ordering::SeqCst);

    // Game over screen
    let msg = b"  GAME OVER  ";
    let score_msg = alloc::format!("  Score: {}  ", score);
    let cx = 40 - msg.len() / 2;
    let cy = 12;
    for (i, &b) in msg.iter().enumerate() {
        put_char(cx + i, cy, b, 0x4F); // red bg, white text
    }
    for (i, b) in score_msg.bytes().enumerate() {
        put_char(cx + i, cy + 1, b, 0x4F);
    }
    let quit = b" Press any key ";
    for (i, &b) in quit.iter().enumerate() {
        put_char(cx + i, cy + 2, b, 0x07);
    }
}

pub fn is_running() -> bool {
    GAME_RUNNING.load(Ordering::SeqCst)
}

fn draw_border() {
    let mut w = vga::WRITER.lock();
    w.clear();
    drop(w);

    // Top/bottom border
    for x in 0..80 {
        put_char(x, 0, b'#', 0x08);
        put_char(x, 24, b'#', 0x08);
    }
    // Left/right border
    for y in 0..25 {
        put_char(0, y, b'#', 0x08);
        put_char(79, y, b'#', 0x08);
    }
    // Title
    let title = b" SNAKE - MerlionOS ";
    for (i, &b) in title.iter().enumerate() {
        put_char(30 + i, 0, b, 0x0E); // yellow
    }
}

fn draw_food(food: Pos) {
    put_char(food.x + 1, food.y + 1, b'*', 0x0C); // red
}

fn draw_snake(snake: &[Pos], len: usize) {
    for i in 0..len {
        let ch = if i == 0 { b'O' } else { b'o' };
        put_char(snake[i].x + 1, snake[i].y + 1, ch, 0x0A); // green
    }
}

fn draw_score(score: usize) {
    let msg = alloc::format!(" Score: {} ", score);
    for (i, b) in msg.bytes().enumerate() {
        put_char(60 + i, 0, b, 0x0E);
    }
}

fn put_char(x: usize, y: usize, ch: u8, attr: u8) {
    if x >= 80 || y >= 25 { return; }
    let vga = 0xB8000 as *mut u8;
    let offset = (y * 80 + x) * 2;
    unsafe {
        vga.add(offset).write_volatile(ch);
        vga.add(offset + 1).write_volatile(attr);
    }
}
