//! Screen-edge maths, shared by both sides so a crossing lands at the same point along the border.

use crate::platform::Rect;
use crate::proto::Side;

fn bounds(ds: &[Rect]) -> Rect {
    let x0 = ds.iter().map(|r| r.x).fold(f64::MAX, f64::min);
    let y0 = ds.iter().map(|r| r.y).fold(f64::MAX, f64::min);
    let x1 = ds.iter().map(|r| r.right()).fold(f64::MIN, f64::max);
    let y1 = ds.iter().map(|r| r.bottom()).fold(f64::MIN, f64::max);
    Rect { x: x0, y: y0, w: (x1 - x0).max(1.0), h: (y1 - y0).max(1.0) }
}

/// The display holding (x, y), or the nearest one.
pub fn display_at(ds: &[Rect], x: f64, y: f64) -> Option<Rect> {
    ds.iter().copied().min_by(|a, b| dist(a, x, y).total_cmp(&dist(b, x, y)))
}

fn dist(r: &Rect, x: f64, y: f64) -> f64 {
    let dx = (r.x - x).max(0.0).max(x - (r.right() - 1.0));
    let dy = (r.y - y).max(0.0).max(y - (r.bottom() - 1.0));
    dx * dx + dy * dy
}

fn clamp_into(r: &Rect, x: f64, y: f64) -> (f64, f64) {
    (x.clamp(r.x, r.right() - 1.0), y.clamp(r.y, r.bottom() - 1.0))
}

fn beyond(side: Side, x: f64, y: f64) -> (f64, f64) {
    match side {
        Side::Left => (x - 2.0, y),
        Side::Right => (x + 2.0, y),
        Side::Top => (x, y - 2.0),
        Side::Bottom => (x, y + 2.0),
    }
}

/// The pointer sits on an outer edge facing `side` (no other display continues past it).
pub fn on_edge(ds: &[Rect], x: f64, y: f64, side: Side) -> bool {
    let Some(r) = display_at(ds, x, y) else { return false };
    let (x, y) = clamp_into(&r, x, y);
    let at = match side {
        Side::Left => x <= r.x,
        Side::Right => x >= r.right() - 1.0,
        Side::Top => y <= r.y,
        Side::Bottom => y >= r.bottom() - 1.0,
    };
    let (bx, by) = beyond(side, x, y);
    at && !ds.iter().any(|d| d.contains(bx, by))
}

/// Movement into the edge facing `side`.
pub fn outward(side: Side, dx: f64, dy: f64) -> f64 {
    match side {
        Side::Left => -dx,
        Side::Right => dx,
        Side::Top => -dy,
        Side::Bottom => dy,
    }
}

/// 0..1 position along the shared edge.
pub fn frac_along(ds: &[Rect], x: f64, y: f64, side: Side) -> f64 {
    let b = bounds(ds);
    let f = match side {
        Side::Left | Side::Right => (y - b.y) / b.h,
        Side::Top | Side::Bottom => (x - b.x) / b.w,
    };
    f.clamp(0.0, 1.0)
}

pub fn displays_bounds(ds: &[crate::proto::DisplayInfo]) -> Rect {
    if ds.is_empty() {
        return Rect { x: 0.0, y: 0.0, w: 1920.0, h: 1080.0 };
    }
    let x0 = ds.iter().map(|d| d.x).fold(f64::MAX, f64::min);
    let y0 = ds.iter().map(|d| d.y).fold(f64::MAX, f64::min);
    let x1 = ds.iter().map(|d| d.x + d.width).fold(f64::MIN, f64::max);
    let y1 = ds.iter().map(|d| d.y + d.height).fold(f64::MIN, f64::max);
    Rect { x: x0, y: y0, w: (x1 - x0).max(1.0), h: (y1 - y0).max(1.0) }
}

