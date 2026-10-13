//! Tahta oyunlarının kuralları: XOX (tic-tac-toe) ve 4'lü bağla. Discord'dan bağımsız, testli.

use crate::rand_util;

pub const EMPTY: u8 = 0;
pub const P1: u8 = 1;
pub const P2: u8 = 2;

pub fn other(player: u8) -> u8 {
    if player == P1 { P2 } else { P1 }
}

// ===================== XOX =====================

const LINES: [[usize; 3]; 8] = [
    [0, 1, 2],
    [3, 4, 5],
    [6, 7, 8],
    [0, 3, 6],
    [1, 4, 7],
    [2, 5, 8],
    [0, 4, 8],
    [2, 4, 6],
];

pub fn xox_winner(board: &[u8; 9]) -> Option<u8> {
    LINES.iter().find_map(|line| {
        let [a, b, c] = line.map(|i| board[i]);
        (a != EMPTY && a == b && b == c).then_some(a)
    })
}

pub fn xox_full(board: &[u8; 9]) -> bool {
    board.iter().all(|c| *c != EMPTY)
}

/// Minimax puanı: `me` açısından +1 kazanır, -1 kaybeder, 0 berabere.
fn minimax(board: &mut [u8; 9], turn: u8, me: u8) -> i32 {
    if let Some(w) = xox_winner(board) {
        return if w == me { 1 } else { -1 };
    }
    if xox_full(board) {
        return 0;
    }
    let mut best = if turn == me { i32::MIN } else { i32::MAX };
    for i in 0..9 {
        if board[i] == EMPTY {
            board[i] = turn;
            let score = minimax(board, other(turn), me);
            board[i] = EMPTY;
            best = if turn == me { best.max(score) } else { best.min(score) };
        }
    }
    best
}

/// Botun hamlesi. `mistake_percent` ihtimalle rastgele oynar (her zaman berabere bitmesin diye).
pub fn xox_bot_move(board: &[u8; 9], me: u8, mistake_percent: u64) -> Option<usize> {
    let free: Vec<usize> = (0..9).filter(|i| board[*i] == EMPTY).collect();
    if free.is_empty() {
        return None;
    }
    if rand_util::below(100) < mistake_percent {
        return free.get(usize::try_from(rand_util::below(free.len() as u64)).unwrap_or(0)).copied();
    }
    let mut scratch = *board;
    let mut best: Option<(i32, usize)> = None;
    for i in free {
        scratch[i] = me;
        let score = minimax(&mut scratch, other(me), me);
        scratch[i] = EMPTY;
        if best.is_none_or(|(s, _)| score > s) {
            best = Some((score, i));
        }
    }
    best.map(|(_, i)| i)
}

// ===================== 4'lü bağla =====================

pub const ROWS: usize = 6;
pub const COLS: usize = 7;
pub type Grid = [[u8; COLS]; ROWS];

/// Taşı sütuna bırakır; düştüğü satırı döner (sütun doluysa `None`). Satır 0 en üst.
pub fn drop_piece(grid: &mut Grid, col: usize, player: u8) -> Option<usize> {
    if col >= COLS {
        return None;
    }
    let row = (0..ROWS).rev().find(|r| grid[*r][col] == EMPTY)?;
    grid[row][col] = player;
    Some(row)
}

pub fn c4_winner(grid: &Grid) -> Option<u8> {
    let dirs: [(isize, isize); 4] = [(0, 1), (1, 0), (1, 1), (1, -1)];
    for r in 0..ROWS {
        for c in 0..COLS {
            let p = grid[r][c];
            if p == EMPTY {
                continue;
            }
            for (dr, dc) in dirs {
                let all = (1..4).all(|k| {
                    let (rr, cc) = (r as isize + dr * k, c as isize + dc * k);
                    rr >= 0
                        && cc >= 0
                        && (rr as usize) < ROWS
                        && (cc as usize) < COLS
                        && grid[rr as usize][cc as usize] == p
                });
                if all {
                    return Some(p);
                }
            }
        }
    }
    None
}

pub fn c4_full(grid: &Grid) -> bool {
    grid[0].iter().all(|c| *c != EMPTY)
}

