float button_surface_rounded_rect(float2 p, float2 size, float radius)
{
    float2 q = abs(p) - (size - float2(radius));
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - radius;
}

float button_surface_tab(float2 p, float2 size, float radius)
{
    float top_splay = min(size.x * 0.20, 0.22);
    float top_half_width = max(size.x - top_splay, 0.001);
    float t = smoothstep(-size.y + radius * 1.40, size.y, p.y);
    float half_width = mix(top_half_width, size.x, t);
    float2 q = float2(abs(p.x) - half_width, abs(p.y) - size.y);
    float d = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0);

    float2 top_left = p - float2(-top_half_width + radius, -size.y + radius);
    float2 top_right = p - float2(top_half_width - radius, -size.y + radius);
    float top_left_d = length(top_left) - radius;
    float top_right_d = length(top_right) - radius;
    d = (p.x < -top_half_width + radius && p.y < -size.y + radius) ? top_left_d : d;
    d = (p.x > top_half_width - radius && p.y < -size.y + radius) ? top_right_d : d;
    return d;
}

float button_surface_distance(float2 p, float2 size, float radius, float shape)
{
    if (shape > 0.5) {
        return button_surface_tab(p, size, radius);
    }
    return button_surface_rounded_rect(p, size, radius);
}

float button_surface_smooth(float2 p, float2 size, float radius, float edge_min, float edge_max)
{
    return smoothstep(edge_min, edge_max, button_surface_distance(p, size, radius, 0.0));
}

float3 button_surface_normal(float2 p, float2 size, float radius, float shape, float eps)
{
    float right = smoothstep(-0.10, 0.92, button_surface_distance(p + float2(eps, 0.0), size, radius, shape));
    float left = smoothstep(-0.10, 0.92, button_surface_distance(p - float2(eps, 0.0), size, radius, shape));
    float down = smoothstep(-0.10, 0.92, button_surface_distance(p + float2(0.0, eps), size, radius, shape));
    float up = smoothstep(-0.10, 0.92, button_surface_distance(p - float2(0.0, eps), size, radius, shape));
    return normalize(float3((right - left) / (2.0 * eps), (down - up) / (2.0 * eps), 1.0));
}

fragment float4 widget_frag(WidgetVaryings in [[stage_in]])
{
    float aspect = max(in.aspect, 0.001);
    float2 p = float2((in.uv.x - 0.5) * 2.0 * aspect, (in.uv.y - 0.5) * 2.0);
    float2 size = float2(aspect, 1.0);
    float shape = in.uniform_a.x;

    float r = in.corner_radius > 0.0 ? in.corner_radius : 0.75;
    r = min(r, min(aspect, 1.0));
    float d = button_surface_distance(p, size, r, shape);

    float edge = fwidth(d) * 1.2;
    float mask = smoothstep(edge, -edge, d);
    if (mask < 0.002) { discard_fragment(); }

    float px = max(max(fwidth(p.x), fwidth(p.y)), 0.001);
    float border_width = 1.00 * px;
    float2 inner_size = max(size - float2(border_width), float2(0.001));
    float inner_d = button_surface_distance(p, inner_size, max(r - border_width, 0.0), shape);
    float inner_edge = fwidth(inner_d) * 1.2;
    float inner_mask = smoothstep(inner_edge, -inner_edge, inner_d);
    float border_mask = clamp(mask - inner_mask, 0.0, 1.0);

    float3 normal = button_surface_normal(p, size, r, shape, max(px * 1.5, 0.004));
    float3 view_dir = float3(0.0, 0.0, 1.0);
    float3 key_light = normalize(float3(-0.72, -0.92, 1.30));
    float3 bounce_light = normalize(float3(0.82, 0.78, 1.10));
    float key_diffuse = max(0.0, dot(normal, key_light));
    float bounce_diffuse = max(0.0, dot(normal, bounce_light));
    float key_specular = pow(max(0.0, dot(normal, normalize(key_light + view_dir))), 56.0);
    float bounce_specular = pow(max(0.0, dot(normal, normalize(bounce_light + view_dir))), 42.0);
    float edge_fade = smoothstep(0.12, -0.04, d);

    float quadrant_shade = (key_diffuse - 0.34 * bounce_diffuse) * 0.16;
    quadrant_shade += (bounce_diffuse - 0.30 * key_diffuse) * 0.10;
    float3 fill_lit = in.color_a.rgb * (1.0 + quadrant_shade * 0.08);
    fill_lit += in.color_c.rgb * in.color_c.a * key_specular * edge_fade * 0.10;
    fill_lit = mix(fill_lit, in.color_d.rgb, in.color_d.a * (1.0 - key_diffuse) * 0.05);

    float3 border_lit = in.color_b.rgb * (0.02 + 0.54 * key_diffuse + 0.26 * bounce_diffuse);
    border_lit += in.color_c.rgb * in.color_c.a * (key_specular * 4.8 + bounce_specular * 2.25) * edge_fade;
    border_lit = mix(border_lit, in.color_d.rgb, in.color_d.a * (1.0 - max(key_diffuse, bounce_diffuse)) * 0.42);

    float4 fill = float4(fill_lit, in.color_a.a * inner_mask);
    float4 border = float4(border_lit, in.color_b.a * border_mask);
    float out_alpha = fill.a + border.a * (1.0 - fill.a);
    if (out_alpha <= 0.002) { discard_fragment(); }
    float3 out_rgb = (fill.rgb * fill.a + border.rgb * border.a * (1.0 - fill.a)) / out_alpha;
    return float4(out_rgb, out_alpha);
}
