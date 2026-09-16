//! Geometric distances and pixel-sized strokes for both shader backends.
//! Ellipse solver reference and CPU counterpart: `lang/sdf_geometry.rs`.

use super::{CodegenError, Expression, ShaderEmitter, ShaderLanguage};

impl ShaderEmitter {
    pub(super) fn emit_ellipse(&mut self, args: &[Expression]) -> Result<String, CodegenError> {
        if args.len() != 2 {
            return Err(CodegenError::UnsupportedExpression("sdf/ellipse requires (radius-x radius-y)".into()));
        }
        let rx = self.emit_expr(&args[0])?;
        let ry = self.emit_expr(&args[1])?;
        let point = format!("abs({}({}, {}))", self.constructor("float2"),
            self.resolve_symbol("x"), self.resolve_symbol("y"));
        let radii = format!("abs({}({}, {}))", self.constructor("float2"), rx, ry);
        let p = self.fresh_var();
        let r = self.fresh_var();
        let result = self.fresh_var();
        self.statements.push(self.mutable_declaration("float2", &p, &point));
        self.statements.push(self.mutable_declaration("float2", &r, &radii));
        self.statements.push(self.mutable_declaration("float", &result, "0.0"));
        // Local names are scoped to this block; nested ellipse arguments were
        // evaluated above. Keep this solver aligned with sdf_geometry.rs.
        self.statements.push("{".into());
        self.statements.push(format!("if ({r}.x < {r}.y) {{ {r} = {r}.yx; {p} = {p}.yx; }}"));
        self.statements.push(format!("if ({r}.y == 0.0) {{"));
        self.statements.push(format!("{result} = length({}(max({p}.x - {r}.x, 0.0), {p}.y));", self.constructor("float2")));
        self.statements.push(format!("}} else if ({r}.x == {r}.y) {{ {result} = length({p}) - {r}.x; }} else {{"));
        self.statements.push(self.declaration("float", "gap", &format!("({r}.x - {r}.y) * ({r}.x + {r}.y)")));
        self.statements.push(self.mutable_declaration("float2", "closest", &format!("{}(0.0, {r}.y)", self.constructor("float2"))));
        self.statements.push(format!("if ({p}.y == 0.0) {{"));
        self.statements.push(format!("if ({r}.x * {p}.x < gap) {{"));
        self.statements.push(self.declaration("float", "unit_x", &format!("{r}.x * {p}.x / gap")));
        self.statements.push(format!("closest = {}({r}.x * unit_x, {r}.y * sqrt(max(0.0, 1.0 - unit_x * unit_x)));", self.constructor("float2")));
        self.statements.push(format!("}} else {{ closest = {}({r}.x, 0.0); }}", self.constructor("float2")));
        self.statements.push(format!("}} else if ({p}.x > 0.0) {{"));
        self.statements.push(self.declaration("float2", "numerator", &format!("{r} * {p}")));
        self.statements.push(self.declaration("float2", "unit_p", &format!("{p} / {r}")));
        self.statements.push(self.mutable_declaration("float", "lo", "1.0"));
        self.statements.push(self.mutable_declaration("float", "hi", "length(numerator) / numerator.y"));
        self.statements.push(format!("if (dot(unit_p, unit_p) < 1.0) {{ hi = {r}.y / {p}.y; }} else {{ lo = max(1.0, {r}.y / {p}.y); }}"));
        // k = (t + minor_radius^2) / numerator.y is at least one. Scaling
        // the root avoids tiny squared intermediates in GPU fast arithmetic.
        self.statements.push(self.mutable_declaration("float", "k", "lo"));
        self.statements.push(match self.language {
            ShaderLanguage::Metal => "for (int iteration = 0; iteration < 48; iteration++) {".into(),
            ShaderLanguage::Wgsl => "for (var iteration = 0; iteration < 48; iteration++) {".into(),
        });
        self.statements.push("k = clamp(lo * sqrt(hi / lo), lo, hi);".into());
        self.statements.push("if (k == lo || k == hi) { break; }".into());
        self.statements.push(self.declaration("float2", "candidate", &format!("{}(numerator.x / (numerator.y * k + gap), 1.0 / k)", self.constructor("float2"))));
        self.statements.push("if (dot(candidate, candidate) > 1.0) { lo = k; } else { hi = k; }".into());
        self.statements.push("}".into());
        self.statements.push(format!("closest = {r} * {}(numerator.x / (numerator.y * k + gap), 1.0 / k);", self.constructor("float2")));
        self.statements.push("}".into());
        self.statements.push(format!("{result} = length({p} - closest);"));
        self.statements.push(format!("if (dot({p} / {r}, {p} / {r}) < 1.0) {{ {result} = -{result}; }}"));
        self.statements.push("}".into());
        self.statements.push("}".into());
        Ok(result)
    }

