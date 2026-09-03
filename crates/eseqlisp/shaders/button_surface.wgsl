// WGSL counterpart of button_surface.metal. Both are hot-reload overrides for
// the editable button surface and are concatenated onto their backend's widget
// shader preamble, so `WidgetVaryings` comes from there.

fn button_surface_rounded_rect(p: vec2<f32>, size: vec2<f32>, radius: f32) -> f32 {
    let q = abs(p) - (size - vec2<f32>(radius));
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

fn button_surface_tab(p: vec2<f32>, size: vec2<f32>, radius: f32) -> f32 {
    let top_splay = min(size.x * 0.20, 0.22);
    let top_half_width = max(size.x - top_splay, 0.001);
    let t = smoothstep(-size.y + radius * 1.40, size.y, p.y);
    let half_width = mix(top_half_width, size.x, t);
    let q = vec2<f32>(abs(p.x) - half_width, abs(p.y) - size.y);
    var d = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0);

    let top_left = p - vec2<f32>(-top_half_width + radius, -size.y + radius);
    let top_right = p - vec2<f32>(top_half_width - radius, -size.y + radius);
    let top_left_d = length(top_left) - radius;
    let top_right_d = length(top_right) - radius;
    d = select(d, top_left_d, p.x < -top_half_width + radius && p.y < -size.y + radius);
    d = select(d, top_right_d, p.x > top_half_width - radius && p.y < -size.y + radius);
    return d;
}

fn button_surface_distance(p: vec2<f32>, size: vec2<f32>, radius: f32, shape: f32) -> f32 {
    if (shape > 0.5) {
        return button_surface_tab(p, size, radius);
    }
    return button_surface_rounded_rect(p, size, radius);
}

fn button_surface_smooth(p: vec2<f32>, size: vec2<f32>, radius: f32, edge_min: f32, edge_max: f32) -> f32 {
    return smoothstep(edge_min, edge_max, button_surface_distance(p, size, radius, 0.0));
}

fn button_surface_normal(p: vec2<f32>, size: vec2<f32>, radius: f32, shape: f32, eps: f32) -> vec3<f32> {
    let right = smoothstep(-0.10, 0.92, button_surface_distance(p + vec2<f32>(eps, 0.0), size, radius, shape));
    let left = smoothstep(-0.10, 0.92, button_surface_distance(p - vec2<f32>(eps, 0.0), size, radius, shape));
    let down = smoothstep(-0.10, 0.92, button_surface_distance(p + vec2<f32>(0.0, eps), size, radius, shape));
    let up = smoothstep(-0.10, 0.92, button_surface_distance(p - vec2<f32>(0.0, eps), size, radius, shape));
    return normalize(vec3<f32>((right - left) / (2.0 * eps), (down - up) / (2.0 * eps), 1.0));
}

@fragment
fn widget_frag(input: WidgetVaryings) -> @location(0) vec4<f32> {
    let aspect = max(input.aspect, 0.001);
    let p = vec2<f32>((input.uv.x - 0.5) * 2.0 * aspect, (input.uv.y - 0.5) * 2.0);
    let size = vec2<f32>(aspect, 1.0);
    let shape = input.uniform_a.x;

    var r = select(0.75, input.corner_radius, input.corner_radius > 0.0);
    r = min(r, min(aspect, 1.0));
    let d = button_surface_distance(p, size, r, shape);

    let edge = fwidth(d) * 1.2;
    let mask = smoothstep(edge, -edge, d);
    if (mask < 0.002) { discard; }

    let px = max(max(fwidth(p.x), fwidth(p.y)), 0.001);
    let border_width = 1.00 * px;
    let inner_size = max(size - vec2<f32>(border_width), vec2<f32>(0.001));
    let inner_d = button_surface_distance(p, inner_size, max(r - border_width, 0.0), shape);
    let inner_edge = fwidth(inner_d) * 1.2;
    let inner_mask = smoothstep(inner_edge, -inner_edge, inner_d);
    let border_mask = clamp(mask - inner_mask, 0.0, 1.0);

    let normal = button_surface_normal(p, size, r, shape, max(px * 1.5, 0.004));
    let view_dir = vec3<f32>(0.0, 0.0, 1.0);
    let key_light = normalize(vec3<f32>(-0.12, -0.32, 1.30));
    let bounce_light = normalize(vec3<f32>(0.82, 0.78, 1.10));
    let key_diffuse = max(0.0, dot(normal, key_light));
    let bounce_diffuse = max(0.0, dot(normal, bounce_light));
    let key_specular = pow(max(0.0, dot(normal, normalize(key_light + view_dir))), 56.0);
    let bounce_specular = pow(max(0.0, dot(normal, normalize(bounce_light + view_dir))), 42.0);
    let edge_fade = smoothstep(0.12, -0.04, d);

    var quadrant_shade = (key_diffuse - 0.34 * bounce_diffuse) * 0.16;
    quadrant_shade = quadrant_shade + (bounce_diffuse - 0.30 * key_diffuse) * 0.10;
    var fill_lit = input.color_a.rgb * (1.0 + quadrant_shade * 0.08);
    fill_lit = fill_lit + input.color_c.rgb * input.color_c.a * key_specular * edge_fade * 0.10;
    fill_lit = mix(fill_lit, input.color_d.rgb, input.color_d.a * (1.0 - key_diffuse) * 0.05);

    var border_lit = input.color_b.rgb * (0.02 + 0.54 * key_diffuse + 0.26 * bounce_diffuse);
    border_lit = border_lit + input.color_c.rgb * input.color_c.a * (key_specular * 4.8 + bounce_specular * 2.25) * edge_fade;
    border_lit = mix(border_lit, input.color_d.rgb, input.color_d.a * (1.0 - max(key_diffuse, bounce_diffuse)) * 0.42);

    let fill = vec4<f32>(fill_lit, input.color_a.a * inner_mask);
    let border = vec4<f32>(border_lit, input.color_b.a * border_mask);
    let out_alpha = fill.a + border.a * (1.0 - fill.a);
    if (out_alpha <= 0.002) { discard; }
    let out_rgb = (fill.rgb * fill.a + border.rgb * border.a * (1.0 - fill.a)) / out_alpha;
    return vec4<f32>(out_rgb, out_alpha);
}
