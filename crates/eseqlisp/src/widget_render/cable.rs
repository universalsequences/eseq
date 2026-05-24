#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CableCurve {
    pub p0: (f32, f32),
    pub p1: (f32, f32),
    pub p2: (f32, f32),
    pub p3: (f32, f32),
}

const SAME_X_EPSILON: f32 = 0.20;
const MIN_Y_HANDLE: f32 = 1.1;
const MAX_Y_HANDLE: f32 = 8.0;
const Y_HANDLE_VERTICAL_WEIGHT: f32 = 0.35;
const Y_HANDLE_HORIZONTAL_WEIGHT: f32 = 0.08;

pub fn should_render_segmented_cable(start: (f32, f32), end: (f32, f32)) -> bool {
    (end.0 - start.0).abs() >= SAME_X_EPSILON
}

/// Generate the patch-editor cable Bezier.
///
/// Cable endpoints keep port-oriented vertical tangents: patcher cables leave
/// bottom outlets downward and enter top inlets from above. The handle length
/// adapts mostly to vertical span, with horizontal distance contributing only
/// when the cable is meaningfully diagonal.
pub fn cable_curve(start: (f32, f32), end: (f32, f32)) -> CableCurve {
    let diff_x = (end.0 - start.0).abs();
    let diff_y = (end.1 - start.1).abs();
    let y_handle = cable_y_handle(diff_x, diff_y);
    let p1 = (start.0, start.1 + y_handle);
    let p2 = (end.0, end.1 - y_handle);

    CableCurve {
        p0: start,
        p1,
        p2,
        p3: end,
    }
}

