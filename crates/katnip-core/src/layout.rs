//! Tiling layouts.
//!
//! All geometry here is dependency-free logical pixels so it can be unit
//! tested in isolation; callers convert to/from smithay types at the boundary.

/// A rectangle in logical pixel coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    pub const fn right(&self) -> i32 {
        self.x + self.w
    }

    pub const fn bottom(&self) -> i32 {
        self.y + self.h
    }

    /// Shrinks the rect by `px` on every side, saturating at an empty rect.
    pub fn shrink(&self, px: i32) -> Self {
        Self {
            x: self.x + px,
            y: self.y + px,
            w: (self.w - 2 * px).max(0),
            h: (self.h - 2 * px).max(0),
        }
    }

    /// True if the rectangles share no interior points.
    pub fn intersects(&self, other: &Rect) -> bool {
        self.x < other.right()
            && other.x < self.right()
            && self.y < other.bottom()
            && other.y < self.bottom()
    }

    /// True if this rect fully contains `other`.
    pub fn contains_rect(&self, other: &Rect) -> bool {
        other.x >= self.x
            && other.y >= self.y
            && other.right() <= self.right()
            && other.bottom() <= self.bottom()
    }
}

/// Dwindling layout (Hyprland-style): recursively splits the usable area in
/// half along its longer axis, giving each half a proportional share of the
/// remaining windows.
///
/// `usable` is expected to already have outer gaps applied. Inner gaps of
/// `gap` logical pixels are inserted between adjacent tiles.
///
/// Returns exactly `count` rects (empty vec for `count == 0`), pairwise
/// non-overlapping and contained within `usable`.
pub fn dwindle(usable: Rect, count: usize, gap: i32) -> Vec<Rect> {
    let mut out = Vec::with_capacity(count);
    split_recursive(usable, count, gap, &mut out);
    out
}

fn split_recursive(area: Rect, count: usize, gap: i32, out: &mut Vec<Rect>) {
    if count == 0 || area.w <= 0 || area.h <= 0 {
        return;
    }

    if count == 1 {
        out.push(area);
        return;
    }

    // Split the window count proportionally to the axis we divide, keeping
    // the resulting tiles as square as practical.
    let left_count = if area.w >= area.h {
        count.div_ceil(2)
    } else {
        count / 2
    };
    let right_count = count - left_count;

    if area.w >= area.h {
        let left_w = ((area.w as i64 * left_count as i64) / count as i64) as i32;
        let half_gap = gap / 2;
        let left = Rect::new(area.x, area.y, (left_w - half_gap).max(0), area.h);
        let right = Rect::new(
            area.x + left_w + half_gap,
            area.y,
            (area.w - left_w - half_gap).max(0),
            area.h,
        );
        split_recursive(left, left_count, gap, out);
        split_recursive(right, right_count, gap, out);
    } else {
        let top_h = ((area.h as i64 * left_count as i64) / count as i64) as i32;
        let half_gap = gap / 2;
        let top = Rect::new(area.x, area.y, area.w, (top_h - half_gap).max(0));
        let bottom = Rect::new(
            area.x,
            area.y + top_h + half_gap,
            area.w,
            (area.h - top_h - half_gap).max(0),
        );
        split_recursive(top, left_count, gap, out);
        split_recursive(bottom, right_count, gap, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Rect = Rect::new(0, 0, 1920, 1080);

    #[test]
    fn zero_windows_is_empty() {
        assert!(dwindle(SCREEN, 0, 8).is_empty());
    }

    #[test]
    fn single_window_fills_usable_area() {
        let usable = SCREEN.shrink(10);
        assert_eq!(dwindle(usable, 1, 8), vec![usable]);
    }

    #[test]
    fn produces_requested_count() {
        for count in 1..=9usize {
            assert_eq!(dwindle(SCREEN.shrink(10), count, 8).len(), count);
        }
    }

    #[test]
    fn tiles_never_overlap() {
        for count in 1..=12usize {
            let tiles = dwindle(SCREEN.shrink(10), count, 8);
            for (i, a) in tiles.iter().enumerate() {
                for b in tiles.iter().skip(i + 1) {
                    assert!(!a.intersects(b), "count={count}: {a:?} overlaps {b:?}");
                }
            }
        }
    }

    #[test]
    fn tiles_stay_inside_usable_area() {
        let usable = SCREEN.shrink(10);
        for count in 1..=12usize {
            for tile in dwindle(usable, count, 8) {
                assert!(
                    usable.contains_rect(&tile),
                    "count={count}: tile {tile:?} escapes usable {usable:?}"
                );
                assert!(tile.w > 0 && tile.h > 0, "degenerate tile {tile:?}");
            }
        }
    }

    #[test]
    fn inner_gaps_are_respected() {
        let usable = SCREEN.shrink(20);
        let gap = 10;
        let tiles = dwindle(usable, 4, gap);
        // No two tiles may be closer than `gap/2` apart on the axis they
        // abut while overlapping on the other axis.
        for (i, a) in tiles.iter().enumerate() {
            for b in tiles.iter().skip(i + 1) {
                let overlap_y = a.y < b.bottom() && b.y < a.bottom();
                let overlap_x = a.x < b.right() && b.x < a.right();
                if overlap_x && overlap_y {
                    panic!("tiles {a:?} and {b:?} intersect");
                }
                if overlap_x {
                    let dist = if a.bottom() <= b.y {
                        b.y - a.bottom()
                    } else {
                        a.y - b.bottom()
                    };
                    assert!(
                        dist >= gap / 2,
                        "vertical gap {dist} < {gap}/2 between {a:?} {b:?}"
                    );
                }
                if overlap_y {
                    let dist = if a.right() <= b.x {
                        b.x - a.right()
                    } else {
                        a.x - b.right()
                    };
                    assert!(
                        dist >= gap / 2,
                        "horizontal gap {dist} < {gap}/2 between {a:?} {b:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn degenerate_areas_yield_nothing() {
        assert!(dwindle(Rect::new(0, 0, 0, 500), 3, 8).is_empty());
        assert!(dwindle(Rect::new(0, 0, 500, 0), 3, 8).is_empty());
    }

    #[test]
    fn first_window_is_left_half_on_wide_screen() {
        let usable = SCREEN.shrink(10);
        let tiles = dwindle(usable, 2, 0);
        // Two windows on a wide screen: side-by-side, equal halves.
        assert_eq!(tiles[0].y, usable.y);
        assert_eq!(tiles[1].y, usable.y);
        assert_eq!(tiles[0].w, tiles[1].w);
        assert!(tiles[0].right() <= tiles[1].x || tiles[1].right() <= tiles[0].x);
    }
}
