use std::time::Instant;
use super::{PaintReasons, PreparedScene};

pub(super) struct PaintProfile {
    enabled: bool,
    window: Instant,
    samples: Vec<f64>,
    counts: [usize; 5],
    reasons: PaintReasons,
}

impl Default for PaintProfile {
    fn default() -> Self {
        Self { enabled: std::env::var_os("ESEQLISP_PROFILE_UI").is_some(), window: Instant::now(),
            samples: Vec::new(), counts: [0; 5], reasons: PaintReasons::default() }
    }
}

impl PaintProfile {
    pub(super) fn start(&self) -> Option<Instant> { self.enabled.then(Instant::now) }

    pub(super) fn record(&mut self, root: u64, started: Option<Instant>, scene: &PreparedScene) {
        let Some(started) = started else { return; };
        if self.samples.len() < 4096 { self.samples.push(started.elapsed().as_secs_f64() * 1000.0); }
        for (sum, value) in self.counts.iter_mut().zip([scene.rebuilt_nodes, scene.reused_nodes,
            scene.culled_nodes, scene.reindexed_nodes, scene.bounds_refreshed_nodes]) { *sum += value; }
        self.reasons.accumulate(scene.paint_reasons);
        if self.window.elapsed().as_secs_f64() < 1.0 { return; }
        self.samples.sort_unstable_by(f64::total_cmp);
        let tails = [50, 95, 99].map(|p| self.samples[(self.samples.len() * p).div_ceil(100).saturating_sub(1)]);
        eprintln!("[ui-profile][paint] root={root} frames={} prepare_ms[p50,p95,p99]={tails:?} nodes[paint,reuse,cull,index,bounds]={:?} reasons={:?}",
            self.samples.len(), self.counts, self.reasons);
        self.samples.clear(); self.counts = [0; 5]; self.reasons = PaintReasons::default(); self.window = Instant::now();
    }
}
