//! WGSL ports of the retained-mode widget fragment shaders.
//!
//! Bodies are kept beside their MSL counterparts by name and assembled with
//! the shared widget preamble in `ui::wgsl_shaders`.

pub const ADSR_EDITOR_SHADER: &str = r#"
fn adsr_sdSegment(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    var pa: vec2<f32> = p - a;
    var ba: vec2<f32> = b - a;
    var h: f32 = clamp(dot(pa, ba) / max(dot(ba, ba), 1e-6), 0.0, 1.0);
    return length(pa - ba * h);
}

fn adsr_logWeight(ms: f32) -> f32 {
    return max(0.0, log(1.0 + (ms / 20.0)));
}

fn adsr_toPlot(data: vec2<f32>) -> vec2<f32> {
    var pad: vec2<f32> = vec2<f32>(0.055, 0.12);
    const envelopeYInset: f32 = 0.08;
    var envelopeY: f32 = mix(envelopeYInset, 1.0 - envelopeYInset, data.y);
    return vec2<f32>(
        pad.x + data.x * (1.0 - pad.x * 2.0),
        pad.y + (1.0 - envelopeY) * (1.0 - pad.y * 2.0));
}

fn adsr_point(idx: i32, x1: f32, x2: f32, x3: f32, x4: f32, sustain: f32) -> vec2<f32> {
    const attackOrigin: f32 = 0.03;
    if (idx == 0) { return adsr_toPlot(vec2<f32>(attackOrigin, 0.0)); }
    if (idx == 1) { return adsr_toPlot(vec2<f32>(x1, 1.0)); }
    if (idx == 2) { return adsr_toPlot(vec2<f32>(x2, sustain)); }
    if (idx == 3) { return adsr_toPlot(vec2<f32>(x3, sustain)); }
    return adsr_toPlot(vec2<f32>(x4, 0.0));
}

fn adsr_expFall(t: f32, start: f32, end: f32) -> f32 {
    const k: f32 = 5.0;
    var normalized: f32 = (exp(-k * clamp(t, 0.0, 1.0)) - exp(-k)) / (1.0 - exp(-k));
    return end + (start - end) * normalized;
}

fn adsr_curveY(x: f32, x1: f32, x2: f32, x3: f32, x4: f32, sustain: f32) -> f32 {
    const attackOrigin: f32 = 0.03;
    if (x < attackOrigin) { return 0.0; }
    if (x <= x1) {
        if (x1 <= attackOrigin + 1e-5) { return 1.0; }
        return clamp((x - attackOrigin) / max(x1 - attackOrigin, 1e-5), 0.0, 1.0);
    }
    if (x <= x2) { return adsr_expFall((x - x1) / max(x2 - x1, 1e-5), 1.0, sustain); }
    if (x <= x3) { return sustain; }
    if (x <= x4) { return adsr_expFall((x - x3) / max(x4 - x3, 1e-5), sustain, 0.0); }
    return 0.0;
}

fn adsr_bracketDistance(p: vec2<f32>, corner: vec2<f32>, inward: vec2<f32>, lengthPx: vec2<f32>) -> f32 {
    var horizontal: f32 = adsr_sdSegment(p, corner, corner + vec2<f32>(inward.x * lengthPx.x, 0.0));
    var vertical: f32 = adsr_sdSegment(p, corner, corner + vec2<f32>(0.0, inward.y * lengthPx.y));
    return min(horizontal, vertical);
}

fn adsr_decayDisplay(input: WidgetVaryings) -> vec4<f32> {
    var uv: vec2<f32> = input.uv;
    var perPixel: vec2<f32> = max(vec2<f32>(fwidth(uv.x), fwidth(uv.y)), vec2<f32>(1e-6));
    var p: vec2<f32> = uv / perPixel;
    var initial: f32 = input.uniform_d.y;
    var zero: f32 = input.uniform_d.w;
    var end: f32 = mix(0.03, 1.0, input.uniform_d.z);
    var col: vec4<f32> = input.color_b;
    var baseline: f32 = abs(p.y - adsr_toPlot(vec2<f32>(0.0, zero)).y / perPixel.y);
    col = vec4<f32>(mix(col.rgb, input.color_c.rgb, (1.0 - smoothstep(0.5, 1.5, baseline)) * input.color_c.a * 0.4), col.a);
    var distance: f32 = 10000.0;
    var previous: vec2<f32> = adsr_toPlot(vec2<f32>(0.03, initial)) / perPixel;
    for (var i: i32 = 1; i <= 64; i = i + 1) {
        var t: f32 = f32(i) / 64.0;
        var current: vec2<f32> = adsr_toPlot(vec2<f32>(mix(0.03, end, t), adsr_expFall(t, initial, zero))) / perPixel;
        distance = min(distance, adsr_sdSegment(p, previous, current));
        previous = current;
    }
    distance = min(distance, adsr_sdSegment(p, previous, adsr_toPlot(vec2<f32>(1.0, zero)) / perPixel));
    col = vec4<f32>(mix(col.rgb, input.color_a.rgb, (1.0 - smoothstep(0.65, 1.55, distance)) * input.color_a.a), col.a);
    var scale: f32 = max(input.uniform_b.z, 0.001);
    for (var i: i32 = 1; i <= 2; i = i + 1) {
        var h: vec2<f32> = adsr_toPlot(select(vec2<f32>(end, zero), vec2<f32>(0.03, initial), i == 1)) / perPixel;
        var highlighted: bool = abs(f32(i) - input.uniform_b.y) < 0.5;
        var halfSize: f32 = select(6.0, 7.2, highlighted) * scale;
        var d: f32 = max(abs(p.x - h.x), abs(p.y - h.y));
        var outer: f32 = 1.0 - smoothstep(halfSize, halfSize + 0.75, d);
        var inner: f32 = 1.0 - smoothstep(halfSize - 1.5 * scale, halfSize - 1.5 * scale + 0.75, d);
        col = vec4<f32>(mix(col.rgb, input.color_d.rgb, select(max(outer - inner, 0.0), outer, highlighted) * input.color_d.a), col.a);
    }
    return col;
}

@fragment
fn widget_frag(input: WidgetVaryings) -> @location(0) vec4<f32>
{
    if (input.uniform_d.x > 0.5) { return adsr_decayDisplay(input); }
    var attack: f32 = input.uniform_a.x;
    var decay: f32 = input.uniform_a.y;
    var sustain: f32 = clamp(input.uniform_a.z, 0.0, 1.0);
    var release: f32 = input.uniform_a.w;
    var attackNorm: f32 = clamp(adsr_logWeight(attack) / adsr_logWeight(input.uniform_c.x), 0.0, 1.0);
    var decayNorm: f32 = clamp(adsr_logWeight(decay) / adsr_logWeight(input.uniform_c.y), 0.0, 1.0);
    var releaseNorm: f32 = clamp(adsr_logWeight(release) / adsr_logWeight(input.uniform_c.z), 0.0, 1.0);
    const releaseStart: f32 = 0.68;
    const attackOrigin: f32 = 0.03;
    var attackEnd: f32 = releaseStart * 0.42;
    var x1: f32 = mix(attackOrigin, attackEnd, attackNorm);
    var x2: f32 = x1 + (releaseStart - x1) * decayNorm;
    var x3: f32 = releaseStart;
    var x4: f32 = mix(x3, 1.0, releaseNorm);

    var uv: vec2<f32> = input.uv;
    var col: vec4<f32> = input.color_b;

    var pad: vec2<f32> = vec2<f32>(0.055, 0.12);
    var plotLeft: f32 = pad.x;
    var plotRight: f32 = 1.0 - pad.x;
    var plotTop: f32 = pad.y;
    var plotBottom: f32 = 1.0 - pad.y;
    var insidePlot: f32 = step(plotLeft, uv.x) * step(uv.x, plotRight)
        * step(plotTop, uv.y) * step(uv.y, plotBottom);

    var gridZero: f32 = adsr_toPlot(vec2<f32>(0.0, 0.0)).y;
    var baselineWidth: f32 = max(fwidth(uv.y), 0.001);
    var baseline: f32 = 1.0 - smoothstep(0.0, baselineWidth, abs(uv.y - gridZero));
    col = vec4<f32>((mix(col.rgb, input.color_c.rgb, baseline * input.color_c.a * 0.25 * insidePlot)), col.a);

    var uvPerPixel: vec2<f32> = max(vec2<f32>(fwidth(uv.x), fwidth(uv.y)), vec2<f32>(1e-6));
    var pxScale: f32 = select(1.0, input.uniform_b.z, input.uniform_b.z > 0.0);
    var pPx: vec2<f32> = uv / uvPerPixel;
    var plotMinPx: vec2<f32> = vec2<f32>(plotLeft, plotTop) / uvPerPixel;
    var plotMaxPx: vec2<f32> = vec2<f32>(plotRight, plotBottom) / uvPerPixel;
    var attackOriginPx: f32 = adsr_toPlot(vec2<f32>(attackOrigin, 0.0)).x / uvPerPixel.x;
    var leftBracketWidthPx: f32 = attackOriginPx - plotMinPx.x;
    var envelopeTopPx: f32 = adsr_toPlot(vec2<f32>(attackOrigin, 1.0)).y / uvPerPixel.y;
    var envelopeBottomPx: f32 = adsr_toPlot(vec2<f32>(attackOrigin, 0.0)).y / uvPerPixel.y;
    var topBracketHeightPx: f32 = envelopeTopPx - plotMinPx.y;
    var bottomBracketHeightPx: f32 = plotMaxPx.y - envelopeBottomPx;
    var bracketDist: f32 = 1000.0;
    bracketDist = min(bracketDist, adsr_bracketDistance(pPx, plotMinPx, vec2<f32>(1.0, 1.0), vec2<f32>(leftBracketWidthPx, topBracketHeightPx)));
    bracketDist = min(bracketDist, adsr_bracketDistance(pPx, vec2<f32>(plotMaxPx.x, plotMinPx.y), vec2<f32>(-1.0, 1.0), vec2<f32>(16.0 * pxScale, topBracketHeightPx)));
    bracketDist = min(bracketDist, adsr_bracketDistance(pPx, vec2<f32>(plotMinPx.x, plotMaxPx.y), vec2<f32>(1.0, -1.0), vec2<f32>(leftBracketWidthPx, bottomBracketHeightPx)));
    bracketDist = min(bracketDist, adsr_bracketDistance(pPx, plotMaxPx, vec2<f32>(-1.0, -1.0), vec2<f32>(16.0 * pxScale, bottomBracketHeightPx)));
    var brackets: f32 = 1.0 - smoothstep(0.5, 1.25, bracketDist);
    col = vec4<f32>((mix(col.rgb, input.color_a.rgb, brackets * input.color_a.a)), col.a);

    var dataX: f32 = clamp((uv.x - pad.x) / (1.0 - pad.x * 2.0), 0.0, 1.0);
    var curveDataY: f32 = adsr_curveY(dataX, x1, x2, x3, x4, sustain);
    var curvePlotY: f32 = adsr_toPlot(vec2<f32>(dataX, curveDataY)).y;
    var fillRegion: f32 = step(attackOrigin, dataX) * step(dataX, x4)
        * step(curvePlotY, uv.y) * step(uv.y, gridZero) * insidePlot;
    var fillGradient: f32 = clamp((gridZero - uv.y) / max(gridZero - curvePlotY, 1e-5), 0.0, 1.0);
    col = vec4<f32>((mix(col.rgb, input.color_a.rgb, fillRegion * fillGradient * input.color_a.a * 0.10)), col.a);

    var minDistPx: f32 = 1000.0;
    var previous: vec2<f32> = adsr_toPlot(vec2<f32>(attackOrigin, 0.0)) / uvPerPixel;
    const subdivisions: i32 = 8;
    const segmentCount: i32 = 4;
    for (var segment: i32 = 0; segment < segmentCount; segment = segment + 1) {
        var segmentStart: f32 = select(select(select(x3, x2, segment == 2), x1, segment == 1), attackOrigin, segment == 0);
        var segmentEnd: f32 = select(select(select(x4, x3, segment == 2), x2, segment == 1), x1, segment == 0);
        for (var stepIndex: i32 = 1; stepIndex <= subdivisions; stepIndex = stepIndex + 1) {
            var segmentT: f32 = f32(stepIndex) / f32(subdivisions);
            var sampleX: f32 = mix(segmentStart, segmentEnd, segmentT);
            var sampleY: f32 = select(
                adsr_curveY(sampleX, x1, x2, x3, x4, sustain),
                segmentT,
                segment == 0);
            var current: vec2<f32> = adsr_toPlot(vec2<f32>(sampleX, sampleY)) / uvPerPixel;
            minDistPx = min(minDistPx, adsr_sdSegment(pPx, previous, current));
            previous = current;
        }
    }
    var activeHandle: f32 = round(input.uniform_b.y);
    var curve: f32 = 1.0 - smoothstep(0.65, 1.55, minDistPx);
    var curveBrightness: f32 = select(1.0, 1.12, activeHandle > 0.5);
    col = vec4<f32>((mix(col.rgb, min(input.color_a.rgb * curveBrightness, vec3<f32>(1.0)), curve * input.color_a.a)), col.a);

    var pixelY: f32 = max(fwidth(uv.y), 0.001);
    for (var i: i32 = 1; i < 5; i = i + 1) {
        var h: vec2<f32> = adsr_point(i, x1, x2, x3, x4, sustain);
        var highlighted: bool = abs(f32(i) - activeHandle) < 0.5;
        var handleHalfPx: f32 = select(6.0, 7.2, highlighted) * pxScale;
        var handleStrokePx: f32 = 1.5 * pxScale;
        var pxDelta: vec2<f32> = vec2<f32>((uv.x - h.x) / uvPerPixel.x,
                                (uv.y - h.y) / pixelY);
        var d: vec2<f32> = abs(pxDelta);
        var outerQ: vec2<f32> = d - vec2<f32>(handleHalfPx);
        var outerDist: f32 = length(max(outerQ, vec2<f32>(0.0))) + min(max(outerQ.x, outerQ.y), 0.0);
        var innerQ: vec2<f32> = d - vec2<f32>(max(handleHalfPx - handleStrokePx, 0.0));
        var innerDist: f32 = length(max(innerQ, vec2<f32>(0.0))) + min(max(innerQ.x, innerQ.y), 0.0);
        var outer: f32 = 1.0 - smoothstep(0.0, 0.75, outerDist);
        var inner: f32 = 1.0 - smoothstep(0.0, 0.75, innerDist);
        var square: f32 = select(max(outer - inner, 0.0), outer, highlighted);
        col = vec4<f32>((mix(col.rgb, input.color_d.rgb, square * input.color_d.a)), col.a);
    }

    return col;
}"#;

pub const BUTTON_ICON_SHADER: &str = r#"
fn button_icon_box(p: vec2<f32>, b: vec2<f32>) -> f32 {
    var q: vec2<f32> = abs(p) - b;
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0);
}

