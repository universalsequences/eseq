//! Euclidean ellipse distance, shared by CPU hit testing and its tests.
//!
//! Closest-point construction and root bounds follow David Eberly,
//! "Distance from a Point to an Ellipse, an Ellipsoid, or a Hyperellipsoid",
//! sections 2.5–2.8 (CC BY 4.0):
//! https://www.geometrictools.com/Documentation/DistancePointEllipseEllipsoid.pdf
//! We solve for k = (t + minor_radius²) / (minor_radius * abs(y)). Its bracket
//! starts at one, avoiding cancellation and tiny squared intermediates near
//! the major axis. The GPU implementation is emitted in sdf_codegen/geometry.rs.

pub(super) fn ellipse_distance(x: f64, y: f64, rx: f64, ry: f64) -> f64 {
    let (mut x, mut y, mut a, mut b) = (x.abs(), y.abs(), rx.abs(), ry.abs());
    if a < b {
        std::mem::swap(&mut x, &mut y);
        std::mem::swap(&mut a, &mut b);
    }
    if b == 0.0 { return (x - a).max(0.0).hypot(y); }
    if a == b { return x.hypot(y) - a; }
    let gap = (a - b) * (a + b);
    let inside = (x / a).powi(2) + (y / b).powi(2) < 1.0;
    let (cx, cy) = if y == 0.0 {
        if a * x < gap {
            let unit_x = a * x / gap;
            (a * unit_x, b * (1.0 - unit_x * unit_x).max(0.0).sqrt())
        } else {
            (a, 0.0)
        }
    } else if x == 0.0 {
        (0.0, b)
    } else {
        let (nx, ny) = (a * x, b * y);
        let mut lo = if inside { 1.0 } else { (b / y).max(1.0) };
        let mut hi = if inside { b / y } else { nx.hypot(ny) / ny };
        let mut k = lo;
        for _ in 0..48 {
            k = (lo * (hi / lo).sqrt()).clamp(lo, hi);
            if k == lo || k == hi { break; }
            if (nx / (ny * k + gap)).powi(2) + (1.0 / k).powi(2) > 1.0 {
                lo = k;
            } else {
                hi = k;
            }
        }
        (a * nx / (ny * k + gap), b / k)
    };
    let distance = (x - cx).hypot(y - cy);
    if inside { -distance } else { distance }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ellipse_distance_preserves_normal_offsets() {
        // A true distance must report the same offset at the tip, top and
        // diagonals. A scaled-circle/gradient approximation fails this check.
        for (a, b) in [(1.0, 1.0), (2.0, 1.0), (4.0, 0.25), (10.0, 0.1)] {
            for i in 0..720 {
                let angle = std::f64::consts::TAU * i as f64 / 720.0;
                let (s, c) = angle.sin_cos();
                let normal_length = (c / a).hypot(s / b);
                for offset in [-0.1, 0.0, 0.1, 1.0] {
                    // Stay within the minimum radius of curvature inside.
                    let distance = offset * b * b / a;
                    let x = a * c + distance * c / a / normal_length;
                    let y = b * s + distance * s / b / normal_length;
                    for (x, y, rx, ry) in [(x, y, a, b), (y, x, b, a)] {
                        let actual = ellipse_distance(x, y, rx, ry);
                        assert!((actual - distance).abs() < 1e-8,
                            "ellipse ({rx}, {ry}) at ({x}, {y}): {actual} != {distance}");
                    }
                }
            }
        }
    }

    #[test]
    fn ellipse_distance_handles_axes_centers_and_degenerate_radii() {
        assert_eq!(ellipse_distance(0.0, 0.0, 4.0, 0.25), -0.25);
        assert_eq!(ellipse_distance(5.0, 0.0, 4.0, 0.25), 1.0);
        assert_eq!(ellipse_distance(0.0, 1.25, 4.0, 0.25), 1.0);
        assert_eq!(ellipse_distance(0.0, 0.0, 1.0, 1.0), -1.0);
        assert_eq!(ellipse_distance(3.0, 4.0, 0.0, 0.0), 5.0);
        assert_eq!(ellipse_distance(3.0, 4.0, 5.0, 0.0), 4.0);
        assert_eq!(ellipse_distance(3.0, 4.0, 0.0, 5.0), 3.0);
        for x in [0.01, 1.0, 3.9, 4.0, 5.0] {
            let axis = ellipse_distance(x, 0.0, 4.0, 0.25);
            for y in [1e-10, 1e-20] {
                assert!((ellipse_distance(x, y, 4.0, 0.25) - axis).abs() < 1e-8);
            }
            assert_eq!(ellipse_distance(-x, 0.0, -4.0, -0.25), axis);
        }
    }
}
