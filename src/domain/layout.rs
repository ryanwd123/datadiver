use std::ops::Range;

#[derive(Clone, Debug)]
pub struct GridGeometry {
    widths: Vec<f32>,
    frozen: usize,
    viewport_width: f32,
    scroll_x: f32,
}

impl GridGeometry {
    pub fn new(widths: Vec<f32>, frozen: usize, viewport_width: f32, scroll_x: f32) -> Self {
        Self {
            frozen: frozen.min(widths.len()),
            widths: widths.into_iter().map(|width| width.max(1.0)).collect(),
            viewport_width: viewport_width.max(0.0),
            scroll_x: scroll_x.max(0.0),
        }
    }

    pub fn frozen_width(&self) -> f32 {
        self.widths[..self.frozen].iter().sum()
    }

    pub fn scrollable_width(&self) -> f32 {
        (self.viewport_width - self.frozen_width()).max(0.0)
    }

    pub fn content_width(&self) -> f32 {
        self.widths[self.frozen..].iter().sum()
    }

    pub fn max_scroll_x(&self) -> f32 {
        (self.content_width() - self.scrollable_width()).max(0.0)
    }

    pub fn visible_columns(&self, overscan: f32) -> (Range<usize>, Range<usize>) {
        let frozen = 0..self.frozen;
        if self.scrollable_width() == 0.0 {
            return (frozen, self.frozen..self.frozen);
        }
        let left = (self.scroll_x.min(self.max_scroll_x()) - overscan).max(0.0);
        let right = self.scroll_x.min(self.max_scroll_x()) + self.scrollable_width() + overscan;
        let mut start = None;
        let mut end = self.frozen;
        let mut x = 0.0;
        for (i, width) in self.widths.iter().enumerate().skip(self.frozen) {
            if x + width > left && x < right {
                start.get_or_insert(i);
                end = i + 1;
            }
            x += width;
        }
        let start = start.unwrap_or(self.frozen);
        (frozen, start..end.max(start))
    }

    pub fn column_x(&self, column: usize) -> Option<f32> {
        if column >= self.widths.len() {
            return None;
        }
        let prior: f32 = if column < self.frozen {
            self.widths[..column].iter().sum()
        } else {
            self.widths[self.frozen..column].iter().sum::<f32>() + self.frozen_width()
                - self.scroll_x.min(self.max_scroll_x())
        };
        Some(prior)
    }

    pub fn hit_test_column(&self, x: f32) -> Option<usize> {
        if x < 0.0 || x >= self.viewport_width {
            return None;
        }
        let frozen_width = self.frozen_width();
        if x < frozen_width {
            let mut current = 0.0;
            for (i, width) in self.widths[..self.frozen].iter().enumerate() {
                current += width;
                if x < current {
                    return Some(i);
                }
            }
            return None;
        }
        if self.scrollable_width() == 0.0 {
            return None;
        }
        let logical = x - frozen_width + self.scroll_x.min(self.max_scroll_x());
        let mut current = 0.0;
        for (i, width) in self.widths.iter().enumerate().skip(self.frozen) {
            current += width;
            if logical < current {
                return Some(i);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frozen_hit_testing_and_visibility() {
        let grid = GridGeometry::new(vec![100.0, 120.0, 80.0, 90.0], 1, 250.0, 60.0);
        assert_eq!(grid.hit_test_column(50.0), Some(0));
        assert_eq!(grid.hit_test_column(110.0), Some(1));
        assert_eq!(grid.hit_test_column(180.0), Some(2));
        assert_eq!(grid.visible_columns(0.0), (0..1, 1..4));
    }

    #[test]
    fn oversized_frozen_pane_is_safe() {
        let grid = GridGeometry::new(vec![200.0, 200.0, 100.0], 2, 100.0, 400.0);
        assert_eq!(grid.scrollable_width(), 0.0);
        assert_eq!(grid.visible_columns(0.0), (0..2, 2..2));
        assert_eq!(grid.hit_test_column(50.0), Some(0));
        assert_eq!(grid.hit_test_column(150.0), None);
    }
}
