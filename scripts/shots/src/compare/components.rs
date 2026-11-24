//! The connected groups of marked cells of a grid.

/// One group of marked cells that share edges, as (column, row).
pub(super) struct Component {
    pub(super) cells: Vec<(usize, usize)>,
}

impl Component {
    /// The leftmost and rightmost column, the top and bottom row, all inclusive:
    /// (left, top, right, bottom).
    pub(super) fn extent(&self) -> (usize, usize, usize, usize) {
        let columns = self.cells.iter().map(|c| c.0);
        let rows = self.cells.iter().map(|c| c.1);
        (
            columns.clone().min().unwrap_or(0),
            rows.clone().min().unwrap_or(0),
            columns.max().unwrap_or(0),
            rows.max().unwrap_or(0),
        )
    }
}

/// The groups of the `columns` by `rows` grid's cells that `marked` accepts,
/// in the order of their first cell by row, then column.
pub(super) fn components(
    columns: usize,
    rows: usize,
    marked: impl Fn(usize, usize) -> bool,
) -> Vec<Component> {
    let mut seen = vec![false; columns * rows];
    let mut found = Vec::new();
    for start in 0..columns * rows {
        if seen[start] || !marked(start % columns, start / columns) {
            continue;
        }
        seen[start] = true;
        let mut stack = vec![start];
        let mut cells = Vec::new();
        while let Some(at) = stack.pop() {
            let (x, y) = (at % columns, at / columns);
            cells.push((x, y));
            let around = [
                (x.wrapping_sub(1), y),
                (x + 1, y),
                (x, y.wrapping_sub(1)),
                (x, y + 1),
            ];
            for (nx, ny) in around {
                if nx < columns && ny < rows && !seen[ny * columns + nx] && marked(nx, ny) {
                    seen[ny * columns + nx] = true;
                    stack.push(ny * columns + nx);
                }
            }
        }
        found.push(Component { cells });
    }
    found
}
