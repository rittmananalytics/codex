//! The Wire mark (wordmark and stepped mark) in Braille cells, for the welcome screen of the Wire-forked Codex.
//!
//! Fields and depth use Float32 sampling; all other math uses Float64.


use super::lighting::Lighting;

pub(super) const MAX_COLUMNS: u16 = 60;
pub(super) const MAX_ROWS: u16 = 24;

fn smooth(value: f64) -> f64 {
    let t = value.clamp(/*min*/ 0.0, /*max*/ 1.0);
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Cell {
    pub(super) dots: u8,
    pub(super) rgb: u32,
}

pub(super) struct Renderer {
    cells: Vec<Cell>,
}

impl Default for Renderer {
    fn default() -> Self {
        Self {
            cells: vec![Cell::default(); usize::from(MAX_COLUMNS) * usize::from(MAX_ROWS)],
        }
    }
}

// The Wire wordmark and the stepped mark, as the Wire CLI draws them: seven
// rows of cells, `#` ink, `b` the brand-blue step, `.` empty.
const WIRE_ROWS: usize = 7;
const WIRE_COLS: usize = 45;
const WIRE_GLYPHS: [&str; WIRE_ROWS] = [
    "............##...............................",
    ".....................................####bbbb",
    "##..##..##..##..##.####...####.......####bbbb",
    "##..##..##..##..####.....##..##......##bbbbbb",
    "##..##..##..##..##.......######......bbbbbbbb",
    "##..##..##..##..##.......##..................",
    "..######....##..##........#####..............",
];
const WIRE_BLUE: u32 = 0x4F60FF;

impl Renderer {
    /// The Wire mark on a `columns` x `rows` stage: the glyphs scaled to fit and
    /// centred, revealed left to right over the first half of `phase`, then held.
    pub(super) fn frame(
        &mut self,
        columns: u16,
        rows: u16,
        phase: f64,
        light: &Lighting,
    ) -> &[Cell] {
        assert!(columns <= MAX_COLUMNS && rows <= MAX_ROWS);
        let columns = usize::from(columns);
        let rows = usize::from(rows);
        let phase = phase.rem_euclid(/*rhs*/ 1.0);
        let reveal = smooth(phase * 2.0);
        let scale_x = columns as f64 / WIRE_COLS as f64;
        let scale_y = rows as f64 / WIRE_ROWS as f64;
        let scale = scale_x.min(scale_y).min(/*max*/ 1.0);
        let drawn_cols = ((WIRE_COLS as f64) * scale).round().max(/*min*/ 1.0) as usize;
        let drawn_rows = ((WIRE_ROWS as f64) * scale).round().max(/*min*/ 1.0) as usize;
        let off_x = columns.saturating_sub(drawn_cols) / 2;
        let off_y = rows.saturating_sub(drawn_rows) / 2;
        let ink: u32 = {
            let [r, g, b] = light.key;
            ((r.round() as u32) << 16) | ((g.round() as u32) << 8) | (b.round() as u32)
        };
        let visible_cols = ((drawn_cols as f64) * reveal).ceil() as usize;
        for cell in self.cells.iter_mut() {
            *cell = Cell::default();
        }
        for r in 0..drawn_rows {
            let gr = ((r as f64 + 0.5) / scale).floor() as usize;
            if gr >= WIRE_ROWS {
                continue;
            }
            let row = WIRE_GLYPHS[gr].as_bytes();
            for c in 0..drawn_cols.min(visible_cols) {
                let gc = ((c as f64 + 0.5) / scale).floor() as usize;
                if gc >= WIRE_COLS {
                    continue;
                }
                let (dots, rgb) = match row[gc] {
                    b'#' => (0b0101_1010u8, ink),
                    b'b' => (0xFFu8, WIRE_BLUE),
                    _ => continue,
                };
                let index = (off_y + r) * columns + off_x + c;
                if let Some(cell) = self.cells.get_mut(index) {
                    *cell = Cell { dots, rgb };
                }
            }
        }
        &self.cells[..columns * rows]
    }
}