    pub(super) fn emit_pixel_stroke(&mut self, args: &[Expression]) -> Result<String, CodegenError> {
        if args.len() != 3 {
            return Err(CodegenError::UnsupportedExpression("sdf/stroke-px requires (distance full-width-px color)".into()));
        }
        // The unshadowed shader y is in isotropic drawing units. Capture its
        // pixel size before evaluating geometry with branches or transforms.
        let pixel = self.fresh_var();
        self.statements.push(self.declaration("float", &pixel, "max(fwidth(y), 0.000001)"));
        let distance = self.emit_expr(&args[0])?;
        let width = self.emit_expr(&args[1])?;
        let color = self.emit_expr(&args[2])?;
        let d = self.fresh_var();
        let half = self.fresh_var();
        let coverage = self.fresh_var();
        let clr = self.fresh_var();
        self.statements.push(self.declaration("float", &d, &format!("({distance}) / {pixel}")));
        self.statements.push(self.declaration("float", &half, &format!("0.5 * max({width}, 0.0)")));
        // Box coverage of the two geometric edges: only the boundary pixel
        // is antialiased. No smoothstep, glow, or frequency-dependent fading.
        self.statements.push(self.declaration("float", &coverage, &format!(
            "clamp({d} + {half} + 0.5, 0.0, 1.0) - clamp({d} - {half} + 0.5, 0.0, 1.0)")));
        self.statements.push(self.declaration("float4", &clr, &color));
        Ok(format!("{}({clr}.rgb * {clr}.a * {coverage}, {clr}.a * {coverage})", self.constructor("float4")))
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use objc2_foundation::NSString;
    use objc2_metal::*;
    use std::ptr::NonNull;

    #[test]
    fn ellipse_metal_distance_matches_cpu_geometry() {
        let device = MTLCreateSystemDefaultDevice().expect("Metal device");
        let mut samples = Vec::<[f32; 4]>::new();
        for (a, b) in [(1.0, 1.0), (4.0, 0.25), (0.25, 4.0), (10.0, 0.1), (4.0, 0.0)] {
            for i in -12..=12 {
                for j in -10..=10 {
                    samples.push([i as f32 * a / 10.0, j as f32 * b / 8.0, a, b]);
                }
            }
            samples.push([a * 0.3, 1e-8, a, b]);
            samples.push([a * 0.3, 1e-20, a, b]);
            samples.push([a * 1.2, 1e-20, a, b]);
        }
        samples.extend([[3.0, 4.0, 0.0, 0.0], [3.0, 4.0, 0.0, 5.0]]);
        let tokens = crate::parser::Parser::new("(sdf/ellipse rx ry)".into()).parse().unwrap();
        let expr = crate::parser::ASTParser::new(tokens).parse().unwrap().remove(0);
        let (statements, distance) = super::super::compile_sdf_expr(&expr).unwrap();
        let source = format!(r#"
            #include <metal_stdlib>
            using namespace metal;
            kernel void distances(device const float4* samples [[buffer(0)]],
                device float* output [[buffer(1)]], uint i [[thread_position_in_grid]]) {{
                float x = samples[i].x, y = samples[i].y;
                float rx = samples[i].z, ry = samples[i].w;
                {}
                output[i] = {};
            }}"#, statements.join("\n"), distance);
        let library = device.newLibraryWithSource_options_error(&NSString::from_str(&source), None)
            .unwrap_or_else(|error| panic!("ellipse Metal compile: {error:?}\n{source}"));
        let function = library.newFunctionWithName(&NSString::from_str("distances")).unwrap();
        let pipeline = device.newComputePipelineStateWithFunction_error(&function).unwrap();
        let input = unsafe { device.newBufferWithBytes_length_options(
            NonNull::new(samples.as_ptr() as *mut std::ffi::c_void).unwrap(),
            std::mem::size_of_val(samples.as_slice()), MTLResourceOptions::StorageModeShared) }.unwrap();
        let output = device.newBufferWithLength_options(samples.len() * 4, MTLResourceOptions::StorageModeShared).unwrap();
        let queue = device.newCommandQueue().unwrap();
        let command = queue.commandBuffer().unwrap();
        let encoder = command.computeCommandEncoder().unwrap();
        encoder.setComputePipelineState(&pipeline);
        unsafe {
            encoder.setBuffer_offset_atIndex(Some(&input), 0, 0);
            encoder.setBuffer_offset_atIndex(Some(&output), 0, 1);
        }
        encoder.dispatchThreads_threadsPerThreadgroup(
            MTLSize { width: samples.len(), height: 1, depth: 1 },
            MTLSize { width: 32, height: 1, depth: 1 });
        encoder.endEncoding();
        command.commit();
        command.waitUntilCompleted();
        assert!(command.error().is_none(), "ellipse GPU execution: {:?}", command.error());
        let actual = unsafe { std::slice::from_raw_parts(output.contents().as_ptr() as *const f32, samples.len()) };
        for (sample, actual) in samples.iter().zip(actual) {
            let [x, y, a, b] = sample.map(f64::from);
            let expected = crate::lang::sdf_geometry::ellipse_distance(x, y, a, b);
            assert!((*actual as f64 - expected).abs() < 2e-5,
                "ellipse {sample:?}: GPU {actual}, CPU {expected}");
        }
    }
}