/// Convert a local edge point (x, y) into a 0..1 fraction along the peer's shared edge,
/// accounting for the relative layout offset and peer monitor bounds.
pub fn peer_frac_along(
    local_ds: &[Rect],
    peer_ds: &[crate::proto::DisplayInfo],
    x: f64,
    y: f64,
    side: Side,
    peer_offset: f64,
) -> f64 {
    if peer_ds.is_empty() {
        return frac_along(local_ds, x, y, side);
    }
    let lb = bounds(local_ds);
    let pb = displays_bounds(peer_ds);
    let vertical = matches!(side, Side::Left | Side::Right);

    let (local_pos, peer_span) = if vertical {
        (y - lb.y, pb.h)
    } else {
        (x - lb.x, pb.w)
    };

    // In the visual designer: peerCluster is placed at `peer_offset` relative to localCluster.
    // So a point on localCluster at `local_pos` maps to `local_pos - peer_offset` on peerCluster.
    let pos_in_peer = local_pos - peer_offset;
    (pos_in_peer / peer_span.max(1.0)).clamp(0.0, 1.0)
}

/// Where a pointer arriving through the edge facing `side`, at `frac` along it, should land.
#[allow(dead_code)]
pub fn entry_point(ds: &[Rect], side: Side, frac: f64) -> (f64, f64) {
    entry_point_with_offset(ds, side, frac, 0.0)
}

/// Where a pointer arriving through the edge facing `side`, at `frac` along it plus a fine-tuned pixel offset, should land.
pub fn entry_point_with_offset(ds: &[Rect], side: Side, frac: f64, offset: f64) -> (f64, f64) {
    let b = bounds(ds);
    let inset = 2.0;
    let vertical = matches!(side, Side::Left | Side::Right);
    let c = (if vertical { b.y + frac * b.h } else { b.x + frac * b.w }) + offset;
    let spans: Vec<&Rect> = ds
        .iter()
        .filter(|r| if vertical { c >= r.y && c < r.bottom() } else { c >= r.x && c < r.right() })
        .collect();
    let pick = |rs: &[&Rect]| -> Option<Rect> {
        rs.iter()
            .copied()
            .max_by(|a, b| {
                let ka = match side { Side::Left => -a.x, Side::Right => a.right(), Side::Top => -a.y, Side::Bottom => a.bottom() };
                let kb = match side { Side::Left => -b.x, Side::Right => b.right(), Side::Top => -b.y, Side::Bottom => b.bottom() };
                ka.total_cmp(&kb)
            })
            .copied()
    };
    let r = pick(&spans).or_else(|| pick(&ds.iter().collect::<Vec<_>>())).unwrap_or(b);
    let c = if vertical { c.clamp(r.y, r.bottom() - 1.0) } else { c.clamp(r.x, r.right() - 1.0) };
    match side {
        Side::Left => (r.x + inset, c),
        Side::Right => (r.right() - 1.0 - inset, c),
        Side::Top => (c, r.y + inset),
        Side::Bottom => (c, r.bottom() - 1.0 - inset),
    }
}

pub fn center(ds: &[Rect]) -> (f64, f64) {
    let r = ds.first().copied().unwrap_or(Rect { x: 0.0, y: 0.0, w: 800.0, h: 600.0 });
    (r.x + r.w / 2.0, r.y + r.h / 2.0)
}

#[allow(dead_code)]
pub enum Step {
    Move(f64, f64),
    /// Pushed off the edge facing the other machine, at this fraction along it.
    Exit(f64),
}

