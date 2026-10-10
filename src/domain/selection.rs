use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cell {
    pub row: usize,
    pub column: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub anchor: Cell,
    pub active: Cell,
}

impl Selection {
    pub fn rectangle(
        self,
    ) -> (
        std::ops::RangeInclusive<usize>,
        std::ops::RangeInclusive<usize>,
    ) {
        (
            self.anchor.row.min(self.active.row)..=self.anchor.row.max(self.active.row),
            self.anchor.column.min(self.active.column)..=self.anchor.column.max(self.active.column),
        )
    }

    pub fn cell_count(self) -> usize {
        let (rows, columns) = self.rectangle();
        rows.count().saturating_mul(columns.count())
    }

    pub fn clamped(self, row_count: usize, column_count: usize) -> Option<Self> {
        if row_count == 0 || column_count == 0 {
            return None;
        }
        let clamp = |cell: Cell| Cell {
            row: cell.row.min(row_count - 1),
            column: cell.column.min(column_count - 1),
        };
        Some(Self {
            anchor: clamp(self.anchor),
            active: clamp(self.active),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rectangle_handles_reverse_and_offscreen_selection() {
        let selected = Selection {
            anchor: Cell {
                row: 999_999,
                column: 8,
            },
            active: Cell { row: 2, column: 1 },
        };
        assert_eq!(selected.rectangle(), (2..=999_999, 1..=8));
        assert_eq!(selected.cell_count(), 7_999_984);
    }

    #[test]
    fn empty_result_has_no_selection() {
        let selected = Selection {
            anchor: Cell { row: 3, column: 2 },
            active: Cell { row: 5, column: 4 },
        };
        assert_eq!(selected.clamped(0, 4), None);
        assert_eq!(
            selected.clamped(3, 2).unwrap().active,
            Cell { row: 2, column: 1 }
        );
    }
}
