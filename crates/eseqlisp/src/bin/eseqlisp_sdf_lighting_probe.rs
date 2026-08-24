//! Headless GPU cost probe for generated SDF material lighting.
//!
//! Renders a dense, representative control panel with the same generated WGSL
//! field under full and flat lighting, waiting for GPU completion after every
//! frame. Output is JSON so measurements can be archived and compared.

use std::process::ExitCode;

use eseqlisp::lang::sdf_codegen::{
    SdfShaderOptions, compile_sdf_to_wgsl, compile_sdf_to_wgsl_with_options,
};
use eseqlisp::parser::{ASTParser, Parser};
use eseqlisp::shader_capture::CaptureRenderer;
use eseqlisp::widget_render::WidgetInstance;
use serde_json::json;

const WIDTH: u32 = 1920;
const HEIGHT: u32 = 1080;
const COLUMNS: u32 = 12;
const ROWS: u32 = 8;
const WARMUP_FRAMES: usize = 30;
const SAMPLE_FRAMES: usize = 120;

const REPRESENTATIVE_CONTROL: &str = r#"
(sdf/layer
  (sdf/fill
    (let ((radius 0.18)
          (qx (+ (- (abs x) 0.82) radius))
          (qy (+ (- (abs y) 0.72) radius)))
      (- (+ (length (vec2 (max qx 0) (max qy 0)))
            (min (max qx qy) 0))
         radius))
    (material
      :lighting (lighting :edge-min -0.215 :edge-max 0.8413
                          :light (vec3 -0.1 -0.61 3.5)
                          :shininess 81.0)
      :color
      (let ((ny (+ y (* 0.3 (dot normal (vec3 0 1 0)))))
            (base (mix (rgba 0.20 0.20 0.92 1.0)
                       (rgba 0.65 0.68 1.0 1.0)
                       (smoothstep 1 5 ny)))
            (edge-fade (smoothstep 0.61 -0.16 d))
            (shine (* specular edge-fade 0.3)))
        (+ base (rgba shine shine shine 0.0))))))
"#;

fn parse_expression() -> Result<eseqlisp::parser::Expression, String> {
    let tokens = Parser::new(REPRESENTATIVE_CONTROL.to_string())
        .parse()
        .map_err(|error| format!("parse tokens: {error:?}"))?;
    ASTParser::new(tokens)
        .parse()
        .map_err(|error| format!("parse expression: {error:?}"))?
        .into_iter()
        .next()
        .ok_or_else(|| "probe expression is empty".to_string())
}

fn panel_instances() -> Vec<WidgetInstance> {
    let mut instances = Vec::with_capacity((COLUMNS * ROWS) as usize);
    for row in 0..ROWS {
        for column in 0..COLUMNS {
            let x0 = column as f32 / COLUMNS as f32 * 2.0 - 1.0;
            let x1 = (column + 1) as f32 / COLUMNS as f32 * 2.0 - 1.0;
            // NDC has its origin at the bottom left. Ordering the rows is not
            // visually important, but ndc_min must remain below ndc_max.
            let y0 = row as f32 / ROWS as f32 * 2.0 - 1.0;
            let y1 = (row + 1) as f32 / ROWS as f32 * 2.0 - 1.0;
            instances.push(WidgetInstance {
                ndc_min: [x0, y0],
                ndc_max: [x1, y1],
                value_t: 0.72,
                orientation: 0.0,
                itime: 0.0,
                uniform_a: [0.0; 4],
                uniform_b: [0.0; 4],
                uniform_c: [0.0; 4],
                uniform_d: [0.0; 4],
                color_a: [0.3, 0.3, 0.8, 1.0],
                color_b: [-1.0, 0.0, 0.0, 1.0],
                color_c: [0.0, 0.0, 1.0, 1.0],
                color_d: [0.0; 4],
                corner_radius: 0.0,
                pixel_aspect: (WIDTH / COLUMNS) as f32 / (HEIGHT / ROWS) as f32,
            });
        }
    }
    instances
}

fn milliseconds(samples: &[std::time::Duration]) -> Vec<f64> {
    samples
        .iter()
        .map(|duration| duration.as_secs_f64() * 1000.0)
        .collect()
}

fn summary(samples: &[std::time::Duration]) -> serde_json::Value {
    let mut values = milliseconds(samples);
    values.sort_by(f64::total_cmp);
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    json!({
        "min_ms": values[0],
        "median_ms": values[values.len() / 2],
        "p95_ms": values[(values.len() * 95 / 100).min(values.len() - 1)],
        "mean_ms": mean,
        "max_ms": values[values.len() - 1],
    })
}

fn run() -> Result<serde_json::Value, String> {
    let expression = parse_expression()?;
    let full = compile_sdf_to_wgsl(&expression).map_err(|error| error.to_string())?;
    let flat = compile_sdf_to_wgsl_with_options(&expression, SdfShaderOptions::flat_lighting())
        .map_err(|error| error.to_string())?;
    let renderer = CaptureRenderer::new().ok_or_else(|| "no wgpu adapter available".to_string())?;
    let instances = panel_instances();

    let full_samples = renderer.benchmark_widget_fragment(
        &full.shader_source,
        &instances,
        WIDTH,
        HEIGHT,
        WARMUP_FRAMES,
        SAMPLE_FRAMES,
    );
    let flat_samples = renderer.benchmark_widget_fragment(
        &flat.shader_source,
        &instances,
        WIDTH,
        HEIGHT,
        WARMUP_FRAMES,
        SAMPLE_FRAMES,
    );
    let mut sorted_full = milliseconds(&full_samples);
    sorted_full.sort_by(f64::total_cmp);
    let mut sorted_flat = milliseconds(&flat_samples);
    sorted_flat.sort_by(f64::total_cmp);
    let full_median = sorted_full[SAMPLE_FRAMES / 2];
    let flat_median = sorted_flat[SAMPLE_FRAMES / 2];

    Ok(json!({
        "adapter": renderer.adapter_name(),
        "backend": renderer.adapter_backend(),
        "target": { "width": WIDTH, "height": HEIGHT },
        "control_instances": instances.len(),
        "warmup_frames_per_tier": WARMUP_FRAMES,
        "sample_frames_per_tier": SAMPLE_FRAMES,
        "full": summary(&full_samples),
        "flat": summary(&flat_samples),
        "median_speedup": full_median / flat_median,
        "median_reduction_percent": (1.0 - flat_median / full_median) * 100.0,
    }))
}

fn main() -> ExitCode {
    match run() {
        Ok(result) => {
            println!("{}", serde_json::to_string_pretty(&result).unwrap());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("SDF lighting probe failed: {error}");
            ExitCode::FAILURE
        }
    }
}
