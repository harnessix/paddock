//! Where a frame goes in the terminal: the largest rectangle of cells
//! that keeps its aspect, centered above the status line, never larger
//! than one frame pixel per terminal pixel. And the way back: a mouse
//! position in the terminal as a point in the frame.

use super::encode::Cells;

/// The size of the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Term {
    pub cols: u16,
    pub rows: u16,
    /// The pixels of one cell, from the terminal or [`GUESS_CELL`].
    pub cell_w: u16,
    pub cell_h: u16,
    /// True when the cell size is the guess.
    pub guessed: bool,
}

/// The cell size when the terminal reports none.
pub const GUESS_CELL: (u16, u16) = (8, 16);
/// The rows at the bottom for the status line.
pub const STATUS_ROWS: u16 = 1;

impl Term {
    /// The cell size of a terminal from its pixel and cell counts
    /// (TIOCGWINSZ), else a reported cell size, else the guess.
    pub fn new(cols: u16, rows: u16, px: Option<(u16, u16)>, cell: Option<(u16, u16)>) -> Term {
        let from_px = px
            .filter(|&(w, h)| w > 0 && h > 0 && cols > 0 && rows > 0)
            .map(|(w, h)| (w / cols, h / rows))
            .filter(|&(w, h)| w > 0 && h > 0);
        // The pixel size of the window is fresh at every read; the reported
        // cell size is from the start.
        let known = from_px.or(cell.filter(|&(w, h)| w > 0 && h > 0));
        let (cell_w, cell_h) = known.unwrap_or(GUESS_CELL);
        Term {
            cols: cols.max(1),
            rows: rows.max(1),
            cell_w,
            cell_h,
            guessed: known.is_none(),
        }
    }
}

impl Term {
    /// The pixels above the status line: the largest frame that shows at
    /// one frame pixel per terminal pixel.
    pub fn room(&self) -> (u32, u32) {
        (
            u32::from(self.cols) * u32::from(self.cell_w),
            u32::from(self.rows.saturating_sub(STATUS_ROWS)) * u32::from(self.cell_h),
        )
    }
}

/// A frame placed in the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub cells: Cells,
    pub cell_w: u16,
    pub cell_h: u16,
    /// The frame size in its own pixels.
    pub frame_w: u32,
    pub frame_h: u32,
}

impl Placement {
    /// The size of the drawn rectangle in terminal pixels.
    pub fn px(&self) -> (u32, u32) {
        (
            u32::from(self.cells.cols) * u32::from(self.cell_w),
            u32::from(self.cells.rows) * u32::from(self.cell_h),
        )
    }

    /// A terminal pixel as a frame point, or None outside the frame.
    pub fn point_px(&self, x: u32, y: u32) -> Option<(u32, u32)> {
        let x0 = u32::from(self.cells.col) * u32::from(self.cell_w);
        let y0 = u32::from(self.cells.row) * u32::from(self.cell_h);
        let (w, h) = self.px();
        if x < x0 || y < y0 || x >= x0 + w || y >= y0 + h {
            return None;
        }
        let fx = (u64::from(x - x0) * u64::from(self.frame_w) / u64::from(w)) as u32;
        let fy = (u64::from(y - y0) * u64::from(self.frame_h) / u64::from(h)) as u32;
        Some((fx.min(self.frame_w - 1), fy.min(self.frame_h - 1)))
    }

    /// A cell as the frame point at its center, or None outside the frame.
    pub fn point_cell(&self, col: u32, row: u32) -> Option<(u32, u32)> {
        let (cw, ch) = (u32::from(self.cell_w), u32::from(self.cell_h));
        self.point_px(col * cw + cw / 2, row * ch + ch / 2)
    }
}

/// Place a `frame_w` x `frame_h` frame in `term`, or None when the
/// terminal has no room (fewer than one row above the status line).
pub fn place(term: &Term, frame_w: u32, frame_h: u32) -> Option<Placement> {
    if frame_w == 0 || frame_h == 0 || term.rows <= STATUS_ROWS {
        return None;
    }
    let (cw, ch) = (u64::from(term.cell_w), u64::from(term.cell_h));
    let avail_w = u64::from(term.cols) * cw;
    let avail_h = u64::from(term.rows - STATUS_ROWS) * ch;
    let (fw, fh) = (u64::from(frame_w), u64::from(frame_h));
    // The scale is the smallest of avail_w / fw, avail_h / fh and 1, as
    // a fraction num / den.
    let (num, den) = if avail_w * fh <= avail_h * fw {
        (avail_w, fw)
    } else {
        (avail_h, fh)
    };
    let (num, den) = if num >= den { (1, 1) } else { (num, den) };
    let w = fw * num / den;
    let h = fh * num / den;
    let cols = (w + cw / 2) / cw;
    let rows = (h + ch / 2) / ch;
    let cols = cols.clamp(1, u64::from(term.cols)) as u16;
    let rows = rows.clamp(1, u64::from(term.rows - STATUS_ROWS)) as u16;
    let col = (term.cols - cols) / 2;
    let row = (term.rows - STATUS_ROWS - rows) / 2;
    Some(Placement {
        cells: Cells {
            col,
            row,
            cols,
            rows,
        },
        cell_w: term.cell_w,
        cell_h: term.cell_h,
        frame_w,
        frame_h,
    })
}
