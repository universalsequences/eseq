#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CableCurve {
    pub p0: (f32, f32),
    pub p1: (f32, f32),
    pub p2: (f32, f32),
    pub p3: (f32, f32),
}

const SAME_X_EPSILON: f32 = 0.001;
const MIN_X_HANDLE: f32 = 1.2;
const MAX_X_HANDLE: f32 = 4.8;
const MIN_Y_HANDLE: f32 = 1.1;
const MAX_Y_HANDLE: f32 = 2.4;

/// Generate the patch-editor cable Bezier used by the standalone Swift editor.
///
/// The original Swift implementation works in a world coordinate system where
/// "down" is negative Y. eseqlisp patcher primitives use screen-style cells,
/// where Y increases downward, so the vertical handle signs are mirrored here.
pub fn cable_curve(start: (f32, f32), end: (f32, f32)) -> CableCurve {
    let diff_y = (end.1 - start.1).abs();
    let y_a = diff_y.clamp(MIN_Y_HANDLE, MAX_Y_HANDLE);
    let y_b = diff_y.clamp(MIN_Y_HANDLE, MAX_Y_HANDLE);
    let diff_x = (end.0 - start.0).abs();

    let (p1, p2) = if diff_x < SAME_X_EPSILON {
        ((start.0, start.1 + y_a), (end.0, end.1 - y_b))
    } else {
        let x_a = diff_x.clamp(MIN_X_HANDLE, MAX_X_HANDLE);
        let x_b = diff_x.clamp(MIN_X_HANDLE, MAX_X_HANDLE);
        let x_sign = if end.0 - start.0 < 0.0 { 1.0 } else { -1.0 };
        let final_y_a = if end.1 < start.1 { MAX_Y_HANDLE } else { y_a };
        (
            (start.0 + (-x_sign * x_a), start.1 + final_y_a),
            (end.0 + (x_sign * x_b), end.1 - y_b),
        )
    };

    CableCurve {
        p0: start,
        p1,
        p2,
        p3: end,
    }
}

pub fn cubic_bezier_point(curve: CableCurve, t: f32) -> (f32, f32) {
    let u = 1.0 - t;
    let tt = t * t;
    let uu = u * u;
    let uuu = uu * u;
    let ttt = tt * t;
    (
        uuu * curve.p0.0 + 3.0 * uu * t * curve.p1.0 + 3.0 * u * tt * curve.p2.0 + ttt * curve.p3.0,
        uuu * curve.p0.1 + 3.0 * uu * t * curve.p1.1 + 3.0 * u * tt * curve.p2.1 + ttt * curve.p3.1,
    )
}

pub fn distance_to_cable_px(start: (f32, f32), end: (f32, f32), point: (f32, f32)) -> f32 {
    let curve = cable_curve(start, end);
    let mut best = f32::MAX;
    let mut prev = curve.p0;
    for i in 1..=8 {
        let t = i as f32 / 8.0;
        let current = cubic_bezier_point(curve, t);
        best = best.min(distance_to_segment(point, prev, current));
        prev = current;
    }
    best
}

fn distance_to_segment(point: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let ab = (b.0 - a.0, b.1 - a.1);
    let ap = (point.0 - a.0, point.1 - a.1);
    let len_sq = ab.0 * ab.0 + ab.1 * ab.1;
    if len_sq <= f32::EPSILON {
        return ((point.0 - a.0).powi(2) + (point.1 - a.1).powi(2)).sqrt();
    }
    let t = ((ap.0 * ab.0 + ap.1 * ab.1) / len_sq).clamp(0.0, 1.0);
    let closest = (a.0 + ab.0 * t, a.1 + ab.1 * t);
    ((point.0 - closest.0).powi(2) + (point.1 - closest.1).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cable_curve_has_expected_endpoints() {
        let curve = cable_curve((10.0, 20.0), (100.0, 40.0));
        assert_eq!(cubic_bezier_point(curve, 0.0), (10.0, 20.0));
        assert_eq!(cubic_bezier_point(curve, 1.0), (100.0, 40.0));
    }

    #[test]
    fn cable_curve_moves_horizontal_handles_toward_each_other() {
        let curve = cable_curve((10.0, 20.0), (100.0, 40.0));
        assert!(curve.p1.0 > curve.p0.0, "{curve:?}");
        assert!(curve.p2.0 < curve.p3.0, "{curve:?}");
        assert!(curve.p1.1 > curve.p0.1, "{curve:?}");
        assert!(curve.p2.1 < curve.p3.1, "{curve:?}");
    }

    #[test]
    fn cable_curve_keeps_vertical_cables_vertical() {
        let curve = cable_curve((10.0, 20.0), (10.0, 60.0));
        assert_eq!(curve.p1.0, curve.p0.0);
        assert_eq!(curve.p2.0, curve.p3.0);
        assert!(curve.p1.1 > curve.p0.1, "{curve:?}");
        assert!(curve.p2.1 < curve.p3.1, "{curve:?}");
    }

    #[test]
    fn distance_to_cable_is_small_near_endpoint() {
        let distance = distance_to_cable_px((10.0, 20.0), (100.0, 40.0), (10.5, 20.5));
        assert!(distance < 1.0, "{distance}");
    }
}
