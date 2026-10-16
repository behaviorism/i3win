#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// Largest coordinate/size magnitude accepted for floating rects.
pub const MAX_PX: i32 = 100_000;
/// Smallest width/height of a floating window.
pub const MIN_FLOAT_SIZE: i32 = 50;

impl Rect {
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }

    pub fn right(&self) -> i32 {
        self.x.saturating_add(self.w)
    }

    pub fn bottom(&self) -> i32 {
        self.y.saturating_add(self.h)
    }

    pub fn center(&self) -> (i32, i32) {
        (
            self.x.saturating_add(self.w / 2),
            self.y.saturating_add(self.h / 2),
        )
    }

    pub fn inset(&self, left: i32, top: i32, right: i32, bottom: i32) -> Rect {
        Rect {
            x: self.x.saturating_add(left),
            y: self.y.saturating_add(top),
            w: self.w.saturating_sub(left).saturating_sub(right).max(1),
            h: self.h.saturating_sub(top).saturating_sub(bottom).max(1),
        }
    }

    pub fn centered_in(&self, outer: &Rect) -> Rect {
        Rect {
            x: outer.x.saturating_add(outer.w.saturating_sub(self.w) / 2),
            y: outer.y.saturating_add(outer.h.saturating_sub(self.h) / 2),
            w: self.w,
            h: self.h,
        }
    }
}

/// Clamps a floating rect to sane bounds: position within +-MAX_PX and
/// size within [MIN_FLOAT_SIZE, MAX_PX].
pub fn sanitize_floating(r: Rect) -> Rect {
    Rect {
        x: r.x.clamp(-MAX_PX, MAX_PX),
        y: r.y.clamp(-MAX_PX, MAX_PX),
        w: r.w.clamp(MIN_FLOAT_SIZE, MAX_PX),
        h: r.h.clamp(MIN_FLOAT_SIZE, MAX_PX),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Orientation {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

impl Direction {
    pub fn orientation(self) -> Orientation {
        match self {
            Direction::Left | Direction::Right => Orientation::Horizontal,
            Direction::Up | Direction::Down => Orientation::Vertical,
        }
    }

    pub fn is_forward(self) -> bool {
        matches!(self, Direction::Right | Direction::Down)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inset_shrinks() {
        assert_eq!(
            Rect::new(0, 0, 100, 50).inset(2, 20, 2, 2),
            Rect::new(2, 20, 96, 28)
        );
    }
    #[test]
    fn inset_never_below_one() {
        assert_eq!(Rect::new(0, 0, 10, 10).inset(8, 8, 8, 8).w, 1);
    }
    #[test]
    fn centered_in_outer() {
        assert_eq!(
            Rect::new(0, 0, 400, 300).centered_in(&Rect::new(0, 0, 1000, 500)),
            Rect::new(300, 100, 400, 300)
        );
    }
    #[test]
    fn rect_math_saturates() {
        let r = Rect::new(i32::MAX, i32::MAX, i32::MAX, i32::MAX);
        assert_eq!(r.right(), i32::MAX);
        assert_eq!(r.bottom(), i32::MAX);
        let _ = r.center();
        let _ = r.inset(i32::MIN, i32::MIN, i32::MIN, i32::MIN);
        let _ = r.inset(i32::MAX, i32::MAX, i32::MAX, i32::MAX);
        let _ = r.centered_in(&Rect::new(i32::MIN, i32::MIN, i32::MAX, i32::MAX));
    }
    #[test]
    fn sanitize_clamps() {
        let r = sanitize_floating(Rect::new(i32::MAX, i32::MIN, 0, i32::MAX));
        assert_eq!(r, Rect::new(MAX_PX, -MAX_PX, MIN_FLOAT_SIZE, MAX_PX));
    }
    #[test]
    fn direction_props() {
        assert_eq!(Direction::Left.orientation(), Orientation::Horizontal);
        assert_eq!(Direction::Down.orientation(), Orientation::Vertical);
        assert!(Direction::Right.is_forward() && Direction::Down.is_forward());
        assert!(!Direction::Left.is_forward() && !Direction::Up.is_forward());
    }
}