fn button_icon_round_rect(p: vec2<f32>, b: vec2<f32>, r: f32) -> f32 {
    var q: vec2<f32> = abs(p) - (b - vec2<f32>(r));
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

fn button_icon_segment(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    var pa: vec2<f32> = p - a;
    var ba: vec2<f32> = b - a;
    var h: f32 = clamp(dot(pa, ba) / max(dot(ba, ba), 0.0001), 0.0, 1.0);
    return length(pa - ba * h);
}

@fragment
fn widget_frag(input: WidgetVaryings) -> @location(0) vec4<f32>
{
    var uv: vec2<f32> = input.uv;
    var aspect: f32 = input.aspect;
    var p: vec2<f32> = vec2<f32>((uv.x - 0.5) * 2.0 * aspect, (uv.y - 0.5) * 2.0);
    var col: vec4<f32> = input.color_a;
    // Glyphs are authored inside roughly +-0.62; stretch them to fill the
    // shorter side of the box so they read at Ableton-like size.
    p = p / (max(min(aspect, 1.0), 0.05) / 0.78);
    // uniform_a.x > 0.5 selects the filled "list" style: the folder becomes a
    // Finder-style filled silhouette in color_b, every other glyph sits in
    // white (color_a) on a rounded color_b tile. 0 keeps the stroke style.
    var style: f32 = input.uniform_a.x;
    var filled: bool = style > 0.5;
    // In the filled style `detail_d` carves darker cutouts (key gaps, the
    // folder fold, a dial's face) out of the solid silhouette in `d`.
    var detail_d: f32 = 1.0;
    // `shade_d` darkens (rather than cuts out) part of a filled silhouette:
    // the folder's back panel behind its lighter front panel.
    var shade_d: f32 = 1.0;

    var d: f32 = 1.0;
    var stroke: f32 = 0.072;
    if (input.value_t < 0.5) {
        // plus
        var bar_v: vec2<f32> = abs(p) - vec2<f32>(0.09, 0.38);
        var dv: f32 = length(max(bar_v, vec2<f32>(0.0))) + min(max(bar_v.x, bar_v.y), 0.0);
        var bar_h: vec2<f32> = abs(p) - vec2<f32>(0.38, 0.09);
        var dh: f32 = length(max(bar_h, vec2<f32>(0.0))) + min(max(bar_h.x, bar_h.y), 0.0);
        d = min(dv, dh);
    } else if (input.value_t < 1.5) {
        // sampler: compact Ableton-style device outline with heavier strokes
        p = p * 1.08;
        var body_q: vec2<f32> = abs(p) - vec2<f32>(0.50, 0.35);
        var body_base: f32 = length(max(body_q, vec2<f32>(0.0))) + min(max(body_q.x, body_q.y), 0.0) - 0.08;
        var body: f32 = abs(body_base) - 0.075;
        var screen_q: vec2<f32> = abs(p - vec2<f32>(-0.18, -0.17)) - vec2<f32>(0.17, 0.045);
        var screen_base: f32 = length(max(screen_q, vec2<f32>(0.0))) + min(max(screen_q.x, screen_q.y), 0.0) - 0.03;
        var screen: f32 = abs(screen_base) - 0.052;
        var pad: f32 = 1.0;
        var pad_fill: f32 = 1.0;
        for (var ix: i32 = 0; ix < 2; ix = ix + 1) {
            for (var iy: i32 = 0; iy < 2; iy = iy + 1) {
                var center: vec2<f32> = vec2<f32>(0.08 + f32(ix) * 0.20, -0.05 + f32(iy) * 0.18);
                var q: vec2<f32> = abs(p - center) - vec2<f32>(0.065, 0.055);
                var pd_base: f32 = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - 0.03;
                var pd: f32 = abs(pd_base) - 0.045;
                pad = min(pad, pd);
                pad_fill = min(pad_fill, pd_base);
            }
        }
        var knob: f32 = abs(length(p - vec2<f32>(-0.32, 0.16)) - 0.055) - 0.045;
        if (filled) {
            d = body_base;
            detail_d = min(screen_base, min(pad_fill, length(p - vec2<f32>(-0.32, 0.16)) - 0.10));
        } else {
            d = min(min(body, screen), min(pad, knob));
        }
    } else if (input.value_t < 2.5) {
        // waveform: vertical sample bars, clearer than a tiny sine curve.
        var b0: f32 = button_icon_segment(p, vec2<f32>(-0.45, -0.16), vec2<f32>(-0.45, 0.16)) - stroke;
        var b1: f32 = button_icon_segment(p, vec2<f32>(-0.25, -0.38), vec2<f32>(-0.25, 0.38)) - stroke;
        var b2: f32 = button_icon_segment(p, vec2<f32>(-0.05, -0.26), vec2<f32>(-0.05, 0.26)) - stroke;
        var b3: f32 = button_icon_segment(p, vec2<f32>(0.15, -0.46), vec2<f32>(0.15, 0.46)) - stroke;
        var b4: f32 = button_icon_segment(p, vec2<f32>(0.35, -0.30), vec2<f32>(0.35, 0.30)) - stroke;
        var b5: f32 = button_icon_segment(p, vec2<f32>(0.52, -0.14), vec2<f32>(0.52, 0.14)) - stroke;
        d = min(min(min(b0, b1), min(b2, b3)), min(b4, b5));
    } else if (input.value_t < 3.5) {
        // piano keys: outlined body, four white keys, solid black keys on the dividers.
        var body: f32 = abs(button_icon_round_rect(p, vec2<f32>(0.56, 0.36), 0.055)) - stroke;
        var div_a: f32 = button_icon_segment(p, vec2<f32>(-0.28, 0.00), vec2<f32>(-0.28, 0.36)) - 0.045;
        var div_b: f32 = button_icon_segment(p, vec2<f32>(0.00, 0.00), vec2<f32>(0.00, 0.36)) - 0.045;
        var div_c: f32 = button_icon_segment(p, vec2<f32>(0.28, 0.00), vec2<f32>(0.28, 0.36)) - 0.045;
        var black_a: f32 = button_icon_box(p - vec2<f32>(-0.28, -0.20), vec2<f32>(0.085, 0.17));
        var black_b: f32 = button_icon_box(p - vec2<f32>(0.00, -0.20), vec2<f32>(0.085, 0.17));
        var black_c: f32 = button_icon_box(p - vec2<f32>(0.28, -0.20), vec2<f32>(0.085, 0.17));
        if (filled) {
            d = button_icon_round_rect(p, vec2<f32>(0.56, 0.36), 0.055);
            detail_d = min(min(div_a, min(div_b, div_c)), min(black_a, min(black_b, black_c)));
        } else {
            d = min(min(body, min(div_a, min(div_b, div_c))), min(black_a, min(black_b, black_c)));
        }
    } else if (input.value_t < 4.5) {
        // sliders
        var a: f32 = button_icon_segment(p, vec2<f32>(-0.52, -0.30), vec2<f32>(0.52, -0.30)) - stroke;
        var b: f32 = button_icon_segment(p, vec2<f32>(-0.52, 0.00), vec2<f32>(0.52, 0.00)) - stroke;
        var c: f32 = button_icon_segment(p, vec2<f32>(-0.52, 0.30), vec2<f32>(0.52, 0.30)) - stroke;
        var ka: f32 = length(p - vec2<f32>(-0.24, -0.30)) - 0.12;
        var kb: f32 = length(p - vec2<f32>(0.20, 0.00)) - 0.12;
        var kc: f32 = length(p - vec2<f32>(-0.02, 0.30)) - 0.12;
        d = min(min(a, b), min(c, min(ka, min(kb, kc))));
    } else if (input.value_t < 5.5) {
        // note with a rightward arrow
        var head_p: vec2<f32> = (p - vec2<f32>(-0.30, 0.24)) * vec2<f32>(1.08, 0.92);
        var head: f32 = abs(length(head_p) - 0.16) - 0.052;
        var stem: f32 = button_icon_segment(p, vec2<f32>(-0.13, 0.21), vec2<f32>(-0.13, -0.42)) - stroke;
        var beam: f32 = button_icon_segment(p, vec2<f32>(-0.13, -0.40), vec2<f32>(0.15, -0.32)) - stroke;
        var shaft: f32 = button_icon_segment(p, vec2<f32>(0.00, 0.18), vec2<f32>(0.46, 0.18)) - stroke;
        var arrow_a: f32 = button_icon_segment(p, vec2<f32>(0.46, 0.18), vec2<f32>(0.27, 0.00)) - stroke;
        var arrow_b: f32 = button_icon_segment(p, vec2<f32>(0.46, 0.18), vec2<f32>(0.27, 0.36)) - stroke;
        d = min(min(head, stem), min(beam, min(shaft, min(arrow_a, arrow_b))));
    } else if (input.value_t < 6.5) {
        // bookmark
        var left: f32 = button_icon_segment(p, vec2<f32>(-0.34, -0.46), vec2<f32>(-0.34, 0.46)) - stroke;
        var right: f32 = button_icon_segment(p, vec2<f32>(0.34, -0.46), vec2<f32>(0.34, 0.46)) - stroke;
        var top: f32 = button_icon_segment(p, vec2<f32>(-0.34, -0.46), vec2<f32>(0.34, -0.46)) - stroke;
        var fold_a: f32 = button_icon_segment(p, vec2<f32>(-0.34, 0.46), vec2<f32>(0.00, 0.20)) - stroke;
        var fold_b: f32 = button_icon_segment(p, vec2<f32>(0.34, 0.46), vec2<f32>(0.00, 0.20)) - stroke;
        if (filled) {
            // Logic-style dial: solid rounded square, dark face, pointer notch.
            d = button_icon_round_rect(p, vec2<f32>(0.50, 0.46), 0.11);
            var face: f32 = length(p) - 0.30;
            var pointer: f32 = button_icon_segment(p, vec2<f32>(0.0, 0.0), vec2<f32>(0.0, -0.27)) - 0.05;
            detail_d = max(face, -pointer);
        } else {
            d = min(min(left, right), min(top, min(fold_a, fold_b)));
        }
    } else if (input.value_t < 7.5) {
        // folder: macOS Finder sidebar glyph, an outlined body-plus-tab
        // silhouette with the fold line across the full width.
        var f_body: f32 = button_icon_round_rect(p - vec2<f32>(0.00, 0.06), vec2<f32>(0.54, 0.36), 0.09);
        var f_tab: f32 = button_icon_round_rect(p - vec2<f32>(-0.32, -0.30), vec2<f32>(0.22, 0.12), 0.07);
        var silhouette: f32 = min(f_body, f_tab);
        var fold: f32 = button_icon_segment(p, vec2<f32>(-0.54, -0.18), vec2<f32>(0.54, -0.18)) - stroke;
        if (filled) {
            // Finder-style: darker back panel (with the tab) behind a lighter
            // front panel that covers the lower two thirds. No gaps.
            d = silhouette;
            var front: f32 = button_icon_round_rect(p - vec2<f32>(0.00, 0.13), vec2<f32>(0.54, 0.29), 0.08);
            shade_d = -front;
        } else {
            d = min(abs(silhouette) - stroke, fold);
        }
    } else if (input.value_t < 8.5) {
        // sine: one cycle sampled from a real sine at 20 short segments so
        // the peaks stay rounded at badge size.
        var s0: f32 = button_icon_segment(p, vec2<f32>(-0.540, 0.000), vec2<f32>(-0.486, 0.117)) - stroke;
        var s1: f32 = button_icon_segment(p, vec2<f32>(-0.486, 0.117), vec2<f32>(-0.432, 0.223)) - stroke;
        var s2: f32 = button_icon_segment(p, vec2<f32>(-0.432, 0.223), vec2<f32>(-0.378, 0.307)) - stroke;
        var s3: f32 = button_icon_segment(p, vec2<f32>(-0.378, 0.307), vec2<f32>(-0.324, 0.361)) - stroke;
        var s4: f32 = button_icon_segment(p, vec2<f32>(-0.324, 0.361), vec2<f32>(-0.270, 0.380)) - stroke;
        var s5: f32 = button_icon_segment(p, vec2<f32>(-0.270, 0.380), vec2<f32>(-0.216, 0.361)) - stroke;
        var s6: f32 = button_icon_segment(p, vec2<f32>(-0.216, 0.361), vec2<f32>(-0.162, 0.307)) - stroke;
        var s7: f32 = button_icon_segment(p, vec2<f32>(-0.162, 0.307), vec2<f32>(-0.108, 0.223)) - stroke;
        var s8: f32 = button_icon_segment(p, vec2<f32>(-0.108, 0.223), vec2<f32>(-0.054, 0.117)) - stroke;
        var s9: f32 = button_icon_segment(p, vec2<f32>(-0.054, 0.117), vec2<f32>(0.000, 0.000)) - stroke;
        var s10: f32 = button_icon_segment(p, vec2<f32>(0.000, 0.000), vec2<f32>(0.054, -0.117)) - stroke;
        var s11: f32 = button_icon_segment(p, vec2<f32>(0.054, -0.117), vec2<f32>(0.108, -0.223)) - stroke;
        var s12: f32 = button_icon_segment(p, vec2<f32>(0.108, -0.223), vec2<f32>(0.162, -0.307)) - stroke;
        var s13: f32 = button_icon_segment(p, vec2<f32>(0.162, -0.307), vec2<f32>(0.216, -0.361)) - stroke;
        var s14: f32 = button_icon_segment(p, vec2<f32>(0.216, -0.361), vec2<f32>(0.270, -0.380)) - stroke;
        var s15: f32 = button_icon_segment(p, vec2<f32>(0.270, -0.380), vec2<f32>(0.324, -0.361)) - stroke;
        var s16: f32 = button_icon_segment(p, vec2<f32>(0.324, -0.361), vec2<f32>(0.378, -0.307)) - stroke;
        var s17: f32 = button_icon_segment(p, vec2<f32>(0.378, -0.307), vec2<f32>(0.432, -0.223)) - stroke;
        var s18: f32 = button_icon_segment(p, vec2<f32>(0.432, -0.223), vec2<f32>(0.486, -0.117)) - stroke;
        var s19: f32 = button_icon_segment(p, vec2<f32>(0.486, -0.117), vec2<f32>(0.540, -0.000)) - stroke;
        d = s0;
        d = min(d, s1);
        d = min(d, s2);
        d = min(d, s3);
        d = min(d, s4);
        d = min(d, s5);
        d = min(d, s6);
        d = min(d, s7);
        d = min(d, s8);
        d = min(d, s9);
        d = min(d, s10);
        d = min(d, s11);
        d = min(d, s12);
        d = min(d, s13);
        d = min(d, s14);
        d = min(d, s15);
        d = min(d, s16);
        d = min(d, s17);
        d = min(d, s18);
        d = min(d, s19);
    } else if (input.value_t < 9.5) {
        // drop: a single water droplet. Pointed apex at the top, round
        // belly below; the cone between the apex and its tangent points on
        // the belly circle joins the two exactly (30 degree half-angle for
        // r = 0.34 at 0.68 from the apex). A short glint on the lower left.
        var dq: vec2<f32> = vec2<f32>(abs(p.x), p.y);
        let apex: vec2<f32> = vec2<f32>(0.0, -0.52);
        let belly: vec2<f32> = vec2<f32>(0.0, 0.16);
        let belly_r: f32 = 0.34;
        let cone_len: f32 = 0.589;
        let cone_dir: vec2<f32> = vec2<f32>(0.5, 0.866);
        let cone_n: vec2<f32> = vec2<f32>(0.866, -0.5);
        var da: vec2<f32> = dq - apex;
        var t: f32 = dot(da, cone_dir);
        var drop: f32 = length(dq - belly) - belly_r;
        if (t < 0.0) {
            drop = length(da);
        } else if (t < cone_len) {
            drop = dot(da, cone_n);
        }
        var glint: f32 = button_icon_segment(p, vec2<f32>(-0.17, 0.08), vec2<f32>(-0.09, 0.28)) - 0.045;
        if (filled) {
            d = drop;
            detail_d = glint;
        } else {
            d = min(abs(drop) - stroke, glint);
        }
    } else {
        // document: Finder-style page. Portrait sheet with a folded top-right
        // corner; the fold is a real triangle sitting inside the cut. Stroke
        // style matches Finder's sidebar "Documents" glyph (no text lines);
        // filled style adds two faint lines like Finder's file-list icon.
        var q: vec2<f32> = p - vec2<f32>(0.0, 0.0);
        let dw: f32 = 0.34;
        let dh: f32 = 0.46;
        let dk: f32 = 0.26;
        let dc: f32 = dw - dk;
        var page: f32 = button_icon_round_rect(q, vec2<f32>(dw, dh), 0.05);
        var corner_cut: f32 = (q.x - q.y - (dw + dh - dk)) / 1.41421;
        var sheet: f32 = max(page, corner_cut);
        var fold_a: f32 = button_icon_segment(q, vec2<f32>(dc, -dh), vec2<f32>(dc, -dh + dk)) - stroke;
        var fold_b: f32 = button_icon_segment(q, vec2<f32>(dc, -dh + dk), vec2<f32>(dw, -dh + dk)) - stroke;
        var line_a: f32 = button_icon_segment(q, vec2<f32>(-0.17, 0.04), vec2<f32>(0.17, 0.04)) - 0.03;
        var line_b: f32 = button_icon_segment(q, vec2<f32>(-0.17, 0.20), vec2<f32>(0.17, 0.20)) - 0.03;
        if (filled) {
            d = sheet;
            var fold_tri: f32 = max(max(dc - q.x, q.y - (-dh + dk)), q.x - q.y - (dw + dh - dk));
            shade_d = fold_tri;
            detail_d = min(line_a, line_b);
        } else {
            var outline: f32 = abs(sheet) - stroke;
            d = min(outline, min(fold_a, fold_b));
        }
    }

    var edge: f32 = max(fwidth(d), 0.001) * 1.2;
    var mask: f32 = smoothstep(edge, -edge, d);
    if (!filled) {
        if (mask < 0.002) { discard; }
        return vec4<f32>(col.rgb, col.a * mask);
    }
    // Filled style: solid silhouette in color_b with details cut out toward
    // color_a (the theme's detail tone, normally near the row background).
    var fill: vec4<f32> = input.color_b;
    if (mask < 0.002) { discard; }
    var detail_edge: f32 = max(fwidth(detail_d), 0.001) * 1.2;
    var detail_mask: f32 = smoothstep(detail_edge, -detail_edge, detail_d);
    var shade_edge: f32 = max(fwidth(shade_d), 0.001) * 1.2;
    var shade_mask: f32 = smoothstep(shade_edge, -shade_edge, shade_d);
    var rgb: vec3<f32> = mix(fill.rgb, fill.rgb * 0.68, shade_mask);
    rgb = mix(rgb, col.rgb, detail_mask * col.a);
    return vec4<f32>(rgb, fill.a * mask);
}"#;

pub const BUTTON_SURFACE_SHADER: &str = r#"
fn button_surface_rounded_rect(p: vec2<f32>, size: vec2<f32>, radius: f32) -> f32 {
    var q: vec2<f32> = abs(p) - (size - vec2<f32>(radius));
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

fn button_surface_tab(p: vec2<f32>, size: vec2<f32>, radius: f32) -> f32 {
    var top_splay: f32 = min(size.x * 0.20, 0.22);
    var top_half_width: f32 = max(size.x - top_splay, 0.001);
    var t: f32 = smoothstep(-size.y + radius * 1.40, size.y, p.y);
    var half_width: f32 = mix(top_half_width, size.x, t);
    var q: vec2<f32> = vec2<f32>(abs(p.x) - half_width, abs(p.y) - size.y);
    var d: f32 = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0);

    var top_left: vec2<f32> = p - vec2<f32>(-top_half_width + radius, -size.y + radius);
    var top_right: vec2<f32> = p - vec2<f32>(top_half_width - radius, -size.y + radius);
    var top_left_d: f32 = length(top_left) - radius;
    var top_right_d: f32 = length(top_right) - radius;
    d = select(d, top_left_d, (p.x < -top_half_width + radius && p.y < -size.y + radius));
    d = select(d, top_right_d, (p.x > top_half_width - radius && p.y < -size.y + radius));
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
    var right: f32 = smoothstep(-0.10, 0.92, button_surface_distance(p + vec2<f32>(eps, 0.0), size, radius, shape));
    var left: f32 = smoothstep(-0.10, 0.92, button_surface_distance(p - vec2<f32>(eps, 0.0), size, radius, shape));
    var down: f32 = smoothstep(-0.10, 0.92, button_surface_distance(p + vec2<f32>(0.0, eps), size, radius, shape));
    var up: f32 = smoothstep(-0.10, 0.92, button_surface_distance(p - vec2<f32>(0.0, eps), size, radius, shape));
    return normalize(vec3<f32>((right - left) / (2.0 * eps), (down - up) / (2.0 * eps), 1.0));
}

@fragment
fn widget_frag(input: WidgetVaryings) -> @location(0) vec4<f32>
{
    var aspect: f32 = max(input.aspect, 0.001);
    var p: vec2<f32> = vec2<f32>((input.uv.x - 0.5) * 2.0 * aspect, (input.uv.y - 0.5) * 2.0);
    var size: vec2<f32> = vec2<f32>(aspect, 1.0);
    var shape: f32 = input.uniform_a.x;

    var r: f32 = select(0.75, input.corner_radius, input.corner_radius > 0.0);
    r = min(r, min(aspect, 1.0));
    var d: f32 = button_surface_distance(p, size, r, shape);

    var edge: f32 = fwidth(d) * 1.2;
    var mask: f32 = smoothstep(edge, -edge, d);
    if (mask < 0.002) { discard; }

    var px: f32 = max(max(fwidth(p.x), fwidth(p.y)), 0.001);
    var border_width: f32 = 1.35 * px;
    var inner_size: vec2<f32> = max(size - vec2<f32>(border_width), vec2<f32>(0.001));
    var inner_d: f32 = button_surface_distance(p, inner_size, max(r - border_width, 0.0), shape);
    var inner_edge: f32 = fwidth(inner_d) * 1.2;
    var inner_mask: f32 = smoothstep(inner_edge, -inner_edge, inner_d);
    var border_mask: f32 = clamp(mask - inner_mask, 0.0, 1.0);

    var normal: vec3<f32> = button_surface_normal(p, size, r, shape, max(px * 1.5, 0.004));
    var view_dir: vec3<f32> = vec3<f32>(0.0, 0.0, 1.0);
    var key_light: vec3<f32> = normalize(vec3<f32>(-0.12, -0.32, 1.30));
    var bounce_light: vec3<f32> = normalize(vec3<f32>(0.82, 0.78, 1.10));
    var key_diffuse: f32 = max(0.0, dot(normal, key_light));
    var bounce_diffuse: f32 = max(0.0, dot(normal, bounce_light));
    var key_specular: f32 = pow(max(0.0, dot(normal, normalize(key_light + view_dir))), 56.0);
    var bounce_specular: f32 = pow(max(0.0, dot(normal, normalize(bounce_light + view_dir))), 42.0);
    var edge_fade: f32 = smoothstep(0.12, -0.04, d);

    var quadrant_shade: f32 = (key_diffuse - 0.34 * bounce_diffuse) * 0.16;
    quadrant_shade += (bounce_diffuse - 0.30 * key_diffuse) * 0.10;
    var fill_lit: vec3<f32> = input.color_a.rgb * (1.0 + quadrant_shade * 0.08);
    fill_lit += input.color_c.rgb * input.color_c.a * key_specular * edge_fade * 0.10;
    fill_lit = mix(fill_lit, input.color_d.rgb, input.color_d.a * (1.0 - key_diffuse) * 0.05);

    var border_lit: vec3<f32> = input.color_b.rgb * (0.72 + 0.34 * key_diffuse + 0.26 * bounce_diffuse);
    border_lit += input.color_c.rgb * input.color_c.a * (key_specular * 1.8 + bounce_specular * 1.25) * edge_fade;
    border_lit = mix(border_lit, input.color_d.rgb, input.color_d.a * (1.0 - max(key_diffuse, bounce_diffuse)) * 0.42);

    var fill: vec4<f32> = vec4<f32>(fill_lit, input.color_a.a * inner_mask);
    var border: vec4<f32> = vec4<f32>(border_lit, input.color_b.a * border_mask);
    var out_alpha: f32 = fill.a + border.a * (1.0 - fill.a);
    if (out_alpha <= 0.002) { discard; }
    var out_rgb: vec3<f32> = (fill.rgb * fill.a + border.rgb * border.a * (1.0 - fill.a)) / out_alpha;
    return vec4<f32>(out_rgb, out_alpha);
}"#;

pub const DROPDOWN_CHEVRON_SHADER: &str = r#"
@fragment
fn widget_frag(input: WidgetVaryings) -> @location(0) vec4<f32>
{
    var uv: vec2<f32> = input.uv;
    var aspect: f32 = input.aspect;
    var col: vec4<f32> = input.color_a;

    var p: vec2<f32> = vec2<f32>((uv.x - 0.5) * 2.0 * aspect, (uv.y - 0.5) * 2.0);

    // Compact up chevron "^"
    var hw: f32 = 0.35 * aspect;
    var up_pt: vec2<f32> = vec2<f32>(0.0, -0.70);
    var up_a: vec2<f32> = vec2<f32>(-hw, -0.22);
    var up_b: vec2<f32> = vec2<f32>( hw, -0.22);

    // Compact down chevron "v"
    var dn_pt: vec2<f32> = vec2<f32>(0.0,  0.70);
    var dn_a: vec2<f32> = vec2<f32>(-hw,  0.22);
    var dn_b: vec2<f32> = vec2<f32>( hw,  0.22);

    // SDF for line segments
    var pa1: vec2<f32> = p - up_a;
    var ba1: vec2<f32> = up_pt - up_a;
    var h1: f32 = clamp(dot(pa1, ba1) / dot(ba1, ba1), 0.0, 1.0);
    var seg1: f32 = length(pa1 - ba1 * h1);

    var pa2: vec2<f32> = p - up_pt;
    var ba2: vec2<f32> = up_b - up_pt;
    var h2: f32 = clamp(dot(pa2, ba2) / dot(ba2, ba2), 0.0, 1.0);
    var seg2: f32 = length(pa2 - ba2 * h2);

    var pa3: vec2<f32> = p - dn_a;
    var ba3: vec2<f32> = dn_pt - dn_a;
    var h3: f32 = clamp(dot(pa3, ba3) / dot(ba3, ba3), 0.0, 1.0);
    var seg3: f32 = length(pa3 - ba3 * h3);

    var pa4: vec2<f32> = p - dn_pt;
    var ba4: vec2<f32> = dn_b - dn_pt;
    var h4: f32 = clamp(dot(pa4, ba4) / dot(ba4, ba4), 0.0, 1.0);
    var seg4: f32 = length(pa4 - ba4 * h4);

    var d: f32 = min(min(seg1, seg2), min(seg3, seg4));

    var stroke: f32 = 0.10;
    var edge: f32 = fwidth(d) * 1.2;
    var mask: f32 = smoothstep(stroke + edge, stroke - edge, d);

    if (mask < 0.002) { discard; }
    return vec4<f32>(col.rgb, col.a * mask);
}"#;

pub const DROPDOWN_CHECKMARK_SHADER: &str = r#"
@fragment
fn widget_frag(input: WidgetVaryings) -> @location(0) vec4<f32>
{
    var aspect: f32 = input.aspect;
    var p: vec2<f32> = vec2<f32>((input.uv.x - 0.5) * 2.0 * aspect, (input.uv.y - 0.5) * 2.0);

    var start: vec2<f32> = vec2<f32>(-0.72 * aspect, -0.02);
    var joint: vec2<f32> = vec2<f32>(-0.25 * aspect,  0.48);
    var end: vec2<f32>   = vec2<f32>( 0.75 * aspect, -0.52);

    var pa1: vec2<f32> = p - start;
    var ba1: vec2<f32> = joint - start;
    var h1: f32 = clamp(dot(pa1, ba1) / dot(ba1, ba1), 0.0, 1.0);
    var seg1: f32 = length(pa1 - ba1 * h1);

    var pa2: vec2<f32> = p - joint;
    var ba2: vec2<f32> = end - joint;
    var h2: f32 = clamp(dot(pa2, ba2) / dot(ba2, ba2), 0.0, 1.0);
    var seg2: f32 = length(pa2 - ba2 * h2);

    var d: f32 = min(seg1, seg2);
    var stroke: f32 = 0.10;
    var edge: f32 = fwidth(d) * 1.2;
    var mask: f32 = smoothstep(stroke + edge, stroke - edge, d);

    if (mask < 0.002) { discard; }
    return vec4<f32>(input.color_a.rgb, input.color_a.a * mask);
}"#;

pub const HSLIDER_FRAGMENT_SHADER: &str = r#"
@fragment
fn widget_frag(input: WidgetVaryings) -> @location(0) vec4<f32>
{
    var uv: vec2<f32> = input.uv;
    var aspect: f32 = input.aspect;
    var t: f32 = input.value_t;

    // ── Fill bar: rounded rect from x=0..t, vertically inset ──
    var yPad: f32 = 0.18;
    var halfH: f32 = 0.5 - yPad;
    var halfW: f32 = max(t * 0.5, 0.0);
    var cr: f32 = 0.18;
    cr = min(cr, min(halfH, max(halfW * aspect, 0.001)));

    var p: vec2<f32> = vec2<f32>((uv.x - halfW) * aspect, uv.y - 0.5);
    var b: vec2<f32> = vec2<f32>(halfW * aspect, halfH);
    var q: vec2<f32> = abs(p) - b + cr;
    var d: f32 = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - cr;
    var aa: f32 = max(fwidth(d), 0.001);
    var fillMask: f32 = smoothstep(aa, -aa, d) * step(0.005, t);

    // ── Track dots: fixed grid, only visible past fill ──
    var dotSpacing: f32 = 0.6 / aspect;
    var dotR: f32 = 0.08;
    var dotMask: f32 = 0.0;

    var snapX: f32 = round(uv.x / dotSpacing) * dotSpacing;
    var margin: f32 = dotSpacing * 0.4;
    if (snapX > t + margin && snapX > margin && snapX < 1.0 - margin * 0.5) {
        var dp: vec2<f32> = vec2<f32>((uv.x - snapX) * aspect, uv.y - 0.5);
        var dd: f32 = length(dp) - dotR;
        var da: f32 = max(fwidth(dd), 0.001);
        dotMask = smoothstep(da, -da, dd);
    }

    // Composite
    var rgb: vec3<f32> = input.color_a.rgb * fillMask + input.color_b.rgb * dotMask * (1.0 - fillMask);
    var alpha: f32 = max(fillMask, dotMask);
    if (alpha < 0.001) { discard; }
    return vec4<f32>(rgb, alpha);
}"#;

pub const KNOB_FRAGMENT_SHADER: &str = r#"
@fragment
fn widget_frag(input: WidgetVaryings) -> @location(0) vec4<f32>
{
    var uv: vec2<f32> = input.uv;
    var aspect: f32 = input.aspect;

    var scale: vec2<f32> = select(
        vec2<f32>(1.0, 1.0 / max(aspect, 0.0001)),
        vec2<f32>(aspect, 1.0),
        aspect >= 1.0);
    var p: vec2<f32> = vec2<f32>((uv.x - 0.5) * 2.0, (uv.y - 0.5) * 2.0) * scale;
    var r: f32 = length(p);
    var a: f32 = atan2(p.y, p.x);

    var start: f32 = 1.57079633;
    var sweep: f32 = 4.71238898;
    var rel: f32 = ((a - start + 6.2831853) % 6.2831853);
    var inRange: f32 = step(rel, sweep);
    var is_active: f32 = step(rel, sweep * input.value_t);

    var aa: f32 = max(fwidth(r), 0.0015);
    var ring: f32 = abs(r - 0.74) - 0.11;
    var activeRing: f32 = abs(r - 0.74) - 0.125;
    var ringMask: f32 = smoothstep(aa, -aa, ring) * inRange;
    var activeMask: f32 = smoothstep(aa, -aa, activeRing) * inRange * is_active;
    var trackMask: f32 = ringMask * (1.0 - is_active);

    var notchAngle: f32 = start + sweep * input.value_t;
    var valueDir: vec2<f32> = vec2<f32>(cos(notchAngle), sin(notchAngle));
    var notchPos: vec2<f32> = valueDir * 0.74;
    var notch: f32 = length(p - notchPos) - 0.105;
    var notchMask: f32 = smoothstep(aa, -aa, notch);
    var lineAlong: f32 = dot(p, valueDir);
    var lineAcross: f32 = abs(p.x * valueDir.y - p.y * valueDir.x);
    var lineSegment: f32 = step(0.0, lineAlong) * step(lineAlong, 0.68);
    var line: f32 = lineAcross - 0.11;
    var lineMask: f32 = smoothstep(aa, -aa, line) * lineSegment;

    var col: vec4<f32> = vec4<f32>(0.0);
    col = mix(col, input.color_b, trackMask);
    col = mix(col, input.color_a, activeMask);
    col = mix(col, input.color_b, lineMask);
    col = mix(col, input.color_a, notchMask);
    if (col.a < 0.01) { discard; }
    return col;
}"#;

pub const MATRIX_FRAGMENT_SHADER: &str = r#"
@fragment
fn widget_frag(input: WidgetVaryings) -> @location(0) vec4<f32>
{
    var uv: vec2<f32> = input.uv;
    var aspect: f32 = input.aspect;
    var value: f32 = clamp(input.value_t, 0.0, 1.0);
    var control: f32 = input.uniform_a.x;
    var isClicked: f32 = input.uniform_a.y;
    var releaseTime: f32 = input.uniform_a.z;

    var p: vec2<f32> = vec2<f32>((uv.x - 0.5) * 2.0 * aspect, (0.5 - uv.y) * 2.0);
    var halfCell: vec2<f32> = vec2<f32>(aspect, 1.0);
    var cellDist: f32 = sdf_rounded_rect(p, halfCell, 0.0);
    var pix: f32 = max(max(fwidth(p.x), fwidth(p.y)), 0.001);
    var inCell: f32 = 1.0 - smoothstep(0.0, pix, cellDist);

    var scale: f32 = 1.0;
    if (isClicked > 0.5) {
        if (releaseTime <= 0.0) {
            scale = 1.05;
        } else {
            var elapsed: f32 = input.itime - releaseTime;
            if (elapsed < 0.2) {
                var t: f32 = clamp(elapsed / 0.2, 0.0, 1.0);
                var progress: f32 = 1.0 - pow(1.0 - t, 3.0);
                scale = mix(1.05, 1.0, progress);
            }
        }
    }

    var radius: f32 = min(aspect, 1.0) * 0.80 * scale;
    var ringThickness: f32 = radius * 0.12;
    var d: f32 = length(p);

    var ringMask: f32 = 0.0;
    var innerMask: f32 = 0.0;
    if (control > 2.5) {
        // Pie: wedge sweep = magnitude from the center value, starting at 12
        // o'clock; positive sweeps clockwise, negative counter-clockwise. Sign
        // arrives input uniform_a.w; zero leaves an empty ring so untouched cells
        // stay visibly neutral.
        var sweepDir: f32 = select(1.0, -1.0, input.uniform_a.w < 0.0);
        var sweep: f32 = value * 6.2831853;
        var ang: f32 = atan2(p.x, p.y) * sweepDir;
        if (ang < 0.0) {
            ang += 6.2831853;
        }
        var discMask: f32 = smoothstep(pix, 0.0, d - radius) * inCell;
        var outlineDist: f32 = abs(d - radius) - ringThickness * 0.5;
        var outlineMask: f32 = smoothstep(pix, 0.0, outlineDist) * inCell;
        // Angular antialias width grows toward the center (arc length per pixel).
        var angAA: f32 = pix / max(d, 0.05);
        var wedgeAngMask: f32 = select(smoothstep(0.0, angAA, sweep - ang), 1.0, value >= 0.999);
        var wedgeMask: f32 = discMask * wedgeAngMask * step(0.0005, value);

        // Composite bg -> neutral outline ring -> signed wedge -> stroke ring.
        var acc: vec4<f32> = input.color_b;
        var outlineA: f32 = outlineMask * input.color_c.a * 0.6;
        acc = vec4<f32>((mix(acc.rgb, input.color_c.rgb, outlineA)), acc.a);
        acc.a = outlineA + acc.a * (1.0 - outlineA);
        var fillA: f32 = wedgeMask * input.color_a.a;
        acc = vec4<f32>((mix(acc.rgb, input.color_a.rgb, fillA)), acc.a);
        acc.a = fillA + acc.a * (1.0 - fillA);
        var strokeHalfP: f32 = input.uniform_b.x * 0.5;
        if (input.uniform_b.y > 0.5 && strokeHalfP > 0.0) {
            var strokeMaskP: f32 = smoothstep(pix, 0.0, abs(d - radius) - strokeHalfP) * inCell;
            var strokeA: f32 = strokeMaskP * input.color_d.a;
            acc = vec4<f32>((mix(acc.rgb, input.color_d.rgb, strokeA)), acc.a);
            acc.a = strokeA + acc.a * (1.0 - strokeA);
        }
        return vec4<f32>(acc.rgb, acc.a * inCell);
    } else if (control > 1.5) {
        var fillAlpha: f32 = clamp(input.color_a.a * value, 0.0, 1.0);
        var bgAlpha: f32 = clamp(input.color_b.a, 0.0, 1.0);
        var outAlpha: f32 = fillAlpha + bgAlpha * (1.0 - fillAlpha);
        if (outAlpha <= 0.0) {
            return vec4<f32>(0.0);
        }
        var outColor: vec3<f32> = (
            input.color_a.rgb * fillAlpha +
            input.color_b.rgb * bgAlpha * (1.0 - fillAlpha)
        ) / outAlpha;
        return vec4<f32>(outColor, outAlpha);
    } else if (control < 0.5) {
        var discMask: f32 = smoothstep(pix, 0.0, d - radius) * inCell;
        var innerRadius: f32 = radius * value;
        var fillMask: f32 = smoothstep(-pix, 0.0, innerRadius - d) * inCell;
        var strokeMask: f32 = 0.0;
        var strokeHalf: f32 = input.uniform_b.x * 0.5;
        if (input.uniform_b.y > 0.5 && strokeHalf > 0.0) {
            var strokeDist: f32 = abs(d - radius) - strokeHalf;
            strokeMask = smoothstep(pix, 0.0, strokeDist) * inCell;
        }

        // Composite bg -> empty disc -> value fill -> stroke ring so each
        // layer's own alpha survives a transparent background.
        var acc: vec4<f32> = input.color_b;
        var discA: f32 = discMask * input.color_c.a;
        acc = vec4<f32>((mix(acc.rgb, input.color_c.rgb, discA)), acc.a);
        acc.a = discA + acc.a * (1.0 - discA);
        var fillA: f32 = fillMask * input.color_a.a;
        acc = vec4<f32>((mix(acc.rgb, input.color_a.rgb, fillA)), acc.a);
        acc.a = fillA + acc.a * (1.0 - fillA);
        var strokeA: f32 = strokeMask * input.color_d.a;
        acc = vec4<f32>((mix(acc.rgb, input.color_d.rgb, strokeA)), acc.a);
        acc.a = strokeA + acc.a * (1.0 - strokeA);
        return vec4<f32>(acc.rgb, acc.a * inCell);
    } else {
        var squareHalfSize: f32 = radius;
        var squareDist: f32 = sdf_rounded_rect(p, vec2<f32>(squareHalfSize), squareHalfSize * 0.15);
        var outlineDist: f32 = abs(squareDist) - ringThickness * 0.5;
        ringMask = smoothstep(pix, 0.0, outlineDist) * inCell;

        var lineThickness: f32 = squareHalfSize * 0.15;
        var lineWidth: f32 = squareHalfSize * 1.6;
        var linePosY: f32 = mix(-squareHalfSize * 0.8, squareHalfSize * 0.8, value);
        var yDist: f32 = abs(p.y - linePosY) - lineThickness * 0.5;
        var xDist: f32 = abs(p.x) - lineWidth * 0.5;
        var lineDist: f32 = max(xDist, yDist);
        innerMask = smoothstep(-pix, 0.0, -lineDist) * inCell;
    }

    var color: vec3<f32> = input.color_b.rgb;
    var borderColor: vec3<f32> = input.color_c.rgb;
    color = mix(color, borderColor, ringMask);
    color = mix(color, input.color_a.rgb, innerMask);
    return vec4<f32>(color, input.color_b.a * inCell);
}"#;

pub const MODULATOR_CURVE_SHADER: &str = r#"
fn mc_sdSegment(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    var pa: vec2<f32> = p - a;
    var ba: vec2<f32> = b - a;
    var h: f32 = clamp(dot(pa, ba) / max(dot(ba, ba), 1e-6), 0.0, 1.0);
    return length(pa - ba * h);
}

fn mc_plot(data: vec2<f32>) -> vec2<f32> {
    var pad: vec2<f32> = vec2<f32>(0.055, 0.12);
    return vec2<f32>(
        pad.x + data.x * (1.0 - pad.x * 2.0),
        pad.y + (1.0 - data.y) * (1.0 - pad.y * 2.0));
}

fn mc_curve_y(x: f32, riseT: f32, fallT: f32, level: f32) -> f32 {
    var startX: f32 = 0.18;
    var endX: f32 = 0.82;
    var riseW: f32 = mix(0.004, 0.26, riseT);
    var fallW: f32 = mix(0.004, 0.26, fallT);

    if (x < startX) {
        return 0.0;
    }
    var raw: f32 = 0.0;
    if (x < startX + riseW) {
        var t: f32 = (x - startX) / max(riseW, 0.0001);
        raw = 1.0 - exp(-t * mix(18.0, 4.2, riseT));
    } else if (x < endX) {
        raw = 1.0;
    } else if (x < endX + fallW) {
        var t: f32 = (x - endX) / max(fallW, 0.0001);
        raw = exp(-t * mix(18.0, 4.2, fallT));
    } else {
        raw = 0.0;
    }
    return raw * clamp(level, 0.0, 1.0);
}

@fragment
fn widget_frag(input: WidgetVaryings) -> @location(0) vec4<f32>
{
    var uv: vec2<f32> = input.uv;
    var aspect: f32 = max(input.aspect, 0.0001);
    var riseT: f32 = clamp(input.uniform_a.x, 0.0, 1.0);
    var fallT: f32 = clamp(input.uniform_a.y, 0.0, 1.0);
    var markerPhase: f32 = input.uniform_b.x;
    var level: f32 = clamp(input.uniform_b.y, 0.0, 1.0);

    var col: vec4<f32> = input.color_b;

    var dataX: f32 = clamp((uv.x - 0.055) / 0.89, 0.0, 1.0);
    var dataY: f32 = clamp((0.88 - uv.y) / 0.76, 0.0, 1.0);
    var curveAtX: f32 = mc_curve_y(dataX, riseT, fallT, level);
    var fill: f32 = smoothstep(curveAtX + 0.008, curveAtX - 0.008, dataY);
    col = vec4<f32>((mix(col.rgb, input.color_d.rgb, fill * input.color_d.a)), col.a);

    var lineMask: f32 = 0.0;
    const steps: i32 = 112;
    var prevX: f32 = 0.0;
    var prevY: f32 = mc_curve_y(0.0, riseT, fallT, level);
    for (var i: i32 = 1; i <= steps; i = i + 1) {
        var x: f32 = f32(i) / f32(steps);
        var y: f32 = mc_curve_y(x, riseT, fallT, level);
        var a: vec2<f32> = mc_plot(vec2<f32>(prevX, prevY));
        var b: vec2<f32> = mc_plot(vec2<f32>(x, y));
        var d: f32 = mc_sdSegment(vec2<f32>(uv.x * aspect, uv.y), vec2<f32>(a.x * aspect, a.y), vec2<f32>(b.x * aspect, b.y));
        var aa: f32 = max(fwidth(d), 0.001);
        lineMask = max(lineMask, smoothstep(0.008 + aa, 0.0025, d));
        prevX = x;
        prevY = y;
    }

    col = vec4<f32>((mix(col.rgb, input.color_a.rgb, lineMask * input.color_a.a)), col.a);
    col.a = max(col.a, max(lineMask * input.color_a.a, input.color_b.a));

    if (markerPhase >= 0.0) {
        var markerX: f32 = clamp(markerPhase, 0.0, 1.0);
        var marker: vec2<f32> = mc_plot(vec2<f32>(markerX, mc_curve_y(markerX, riseT, fallT, level)));
        var markerDist: f32 = length(vec2<f32>((uv.x - marker.x) * aspect, uv.y - marker.y));
        var outer: f32 = smoothstep(0.052, 0.038, markerDist);
        var inner: f32 = smoothstep(0.032, 0.020, markerDist);
        col = vec4<f32>((mix(col.rgb, vec3<f32>(0.02, 0.025, 0.03), outer)), col.a);
        col = vec4<f32>((mix(col.rgb, input.color_a.rgb, inner)), col.a);
        col.a = max(col.a, outer);
    }
    return col;
}"#;

pub const LFO_CURVE_SHADER: &str = r#"
fn lc_sdSegment(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    var pa: vec2<f32> = p - a;
    var ba: vec2<f32> = b - a;
    var h: f32 = clamp(dot(pa, ba) / max(dot(ba, ba), 1e-6), 0.0, 1.0);
    return length(pa - ba * h);
}

fn lc_plot(data: vec2<f32>) -> vec2<f32> {
    var pad: vec2<f32> = vec2<f32>(0.04, 0.14);
    return vec2<f32>(
        pad.x + data.x * (1.0 - pad.x * 2.0),
        pad.y + (1.0 - (data.y * 0.5 + 0.5)) * (1.0 - pad.y * 2.0));
}

fn lc_random(cycle: f32) -> f32 {
    let levels = array<f32, 8>(0.7, -0.45, 0.15, -0.8, 0.5, -0.1, 0.9, -0.6);
    return levels[u32(cycle - floor(cycle / 8.0) * 8.0)];
}
fn lc_shape(shape: i32, x: f32, pw: f32) -> f32 {
    var phase: f32 = x - floor(x);
    if (shape == 1) {
        return sin(6.28318530718 * phase);
    } else if (shape == 2) {
        return select(-1.0, 1.0, phase < clamp(pw, 0.05, 0.95));
    } else if (shape == 3) {
        return phase * 2.0 - 1.0;
    }
    if (shape == 4) { return lc_random(floor(x)); }
    if (shape == 5) { return mix(lc_random(floor(x) - 1.0), lc_random(floor(x)), clamp(phase / 0.4, 0.0, 1.0)); }
    if (shape == 6) {
        let duty = clamp(pw, 0.0, 1.0);
        let skew = 0.05 + 0.9 * duty;
        return min(1.0, select(1.0 - 2.0 * (1.0 - phase) / (1.0 - skew), 1.0 - 2.0 * phase / skew, phase < duty));
    }
    if (shape == 7) { return select(-1.0, 1.0, phase < clamp(pw, 0.0, 1.0)); }
    var peak: f32 = clamp(pw, 0.05, 0.95);
    return select(1.0 - 2.0 * (phase - peak) / (1.0 - peak), -1.0 + 2.0 * phase / peak, phase < peak);
}

@fragment
fn widget_frag(input: WidgetVaryings) -> @location(0) vec4<f32>
{
    var uv: vec2<f32> = input.uv;
    var aspect: f32 = max(input.aspect, 0.0001);
    var shape: i32 = i32(round(clamp(input.uniform_a.x, 0.0, 7.0)));
    var pw: f32 = input.uniform_a.y;
    var offset: f32 = input.uniform_a.z;
    var markerPhase: f32 = input.uniform_a.w;
    let cycles = input.uniform_b.x;

    var col: vec4<f32> = input.color_b;

    // A single faint zero line; no other grid.
    var zero: vec2<f32> = lc_plot(vec2<f32>(0.0, 0.0));
    var zeroDist: f32 = abs(uv.y - zero.y);
    var zeroMask: f32 = smoothstep(0.006, 0.002, zeroDist);
    col = vec4<f32>((mix(col.rgb, input.color_c.rgb, zeroMask * input.color_c.a * 0.5)), col.a);

    // Fill between the zero line and the curve.
    var dataX: f32 = clamp((uv.x - 0.04) / 0.92, 0.0, 1.0);
    var curveAtX: f32 = lc_shape(shape, dataX * cycles + offset, pw);
    var curveY: f32 = lc_plot(vec2<f32>(dataX, curveAtX)).y;
    var between: f32 = step(min(curveY, zero.y) - 0.002, uv.y) * step(uv.y, max(curveY, zero.y) + 0.002);
    col = vec4<f32>((mix(col.rgb, input.color_d.rgb, between * input.color_d.a)), col.a);

    var lineMask: f32 = 0.0;
    const steps: i32 = 96;
    var prevX: f32 = 0.0;
    var prevY: f32 = lc_shape(shape, offset, pw);
    for (var i: i32 = 1; i <= steps; i = i + 1) {
        var x: f32 = f32(i) / f32(steps);
        var y: f32 = lc_shape(shape, x * cycles + offset, pw);
        var a: vec2<f32> = lc_plot(vec2<f32>(prevX, prevY));
        var b: vec2<f32> = lc_plot(vec2<f32>(x, y));
        var d: f32 = lc_sdSegment(vec2<f32>(uv.x * aspect, uv.y), vec2<f32>(a.x * aspect, a.y), vec2<f32>(b.x * aspect, b.y));
        var aa: f32 = max(fwidth(d), 0.001);
        lineMask = max(lineMask, smoothstep(0.009 + aa, 0.003, d));
        prevX = x;
        prevY = y;
    }

    col = vec4<f32>((mix(col.rgb, input.color_a.rgb, lineMask * input.color_a.a)), col.a);
    col.a = max(col.a, max(lineMask * input.color_a.a, input.color_b.a));

    if (markerPhase >= 0.0) {
        var running: f32 = markerPhase - offset;
        var markerX: f32 = clamp(running - floor(running), 0.0, 1.0) / cycles;
        var marker: vec2<f32> = lc_plot(vec2<f32>(markerX, lc_shape(shape, markerX * cycles + offset, pw)));
        var markerDist: f32 = length(vec2<f32>((uv.x - marker.x) * aspect, uv.y - marker.y));
        var outer: f32 = smoothstep(0.058, 0.042, markerDist);
        var inner: f32 = smoothstep(0.036, 0.022, markerDist);
        col = vec4<f32>((mix(col.rgb, vec3<f32>(0.02, 0.025, 0.03), outer)), col.a);
        col = vec4<f32>((mix(col.rgb, input.color_a.rgb, inner)), col.a);
        col.a = max(col.a, outer);
    }
    return col;
}"#;

pub const MULTIBAND_METER_SHADER: &str = r#"
@fragment
fn widget_frag(input: WidgetVaryings) -> @location(0) vec4<f32>
{
    var uv: vec2<f32> = input.uv;
    var col: vec4<f32> = input.color_b;
    var flags: i32 = i32(round(input.value_t));

    // Rows top to bottom: high (band 2), mid (1), low (0).
    var rowF: f32 = uv.y * 3.0;
    var row: i32 = clamp(i32(floor(rowF)), 0, 2);
    var band: i32 = 2 - row;
    var rowY: f32 = rowF - f32(row); // 0 at row top, 1 at row bottom

    var levelL: f32 = select(select(input.uniform_b.x, input.uniform_a.z, band == 1), input.uniform_a.x, band == 0);
    var levelR: f32 = select(select(input.uniform_b.y, input.uniform_a.w, band == 1), input.uniform_a.y, band == 0);
    var gain: f32 = select(select(input.uniform_c.x, input.uniform_b.w, band == 1), input.uniform_b.z, band == 0);
    var belowX: f32 = select(select(input.uniform_c.w, input.uniform_c.z, band == 1), input.uniform_c.y, band == 0);
    var aboveX: f32 = select(select(input.uniform_d.z, input.uniform_d.y, band == 1), input.uniform_d.x, band == 0);
    var bandOn: bool = (flags & (1i << u32(band))) != 0;
    var bandActive: bool = (band == 1)
        || (band == 0 && (flags & (1i << 3u)) != 0)
        || (band == 2 && (flags & (1i << 4u)) != 0);
    var dim: f32 = select(0.22, select(0.45, 1.0, bandOn), bandActive);

    // Lighter zone between the below and above thresholds.
    var zone: f32 = step(belowX, uv.x) * step(uv.x, aboveX);
    col = vec4<f32>((mix(col.rgb, col.rgb + vec3<f32>(0.045, 0.055, 0.06), zone * dim)), col.a);

    // Vertical grid every 10 dB, horizontal row separators.
    var gridT: f32 = fract(uv.x * 8.0);
    var gridDist: f32 = min(gridT, 1.0 - gridT) / 8.0;
    var gridAA: f32 = max(fwidth(uv.x), 0.0008);
    var gridMask: f32 = smoothstep(gridAA * 1.6, 0.0, gridDist) * 0.55;
    col = vec4<f32>((mix(col.rgb, input.color_c.rgb, gridMask * input.color_c.a)), col.a);
    var sepT: f32 = fract(uv.y * 3.0);
    var sepDist: f32 = min(sepT, 1.0 - sepT) / 3.0;
    var sepAA: f32 = max(fwidth(uv.y), 0.0012);
    var sepMask: f32 = smoothstep(sepAA * 1.8, 0.0, sepDist) * 0.8;
    col = vec4<f32>((mix(col.rgb, input.color_c.rgb, sepMask * input.color_c.a)), col.a);

    // Two thin level bars (L above R) plus the orange gain marker between
    // them: it spans from the louder channel's tip to tip + gain.
    var barL: f32 = select(0.0, 1.0, (rowY > 0.16 && rowY < 0.42));
    var barR: f32 = select(0.0, 1.0, (rowY > 0.58 && rowY < 0.84));
    var gainBar: f32 = select(0.0, 1.0, (rowY > 0.42 && rowY < 0.58));
    var aa: f32 = max(fwidth(uv.x), 0.0008);
    var maskL: f32 = barL * step(0.004, levelL) * smoothstep(levelL + aa, levelL - aa, uv.x);
    var maskR: f32 = barR * step(0.004, levelR) * smoothstep(levelR + aa, levelR - aa, uv.x);
    col = vec4<f32>((mix(col.rgb, input.color_a.rgb, max(maskL, maskR) * dim)), col.a);

    // Only draw the gain marker once a signal is present.
    var anchor: f32 = max(levelL, levelR);
    var present: f32 = step(0.02, anchor);
    var gLo: f32 = min(anchor, anchor + gain);
    var gHi: f32 = max(anchor, anchor + gain);
    var gainMask: f32 = gainBar * present * step(gLo - aa, uv.x) * step(uv.x, gHi + aa);
    // Keep a minimal tick visible at the anchor so the marker reads even at
    // unity gain.
    var tick: f32 = gainBar * present
        * smoothstep(aa * 2.4, aa * 0.6, abs(uv.x - anchor));
    col = vec4<f32>((mix(col.rgb, input.color_d.rgb, max(gainMask, tick) * dim)), col.a);

    return col;
}"#;

pub const RESPONSE_CURVE_EDITOR_SHADER: &str = r#"
fn rce_sdSegment(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    var pa: vec2<f32> = p - a;
    var ba: vec2<f32> = b - a;
    var h: f32 = clamp(dot(pa, ba) / max(dot(ba, ba), 1e-6), 0.0, 1.0);
    return length(pa - ba * h);
}

fn rce_plot(data: vec2<f32>) -> vec2<f32> {
    var pad: vec2<f32> = vec2<f32>(0.0, 0.0);
    return vec2<f32>(
        pad.x + data.x * (1.0 - pad.x * 2.0),
        pad.y + (1.0 - data.y) * (1.0 - pad.y * 2.0));
}

fn rce_filterResponseY(bandType: f32, x: f32, freqT: f32, q: f32, octaveSpan: f32) -> f32 {
    // The production filter is a topology-preserving state-variable filter.
    // Its analog prototype gives a stable, physically meaningful display
    // response without making the rolloff collapse at either plot boundary.
    var ratio: f32 = exp2(clamp((x - freqT) * octaveSpan, -24.0, 24.0));
    var ratio2: f32 = ratio * ratio;
    var damping: f32 = 1.0 / max(q, 0.001);
    var denominator: f32 = sqrt(
        (1.0 - ratio2) * (1.0 - ratio2)
        + damping * damping * ratio2);
    var magnitude: f32 = 1.0 / max(denominator, 0.000001);
    if (bandType > 0.5 && bandType < 1.5) {
        magnitude = ratio2 / max(denominator, 0.000001);
    } else if (bandType > 1.5 && bandType < 2.5) {
        magnitude = ratio / max(denominator, 0.000001);
    } else if (bandType > 6.5 && bandType < 7.5) {
        magnitude = abs(1.0 - ratio2) / max(denominator, 0.000001);
    }
    var decibels: f32 = 20.0 * (log2(max(magnitude, 0.000001)) * 0.30102999566);
    return 0.5 + decibels / 48.0;
}

// Contribution (in plot units, 0 at unity) of a second filter band so two
// bands (e.g. highpass + lowpass) draw as one combined response curve.
// otherType is the band type code + 1; 0 means no second band.
fn rce_curveY(bandType: f32, x: f32, freqT: f32, yT: f32, qT: f32, isFilter: f32, filterQ: f32, octaveSpan: f32) -> f32 {
    var dist: f32 = x - freqT;
    var q: f32 = mix(0.22, 0.045, qT);
    if (isFilter > 0.5) {
        if (bandType > 5.5 && bandType < 6.5) {
            var passbandOctaveSpan: f32 = 9.97;
            var widthOctaves: f32 = mix(0.25, 6.0, qT);
            var halfWidth: f32 = (widthOctaves * 0.5) / passbandOctaveSpan;
            var lowEdge: f32 = freqT - halfWidth;
            var highEdge: f32 = freqT + halfWidth;
            var hpOctaves: f32 = (x - lowEdge) * passbandOctaveSpan;
            var lpOctaves: f32 = (highEdge - x) * passbandOctaveSpan;
            var hp: f32 = 1.0 / sqrt(1.0 + pow(2.0, -hpOctaves * 3.6));
            var lp: f32 = 1.0 / sqrt(1.0 + pow(2.0, -lpOctaves * 3.6));
            var pass_value: f32 = clamp(hp * lp * 1.14, 0.0, 1.0);
            var eased: f32 = smoothstep(0.0, 1.0, pass_value);
            return -0.95 + eased * 1.45;
        }
        return rce_filterResponseY(bandType, x, freqT, filterQ, octaveSpan);
    }

    if (bandType > 2.5 && bandType < 3.5) {
        var shelf: f32 = 1.0 - smoothstep(freqT - q, freqT + q, x);
        return mix(0.5, yT, shelf);
    }
    if (bandType > 3.5 && bandType < 4.5) {
        var shelf: f32 = smoothstep(freqT - q, freqT + q, x);
        return mix(0.5, yT, shelf);
    }
    var peak: f32 = exp(-dist * dist / max(q * q, 0.0001));
    return mix(0.5, yT, peak);
}

// Filter mode: (otherQ, otherSpan) = display Q + octave span; eq mode
// (`:combine true`): (otherQ, otherSpan) = gain t + q t.
fn rce_otherY(otherType: f32, x: f32, otherFreqT: f32, otherQ: f32, otherSpan: f32, isFilter: f32) -> f32 {
    if (otherType < 0.5) { return 0.0; }
    if (isFilter > 0.5) {
        return rce_filterResponseY(otherType - 1.0, x, otherFreqT, otherQ, otherSpan) - 0.5;
    }
    return rce_curveY(otherType - 1.0, x, otherFreqT, clamp(otherQ, 0.0, 1.0), clamp(otherSpan, 0.0, 1.0), 0.0, 1.0, 1.0) - 0.5;
}

@fragment
fn widget_frag(input: WidgetVaryings) -> @location(0) vec4<f32>
{
    var uv: vec2<f32> = input.uv;
    var aspect: f32 = max(input.aspect, 0.0001);
    var bandType: f32 = input.uniform_a.x;
    var freqT: f32 = clamp(input.uniform_a.y, 0.0, 1.0);
    var yT: f32 = clamp(input.uniform_a.z, 0.0, 1.0);
    var qT: f32 = clamp(input.uniform_a.w, 0.0, 1.0);
    var bandIndex: f32 = input.uniform_b.x;
    var isFilter: f32 = input.uniform_b.w;
    // Curve stroke half-width in plot-height units (from the stroke-width
    // prop, in px); 0 falls back to the widget's historical hairline.
    var strokeHalf: f32 = select(input.uniform_b.z, 0.004, input.uniform_b.z <= 0.0);
    var filterQ: f32 = max(input.uniform_c.x, 0.001);
    var octaveSpan: f32 = max(input.uniform_c.y, 0.001);
    var otherType: f32 = input.uniform_c.z;
    var otherFreqT: f32 = clamp(input.uniform_c.w, 0.0, 1.0);
    var otherQ: f32 = select(input.uniform_d.z, max(input.uniform_d.z, 0.001), isFilter > 0.5);
    var otherSpan: f32 = select(input.uniform_d.w, max(input.uniform_d.w, 0.001), isFilter > 0.5);
    // With a combined curve only the first instance draws it; the others
    // contribute just their handle.
    var drawCurve: f32 = select(1.0, step(bandIndex, 0.5), otherType > 0.5);

    var col: vec4<f32> = vec4<f32>(0.0);
    var clipMask: f32 = 1.0;
    if (input.corner_radius > 0.0) {
        var r: f32 = min(input.corner_radius, min(aspect, 1.0));
        var p: vec2<f32> = vec2<f32>((uv.x - 0.5) * 2.0 * aspect, (uv.y - 0.5) * 2.0);
        var halfSize: vec2<f32> = vec2<f32>(aspect - r, 1.0 - r);
        var qr: vec2<f32> = abs(p) - halfSize;
        var d: f32 = length(max(qr, vec2<f32>(0.0))) + min(max(qr.x, qr.y), 0.0) - r;
        var edge: f32 = max(fwidth(d) * 1.2, 0.001);
        clipMask = smoothstep(edge, -edge, d);
    }

    if (bandIndex < 0.5) {
        col = input.color_b;
        var grid: f32 = 0.0;
        let majorXs = array<f32, 4>(0.285, 0.50, 0.715, 0.93);
        for (var i: i32 = 0; i < 4; i = i + 1) {
            var d: f32 = abs(uv.x - majorXs[i]);
            grid = max(grid, 1.0 - smoothstep(0.0015, 0.004, d));
        }
        for (var i: i32 = 1; i < 4; i = i + 1) {
            var y: f32 = f32(i) / 4.0;
            var d: f32 = abs(uv.y - y);
            grid = max(grid, 0.75 * (1.0 - smoothstep(0.0015, 0.004, d)));
        }
        col = vec4<f32>((mix(col.rgb, input.color_c.rgb, grid * input.color_c.a)), col.a);
    }

    var lineMask: f32 = 0.0;
    const steps: i32 = 96;
    var prevX: f32 = 0.0;
    var prevY: f32 = rce_curveY(
        bandType, 0.0, freqT, yT, qT, isFilter, filterQ, octaveSpan)
        + rce_otherY(otherType, 0.0, otherFreqT, otherQ, otherSpan, isFilter);
    for (var i: i32 = 1; i <= steps; i = i + 1) {
        var x: f32 = f32(i) / f32(steps);
        var y: f32 = rce_curveY(
            bandType, x, freqT, yT, qT, isFilter, filterQ, octaveSpan)
            + rce_otherY(otherType, x, otherFreqT, otherQ, otherSpan, isFilter);
        var a: vec2<f32> = rce_plot(vec2<f32>(prevX, prevY));
        var b: vec2<f32> = rce_plot(vec2<f32>(x, y));
        var d: f32 = rce_sdSegment(vec2<f32>(uv.x * aspect, uv.y), vec2<f32>(a.x * aspect, a.y), vec2<f32>(b.x * aspect, b.y));
        var aa: f32 = max(fwidth(d), 0.001);
        lineMask = max(lineMask, smoothstep(strokeHalf + aa, max(strokeHalf - aa, 0.0), d));
        prevX = x;
        prevY = y;
    }
    lineMask = lineMask * drawCurve;

    var handle_pos: vec2<f32> = rce_plot(vec2<f32>(freqT, yT));
    var handleInset: vec2<f32> = clamp(input.uniform_d.xy, vec2<f32>(0.0), vec2<f32>(0.49));
    handle_pos = clamp(handle_pos, handleInset, vec2<f32>(1.0) - handleInset);
    var hd: f32 = length(vec2<f32>((uv.x - handle_pos.x) * aspect, uv.y - handle_pos.y));
    var selected: f32 = input.value_t;
    var handleOuter: f32 = smoothstep(0.068, 0.052, hd);
    var handleInner: f32 = smoothstep(0.048, 0.032, hd);

    col = vec4<f32>((mix(col.rgb, input.color_a.rgb, lineMask * input.color_a.a)), col.a);
    col.a = max(col.a, lineMask * input.color_a.a);
    col = vec4<f32>((mix(col.rgb, input.color_d.rgb, handleOuter * input.color_d.a)), col.a);
    if (selected < 0.5) {
        col = vec4<f32>((mix(col.rgb, input.color_b.rgb, handleInner)), col.a);
    }
    // The handle_pos is constrained to the primitive bounds above, so preserve it
    // over the rounded background mask. Otherwise a corner handle_pos would still
    // lose its diagonal edge even though its center was correctly inset.
    col.a = max(col.a * clipMask, handleOuter);
    return col;
}"#;

pub const SCROLL_FRAGMENT_SHADER: &str = r#"
@fragment
fn widget_frag(input: WidgetVaryings) -> @location(0) vec4<f32>
{
    var uv: vec2<f32> = input.uv;
    var aspect: f32 = input.aspect;

    // Uniforms: value_t = scroll position [0,1]
    //           uniform_a.x = thumb height ratio (viewport/content)
    //           uniform_a.y = horizontal padding input normalized coords
    var scroll_t: f32 = input.value_t;
    var thumb_ratio: f32 = input.uniform_a.x;
    var pad: f32 = input.uniform_a.y;

    // Thumb vertical position and size input UV space
    var thumb_h: f32 = max(thumb_ratio, 0.04);
    var thumb_y: f32 = scroll_t * (1.0 - thumb_h);

    // Thumb horizontal bounds (centered pill, narrow)
    var bar_left: f32 = pad;
    var bar_right: f32 = 1.0 - pad;
    var bar_cx: f32 = 0.5;
    var bar_hw: f32 = (bar_right - bar_left) * 0.5;

    // SDF for the thumb pill (rounded rect)
    // Map to centered coords for the thumb
    var thumb_center: vec2<f32> = vec2<f32>(bar_cx, thumb_y + thumb_h * 0.5);
    var half_size: vec2<f32> = vec2<f32>(bar_hw, thumb_h * 0.5);

    // Pill radius = half the width (fully round on short axis)
    var radius: f32 = min(half_size.x * aspect, half_size.y);

    // Aspect-correct SDF
    var p: vec2<f32> = vec2<f32>((uv.x - thumb_center.x) * aspect, uv.y - thumb_center.y);
    var q: vec2<f32> = abs(p) - vec2<f32>(half_size.x * aspect - radius, half_size.y - radius);
    var dist: f32 = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - radius;

    // Anti-aliased edge
    var edge: f32 = fwidth(dist) * 1.0;
    var thumb_mask: f32 = smoothstep(edge, -edge, dist);

    // Thumb color with alpha
    var thumb_color: vec4<f32> = input.color_a;
    var result: vec4<f32> = vec4<f32>(thumb_color.rgb, thumb_color.a * thumb_mask);

    // Discard fully transparent pixels
    if (result.a < 0.002) { discard; }

    return result;
}"#;

pub const DELTA_GLYPH_SHADER: &str = r#"
// Geometry constants mirror sequencer::delta_glyph (spec §6). Radius and k are
// FIXED: magnitude rides occupancy (which cells a piece claims) and luminance.
// Two equal discs weld iff their surface gap is within 0.6452*k, and all three
// lattice adjacencies (0.3672 / 0.40572 / 0.40896) clear that at R = 0.18.
const DG_K: f32 = 0.155;
const DG_R: f32 = 0.18;
const DG_STEP_X: f32 = 0.3672;
const DG_STEP_Y: f32 = 0.3636;
const DG_STAGGER: f32 = 0.09;
const DG_SUB_MIN: f32 = 0.155;
const DG_SUB_MAX: f32 = 0.185;
const DG_PIECE_WORD: i32 = 5;
const DG_MAX_PIECES: i32 = 5;

// Dual-maintained with delta_glyph::GROUP_PALETTE. Index 6 is the deliberately
// un-hued "unclassified" tone.
const DG_HUES: array<vec3<f32>, 7> = array<vec3<f32>, 7>(
    vec3<f32>(0.98, 0.72, 0.22), vec3<f32>(0.95, 0.30, 0.62), vec3<f32>(0.55, 0.95, 0.30),
    vec3<f32>(0.25, 0.80, 0.95), vec3<f32>(0.62, 0.45, 0.98), vec3<f32>(0.96, 0.46, 0.28),
    vec3<f32>(0.56, 0.57, 0.60),
);

fn dg_packed(input: WidgetVaryings, index: i32) -> f32 {
    switch (index) {
        case 0: { return input.uniform_a.x; }
        case 1: { return input.uniform_a.y; }
        case 2: { return input.uniform_a.z; }
        case 3: { return input.uniform_a.w; }
        case 4: { return input.uniform_b.x; }
        case 5: { return input.uniform_b.y; }
        case 6: { return input.uniform_b.z; }
        case 7: { return input.uniform_b.w; }
        case 8: { return input.uniform_c.x; }
        case 9: { return input.uniform_c.y; }
        case 10: { return input.uniform_c.z; }
        case 11: { return input.uniform_c.w; }
        case 12: { return input.uniform_d.x; }
        case 13: { return input.uniform_d.y; }
        case 14: { return input.uniform_d.z; }
        case 15: { return input.uniform_d.w; }
        case 16: { return input.value_t; }
        default: { return input.itime; }
    }
}

fn dg_cols(input: WidgetVaryings) -> i32 { return clamp(i32(round(input.color_d.x)), 1, 5); }
fn dg_rows(input: WidgetVaryings) -> i32 { return clamp(i32(round(input.color_d.y)), 1, 5); }
fn dg_anchor(input: WidgetVaryings) -> bool { return (u32(round(input.color_d.w)) & 1u) != 0u; }
fn dg_incompatible(input: WidgetVaryings) -> bool { return (u32(round(input.color_d.w)) & 2u) != 0u; }
// Virtual pixel count across the glyph (bits 2..9 of the flag word); 0 = off.
fn dg_pixelate(input: WidgetVaryings) -> f32 {
    return f32((u32(round(input.color_d.w)) >> 2) & 255u);
}

fn dg_play_color(input: WidgetVaryings) -> vec3<f32> {
    var rgb: u32 = u32(round(input.corner_radius));
    return vec3<f32>(f32(rgb & 255u),
                  f32((rgb >> 8) & 255u),
                  f32((rgb >> 16) & 255u)) / 255.0;
}

// 0 = unassigned slot, else 1..15 across the substrate radius band.
fn dg_substrate(input: WidgetVaryings, slot: i32) -> u32 {
    var word: u32 = u32(round(dg_packed(input, slot / 5)));
    return (word >> u32(4 * (slot % 5))) & 15u;
}

fn dg_piece(input: WidgetVaryings, index: i32) -> u32 {
    return u32(round(dg_packed(input, DG_PIECE_WORD + index)));
}

fn dg_smin(a: f32, b: f32, k: f32) -> f32 {
    var h: f32 = max(k - abs(a - b), 0.0) / max(k, 0.0001);
    return min(a, b) - pow(h, 1.55) * 0.5 * k / 1.55;
}

// Plain column-major: slot = col*rows + row. Rev 2 reversed odd columns, which
// broke horizontal adjacency and therefore every piece built from it.
fn dg_center(col: i32, row: i32, cols: i32, rows: i32) -> vec2<f32> {
    var x: f32 = (f32(col) - 0.5 * f32(cols - 1)) * DG_STEP_X
            + select(-DG_STAGGER, DG_STAGGER, (row & 1) == 1);
    var y: f32 = (f32(row) - 0.5 * f32(rows - 1)) * DG_STEP_Y;
    return vec2<f32>(x, y);
}

fn dg_fit(input: WidgetVaryings) -> f32 {
    var extentX: f32 = f32(dg_cols(input) - 1) * DG_STEP_X + 2.0 * DG_STAGGER + 2.0 * DG_R + 0.06;
    var extentY: f32 = f32(dg_rows(input) - 1) * DG_STEP_Y + 2.0 * DG_R + 0.06;
    return 2.0 / max(extentX, extentY);
}

// The substrate: one disc per assigned slot, radius from the patch's ABSOLUTE
// parameter values, over a band that lies entirely inside the fusion zone — so
// this is always one molten mass whose silhouette varies with the whole vector.
fn dg_substrate_field(p_input: vec2<f32>, input: WidgetVaryings) -> f32 {
    var p = p_input;
    var cols: i32 = dg_cols(input);
    var rows: i32 = dg_rows(input);
    var fit: f32 = dg_fit(input);
    p = p / fit;
    var scene: f32 = 1000.0;
    for (var slot: i32 = 0; slot < 25; slot = slot + 1) {
        if (slot >= cols * rows) { break; }
        var level: u32 = dg_substrate(input, slot);
        if (level == 0u) { continue; }
        var radius: f32 = mix(DG_SUB_MIN, DG_SUB_MAX, f32(level - 1u) / 14.0);
        var c: vec2<f32> = dg_center(slot / rows, slot % rows, cols, rows);
        scene = dg_smin(scene, length(p - c) - radius, DG_K);
    }
    return scene * fit;
}

// One accent piece: a contiguous polyomino of 1..5 cells, welded at fixed radius.
// The table mirrors delta_glyph::PIECES (tier*3 + variant).
fn dg_piece_field(p_input: vec2<f32>, input: WidgetVaryings, record: u32) -> f32 {
    var p = p_input;
    var cols: i32 = dg_cols(input);
    var rows: i32 = dg_rows(input);
    var fit: f32 = dg_fit(input);
    p = p / fit;
    var slot: i32 = i32(record & 31u);
    var id: i32 = i32((record >> 5) & 15u);
    var mirror: f32 = select(1.0, -1.0, ((record >> 15) & 1u) != 0u);
    var anchorCol: i32 = slot / rows;
    var anchorRow: i32 = slot % rows;

    var scene: f32 = 1000.0;
    for (var prim: i32 = 0; prim < 5; prim = prim + 1) {
        var dcol: i32 = 0;
        var drow: i32 = 0;
        var capsule: bool = false;
        var present: bool = false;
        // tier 0: 1 cell
        if (id <= 2) {
            present = prim == 0;
        } else if (id == 3) {                 // capsule
            present = prim == 0; capsule = true;
        } else if (id == 4) {                 // vertical pair
            present = prim < 2; drow = prim;
        } else if (id == 5) {                 // diagonal pair
            present = prim < 2; dcol = prim; drow = prim;
        } else if (id == 6) {                 // capsule + disc
            present = prim < 2; capsule = prim == 0; drow = prim;
        } else if (id == 7) {                 // L
            present = prim < 3; drow = min(prim, 1); dcol = select(0, 1, prim == 2);
        } else if (id == 8) {                 // vertical run
            present = prim < 3; drow = prim;
        } else if (id == 9) {                 // stacked capsules
            present = prim < 2; capsule = true; drow = prim;
        } else if (id == 10) {                // 2x2
            present = prim < 4; dcol = prim / 2; drow = prim % 2;
        } else if (id == 11) {                // capsule + 2 discs
            present = prim < 3; capsule = prim == 0;
            drow = select(1, 0, prim == 0); dcol = select(0, 1, prim == 2);
        } else if (id == 12) {                // 2 capsules + disc
            present = prim < 3; capsule = prim < 2; drow = prim;
        } else if (id == 13) {                // P-pentomino
            present = prim < 5; dcol = prim / 3; drow = prim % 3;
        } else {                              // capsule + 3 discs
            present = prim < 4; capsule = prim == 0;
            dcol = select(0, 1, prim >= 2); drow = select(select(prim - 1, 1, prim == 1), 0, prim == 0);
        }
        if (!present) { continue; }

        var col: i32 = anchorCol + i32(mirror) * dcol;
        var row: i32 = anchorRow + drow;
        if (col < 0 || col >= cols || row < 0 || row >= rows) { continue; }
        var c: vec2<f32> = dg_center(col, row, cols, rows);
        var sdf: f32;
        if (capsule) {
            // A stadium welding this cell to its horizontal neighbour: the
            // two-cell pair as one CONVEX primitive, no waist. This is where the
            // elongated lobes come from.
            var farCol: i32 = col + i32(mirror);
            if (farCol < 0 || farCol >= cols) {
                sdf = length(p - c) - DG_R;
            } else {
                var far: vec2<f32> = dg_center(farCol, row, cols, rows);
                var seg: vec2<f32> = far - c;
                var t: f32 = clamp(dot(p - c, seg) / max(dot(seg, seg), 0.0001), 0.0, 1.0);
                sdf = length(p - c - seg * t) - DG_R;
            }
        } else {
            sdf = length(p - c) - DG_R;
        }
        scene = dg_smin(scene, sdf, DG_K);
    }
    return scene * fit;
}

// ── tuning uniforms ──
// Live-tunable from lisp via widget props (see TUNING_PROPS on the Rust side).
// Words 10..17 and color_b/color_c; every default reproduces the shipped look.
fn dg_tune(input: WidgetVaryings, word: i32) -> f32 { return dg_packed(input, word); }

// The fake-3D height profile: a smoothstep window over the SDF raised to a
// power. :height-input/:height-out move the window, :height-pow shapes the
// shoulder, :height-amp scales the relief the normals read.
fn dg_height(sdf: f32, fit: f32, input: WidgetVaryings) -> f32 {
    var lo: f32 = dg_tune(input, 12) * fit;
    var hi: f32 = max(dg_tune(input, 13) * fit, lo + 1e-5);
    return dg_tune(input, 10) * pow(smoothstep(lo, hi, sdf), max(dg_tune(input, 11), 0.01));
}

fn dg_normal_substrate(p: vec2<f32>, input: WidgetVaryings) -> vec3<f32> {
    var fit: f32 = dg_fit(input);
    var e: f32 = max(dg_tune(input, 14), 1e-6) * fit;
    var x: f32 = dg_height(dg_substrate_field(p + vec2<f32>(e, 0.0), input), fit, input)
            - dg_height(dg_substrate_field(p - vec2<f32>(e, 0.0), input), fit, input);
    var y: f32 = dg_height(dg_substrate_field(p + vec2<f32>(0.0, e), input), fit, input)
            - dg_height(dg_substrate_field(p - vec2<f32>(0.0, e), input), fit, input);
    return normalize(vec3<f32>(x, y, 2.0 * e));
}

fn dg_normal_piece(p: vec2<f32>, input: WidgetVaryings, record: u32) -> vec3<f32> {
    var fit: f32 = dg_fit(input);
    var e: f32 = max(dg_tune(input, 14), 1e-6) * fit;
    var x: f32 = dg_height(dg_piece_field(p + vec2<f32>(e, 0.0), input, record), fit, input)
            - dg_height(dg_piece_field(p - vec2<f32>(e, 0.0), input, record), fit, input);
    var y: f32 = dg_height(dg_piece_field(p + vec2<f32>(0.0, e), input, record), fit, input)
            - dg_height(dg_piece_field(p - vec2<f32>(0.0, e), input, record), fit, input);
    return normalize(vec3<f32>(x, y, 2.0 * e));
}

fn dg_material(p: vec2<f32>, n: vec3<f32>, tint: vec4<f32>, crease: bool, input: WidgetVaryings) -> vec4<f32> {
    var l1: vec3<f32> = vec3<f32>(-0.11, -0.8138, 0.3);
    var l2: vec3<f32> = vec3<f32>(-0.5238, 0.3, 1.4);
    var specPow: f32 = max(input.color_b.x, 0.05);
    var viewer: vec3<f32> = vec3<f32>(p, 1.0) - vec3<f32>(-0.81891595, 1.39159394, 0.87441919);
    var spec1: f32 = pow(max(0.0, 0.99 * dot(n, normalize(l1 + viewer))), 24.0 * specPow);
    var spec2: f32 = pow(max(0.0, 0.969 * dot(n, normalize(l2 + viewer))), 22.0 * specPow);
    var scale: f32 = select(0.51513593, 0.321513593 * max(input.color_b.y, 0.0), crease);
    // The original adds this achromatically (a precedence artifact there — see
    // docs/sdf-blob-glyph-algorithm.md §6.4). Damped: at full strength it
    // desaturates every cell toward white, and hue is this glyph's group legend.
    var white: f32 = dg_tune(input, 16) * (scale * spec1 + spec2 + 0.293913139 * dot(l1, n));
    // Diffuse rides color_d.z — itime is excluded from the run-cache hash
    // for non-animated widgets, so data there would never invalidate.
    var color: vec4<f32> = vec4<f32>(white) + input.color_d.z * dot(l2, n) * tint;
    color = vec4<f32>((clamp(color.rgb, vec3<f32>(0.0), vec3<f32>(1.0))), color.a);
    color.a = tint.a;
    return color;
}

// Composite one layer over the running color: interior shading inside the
// surface, coverage AA at the silhouette, then the optional neon rim hugging
// the inside edge and the optional emissive halo falling off outside it.
fn dg_compose(color_input: vec4<f32>, material_input: vec4<f32>, sdf: f32, fit: f32, tint: vec3<f32>, input: WidgetVaryings) -> vec4<f32> {
    var color = color_input;
    var material = material_input;
    var edge: f32 = max(dg_tune(input, 15), 0.0005) * fit;
    var shade: f32 = clamp(input.color_c.z, 0.0, 1.0);
    if (shade > 0.0) {
        // SDF-depth shading: the fill darkens toward the middle of the mass, so
        // it reads as lit volume rather than one static color.
        var width: f32 = max(input.color_c.w, 0.01) * fit;
        material = vec4<f32>(material.rgb * (1.0 - shade * smoothstep(0.0, width, -sdf)), material.a);
    }
    color = mix(color, material, 1.0 - smoothstep(0.0, edge, sdf));
    var rimGain: f32 = input.color_b.w;
    if (rimGain > 0.0) {
        var band: f32 = max(input.color_b.z, 0.005) * fit;
        var rim: f32 = rimGain * (1.0 - smoothstep(0.0, band, abs(sdf + 0.5 * band)));
        color = vec4<f32>(color.rgb + (rim * mix(tint, vec3<f32>(1.0), 0.6)), color.a);
        color.a = max(color.a, min(rim, 1.0));
    }
    var glowGain: f32 = input.color_c.y;
    if (glowGain > 0.0 && sdf > 0.0) {
        var glow: f32 = glowGain * exp(-sdf / max(input.color_c.x * fit, 0.001));
        color = vec4<f32>(color.rgb + (glow * tint), color.a);
        color.a = max(color.a, min(glow, 1.0));
    }
    return color;
}

// Exact signed distance to the play triangle (right-pointing, 2x the vertices
// the mixer's cell background used to draw): negative inside, so the edge
// anti-aliases at true pixel width via fwidth.
fn dg_play_triangle(p: vec2<f32>) -> f32 {
    var p0: vec2<f32> = vec2<f32>(-0.52, -0.72);
    var p1: vec2<f32> = vec2<f32>(-0.52, 0.72);
    var p2: vec2<f32> = vec2<f32>(0.72, 0.0);
    var e0: vec2<f32> = p1 - p0;
    var e1: vec2<f32> = p2 - p1;
    var e2: vec2<f32> = p0 - p2;
    var v0: vec2<f32> = p - p0;
    var v1: vec2<f32> = p - p1;
    var v2: vec2<f32> = p - p2;
    var pq0: vec2<f32> = v0 - e0 * clamp(dot(v0, e0) / dot(e0, e0), 0.0, 1.0);
    var pq1: vec2<f32> = v1 - e1 * clamp(dot(v1, e1) / dot(e1, e1), 0.0, 1.0);
    var pq2: vec2<f32> = v2 - e2 * clamp(dot(v2, e2) / dot(e2, e2), 0.0, 1.0);
    var s: f32 = sign(e0.x * e2.y - e0.y * e2.x);
    var d: vec2<f32> = min(min(vec2<f32>(dot(pq0, pq0), s * (v0.x * e0.y - v0.y * e0.x)),
                       vec2<f32>(dot(pq1, pq1), s * (v1.x * e1.y - v1.y * e1.x))),
                   vec2<f32>(dot(pq2, pq2), s * (v2.x * e2.y - v2.y * e2.x)));
    return -sqrt(d.x) * sign(d.y);
}

@fragment
fn widget_frag(input: WidgetVaryings) -> @location(0) vec4<f32> {
    // Centered uv, +y upward, as required by the delta-glyph lattice.
    var p: vec2<f32> = vec2<f32>(input.uv.x * 2.0 - 1.0, 1.0 - input.uv.y * 2.0);
    var play: bool = input.color_a.w > 0.5;
    // Padding is a fraction of the glyph's half-extent on every side. It and
    // opacity apply only while the play indicator is present; the triangle
    // remains full-size and fully opaque above the quieter identity glyph.
    var glyphScale: f32 = select(1.0, 1.0 - 2.0 * clamp(input.itime, 0.0, 0.45), play);
    var glyphOpacity: f32 = select(1.0, clamp(input.aspect, 0.0, 1.0), play);
    var glyphP: vec2<f32> = p / max(glyphScale, 0.10);
    // Pixelation: snap the sample coordinate to an N-cell virtual grid before
    // any field evaluation. Everything downstream (normals, lighting, rim,
    // glow, interior shade) evaluates at the cell center, so the whole glyph
    // quantizes coherently; the silhouette smoothstep then gives boundary
    // cells partial coverage, which reads as a cleanly downsampled image. The
    // play triangle deliberately stays on the raw coordinate (crisp on top).
    var pixelCells: f32 = dg_pixelate(input);
    if (pixelCells > 0.5) {
        glyphP = (floor((glyphP * 0.5 + 0.5) * pixelCells) + 0.5) / pixelCells * 2.0 - 1.0;
    }
    var fit: f32 = dg_fit(input);
    var color: vec4<f32> = vec4<f32>(0.0);

    var edge: f32 = max(dg_tune(input, 15), 0.0005) * fit;
    var substrate: f32 = dg_substrate_field(glyphP, input);
    // color_a.w is the play flag, NOT the tint alpha — rebuild alpha as 1.0.
    var baseTint: vec4<f32> = vec4<f32>(input.color_a.rgb, 1.0);
    color = dg_compose(color,
                       dg_material(glyphP, dg_normal_substrate(glyphP, input), baseTint, false, input),
                       substrate, fit, baseTint.rgb, input);

    // One layer per lit parameter, all anchored into the SHARED lattice so they
    // interpenetrate — the original's second tier of richness, which rev 2's
    // per-slot layer encoding made structurally impossible.
    var unionSoFar: f32 = substrate;
    for (var index: i32 = 0; index < DG_MAX_PIECES; index = index + 1) {
        var record: u32 = dg_piece(input, index);
        if (((record >> 17) & 1u) == 0u) { continue; }
        var sdf: f32 = dg_piece_field(glyphP, input, record);
        var n: vec3<f32> = dg_normal_piece(glyphP, input, record);

        var hue: vec3<f32> = DG_HUES[min((record >> 9) & 7u, 6u)];
        var magnitude: f32 = f32((record >> 12) & 7u) / 7.0;
        // Sign rides hue temperature, not position: a positional offset large
        // enough to read is larger than the entire fusion budget (spec §6.2).
        hue *= select(vec3<f32>(1.08, 1.00, 0.92), vec3<f32>(0.92, 1.00, 1.10),
                      ((record >> 16) & 1u) != 0u);
        var tint: vec4<f32> = vec4<f32>(clamp(hue * (0.5 + 0.5 * magnitude), vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);

        color = dg_compose(color, dg_material(glyphP, n, tint, false, input),
                           sdf, fit, tint.rgb, input);
        var intersection: f32 = max(unionSoFar, sdf + 0.05 * fit);
        color = mix(color, dg_material(glyphP, n, tint, true, input),
                    1.0 - smoothstep(0.0, edge, intersection + 0.001 * fit));
        unionSoFar = min(unionSoFar, sdf);
    }
    color = vec4<f32>((clamp(color.rgb, vec3<f32>(0.0), vec3<f32>(1.0))), color.a);

    // The anchor tile carries no accents by definition; ring it so it reads as
    // the zero point rather than as an empty patch.
    if (dg_anchor(input)) {
        var ring: f32 = 1.0 - smoothstep(0.008, 0.017, abs(length(glyphP) - 0.055));
        color = mix(color, vec4<f32>(0.85, 0.87, 0.85, 0.8), ring);
    }
    if (dg_incompatible(input)) {
        var ring: f32 = 1.0 - smoothstep(0.010, 0.022, abs(length(glyphP) - 0.91));
        color = mix(color, vec4<f32>(0.96, 0.50, 0.24, 0.9), ring);
    }
    color.a *= glyphOpacity;
    // Play indicator ON TOP of the glyph (color_a.w > 0.5 = playing): caller-
    // colored triangle with ~1px coverage AA, over a soft dark ring so the
    // edge stays legible against bright accent pieces.
    if (play) {
        var d: f32 = dg_play_triangle(p);
        var aa: f32 = max(fwidth(d), 0.002);
        var halo: f32 = (1.0 - smoothstep(0.0, 0.10, d)) * smoothstep(-aa, aa, d);
        color = vec4<f32>((mix(color.rgb, vec3<f32>(0.01, 0.03, 0.015), 0.62 * halo)), color.a);
        color.a = max(color.a, 0.62 * halo);
        var tri: f32 = 1.0 - smoothstep(-aa, aa, d);
        color = vec4<f32>((mix(color.rgb, dg_play_color(input), tri)), color.a);
        color.a = max(color.a, tri);
    }
    if (color.a <= 0.002) { discard; }
    return color;
}"#;

pub const TIMELINE_CURSOR_MARKER_FRAGMENT_SHADER: &str = r#"
@fragment
fn widget_frag(input: WidgetVaryings) -> @location(0) vec4<f32>
{
    var instance_size_px: vec2<f32> = input.uniform_a.xy;
    var marker_size_px: vec2<f32> = input.uniform_a.zw;
    var padding_px: f32 = input.uniform_b.x;

    // Work input physical pixels so the edge transition remains one pixel wide
    // regardless of cell size or display scale.
    var p: vec2<f32> = input.uv * instance_size_px - vec2<f32>(padding_px);
    var half_width: f32 = marker_size_px.x * 0.5;
    p.x -= half_width;

    var side_length: f32 = max(length(vec2<f32>(marker_size_px.y, half_width)), 0.0001);
    var top_distance: f32 = p.y;
    var left_distance: f32 = (marker_size_px.y * (p.x + half_width) - half_width * p.y) / side_length;
    var right_distance: f32 = (marker_size_px.y * (half_width - p.x) - half_width * p.y) / side_length;
    var inside_distance: f32 = min(top_distance, min(left_distance, right_distance));

    var edge_width: f32 = max(fwidth(inside_distance), 0.75);
    var alpha: f32 = smoothstep(-edge_width * 0.5, edge_width * 0.5, inside_distance);
    if (alpha <= 0.001) {
        discard;
    }
    return vec4<f32>(input.color_a.rgb, input.color_a.a * alpha);
}"#;

pub const TOGGLE_FRAGMENT_SHADER: &str = r#"
@fragment
fn widget_frag(input: WidgetVaryings) -> @location(0) vec4<f32>
{
    var uv: vec2<f32> = input.uv;
    var aspect: f32 = input.aspect;

    var localPos: vec2<f32> = vec2<f32>((uv.x - 0.5) * 2.0 * aspect, (uv.y - 0.5) * 2.0);
    var sdfSize: vec2<f32> = vec2<f32>(aspect, 1.0);
    var cornerRadius: f32 = 1.0;

    var borderColor: vec3<f32> = vec3<f32>(0.25, 0.25, 0.28);
    var outerMask: f32;
    var borderMask: f32 = compute_border_mask(localPos, sdfSize, cornerRadius, 1.5, &outerMask);
    if (outerMask <= 0.001) { discard; }

    var on: f32 = input.value_t;
    var bg: vec4<f32> = mix(input.color_b, input.color_a, on);

    var knob_x: f32 = mix(0.3, 0.7, on);
    var knob_pos: vec2<f32> = vec2<f32>((uv.x - knob_x) * aspect, uv.y - 0.5);
    var knob_radius: f32 = 0.28;
    var knobDist: f32 = length(knob_pos) - knob_radius;
    var knobDeriv: f32 = max(fwidth(knobDist), 0.001);
    var knobMask: f32 = smoothstep(knobDeriv, -knobDeriv, knobDist);

    var knobColor: vec4<f32> = mix(input.color_d, input.color_c, on);
    var rgb: vec3<f32> = mix(bg.rgb, borderColor, borderMask);
    rgb = mix(rgb, knobColor.rgb, knobMask);

    return vec4<f32>(rgb, outerMask);
}"#;

pub const TREE_CHEVRON_SHADER: &str = r#"
@fragment
fn widget_frag(input: WidgetVaryings) -> @location(0) vec4<f32>
{
    var uv: vec2<f32> = input.uv;
    var expanded: f32 = input.value_t;
    var col: vec4<f32> = input.color_a;

    // Aspect-corrected coordinates: x input [-a, a], y input [-1, 1]
    var a: f32 = input.aspect;
    var p: vec2<f32> = vec2<f32>((uv.x - 0.5) * 2.0 * a, (uv.y - 0.5) * 2.0);
    // Finder-sized disclosure: about half the box, thin stroke.
    p = p * 1.85;

    // Right chevron ">" — endpoints input aspect-corrected space
    var r_pt: vec2<f32> = vec2<f32>(0.25 * a, 0.0);
    var r_a: vec2<f32> = vec2<f32>(-0.25 * a, -0.5);
    var r_b: vec2<f32> = vec2<f32>(-0.25 * a, 0.5);

    // Down chevron "v" — endpoints input aspect-corrected space
    var d_pt: vec2<f32> = vec2<f32>(0.0, 0.3);
    var d_a: vec2<f32> = vec2<f32>(-0.55 * a, -0.3);
    var d_b: vec2<f32> = vec2<f32>(0.55 * a, -0.3);

    // Interpolate between right and down chevron
    var s: f32 = expanded;
    var pt: vec2<f32> = r_pt * (1.0 - s) + d_pt * s;
    var arm_a: vec2<f32> = r_a * (1.0 - s) + d_a * s;
    var arm_b: vec2<f32> = r_b * (1.0 - s) + d_b * s;

    // SDF for two line segments (arm_a -> pt, pt -> arm_b)
    var pa1: vec2<f32> = p - arm_a;
    var ba1: vec2<f32> = pt - arm_a;
    var h1: f32 = clamp(dot(pa1, ba1) / dot(ba1, ba1), 0.0, 1.0);
    var seg1: f32 = length(pa1 - ba1 * h1);

    var pa2: vec2<f32> = p - pt;
    var ba2: vec2<f32> = arm_b - pt;
    var h2: f32 = clamp(dot(pa2, ba2) / dot(ba2, ba2), 0.0, 1.0);
    var seg2: f32 = length(pa2 - ba2 * h2);

    var d: f32 = min(seg1, seg2);

    // Stroke width + anti-aliasing
    var stroke: f32 = 0.19;
    var edge: f32 = fwidth(d) * 1.2;
    var mask: f32 = smoothstep(stroke + edge, stroke - edge, d);

    if (mask < 0.002) { discard; }

    return vec4<f32>(col.rgb, col.a * mask);
}"#;

pub const VSLIDER_FRAGMENT_SHADER: &str = r#"
@fragment
fn widget_frag(input: WidgetVaryings) -> @location(0) vec4<f32>
{
    var uv: vec2<f32> = input.uv;
    var aspect: f32 = input.aspect;
    var t: f32 = input.value_t;
    var origin_t: f32 = input.uniform_a[0]; // 0 = fill from bottom (default), 0.5 = bipolar center

    // ── Fill bar: rounded rect between origin and value ──
    var fill_lo: f32 = min(t, origin_t);
    var fill_hi: f32 = max(t, origin_t);
    var fill_span: f32 = fill_hi - fill_lo;

    var xPad: f32 = 0.18;
    var halfW: f32 = 0.5 - xPad;
    var halfH: f32 = max(fill_span * 0.5, 0.0);
    var cr: f32 = 0.063;
    cr = min(cr, min(halfW * aspect, max(halfH, 0.001)));

    // Center of fill bar input uv space (uv.y: 0=top, 1=bottom; t=1 → top)
    var fillCenterY: f32 = 1.0 - (fill_lo + fill_hi) * 0.5;
    var p: vec2<f32> = vec2<f32>((uv.x - 0.5) * aspect, uv.y - fillCenterY);
    var b: vec2<f32> = vec2<f32>(halfW * aspect, halfH);
    var q: vec2<f32> = abs(p) - b + cr;
    var d: f32 = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - cr;
    var aa: f32 = max(fwidth(d), 0.001);
    var fillMask: f32 = smoothstep(aa, -aa, d) * step(0.005, fill_span);

    // ── Track dots: fixed grid, only visible outside fill ──
    var dotSpacing: f32 = 0.6 * aspect;
    var dotR: f32 = 0.08 * aspect;
    var dotMask: f32 = 0.0;

    var snapY: f32 = round(uv.y / dotSpacing) * dotSpacing;
    var fillTopUV: f32 = 1.0 - fill_hi;
    var fillBotUV: f32 = 1.0 - fill_lo;
    var margin: f32 = dotSpacing * 0.4;
    if ((snapY < fillTopUV - margin || snapY > fillBotUV + margin)
        && snapY > margin * 0.5 && snapY < 1.0 - margin * 0.5) {
        var dp: vec2<f32> = vec2<f32>((uv.x - 0.5) * aspect, uv.y - snapY);
        var dd: f32 = length(dp) - dotR;
        var da: f32 = max(fwidth(dd), 0.001);
        dotMask = smoothstep(da, -da, dd);
    }

    // Composite
    var rgb: vec3<f32> = input.color_a.rgb * fillMask + input.color_b.rgb * dotMask * (1.0 - fillMask);
    var alpha: f32 = max(fillMask, dotMask);
    if (alpha < 0.001) { discard; }
    return vec4<f32>(rgb, alpha);
}"#;

pub const PHASER_NOTCH_SHADER: &str = r#"
fn phaserFlangerLfo(shape: i32, phase_input: f32) -> f32 {
    var phase = fract(phase_input);
    if (shape == 1) {
        return select((3.0 - 4.0 * phase), (4.0 * phase - 1.0), (phase < 0.5));
    }
    if (shape == 2) { return 2.0 * phase - 1.0; }
    if (shape == 3) { return select(-1.0, 1.0, phase < 0.5); }
    return sin(6.28318530718 * phase);
}

@fragment
fn widget_frag(input: WidgetVaryings) -> @location(0) vec4<f32>
{
    var uv: vec2<f32> = input.uv;
    var aspect: f32 = max(input.aspect, 0.0001);
    var mode: i32 = i32(round(input.uniform_a.x));
    var count: i32 = clamp(i32(round(input.uniform_a.y)), 0, 12);
    var sweep: f32 = clamp(input.uniform_a.z, 0.0, 0.5);
    var rateHz: f32 = max(input.uniform_a.w, 0.0001);
    var shape: i32 = clamp(i32(round(input.value_t)), 0, 3);
    var phaseL: f32 = fract(input.itime * rateHz);
    var phaseR: f32 = fract(phaseL + clamp(input.uniform_b.w, 0.0, 0.5));
    var lfoL: f32 = phaserFlangerLfo(shape, phaseL);
    var lfoR: f32 = phaserFlangerLfo(shape, phaseR);
    var sweepL: f32 = sweep * lfoL;
    var sweepR: f32 = sweep * lfoR;
    var anchorX: f32 = input.uniform_b.x;
    var spread: f32 = clamp(input.uniform_b.y, 0.0, 1.0);
    var blend: f32 = clamp(input.uniform_b.z, 0.0, 1.0);
    var amount: f32 = clamp(input.uniform_c.x, 0.0, 1.0);
    var spreadSweep: f32 = max(input.uniform_c.y, 0.0);
    var circuit: i32 = clamp(i32(round(input.uniform_c.z)), 0, 1);

    var col: vec4<f32> = input.color_b;

    // Octave grid ticks along the log axis.
    var axisSpan: f32 = select(12.0, 10.0, (mode == 0));
    var gridT: f32 = fract(uv.x * axisSpan);
    var gridDist: f32 = min(gridT, 1.0 - gridT) / axisSpan;
    var gridAA: f32 = max(fwidth(uv.x), 0.0008);
    var gridMask: f32 = smoothstep(gridAA * 1.6, 0.0, gridDist) * 0.6;
    col = vec4<f32>((mix(col.rgb, input.color_c.rgb, gridMask * input.color_c.a)), col.a);

    var aa: f32 = max(fwidth(uv.x), 0.0008);
    if (mode == 0) {
        // BLEND routes the LFO from common CENTER motion to SPREAD motion,
        // where outer notches fan input opposite directions around the anchor.
        var yPad: f32 = smoothstep(0.04, 0.14, uv.y) * smoothstep(0.96, 0.86, uv.y);
        for (var i: i32 = 0; i < 12; i = i + 1) {
            if (i >= count) { break; }
            var offset: f32 = f32(i) - (f32(count) - 1.0) * 0.5;
            if (circuit == 0) {
                // Stack preserves the original layout and common center sweep.
                var exponentialX: f32 = anchorX + spread * offset * 1.2 / 10.0;
                var linearFactor: f32 = max(0.1, 1.0 + spread * offset * 1.6);
                var linearX: f32 = anchorX + log2(linearFactor) / 10.0;
                var baseX: f32 = clamp(mix(exponentialX, linearX, blend), 0.0, 1.0);
                var band: f32 = step(baseX - sweep, uv.x) * step(uv.x, baseX + sweep);
                col = vec4<f32>((mix(col.rgb, input.color_a.rgb, band * 0.10 * yPad)), col.a);
                var lineL: f32 = smoothstep(aa * 2.2, aa * 0.4, abs(uv.x - (baseX + sweepL)));
                var lineR: f32 = smoothstep(aa * 2.2, aa * 0.4, abs(uv.x - (baseX + sweepR)));
                col = vec4<f32>((mix(col.rgb, input.color_a.rgb, lineL * 0.92 * yPad)), col.a);
                col = vec4<f32>((mix(col.rgb, input.color_d.rgb, lineR * 0.88 * yPad)), col.a);
                continue;
            }
            var spreadL: f32 = clamp(
                spread + amount * lfoL * spreadSweep * blend,
                0.0,
                1.0
            );
            var spreadR: f32 = clamp(
                spread + amount * lfoR * spreadSweep * blend,
                0.0,
                1.0
            );
            var lineXL: f32 = clamp(
                anchorX + sweepL * (1.0 - blend) + spreadL * offset * 1.2 / 10.0,
                0.0,
                1.0
            );
            var lineXR: f32 = clamp(
                anchorX + sweepR * (1.0 - blend) + spreadR * offset * 1.2 / 10.0,
                0.0,
                1.0
            );
            var spreadNeg: f32 = clamp(spread - amount * spreadSweep * blend, 0.0, 1.0);
            var spreadPos: f32 = clamp(spread + amount * spreadSweep * blend, 0.0, 1.0);
            var rangeNeg: f32 = clamp(
                anchorX - sweep * (1.0 - blend) + spreadNeg * offset * 1.2 / 10.0,
                0.0,
                1.0
            );
            var rangePos: f32 = clamp(
                anchorX + sweep * (1.0 - blend) + spreadPos * offset * 1.2 / 10.0,
                0.0,
                1.0
            );
            var bandMin: f32 = min(rangeNeg, rangePos);
            var bandMax: f32 = max(rangeNeg, rangePos);
            var band: f32 = step(bandMin, uv.x) * step(uv.x, bandMax);
            col = vec4<f32>((mix(col.rgb, input.color_a.rgb, band * 0.10 * yPad)), col.a);
            var lineL: f32 = smoothstep(aa * 2.2, aa * 0.4, abs(uv.x - lineXL));
            var lineR: f32 = smoothstep(aa * 2.2, aa * 0.4, abs(uv.x - lineXR));
            col = vec4<f32>((mix(col.rgb, input.color_a.rgb, lineL * 0.92 * yPad)), col.a);
            col = vec4<f32>((mix(col.rgb, input.color_d.rgb, lineR * 0.88 * yPad)), col.a);
        }
    } else {
        // Two channel dots on the log-time axis with the sweep range behind.
        var baseX: f32 = anchorX;
        var dotL: vec2<f32> = vec2<f32>(baseX + sweepL, 0.38);
        var dotR: vec2<f32> = vec2<f32>(baseX + sweepR, 0.62);
        var band: f32 = step(baseX - sweep, uv.x) * step(uv.x, baseX + sweep);
        var yBand: f32 = smoothstep(0.26, 0.36, uv.y) * smoothstep(0.74, 0.64, uv.y);
        col = vec4<f32>((mix(col.rgb, input.color_a.rgb, band * 0.07 * yBand)), col.a);

        var dL: f32 = length(vec2<f32>((uv.x - dotL.x) * aspect, uv.y - dotL.y));
        var dR: f32 = length(vec2<f32>((uv.x - dotR.x) * aspect, uv.y - dotR.y));
        var rAA: f32 = max(fwidth(dL), 0.002);
        var maskL: f32 = smoothstep(0.045 + rAA, 0.045 - rAA, dL);
        var maskR: f32 = smoothstep(0.045 + rAA, 0.045 - rAA, dR);
        col = vec4<f32>((mix(col.rgb, input.color_a.rgb, maskL)), col.a);
        col = vec4<f32>((mix(col.rgb, input.color_d.rgb, maskR)), col.a);
    }
    return col;
}"#;

pub const ROAR_SHAPER_SHADER: &str = r#"
fn roarShaperCurve(shaper: i32, a: f32, x: f32) -> f32 {
    if (shaper == 1) { return clamp(x, -1.0, 1.0); }
    if (shaper == 2) {
        var levels: f32 = 64.0 + (2.0 - 64.0) * a;
        return round(x * levels) / levels;
    }
    if (shaper == 3) {
        if (x >= 0.0) {
            var t: f32 = 0.35;
            return select(t + (1.0 - exp(-(x - t) * 3.0)) / 3.0, x, (x <= t));
        }
        return 1.2 * tanh(x / 1.2);
    }
    if (shaper == 4) {
        var u: f32 = max(x, -2.4);
        return tanh(u + 0.2 * u * u);
    }
    if (shaper == 5) { return 2.0 * max(x, 0.0); }
    if (shaper == 6) { return abs(x); }
    if (shaper == 7) {
        var cheb: f32 = (3.0 * x - 4.0 * x * x * x) / 3.0;
        return clamp((1.0 - a) * x + a * cheb, -1.0, 1.0);
    }
    if (shaper == 8) {
        var y: f32 = x;
        y = sin(1.9 * y) / 1.9;
        y = sin(1.5 * y) / 1.5;
        y = sin(1.2 * y) / 1.2;
        return y;
    }
    if (shaper == 9) {
        var t: f32 = fract((x + 1.0) * 0.25);
        return 1.0 - 4.0 * abs(t - 0.5);
    }
    return sin(clamp(x, -1.5707963, 1.5707963));
}

@fragment
fn widget_frag(input: WidgetVaryings) -> @location(0) vec4<f32>
{
    var uv: vec2<f32> = input.uv;
    var shaper: i32 = clamp(i32(round(input.value_t)), 0, 11);
    var amount: f32 = clamp(input.uniform_a.x, 0.0, 1.0);
    var bias: f32 = clamp(input.uniform_a.y, -1.0, 1.0);
    var driveMin: f32 = clamp(input.uniform_a.z, -2.0, 2.0);
    var driveMax: f32 = clamp(input.uniform_a.w, -2.0, 2.0);

    var x: f32 = (uv.x * 2.0 - 1.0) * 1.5; // stage-input domain -1.5..1.5
    var col: vec4<f32> = input.color_b;

    // Axis grid: center lines and ±1 ticks.
    var aa: f32 = max(fwidth(uv.x), 0.0008);
    var axisX: f32 = smoothstep(aa * 1.8, 0.0, abs(uv.x - 0.5));
    var axisY: f32 = smoothstep(aa * 1.8, 0.0, abs(uv.y - 0.5));
    var tick1: f32 = smoothstep(aa * 1.4, 0.0, abs(abs(x) - 1.0));
    var grid: f32 = max(max(axisX, axisY) * 0.8, tick1 * 0.4);
    col = vec4<f32>((mix(col.rgb, input.color_c.rgb, grid * input.color_c.a)), col.a);

    // Live drive region: the input span currently exercising the curve.
    if (driveMax > driveMin + 0.001) {
        var band: f32 = step(driveMin, x) * step(x, driveMax);
        col = vec4<f32>((mix(col.rgb, input.color_a.rgb, band * 0.14)), col.a);
    }

    // Dashed marker at the curve's shifted center (shaper input = 0).
    var gain: f32 = exp2(amount * 6.0);
    var biasU: f32 = clamp((-bias / gain) / 3.0 + 0.5, 0.0, 1.0);
    var dash: f32 = step(0.5, fract(uv.y * 9.0));
    var biasLine: f32 = smoothstep(aa * 1.8, aa * 0.3, abs(uv.x - biasU)) * dash;
    col = vec4<f32>((mix(col.rgb, input.color_d.rgb, biasLine * input.color_d.a)), col.a);

    // Composite transfer curve (gain + bias applied), y input -1.4..1.4.
    var y: f32 = roarShaperCurve(shaper, amount, gain * x + bias);
    var curveV: f32 = 0.5 - y / 2.8;
    var aaY: f32 = max(fwidth(uv.y), 0.0015);
    var line: f32 = smoothstep(aaY * 2.6, aaY * 0.5, abs(uv.y - curveV));
    col = vec4<f32>((mix(col.rgb, input.color_a.rgb, line * 0.95)), col.a);
    return col;
}"#;

pub const KNOB_NUMBER_SHADER: &str = r#"
@fragment
fn widget_frag(input: WidgetVaryings) -> @location(0) vec4<f32>
{
    var uv: vec2<f32> = input.uv;
    var p: vec2<f32> = vec2<f32>((uv.x - 0.5) * 2.0, (uv.y - 0.5) * 2.0);
    var r: f32 = length(p);
    var a: f32 = atan2(p.y, p.x);

    var start: f32 = 1.57079633;
    var sweep: f32 = 4.71238898;
    var rel: f32 = ((a - start + 6.2831853) % 6.2831853);
    var inRange: f32 = step(rel, sweep);
    var valueRel: f32 = sweep * clamp(input.value_t, 0.0, 1.0);
    var originRel: f32 = sweep * clamp(input.uniform_a.w, 0.0, 1.0);
    var fillLo: f32 = min(valueRel, originRel);
    var fillHi: f32 = max(valueRel, originRel);
    var fillSpan: f32 = fillHi - fillLo;
    var is_active: f32 = step(fillLo, rel) * step(rel, fillHi) * step(0.001, fillSpan);

    var knobRadius: f32 = 0.64;
    var ring: f32 = abs(r - knobRadius) - 0.070;
    var activeRing: f32 = abs(r - knobRadius) - 0.082;
    var aa: f32 = max(fwidth(r), 0.0015);
    var ringMask: f32 = smoothstep(aa, -aa, ring) * inRange;
    var activeMask: f32 = smoothstep(aa, -aa, activeRing) * inRange * is_active;
    var glowRing: f32 = abs(r - knobRadius) - 0.150;
    var glowMask: f32 = smoothstep(aa * 4.0, -aa * 4.0, glowRing) * inRange * is_active * step(0.5, input.uniform_a.y);
    var trackMask: f32 = ringMask * (1.0 - is_active);

    var notchAngle: f32 = start + valueRel;
    var n: vec2<f32> = vec2<f32>(cos(notchAngle), sin(notchAngle));
    var notch: f32 = length(p - n * knobRadius) - 0.070;
    var notchMask: f32 = smoothstep(aa, -aa, notch);
    var lineAlong: f32 = dot(p, n);
    var lineAcross: f32 = abs(p.x * n.y - p.y * n.x);
    var lineSegment: f32 = step(0.0, lineAlong) * step(lineAlong, 0.58);
    var line: f32 = lineAcross - 0.070;
    var lineMask: f32 = smoothstep(aa, -aa, line) * lineSegment;
    var defaultAngle: f32 = start + sweep * clamp(input.uniform_a.z, 0.0, 1.0);
    var dn: vec2<f32> = vec2<f32>(cos(defaultAngle), sin(defaultAngle));
    var defaultNotch: f32 = length(p - dn * knobRadius) - 0.046;
    var defaultMask: f32 = smoothstep(aa, -aa, defaultNotch)
        * step(0.5, input.uniform_a.y)
        * step(0.01, abs(input.uniform_a.z - input.value_t));

    var col: vec4<f32> = vec4<f32>(0.0);
    col = mix(col, vec4<f32>(input.color_d.rgb, 0.20), glowMask);
    col = mix(col, input.color_b, trackMask);
    col = mix(col, input.color_a, activeMask);
    col = mix(col, input.color_b, lineMask);
    col = mix(col, input.color_a, notchMask);
    col = mix(col, vec4<f32>(0.36, 0.36, 0.41, 0.95), defaultMask);
    if (col.a < 0.01) { discard; }
    return col;
}"#;

pub const KNOB_NUMBER_MOD_DOT_SHADER: &str = r#"
@fragment
fn widget_frag(input: WidgetVaryings) -> @location(0) vec4<f32>
{
    var uv: vec2<f32> = input.uv;
    var p: vec2<f32> = vec2<f32>((uv.x - 0.5) * 2.0, (uv.y - 0.5) * 2.0);
    var r: f32 = length(p);

    var start: f32 = 1.57079633;
    var sweep: f32 = 4.71238898;
    var t: f32 = clamp(input.uniform_b.x, 0.0, 1.0);
    var ringRadius: f32 = clamp(input.uniform_b.y, 0.10, 1.0);
    var dotRadius: f32 = clamp(input.uniform_b.z, 0.005, 0.40);
    var angle: f32 = start + sweep * t;
    var n: vec2<f32> = vec2<f32>(cos(angle), sin(angle));
    var aa: f32 = max(fwidth(r), 0.0015);
    var d: f32 = length(p - n * ringRadius) - dotRadius;
    var mask: f32 = smoothstep(aa, -aa, d);
    var col: vec4<f32> = vec4<f32>(input.color_a.rgb, input.color_a.a * mask);
    if (col.a < 0.01) { discard; }
    return col;
}"#;

pub const KNOB_NUMBER_MOD_RANGE_SHADER: &str = r#"
@fragment
fn widget_frag(input: WidgetVaryings) -> @location(0) vec4<f32>
{
    var uv: vec2<f32> = input.uv;
    var p: vec2<f32> = vec2<f32>((uv.x - 0.5) * 2.0, (uv.y - 0.5) * 2.0);
    var r: f32 = length(p);
    var a: f32 = atan2(p.y, p.x);

    var start: f32 = 1.57079633;
    var sweep: f32 = 4.71238898;
    var rel: f32 = ((a - start + 6.2831853) % 6.2831853);
    var inRange: f32 = step(rel, sweep);
    var aa: f32 = max(fwidth(r), 0.0015);

    var ringRadius: f32 = clamp(input.uniform_b.x, 0.62, 1.02);
    var t0: f32 = clamp(input.uniform_b.y, 0.0, 1.0);
    var t1: f32 = clamp(input.uniform_b.z, 0.0, 1.0);
    var lo: f32 = min(t0, t1) * sweep;
    var hi: f32 = max(t0, t1) * sweep;
    var selected: f32 = step(0.5, input.uniform_b.w);
    var radius: f32 = ringRadius;
    var halfWidth: f32 = mix(0.040, 0.056, selected);
    var modRing: f32 = abs(r - radius) - halfWidth;
    var arcMask: f32 = step(lo, rel) * step(rel, hi) * inRange;
    var mask: f32 = smoothstep(aa, -aa, modRing) * arcMask;
    var col: vec4<f32> = vec4<f32>(input.color_a.rgb, input.color_a.a * mask);
    if (col.a < 0.01) { discard; }
    return col;
}"#;

pub const ROAR_FILTER_SHADER: &str = r#"
fn roarFilterMagnitude(filterType: i32, cutoff: f32, res: f32, freq: f32) -> f32 {
    var omega: f32 = freq / max(cutoff, 20.0);
    var q: f32 = 0.5 * pow(24.0, clamp(res, 0.0, 1.0));
    var k: f32 = 1.0 / q;
    var one: f32 = 1.0 - omega * omega;
    var denom: f32 = max(sqrt(one * one + k * k * omega * omega), 1.0e-6);
    if (filterType == 1) { return omega / denom; }
    if (filterType == 2) { return omega * omega / denom; }
    if (filterType == 3) { return abs(one) / denom; }
    if (filterType == 4) { return (1.0 + omega * omega) / denom; }
    if (filterType == 5) {
        var re: f32 = 0.3 * one;
        var im: f32 = 0.4 * k * omega;
        return sqrt(re * re + im * im) / denom;
    }
    if (filterType == 6) {
        var theta: f32 = 6.28318530718 * freq / max(cutoff, 20.0);
        var fb: f32 = res * 0.9;
        var num: f32 = 0.5 * sqrt(max(2.0 + 2.0 * cos(theta), 0.0));
        var den: f32 = sqrt(max(1.0 + fb * fb - 2.0 * fb * cos(theta), 1.0e-6));
        return num / den;
    }
    if (filterType == 7) {
        if (cutoff >= 15900.0) { return 1.0; }
        var t: f32 = min(3.14159265 * freq / cutoff, 3.14159265);
        return select(abs(sin(t) / t), 1.0, (t < 1.0e-3));
    }
    if (filterType == 8) { return 1.0; }
    return 1.0 / denom;
}

@fragment
fn widget_frag(input: WidgetVaryings) -> @location(0) vec4<f32>
{
    var uv: vec2<f32> = input.uv;
    var filterType: i32 = clamp(i32(round(input.value_t)), 0, 8);
    var cutoff: f32 = clamp(input.uniform_a.x, 20.0, 16000.0);
    var res: f32 = clamp(input.uniform_a.y, 0.0, 1.0);

    var col: vec4<f32> = input.color_b;

    // Octave grid + 0 dB line.
    var gridT: f32 = fract(uv.x * 10.0);
    var gridDist: f32 = min(gridT, 1.0 - gridT) / 10.0;
    var aa: f32 = max(fwidth(uv.x), 0.0008);
    var gridMask: f32 = smoothstep(aa * 1.6, 0.0, gridDist) * 0.5;
    var zeroLine: f32 = smoothstep(aa * 1.6, 0.0, abs(uv.y - 0.5)) * 0.7;
    col = vec4<f32>((mix(col.rgb, input.color_c.rgb, max(gridMask, zeroLine) * input.color_c.a)), col.a);

    // Response curve: -24..+24 dB across the height.
    var freq: f32 = 20.0 * exp2(uv.x * 10.0);
    var mag: f32 = roarFilterMagnitude(filterType, cutoff, res, freq);
    var db: f32 = clamp(20.0 * (log2(max(mag, 1.0e-6)) * 0.30102999566), -24.0, 24.0);
    var curveV: f32 = (24.0 - db) / 48.0;
    var aaY: f32 = max(fwidth(uv.y), 0.0015);
    var line: f32 = smoothstep(aaY * 2.6, aaY * 0.5, abs(uv.y - curveV));
    col = vec4<f32>((mix(col.rgb, input.color_a.rgb, line * 0.95)), col.a);
    // Soft fill below the curve.
    var fill: f32 = step(curveV, uv.y) * 0.10;
    col = vec4<f32>((mix(col.rgb, input.color_a.rgb, fill)), col.a);
    return col;
}"#;

pub const NUMBER_PICKER_SLIDER_SHADER: &str = r#"
fn number_picker_slider_rounded_rect(p: vec2<f32>, size: vec2<f32>, radius: f32) -> f32 {
    var q: vec2<f32> = abs(p) - (size - vec2<f32>(radius));
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

@fragment
fn widget_frag(input: WidgetVaryings) -> @location(0) vec4<f32>
{
    var aspect: f32 = max(input.aspect, 0.001);
    var p: vec2<f32> = vec2<f32>((input.uv.x - 0.5) * 2.0 * aspect, (input.uv.y - 0.5) * 2.0);
    var size: vec2<f32> = vec2<f32>(aspect, 1.0);
    var radius: f32 = min(input.corner_radius, min(aspect, 1.0));

    var outer_distance: f32 = number_picker_slider_rounded_rect(p, size, radius);
    var outer_edge: f32 = fwidth(outer_distance) * 1.2;
    var outer_mask: f32 = smoothstep(outer_edge, -outer_edge, outer_distance);

    var px: f32 = max(max(fwidth(p.x), fwidth(p.y)), 0.001);
    var border_width: f32 = input.uniform_a.x * px;
    var inner_size: vec2<f32> = max(size - vec2<f32>(border_width), vec2<f32>(0.001));
    var inner_distance: f32 = number_picker_slider_rounded_rect(
        p,
        inner_size,
        max(radius - border_width, 0.0));
    var inner_edge: f32 = fwidth(inner_distance) * 1.2;
    var inner_mask: f32 = smoothstep(inner_edge, -inner_edge, inner_distance);
    var border_mask: f32 = clamp(outer_mask - inner_mask, 0.0, 1.0);

    var cutoff_edge: f32 = max(fwidth(input.uv.x), 0.0005);
    var segment_start: f32 = min(input.value_t, input.uniform_a.y);
    var segment_end: f32 = max(input.value_t, input.uniform_a.y);
    var left_mask: f32 = smoothstep(
        segment_start - cutoff_edge,
        segment_start + cutoff_edge,
        input.uv.x);
    var right_mask: f32 = smoothstep(
        segment_end + cutoff_edge,
        segment_end - cutoff_edge,
        input.uv.x);
    var segment_mask: f32 = left_mask * right_mask
        * step(0.0001, abs(input.value_t - input.uniform_a.y));
    var fill_alpha: f32 = input.color_c.a * segment_mask;
    var inner_alpha: f32 = fill_alpha + input.color_a.a * (1.0 - fill_alpha);
    var inner_rgb: vec3<f32> = (
        input.color_c.rgb * fill_alpha
        + input.color_a.rgb * input.color_a.a * (1.0 - fill_alpha)
    ) / max(inner_alpha, 0.0001);

    var surface_alpha: f32 = inner_alpha * inner_mask;
    var border_alpha: f32 = input.color_b.a * border_mask;
    var out_alpha: f32 = border_alpha + surface_alpha * (1.0 - border_alpha);
    if (out_alpha < 0.002) { discard; }
    var out_rgb: vec3<f32> = (
        input.color_b.rgb * border_alpha
        + inner_rgb * surface_alpha * (1.0 - border_alpha)
    ) / out_alpha;
    return vec4<f32>(out_rgb, out_alpha);
}
"#;

pub const NUMBER_PICKER_TRI_SHADER: &str = r#"
fn number_picker_segment_distance(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    var pa: vec2<f32> = p - a;
    var ba: vec2<f32> = b - a;
    var h: f32 = clamp(dot(pa, ba) / max(dot(ba, ba), 0.0001), 0.0, 1.0);
    return length(pa - ba * h);
}

@fragment
fn widget_frag(input: WidgetVaryings) -> @location(0) vec4<f32>
{
    var uv: vec2<f32> = input.uv;
    var aspect: f32 = input.aspect;
    var col: vec4<f32> = input.color_a;

    // Aspect-corrected coordinates
    var p: vec2<f32> = vec2<f32>((uv.x - 0.5) * 2.0 * aspect, (uv.y - 0.5) * 2.0);

    // Right-pointing filled triangle (play button style)
    // Vertices: left-top (-0.5a, -0.7), left-bottom (-0.5a, 0.7), right (0.6a, 0)
    var a: vec2<f32> = vec2<f32>(-0.5 * aspect, -0.7);
    var b: vec2<f32> = vec2<f32>(-0.5 * aspect,  0.7);
    var c: vec2<f32> = vec2<f32>( 0.6 * aspect,  0.0);

    var d1: f32 = (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
    var d2: f32 = (c.x - b.x) * (p.y - b.y) - (c.y - b.y) * (p.x - b.x);
    var d3: f32 = (a.x - c.x) * (p.y - c.y) - (a.y - c.y) * (p.x - c.x);

    var has_neg: bool = (d1 < 0.0) || (d2 < 0.0) || (d3 < 0.0);
    var has_pos: bool = (d1 > 0.0) || (d2 > 0.0) || (d3 > 0.0);
    var inside: bool = !(has_neg && has_pos);
    var edge_distance: f32 = min(
        number_picker_segment_distance(p, a, b),
        min(number_picker_segment_distance(p, b, c), number_picker_segment_distance(p, c, a))
    );
    var signed_distance: f32 = select(edge_distance, -edge_distance, inside);
    var aa: f32 = max(fwidth(signed_distance), 0.001) * 1.35;
    var mask: f32 = smoothstep(aa, -aa, signed_distance);

    if (mask < 0.002) { discard; }
    return vec4<f32>(col.rgb, col.a * mask);
}"#;

pub const TILE_CHROME_SHADER: &str = r#"
@fragment
fn widget_frag(input: WidgetVaryings) -> @location(0) vec4<f32>
{
    var aspect: f32 = max(input.aspect, 0.0001);
    var p: vec2<f32> = vec2<f32>((input.uv.x - 0.5) * 2.0 * aspect, (input.uv.y - 0.5) * 2.0);

    var radius: f32 = clamp(input.corner_radius, 0.0, min(aspect, 1.0));
    var half_size: vec2<f32> = vec2<f32>(aspect, 1.0);
    var d: f32 = sdf_rounded_rect(p, half_size, radius);
    // Isotropic pixel size: fwidth(d) grows up to ~1.41x where the corner arc
    // runs diagonally, fattening the stroke there; fwidth(p) does not.
    var pixel: f32 = max(max(fwidth(p.x), fwidth(p.y)), 0.001);
    var outer_mask: f32 = smoothstep(pixel, -pixel, d);

    var border_px: f32 = max(input.uniform_a.x, 0.0);
    var border_mask: f32 = 0.0;
    var fill_mask: f32 = outer_mask;
    if (border_px > 0.0) {
        // Euclidean SDF: the inner contour is the outer one offset inward,
        // uniform thickness by construction.
        var inner_d: f32 = d + border_px * pixel;
        var inner_mask: f32 = smoothstep(pixel, -pixel, inner_d);
        border_mask = clamp(outer_mask - inner_mask, 0.0, 1.0);
        fill_mask = inner_mask;
    }

    var fill: vec4<f32> = vec4<f32>(input.color_a.rgb, input.color_a.a * fill_mask);
    var border: vec4<f32> = vec4<f32>(input.color_b.rgb, input.color_b.a * border_mask);
    var out_alpha: f32 = fill.a + border.a * (1.0 - fill.a);
    if (out_alpha <= 0.002) {
        discard;
    }
    var out_rgb: vec3<f32> = (fill.rgb * fill.a + border.rgb * border.a * (1.0 - fill.a)) / out_alpha;
    return vec4<f32>(out_rgb, out_alpha);
}"#;

pub const TILE_TAB_SHADER: &str = r#"
@fragment
fn widget_frag(input: WidgetVaryings) -> @location(0) vec4<f32>
{
    var p: vec2<f32> = vec2<f32>((input.uv.x - 0.5) * 2.0 * input.aspect, (input.uv.y - 0.5) * 2.0);

    var r: f32 = select(0.75, input.corner_radius, input.corner_radius > 0.0);
    r = min(r, min(input.aspect, 1.0));
    var half_size: vec2<f32> = vec2<f32>(input.aspect - r, 1.0 - r);
    var q: vec2<f32> = abs(p) - half_size;
    var d: f32 = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;

    // Isotropic pixel size (fwidth(d) fattens the stroke on corner arcs).
    var pixel: f32 = max(max(fwidth(p.x), fwidth(p.y)), 0.001);
    var edge: f32 = pixel * 1.2;
    var mask: f32 = smoothstep(edge, -edge, d);
    if (mask < 0.002) { discard; }

    var border_px: f32 = select(0.0, 1.25, input.value_t > 0.5);
    var inner_d: f32 = d + border_px * pixel;
    var inner_mask: f32 = smoothstep(edge, -edge, inner_d);
    var border_mask: f32 = clamp(mask - inner_mask, 0.0, 1.0);

    var top_light: f32 = smoothstep(1.0, 0.10, input.uv.y);
    var bottom_shadow: f32 = smoothstep(0.35, 1.0, input.uv.y);
    var fill_lit: vec3<f32> = input.color_a.rgb;
    fill_lit = mix(fill_lit, input.color_c.rgb, input.color_c.a * top_light * 0.05);
    fill_lit = mix(fill_lit, input.color_d.rgb, input.color_d.a * bottom_shadow * 0.05);
    var border_lit: vec3<f32> = input.color_b.rgb;
    border_lit = mix(border_lit, input.color_c.rgb, input.color_c.a * top_light);
    border_lit = mix(border_lit, input.color_d.rgb, input.color_d.a * bottom_shadow);

    var fill: vec4<f32> = vec4<f32>(fill_lit, input.color_a.a * inner_mask);
    var border: vec4<f32> = vec4<f32>(border_lit, input.color_b.a * border_mask);
    var out_alpha: f32 = fill.a + border.a * (1.0 - fill.a);
    if (out_alpha <= 0.002) { discard; }
    var out_rgb: vec3<f32> = (fill.rgb * fill.a + border.rgb * border.a * (1.0 - fill.a)) / out_alpha;
    return vec4<f32>(out_rgb, out_alpha);
}"#;

pub const ROUNDED_RECT_SHADER: &str = r#"
@fragment
fn widget_frag(input: WidgetVaryings) -> @location(0) vec4<f32>
{
    var uv: vec2<f32> = input.uv;
    var aspect: f32 = input.aspect;
    var col: vec4<f32> = input.color_a;

    var p: vec2<f32> = vec2<f32>((uv.x - 0.5) * 2.0 * aspect, (uv.y - 0.5) * 2.0);

    var r: f32 = select(0.75, input.corner_radius, input.corner_radius > 0.0);
    r = min(r, min(aspect, 1.0));
    var half_size: vec2<f32> = vec2<f32>(aspect - r, 1.0 - r);
    var q: vec2<f32> = abs(p) - half_size;
    var d: f32 = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;

    var edge: f32 = fwidth(d) * 1.2;
    var mask: f32 = smoothstep(edge, -edge, d);

    if (mask < 0.002) { discard; }
    return vec4<f32>(col.rgb, col.a * mask);
}"#;

pub const PATCHER_PANEL_SHADER: &str = r#"
@fragment
fn widget_frag(input: WidgetVaryings) -> @location(0) vec4<f32>
{
    var aspect: f32 = max(input.aspect, 0.001);
    var localPos: vec2<f32> = vec2<f32>((input.uv.x - 0.5) * 2.0 * aspect, (input.uv.y - 0.5) * 2.0);
    var sdfSize: vec2<f32> = vec2<f32>(aspect, 1.0);
    var cornerRadius: f32 = min(input.corner_radius, min(aspect, 1.0));

    var dist: f32 = sdf_rounded_rect(localPos, sdfSize, cornerRadius);
    // Isotropic pixel size: fwidth(dist) grows up to ~1.41x where the corner
    // arc runs diagonally, fattening the stroke there.
    var pixel: f32 = max(max(fwidth(localPos.x), fwidth(localPos.y)), 0.001);
    var outerAlpha: f32 = smoothstep(pixel, -pixel, dist);
    if (outerAlpha <= 0.001) {
        discard;
    }

    // Euclidean SDF: the inner contour is the outer one offset inward,
    // uniform thickness by construction.
    var innerDist: f32 = dist + max(input.uniform_a.x, 0.0) * pixel;
    var innerAlpha: f32 = smoothstep(pixel, -pixel, innerDist);
    var borderMask: f32 = outerAlpha * (1.0 - innerAlpha);

    var color: vec3<f32> = mix(input.color_b.rgb, input.color_a.rgb, borderMask);
    var alpha: f32 = mix(input.color_b.a, input.color_a.a, borderMask);
    return vec4<f32>(color, alpha * outerAlpha);
}"#;

pub const PATCHER_PORT_SHADER: &str = r#"
@fragment
fn widget_frag(input: WidgetVaryings) -> @location(0) vec4<f32>
{
    var p: vec2<f32> = (input.uv - vec2<f32>(0.5)) * 2.0;

    if ((input.value_t > 0.0 && p.y < 0.0) || (input.value_t < 0.0 && p.y > 0.0)) {
        discard;
    }

    var d: f32 = length(p);
    var aa: f32 = max(fwidth(d), 0.001);
    var outerMask: f32 = 1.0 - smoothstep(1.0 - aa, 1.0 + aa, d);
    if (outerMask < 0.002) {
        discard;
    }

    var innerRadius: f32 = clamp(input.uniform_a.x, 0.0, 0.98);
    var innerMask: f32 = 1.0 - smoothstep(innerRadius - aa, innerRadius + aa, d);
    var col: vec4<f32> = mix(input.color_a, input.color_b, innerMask);
    return vec4<f32>(col.rgb, col.a * outerMask);
}"#;

pub const PATCHER_BACK_CHEVRON_SHADER: &str = r#"
fn patcher_chevron_segment_distance(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    var pa: vec2<f32> = p - a;
    var ba: vec2<f32> = b - a;
    var h: f32 = clamp(dot(pa, ba) / max(dot(ba, ba), 0.0001), 0.0, 1.0);
    return length(pa - ba * h);
}

@fragment
fn widget_frag(input: WidgetVaryings) -> @location(0) vec4<f32>
{
    var aspect: f32 = max(input.aspect, 0.001);
    var p: vec2<f32> = vec2<f32>(input.uv.x * aspect, input.uv.y);
    var scale: f32 = min(aspect, 1.0);
    var center_x: f32 = aspect * 0.5;
    var tip: vec2<f32> = vec2<f32>(center_x - 0.18 * scale, 0.50);
    var upper: vec2<f32> = vec2<f32>(center_x + 0.18 * scale, 0.25);
    var lower: vec2<f32> = vec2<f32>(center_x + 0.18 * scale, 0.75);

    var d: f32 = min(
        patcher_chevron_segment_distance(p, upper, tip),
        patcher_chevron_segment_distance(p, tip, lower));
    var thickness: f32 = 0.055 * scale;
    var aa: f32 = max(fwidth(d), 0.001);
    var mask: f32 = smoothstep(thickness + aa, thickness - aa, d);
    if (mask < 0.002) {
        discard;
    }

    return vec4<f32>(input.color_a.rgb, input.color_a.a * mask);
}"#;

pub const PATCHER_NODE_SHADER: &str = r#"
fn patcher_node_smooth_rounded_rect(pos: vec2<f32>, size: vec2<f32>, radius: f32, smin: f32, smax: f32) -> f32 {
    return smoothstep(smin, smax, sdf_rounded_rect(pos, size, radius));
}

fn patcher_node_normal(pos: vec2<f32>, size: vec2<f32>, radius: f32, eps: f32, ratio: f32) -> vec3<f32> {
    var smin: f32 = -0.1 * ratio;
    var smax: f32 = 1.118;
    var right: f32 = patcher_node_smooth_rounded_rect(pos + vec2<f32>(eps, 0.0), size, radius, smin, smax);
    var left: f32 = patcher_node_smooth_rounded_rect(pos - vec2<f32>(eps, 0.0), size, radius, smin, smax);
    var up: f32 = patcher_node_smooth_rounded_rect(pos + vec2<f32>(0.0, eps), size, radius, smin, smax);
    var down: f32 = patcher_node_smooth_rounded_rect(pos - vec2<f32>(0.0, eps), size, radius, smin, smax);
    return normalize(vec3<f32>((right - left) / (2.0 * eps), (up - down) / (2.0 * eps), 1.0));
}

@fragment
fn widget_frag(input: WidgetVaryings) -> @location(0) vec4<f32>
{
    var aspect: f32 = max(input.aspect, 0.001);
    var localPos: vec2<f32> = vec2<f32>((input.uv.x - 0.5) * 2.0 * aspect, (input.uv.y - 0.5) * 2.0);
    var sdfSize: vec2<f32> = vec2<f32>(aspect, 1.0);
    var cornerRadius: f32 = min(input.corner_radius * 1.5, min(aspect, 1.0));

    var nodeDist: f32 = sdf_rounded_rect(localPos, sdfSize, cornerRadius);
    // Isotropic pixel size input local units. fwidth(nodeDist) would grow with the
    // gradient direction (up to ~1.41x on the 45 degree stretch of a corner),
    // which fattens the stroke around the curves; fwidth(localPos) does not.
    var pixel: f32 = max(max(fwidth(localPos.x), fwidth(localPos.y)), 0.0001);
    var outerAlpha: f32 = smoothstep(pixel, -pixel, nodeDist);
    if (outerAlpha <= 0.001) {
        discard;
    }

    // sdf_rounded_rect is a true euclidean distance, so the inner contour is
    // just the outer one offset inward - uniform thickness by construction.
    var borderThickness: f32 = max(input.uniform_a.x, 0.0) * pixel;
    var innerDist: f32 = nodeDist + borderThickness;
    var innerAlpha: f32 = smoothstep(pixel, -pixel, innerDist);
    var borderMask: f32 = clamp(outerAlpha - innerAlpha, 0.0, 1.0);

    var normal: vec3<f32> = patcher_node_normal(
        localPos,
        sdfSize,
        cornerRadius,
        0.01,
        0.83 / max(min(aspect, 1.0), 0.001));
    var viewDir: vec3<f32> = vec3<f32>(0.0, 0.0, 1.0);
    var lightDir: vec3<f32> = normalize(vec3<f32>(-0.9, -0.9, 1.3));
    var diffuse: f32 = max(0.0, dot(normal, lightDir));
    var halfVector: vec3<f32> = normalize(lightDir + viewDir);
    var specularRaw: f32 = pow(max(0.0, dot(normal, halfVector)), 48.0);
    var specularFadeDistance: f32 = clamp(pixel * 2.5, 0.01, 0.06);
    var specular: f32 = specularRaw * smoothstep(0.0, -specularFadeDistance, nodeDist);

    var bg: vec3<f32> = input.color_b.rgb;
    var border: vec3<f32> = input.color_a.rgb;
    var litBg: vec3<f32> = bg * (0.82 + 0.18 * diffuse) + vec3<f32>(0.20) * specular;
    var litBorder: vec3<f32> = border * (0.76 + 0.24 * diffuse) + vec3<f32>(0.55) * specular;

    var edgeShade: f32 = smoothstep(0.18, 0.98, localPos.y * 0.5 + 0.5);
    litBg = litBg * mix(0.94, 1.04, edgeShade);
    litBorder = litBorder * mix(0.88, 1.12, edgeShade);

    // Flatness dials the whole fake-3d treatment out: the bevel's diffuse, the
    // specular, and the vertical edge shade all land input litBg/litBorder, so
    // mixing back to the raw colours at 1.0 leaves a flat card with a clean SDF
    // edge. A node wants the shading - at pill size it is most of what gives
    // the node its physicality - but on a surface as large as an agentic
    // bubble the same treatment reads as a smudge near the border.
    var flatness: f32 = clamp(input.uniform_a.y, 0.0, 1.0);
    litBg = mix(litBg, bg, flatness);
    litBorder = mix(litBorder, border, flatness);

    var color: vec3<f32> = mix(litBg, litBorder, borderMask);
    return vec4<f32>(color, outerAlpha * max(input.color_a.a, input.color_b.a));
}"#;
