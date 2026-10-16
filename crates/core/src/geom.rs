#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }

    pub fn right(&self) -> i32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }

    pub fn center(&self) -> (i32, i32) {
        (self.x + self.w / 2, self.y + self.h / 2)
    }

    pub fn inset(&self, left: i32, top: i32, right: i32, bottom: i32) -> Rect {
        Rect {
            x: self.x + left,
            y: self.y + top,
            w: (self.w - left - right).max(1),
            h: (self.h - top - bottom).max(1),
        }
    }

    pub fn centered_in(&self, outer: &Rect) -> Rect {
        Rect {
            x: outer.x + (outer.w - self.w) / 2,
            y: outer.y + (outer.h - self.h) / 2,
            w: self.w,
            h: self.h,
        }
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
    fn direction_props() {
        assert_eq!(Direction::Left.orientation(), Orientation::Horizontal);
        assert_eq!(Direction::Down.orientation(), Orientation::Vertical);
        assert!(Direction::Right.is_forward() && Direction::Down.is_forward());
        assert!(!Direction::Left.is_forward() && !Direction::Up.is_forward());
    }
}
