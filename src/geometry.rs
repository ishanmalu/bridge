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

/// Where a pointer arriving through the edge facing `side`, at `frac` along it, should land.
pub fn entry_point(ds: &[Rect], side: Side, frac: f64) -> (f64, f64) {
    let b = bounds(ds);
    let inset = 2.0;
    let vertical = matches!(side, Side::Left | Side::Right);
    let c = if vertical { b.y + frac * b.h } else { b.x + frac * b.w };
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
}