/// Move a pointer that the other machine is driving.
pub fn step(ds: &[Rect], x: f64, y: f64, dx: f64, dy: f64, side: Side) -> Step {
    let (tx, ty) = (x + dx, y + dy);
    if ds.iter().any(|r| r.contains(tx, ty)) {
        return Step::Move(tx, ty);
    }
    let Some(r) = display_at(ds, x, y) else { return Step::Move(x, y) };
    let leaving = match side {
        Side::Left => tx < r.x,
        Side::Right => tx >= r.right(),
        Side::Top => ty < r.y,
        Side::Bottom => ty >= r.bottom(),
    };
    let (cx, cy) = clamp_into(&r, tx, ty);
    if leaving && on_edge(ds, cx, cy, side) {
        return Step::Exit(frac_along(ds, cx, cy, side));
    }
    Step::Move(cx, cy)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two() -> Vec<Rect> {
        // A laptop with a taller external display to its right.
        vec![Rect { x: 0.0, y: 0.0, w: 1512.0, h: 982.0 }, Rect { x: 1512.0, y: -200.0, w: 2560.0, h: 1440.0 }]
    }

    #[test]
    fn inner_border_is_not_an_edge() {
        assert!(!on_edge(&two(), 1511.0, 500.0, Side::Right));
        assert!(on_edge(&two(), 4071.0, 500.0, Side::Right));
        assert!(on_edge(&two(), 0.0, 500.0, Side::Left));
    }

    #[test]
    fn exits_and_reenters_at_same_fraction() {
        let ds = two();
        let Step::Exit(f) = step(&ds, 4071.0, 520.0, 5.0, 0.0, Side::Right) else { panic!() };
        let (x, y) = entry_point(&ds, Side::Right, f);
        assert!((y - 520.0).abs() < 1.0 && x > 4000.0);
    }

    #[test]
    fn moves_between_displays() {
        let ds = two();
        assert!(matches!(step(&ds, 1500.0, 100.0, 30.0, 0.0, Side::Right), Step::Move(x, _) if x > 1512.0));
        assert!(matches!(step(&ds, 10.0, 100.0, -30.0, 0.0, Side::Right), Step::Move(x, _) if x == 0.0));
    }

    #[test]
    fn peer_multi_monitor_alignment_and_exit_blocking() {
        let mac_ds = vec![Rect { x: 0.0, y: 0.0, w: 1470.0, h: 956.0 }];
        let win_peer_ds = vec![
            crate::proto::DisplayInfo {
                id: 1,
                name: "Display 2".into(),
                x: -1920.0,
                y: 165.0,
                width: 1920.0,
                height: 1080.0,
                width_mm: 508.0,
                height_mm: 285.0,
                inches: 22.9,
                scale_factor: 1.0,
                pixel_width: 1920,
                pixel_height: 1080,
                is_primary: false,
            },
            crate::proto::DisplayInfo {
                id: 2,
                name: "Display 1".into(),
                x: 0.0,
                y: 0.0,
                width: 2560.0,
                height: 1440.0,
                width_mm: 677.0,
                height_mm: 381.0,
                inches: 30.6,
                scale_factor: 1.0,
                pixel_width: 2560,
                pixel_height: 1440,
                is_primary: true,
            },
        ];

        // 1. Moving up on Mac at right edge (x = 1470.0) with offset 0.0:
        // Must land inside Display 2 on Windows (-450.0), NOT at the far right of Display 1 (+2560.0)
        let f_right = peer_frac_along(&mac_ds, &win_peer_ds, 1470.0, 0.0, Side::Top, 0.0);
        let win_rects: Vec<Rect> = win_peer_ds.iter().map(|d| Rect { x: d.x, y: d.y, w: d.width, h: d.height }).collect();
        let (win_x, _) = entry_point(&win_rects, Side::Bottom, f_right);
        assert!((win_x - (-450.0)).abs() < 1.0, "Expected -450.0 but got {win_x}");

        // 2. On Windows moving down at x = -1000.0 (inside Display 2):
        let mac_peer_ds = vec![crate::proto::DisplayInfo {
            id: 1,
            name: "Mac".into(),
            x: 0.0,
            y: 0.0,
            width: 1470.0,
            height: 956.0,
            width_mm: 300.0,
            height_mm: 200.0,
            inches: 13.6,
            scale_factor: 2.0,
            pixel_width: 2940,
            pixel_height: 1912,
            is_primary: true,
        }];
        let f_win = peer_frac_along(&win_rects, &mac_peer_ds, -1000.0, 1244.0, Side::Bottom, 0.0);
        assert!(f_win > 0.0 && f_win < 1.0, "Should cross into Mac");
    }
}