/// Basit ama akıllı bot: kazanabiliyorsa kazanır, rakibin kazancını keser, rakibe zafer
/// hazırlayan hamleden kaçınır, ortaya yakın oynamayı tercih eder.
pub fn c4_bot_move(grid: &Grid, me: u8) -> Option<usize> {
    let valid: Vec<usize> = (0..COLS).filter(|c| grid[0][*c] == EMPTY).collect();
    if valid.is_empty() {
        return None;
    }
    let wins_with = |player: u8, col: usize| {
        let mut g = *grid;
        drop_piece(&mut g, col, player).is_some() && c4_winner(&g) == Some(player)
    };
    if let Some(c) = valid.iter().copied().find(|c| wins_with(me, *c)) {
        return Some(c);
    }
    if let Some(c) = valid.iter().copied().find(|c| wins_with(other(me), *c)) {
        return Some(c);
    }
    // Hamleden sonra rakip hemen kazanabiliyorsa o sütun "tehlikeli".
    let safe: Vec<usize> = valid
        .iter()
        .copied()
        .filter(|c| {
            let mut g = *grid;
            drop_piece(&mut g, *c, me);
            !(0..COLS).any(|oc| {
                let mut g2 = g;
                drop_piece(&mut g2, oc, other(me)).is_some() && c4_winner(&g2) == Some(other(me))
            })
        })
        .collect();
    let pool = if safe.is_empty() { &valid } else { &safe };
    // Ortaya yakınlık + biraz rastgelelik.
    pool.iter()
        .copied()
        .min_by_key(|c| (c.abs_diff(COLS / 2) as u64) * 10 + rand_util::below(8))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xox_detects_lines() {
        assert_eq!(xox_winner(&[1, 1, 1, 0, 2, 2, 0, 0, 0]), Some(1));
        assert_eq!(xox_winner(&[2, 1, 0, 1, 2, 0, 0, 1, 2]), Some(2));
        assert_eq!(xox_winner(&[1, 2, 1, 1, 2, 2, 2, 1, 1]), None);
        assert!(xox_full(&[1, 2, 1, 1, 2, 2, 2, 1, 1]));
    }

    #[test]
    fn xox_bot_wins_and_blocks() {
        // Bot (2) kazanabilir: 2-2-_ üst satır.
        assert_eq!(xox_bot_move(&[2, 2, 0, 1, 1, 0, 0, 0, 0], 2, 0), Some(2));
        // Bot kazanamıyor ama 1'in 3,4,_ satırını kesmeli.
        assert_eq!(xox_bot_move(&[2, 0, 0, 1, 1, 0, 0, 0, 2], 2, 0), Some(5));
    }

    #[test]
    fn perfect_bots_draw() {
        let mut board = [EMPTY; 9];
        let mut turn = P1;
        while xox_winner(&board).is_none() && !xox_full(&board) {
            let Some(i) = xox_bot_move(&board, turn, 0) else { break };
            board[i] = turn;
            turn = other(turn);
        }
        assert_eq!(xox_winner(&board), None, "iki kusursuz oyuncu berabere kalmalı");
    }

    #[test]
    fn c4_drops_and_wins() {
        let mut g: Grid = [[EMPTY; COLS]; ROWS];
        assert_eq!(drop_piece(&mut g, 3, P1), Some(5));
        assert_eq!(drop_piece(&mut g, 3, P2), Some(4));
        for c in 0..3 {
            drop_piece(&mut g, c, P1);
        }
        assert_eq!(c4_winner(&g), Some(P1)); // alt satır 0-3
        let mut v: Grid = [[EMPTY; COLS]; ROWS];
        for _ in 0..4 {
            drop_piece(&mut v, 6, P2);
        }
        assert_eq!(c4_winner(&v), Some(P2)); // dikey
        let mut full: Grid = [[EMPTY; COLS]; ROWS];
        for _ in 0..ROWS {
            drop_piece(&mut full, 0, P1);
        }
        assert_eq!(drop_piece(&mut full, 0, P2), None);
    }

    #[test]
    fn c4_diagonal() {
        let mut g: Grid = [[EMPTY; COLS]; ROWS];
        // Çapraz merdiven: (5,0) (4,1) (3,2) (2,3)
        for (col, fillers) in [(0usize, 0usize), (1, 1), (2, 2), (3, 3)] {
            for _ in 0..fillers {
                drop_piece(&mut g, col, P2);
            }
            drop_piece(&mut g, col, P1);
        }
        assert_eq!(c4_winner(&g), Some(P1));
    }

    #[test]
    fn c4_bot_wins_or_blocks() {
        let mut g: Grid = [[EMPTY; COLS]; ROWS];
        for c in 0..3 {
            drop_piece(&mut g, c, P1);
        }
        // Rakip 0-1-2'yi doldurdu; bot 3'ü kapatmalı.
        assert_eq!(c4_bot_move(&g, P2), Some(3));
        let mut w: Grid = [[EMPTY; COLS]; ROWS];
        for _ in 0..3 {
            drop_piece(&mut w, 5, P2);
        }
        assert_eq!(c4_bot_move(&w, P2), Some(5));
    }
}
