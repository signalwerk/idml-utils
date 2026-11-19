//! Explicit local geometry for producers. Values are points, with no conversion
//! from source-application rulers, page coordinates or inherited transforms.
use crate::{Error, ErrorKind};

/// A rectangle in a producer element's local coordinate system, in points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
}
impl Rect {
    pub fn new(left: f64, top: f64, right: f64, bottom: f64) -> Result<Self, Error> {
        if ![left, top, right, bottom, right - left, bottom - top].iter().all(|v| v.is_finite()) || right <= left || bottom <= top {
            return Err(Error::new(ErrorKind::Write, "rectangle requires finite, increasing left/right and top/bottom coordinates"));
        }
        Ok(Self { left, top, right, bottom })
    }
    /// Corners in the order used by a closed IDML rectangle contour.
    pub fn corners(&self) -> [[f64; 2]; 4] {
        [[self.left, self.top], [self.left, self.bottom], [self.right, self.bottom], [self.right, self.top]]
    }
    pub fn width(&self) -> f64 {
        self.right - self.left
    }
    pub fn height(&self) -> f64 {
        self.bottom - self.top
    }
}

/// IDML's stored six-value affine transform `[a,b,c,d,tx,ty]`.
/// Maps local `(x,y)` to `(a*x+c*y+tx, b*x+d*y+ty)` in the parent space.
/// No page-origin or ruler conversion is inferred.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform([f64; 6]);
impl Transform {
    pub const IDENTITY: Self = Self([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    pub fn new(values: [f64; 6]) -> Result<Self, Error> {
        if !values.iter().all(|v| v.is_finite()) {
            return Err(Error::new(ErrorKind::Write, "transform values must be finite"));
        }
        Ok(Self(values))
    }
    pub fn translation(x: f64, y: f64) -> Result<Self, Error> {
        Self::new([1.0, 0.0, 0.0, 1.0, x, y])
    }
    pub fn values(&self) -> &[f64; 6] {
        &self.0
    }
    pub fn apply(&self, point: [f64; 2]) -> [f64; 2] {
        let [a, b, c, d, x, y] = self.0;
        [a * point[0] + c * point[1] + x, b * point[0] + d * point[1] + y]
    }
}