fn cable_y_handle(diff_x: f32, diff_y: f32) -> f32 {
    let span = diff_x + diff_y;
    let verticalness = if span > f32::EPSILON {
        diff_y / span
    } else {
        0.0
    };
    (diff_y * Y_HANDLE_VERTICAL_WEIGHT + diff_x * Y_HANDLE_HORIZONTAL_WEIGHT * verticalness)
        .clamp(MIN_Y_HANDLE, MAX_Y_HANDLE)
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

pub fn cable_edit_points(
    start: (f32, f32),
    end: (f32, f32),
    distance: f32,
) -> ((f32, f32), (f32, f32)) {
    let curve = cable_curve(start, end);
    let start_t = find_t_for_fixed_distance(curve, 0.0, distance);
    let end_t = find_t_for_fixed_distance_from_end(curve, distance);
    (
        cubic_bezier_point(curve, start_t),
        cubic_bezier_point(curve, end_t),
    )
}

fn find_t_for_fixed_distance(curve: CableCurve, start_t: f32, distance: f32) -> f32 {
    let is_forward = distance > 0.0;
    let target_distance = distance.abs();
    let start_point = cubic_bezier_point(curve, start_t);
    let mut min_t = start_t;
    let mut max_t = if is_forward { 1.0 } else { 0.0 };

    for _ in 0..20 {
        let mid_t = (min_t + max_t) * 0.5;
        let mid_point = cubic_bezier_point(curve, mid_t);
        let current_distance = point_distance(start_point, mid_point);
        if (current_distance - target_distance).abs() < 0.001 {
            return mid_t;
        }
        if current_distance < target_distance {
            if is_forward {
                min_t = mid_t;
            } else {
                max_t = mid_t;
            }
        } else if is_forward {
            max_t = mid_t;
        } else {
            min_t = mid_t;
        }
    }

    (min_t + max_t) * 0.5
}

fn find_t_for_fixed_distance_from_end(curve: CableCurve, fixed_distance: f32) -> f32 {
    let end_point = curve.p3;
    let mut min_t = 0.0;
    let mut max_t = 1.0;

    for _ in 0..20 {
        let mid_t = (min_t + max_t) * 0.5;
        let mid_point = cubic_bezier_point(curve, mid_t);
        let current_distance = point_distance(end_point, mid_point);
        if (current_distance - fixed_distance).abs() < 0.001 {
            return mid_t;
        }
        if current_distance < fixed_distance {
            max_t = mid_t;
        } else {
            min_t = mid_t;
        }
    }

    (min_t + max_t) * 0.5
}

fn point_distance(a: (f32, f32), b: (f32, f32)) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
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

pub fn segmented_cable_edit_points(
    start: (f32, f32),
    end: (f32, f32),
    distance: f32,
) -> ((f32, f32), (f32, f32)) {
    if !should_render_segmented_cable(start, end) {
        return cable_edit_points(start, end, distance);
    }
    ((start.0, start.1 + distance), (end.0, end.1 - distance))
}

pub fn segment_row_for_drag(
    start: (f32, f32),
    end: (f32, f32),
    pointer_row: f32,
    padding: f32,
    extra_range: f32,
) -> f32 {
    if end.1 < start.1 {
        let min_row = start.1 + padding;
        let max_row = start.1.max(end.1) + extra_range;
        pointer_row.clamp(min_row, max_row)
    } else {
        let min_row = start.1.min(end.1) + padding;
        let max_row = start.1.max(end.1) - padding;
        if min_row <= max_row {
            pointer_row.clamp(min_row, max_row)
        } else {
            (start.1 + end.1) * 0.5
        }
    }
}

pub fn distance_to_segmented_cable_px(
    start: (f32, f32),
    end: (f32, f32),
    segment_row: f32,
    corner_radius: f32,
    point: (f32, f32),
) -> f32 {
    if !should_render_segmented_cable(start, end) {
        return distance_to_cable_px(start, end, point);
    }
    let start = (start.0, -start.1);
    let end = (end.0, -end.1);
    let point = (point.0, -point.1);
    let segment_y = -segment_row;
    distance_to_segmented_path_y_up(point, start, end, segment_y, corner_radius)
}

pub fn segmented_horizontal_segment_hit(
    start: (f32, f32),
    end: (f32, f32),
    segment_row: f32,
    corner_radius: f32,
    hit_radius: f32,
    point: (f32, f32),
) -> bool {
    if !should_render_segmented_cable(start, end) {
        return false;
    }
    let going_right = end.0 > start.0;
    let start_x = if going_right {
        start.0 + corner_radius
    } else {
        start.0 - corner_radius
    };
    let end_x = if going_right {
        end.0 - corner_radius
    } else {
        end.0 + corner_radius
    };
    point.0 >= start_x.min(end_x)
        && point.0 <= start_x.max(end_x)
        && (point.1 - segment_row).abs() <= hit_radius
}

fn distance_to_segmented_path_y_up(
    point: (f32, f32),
    start: (f32, f32),
    end: (f32, f32),
    segment_y: f32,
    corner_radius: f32,
) -> f32 {
    if end.1 > segment_y {
        distance_to_five_segment_path_y_up(point, start, end, segment_y, corner_radius)
    } else {
        distance_to_three_segment_path_y_up(point, start, end, segment_y, corner_radius)
    }
}

fn distance_to_three_segment_path_y_up(
    point: (f32, f32),
    start: (f32, f32),
    end: (f32, f32),
    segment_y: f32,
    corner_radius: f32,
) -> f32 {
    let going_down1 = start.1 > segment_y;
    let going_right = end.0 > start.0;
    let going_down2 = end.1 < segment_y;
    let corner1 = (start.0, segment_y);
    let corner2 = (end.0, segment_y);
    let corner1_center = match (going_down1, going_right) {
        (true, true) => (start.0 + corner_radius, segment_y + corner_radius),
        (true, false) => (start.0 - corner_radius, segment_y + corner_radius),
        (false, true) => (start.0 + corner_radius, segment_y - corner_radius),
        (false, false) => (start.0 - corner_radius, segment_y - corner_radius),
    };
    let corner2_center = match (going_down2, going_right) {
        (true, true) => (end.0 - corner_radius, segment_y - corner_radius),
        (true, false) => (end.0 + corner_radius, segment_y - corner_radius),
        (false, true) => (end.0 - corner_radius, segment_y + corner_radius),
        (false, false) => (end.0 + corner_radius, segment_y + corner_radius),
    };
    let seg1_end = (
        start.0,
        if going_down1 {
            segment_y + corner_radius
        } else {
            segment_y - corner_radius
        },
    );
    let seg3_start = (
        end.0,
        if going_down2 {
            segment_y - corner_radius
        } else {
            segment_y + corner_radius
        },
    );
    let seg2_start = (
        if going_right {
            start.0 + corner_radius
        } else {
            start.0 - corner_radius
        },
        segment_y,
    );
    let seg2_end = (
        if going_right {
            end.0 - corner_radius
        } else {
            end.0 + corner_radius
        },
        segment_y,
    );
    distance_to_segment(point, start, seg1_end)
        .min(distance_to_segment(point, seg2_start, seg2_end))
        .min(distance_to_segment(point, seg3_start, end))
        .min(distance_to_quarter_arc(
            point,
            corner1_center,
            corner_radius,
            corner1,
        ))
        .min(distance_to_quarter_arc(
            point,
            corner2_center,
            corner_radius,
            corner2,
        ))
}

fn distance_to_five_segment_path_y_up(
    point: (f32, f32),
    start: (f32, f32),
    end: (f32, f32),
    segment_y: f32,
    corner_radius: f32,
) -> f32 {
    let going_right = end.0 > start.0;
    let clearance = corner_radius * 2.0;
    let turnaround_y = end.1 + clearance;
    let turnaround_x = end.0 - clearance;
    let seg4_going_right = end.0 > turnaround_x;

    let corner1 = (start.0, segment_y);
    let corner1_center = if going_right {
        (start.0 + corner_radius, segment_y + corner_radius)
    } else {
        (start.0 - corner_radius, segment_y + corner_radius)
    };
    let seg1_end = (start.0, segment_y + corner_radius);

    let corner2 = (turnaround_x, segment_y);
    let corner2_center = if going_right {
        (turnaround_x - corner_radius, segment_y + corner_radius)
    } else {
        (turnaround_x + corner_radius, segment_y + corner_radius)
    };
    let seg2_start = (
        if going_right {
            start.0 + corner_radius
        } else {
            start.0 - corner_radius
        },
        segment_y,
    );
    let seg2_end = (
        if going_right {
            turnaround_x - corner_radius
        } else {
            turnaround_x + corner_radius
        },
        segment_y,
    );

    let corner3 = (turnaround_x, turnaround_y);
    let corner3_center = if seg4_going_right {
        (turnaround_x + corner_radius, turnaround_y - corner_radius)
    } else {
        (turnaround_x - corner_radius, turnaround_y - corner_radius)
    };
    let seg3_start = (turnaround_x, segment_y + corner_radius);
    let seg3_end = (turnaround_x, turnaround_y - corner_radius);

    let corner4 = (end.0, turnaround_y);
    let corner4_center = if seg4_going_right {
        (end.0 - corner_radius, turnaround_y - corner_radius)
    } else {
        (end.0 + corner_radius, turnaround_y - corner_radius)
    };
    let seg4_start = (
        if seg4_going_right {
            turnaround_x + corner_radius
        } else {
            turnaround_x - corner_radius
        },
        turnaround_y,
    );
    let seg4_end = (
        if seg4_going_right {
            end.0 - corner_radius
        } else {
            end.0 + corner_radius
        },
        turnaround_y,
    );
    let seg5_start = (end.0, turnaround_y - corner_radius);

    distance_to_segment(point, start, seg1_end)
        .min(distance_to_segment(point, seg2_start, seg2_end))
        .min(distance_to_segment(point, seg3_start, seg3_end))
        .min(distance_to_segment(point, seg4_start, seg4_end))
        .min(distance_to_segment(point, seg5_start, end))
        .min(distance_to_quarter_arc(
            point,
            corner1_center,
            corner_radius,
            corner1,
        ))
        .min(distance_to_quarter_arc(
            point,
            corner2_center,
            corner_radius,
            corner2,
        ))
        .min(distance_to_quarter_arc(
            point,
            corner3_center,
            corner_radius,
            corner3,
        ))
        .min(distance_to_quarter_arc(
            point,
            corner4_center,
            corner_radius,
            corner4,
        ))
}

fn distance_to_quarter_arc(
    point: (f32, f32),
    center: (f32, f32),
    radius: f32,
    corner: (f32, f32),
) -> f32 {
    let to_corner = (corner.0 - center.0, corner.1 - center.1);
    let to_point = (point.0 - center.0, point.1 - center.1);
    let valid_x = if to_corner.0 > 0.0 {
        to_point.0 >= 0.0
    } else {
        to_point.0 <= 0.0
    };
    let valid_y = if to_corner.1 > 0.0 {
        to_point.1 >= 0.0
    } else {
        to_point.1 <= 0.0
    };
    if valid_x && valid_y {
        ((to_point.0 * to_point.0 + to_point.1 * to_point.1).sqrt() - radius).abs()
    } else {
        1000.0
    }
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
    fn cable_curve_uses_vertical_endpoint_tangents_for_horizontal_offsets() {
        let curve = cable_curve((10.0, 20.0), (100.0, 40.0));
        assert_eq!(curve.p1.0, curve.p0.0);
        assert_eq!(curve.p2.0, curve.p3.0);
        assert!(curve.p1.1 > curve.p0.1, "{curve:?}");
        assert!(curve.p2.1 < curve.p3.1, "{curve:?}");
    }

    #[test]
    fn cable_curve_scales_vertical_handles_with_span() {
        let short = cable_curve((10.0, 20.0), (18.0, 30.0));
        let long = cable_curve((10.0, 20.0), (18.0, 120.0));
        let short_handle = (short.p1.1 - short.p0.1).abs();
        let long_handle = (long.p1.1 - long.p0.1).abs();
        assert!(short_handle >= MIN_Y_HANDLE, "{short:?}");
        assert!(long_handle > short_handle, "{short:?} {long:?}");
        assert!(long_handle <= MAX_Y_HANDLE, "{long:?}");
    }

    #[test]
    fn cable_curve_keeps_mostly_horizontal_cables_tight() {
        let shallow = cable_curve((10.0, 20.0), (130.0, 24.0));
        let diagonal = cable_curve((10.0, 20.0), (130.0, 70.0));
        let shallow_handle = (shallow.p1.1 - shallow.p0.1).abs();
        let diagonal_handle = (diagonal.p1.1 - diagonal.p0.1).abs();
        assert!(shallow_handle < 2.0, "{shallow:?}");
        assert!(diagonal_handle > shallow_handle, "{shallow:?} {diagonal:?}");
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
    fn cable_curve_keeps_port_oriented_tangents_when_destination_is_above_source() {
        let curve = cable_curve((10.0, 60.0), (10.0, 20.0));
        assert_eq!(curve.p1.0, curve.p0.0);
        assert_eq!(curve.p2.0, curve.p3.0);
        assert!(curve.p1.1 > curve.p0.1, "{curve:?}");
        assert!(curve.p2.1 < curve.p3.1, "{curve:?}");
    }

    #[test]
    fn cable_curve_treats_nearly_aligned_ports_as_vertical() {
        let curve = cable_curve((10.0, 20.0), (10.12, 60.0));
        assert_eq!(curve.p1.0, curve.p0.0);
        assert_eq!(curve.p2.0, curve.p3.0);
    }

    #[test]
    fn segmented_cable_collapses_to_vertical_curve_when_ports_align() {
        let start = (10.0, 20.0);
        let end = (10.0, 60.0);
        let point = (10.0, 40.0);
        assert!(!should_render_segmented_cable(start, end));
        assert_eq!(
            segmented_cable_edit_points(start, end, 3.0),
            cable_edit_points(start, end, 3.0)
        );
        assert_eq!(
            distance_to_segmented_cable_px(start, end, 34.0, 0.7, point),
            distance_to_cable_px(start, end, point)
        );
        assert!(!segmented_horizontal_segment_hit(
            start, end, 34.0, 0.7, 0.4, point
        ));
    }

    #[test]
    fn distance_to_cable_is_small_near_endpoint() {
        let distance = distance_to_cable_px((10.0, 20.0), (100.0, 40.0), (10.5, 20.5));
        assert!(distance < 1.0, "{distance}");
    }

    #[test]
    fn cable_edit_points_are_inside_curve_endpoints() {
        let (start_handle, end_handle) = cable_edit_points((10.0, 20.0), (100.0, 40.0), 3.0);
        assert!(start_handle.0 > 10.0, "{start_handle:?}");
        assert!(start_handle.0 < 100.0, "{start_handle:?}");
        assert!(end_handle.0 > 10.0, "{end_handle:?}");
        assert!(end_handle.0 < 100.0, "{end_handle:?}");
        assert!(point_distance((10.0, 20.0), start_handle) <= 3.1);
        assert!(point_distance((100.0, 40.0), end_handle) <= 3.1);
    }
}
