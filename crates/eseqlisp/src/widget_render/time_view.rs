use crate::layout::Rect;

#[derive(Clone)]
pub struct TimeRuler {
    pub mode: TimeRulerMode,
}

#[derive(Clone)]
pub enum TimeRulerMode {
    BarsBeats { beats_per_bar: i64 },
    Seconds,
}

#[derive(Clone, Copy)]
pub struct TimeViewport {
    pub rect: Rect,
    pub header_height: f32,
    pub sidebar_width: f32,
    pub view_start: f64,
    pub view_duration: f64,
    pub zoom_min_duration: f64,
    pub zoom_max_duration: f64,
    /// Divides the zoom-adaptive grid step, so `2.0` gives twice as many grid
    /// cells at every zoom level. The ladder rungs are powers of two, so a
    /// density of 2 is exactly "one rung finer" and every rung stays aligned
    /// to the coarser one it came from — bar lines never move, there are just
    /// more lines between them. Hosts that want the stock ladder pass `1.0`.
    pub grid_density: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimeZoom {
    pub anchor_time: f64,
    pub factor: f64,
}

impl TimeViewport {
    pub fn content_rect(&self) -> Rect {
        let top = self.rect.row + self.header_height.min(self.rect.height);
        let left = self.rect.col + self.sidebar_width.min(self.rect.width);
        Rect {
            row: top,
            col: left,
            width: (self.rect.width - self.sidebar_width.min(self.rect.width)).max(0.0),
            height: (self.rect.height - self.header_height.min(self.rect.height)).max(0.0),
        }
    }

    pub fn time_at_col(&self, local_col: f32) -> f64 {
        let content = self.content_rect();
        if content.width == 0.0 {
            return self.view_start;
        }
        let t = ((local_col - content.col) / content.width).clamp(0.0, 1.0) as f64;
        self.view_start + self.view_duration * t
    }

    pub fn col_for_time(&self, time: f64) -> u16 {
        let content = self.content_rect();
        if content.width <= 1.0 {
            return content.col.round() as u16;
        }
        let edge = self.edge_for_time(time);
        (content.col + edge.min((content.width - 1.0).max(0.0))).round() as u16
    }

    pub fn edge_for_time(&self, time: f64) -> f32 {
        let content = self.content_rect();
        if content.width == 0.0 {
            return 0.0;
        }
        let span = self.view_duration.max(0.0001);
        let t = ((time - self.view_start) / span).clamp(0.0, 1.0);
        (content.width as f64 * t).floor() as f32
    }

    pub fn x_for_time(&self, time: f64) -> f32 {
        let content = self.content_rect();
        if content.width == 0.0 {
            return content.col;
        }
        let span = self.view_duration.max(0.0001);
        let t = ((time - self.view_start) / span).clamp(0.0, 1.0);
        content.col + (content.width as f64 * t) as f32
    }

    pub fn playhead_col(&self, playhead_time: Option<f64>) -> Option<u16> {
        let playhead_time = playhead_time?;
        if playhead_time < self.view_start || playhead_time > self.view_start + self.view_duration {
            return None;
        }
        Some(self.col_for_time(playhead_time))
    }

    pub fn metal_playhead_x(&self, playhead_time: Option<f64>) -> Option<f32> {
        let playhead_time = playhead_time?;
        if playhead_time < self.view_start || playhead_time > self.view_start + self.view_duration {
            return None;
        }
        Some(self.x_for_time(playhead_time))
    }

    pub fn grid_columns(&self, ruler: Option<&TimeRuler>) -> Vec<(u16, bool)> {
        self.visible_grid_marks(ruler)
            .into_iter()
            .map(|(_, col, is_major)| (col, is_major))
            .collect()
    }

    pub fn time_ruler_labels(&self, ruler: Option<&TimeRuler>) -> Vec<(u16, String)> {
        let Some(ruler) = ruler else {
            return Vec::new();
        };
        let mut labels = Vec::new();
        let mut last_end = None;
        for (mark, col, is_major) in self.visible_grid_marks(Some(ruler)) {
            let Some(label) = ruler.label_for_mark(mark, is_major) else {
                continue;
            };
            let label_len = label.chars().count() as u16;
            let min_col = self.content_rect().col.round() as u16;
            let max_col = (self.rect.col + self.rect.width).round() as u16;
            let start_col = col.max(min_col);
            let end_col = start_col + label_len;
            if end_col > max_col {
                continue;
            }
            if last_end.is_some_and(|prev| start_col <= prev) {
                continue;
            }
            labels.push((start_col, label));
            last_end = Some(end_col);
        }
        labels
    }

    pub fn metal_grid_lines(&self, ruler: Option<&TimeRuler>) -> Vec<(f32, bool)> {
        let mut lines = Vec::new();
        let mut last_x: Option<f32> = None;
        for (mark, _, is_major) in self.visible_grid_marks(ruler) {
            let x = self.x_for_time(mark);
            if last_x.is_some_and(|last| (x - last).abs() < 0.01) {
                continue;
            }
            lines.push((x, is_major));
            last_x = Some(x);
        }
        lines
    }

    pub fn metal_time_ruler_labels(&self, ruler: Option<&TimeRuler>) -> Vec<(f32, String)> {
        let Some(ruler) = ruler else {
            return Vec::new();
        };
        let mut labels = Vec::new();
        let mut last_end = None;
        let content = self.content_rect();
        let max_x = self.rect.col + self.rect.width;
        for (mark, _, is_major) in self.visible_grid_marks(Some(ruler)) {
            let Some(label) = ruler.label_for_mark(mark, is_major) else {
                continue;
            };
            let x = self.x_for_time(mark).max(content.col);
            let label_width = label.chars().count() as f32 * 0.58 + 0.28;
            let end_x = x + label_width;
            if end_x > max_x {
                continue;
            }
            if last_end.is_some_and(|prev| x <= prev) {
                continue;
            }
            labels.push((x, label));
            last_end = Some(end_x);
        }
        labels
    }

    pub fn zoom_action(&self, anchor_time: f64, factor: f64) -> Option<TimeZoom> {
        let min_duration = self.zoom_min_duration.min(self.zoom_max_duration);
        let max_duration = self.zoom_max_duration.max(self.zoom_min_duration);
        let next_duration = (self.view_duration / factor).clamp(min_duration, max_duration);
        if (next_duration - self.view_duration).abs() < f64::EPSILON {
            return None;
        }
        Some(TimeZoom {
            anchor_time,
            factor,
        })
    }

    pub fn grid_step(&self, ruler: Option<&TimeRuler>) -> f64 {
        self.grid_step_and_major_every(ruler).0
    }

    fn grid_step_and_major_every(&self, ruler: Option<&TimeRuler>) -> (f64, f64) {
        let content = self.content_rect();
        match ruler.map(|r| &r.mode) {
            Some(TimeRulerMode::BarsBeats { .. }) => {
                let pixels_per_beat = content.width as f64 / self.view_duration.max(0.0001);
                let step = if pixels_per_beat >= 384.0 {
                    0.03125
                } else if pixels_per_beat >= 192.0 {
                    0.0625
                } else if pixels_per_beat >= 96.0 {
                    0.125
                } else if pixels_per_beat >= 48.0 {
                    0.25
                } else if pixels_per_beat >= 24.0 {
                    0.5
                } else if pixels_per_beat >= 8.0 {
                    1.0
                } else if pixels_per_beat >= 4.0 {
                    2.0
                } else if pixels_per_beat >= 2.0 {
                    4.0
                } else if pixels_per_beat >= 1.0 {
                    8.0
                } else {
                    16.0
                };
                let step = step / self.grid_density.clamp(1.0, 8.0);
                // Derived from the DIVIDED step so majors keep landing on the
                // same absolute times (bar lines) however dense the grid is.
                let major_every = if step < 1.0 {
                    (1.0_f64 / step).round()
                } else {
                    (4.0_f64 / step).max(1.0)
                };
                (step, major_every)
            }
            _ => {
                let pixels_per_second = content.width as f64 / self.view_duration.max(0.0001);
                let step = if pixels_per_second >= 200.0 {
                    0.01
                } else if pixels_per_second >= 100.0 {
                    0.02
                } else if pixels_per_second >= 50.0 {
                    0.05
                } else if pixels_per_second >= 20.0 {
                    0.1
                } else if pixels_per_second >= 10.0 {
                    0.25
                } else if pixels_per_second >= 5.0 {
                    0.5
                } else if pixels_per_second >= 2.0 {
                    1.0
                } else if pixels_per_second >= 1.0 {
                    2.0
                } else if pixels_per_second >= 0.5 {
                    5.0
                } else {
                    10.0
                };
                let step = step / self.grid_density.clamp(1.0, 8.0);
                let major_every = if step < 0.1 {
                    10.0
                } else if step < 1.0 {
                    5.0
                } else {
                    4.0
                };
                (step, major_every)
            }
        }
    }

    fn visible_grid_marks(&self, ruler: Option<&TimeRuler>) -> Vec<(f64, u16, bool)> {
        let content = self.content_rect();
        if content.width == 0.0 {
            return Vec::new();
        }

        let (step, major_every) = self.grid_step_and_major_every(ruler);

        // First mark at or after view_start: a mark before it would clamp to
        // the content's left edge and draw an invented grid line/label there.
        let start = (self.view_start / step).ceil() as i64;
        let end = ((self.view_start + self.view_duration) / step).ceil() as i64;
        let mut cols = Vec::new();
        let mut last = None;

        for index in start..=end {
            let mark = index as f64 * step;
            if mark < 0.0 {
                continue;
            }
            let edge = self.edge_for_time(mark);
            let col = (content.col + edge.min((content.width - 1.0).max(0.0))).round() as u16;
            let is_major =
                ((index as f64 / major_every).round() - (index as f64 / major_every)).abs() < 1e-6;
            if last != Some(col) {
                cols.push((mark, col, is_major));
                last = Some(col);
            }
        }
        cols
    }
}

impl TimeRuler {
    pub fn label_for_mark(&self, mark: f64, is_major: bool) -> Option<String> {
        match self.mode {
            TimeRulerMode::BarsBeats { beats_per_bar } => {
                if (mark - mark.round()).abs() > 1e-6 {
                    return None;
                }
                let beat = mark.round() as i64;
                let beats_per_bar = beats_per_bar.max(1);
                let bar = beat.div_euclid(beats_per_bar) + 1;
                let beat_in_bar = beat.rem_euclid(beats_per_bar) + 1;
                if beat_in_bar == 1 {
                    Some(format!("{bar}"))
                } else {
                    Some(format!("{bar}.{beat_in_bar}"))
                }
            }
            TimeRulerMode::Seconds => {
                if !is_major {
                    return None;
                }
                if mark >= 1.0 {
                    Some(format!("{mark:.2}s"))
                } else {
                    Some(format!("{:.0}ms", mark * 1000.0))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars_beats_labels_include_sub_beats() {
        let viewport = TimeViewport {
            rect: Rect {
                row: 0.0,
                col: 0.0,
                width: 129.0,
                height: 8.0,
            },
            header_height: 1.0,
            sidebar_width: 0.0,
            view_start: 0.0,
            view_duration: 16.0,
            zoom_min_duration: 8.0,
            zoom_max_duration: 128.0,
            grid_density: 1.0,
        };
        let ruler = TimeRuler {
            mode: TimeRulerMode::BarsBeats { beats_per_bar: 4 },
        };
        let labels: Vec<String> = viewport
            .time_ruler_labels(Some(&ruler))
            .into_iter()
            .map(|(_, label)| label)
            .collect();
        assert!(labels.iter().any(|label| label == "1"));
        assert!(labels.iter().any(|label| label == "1.2"));
        assert!(labels.iter().any(|label| label == "2"));
    }

    #[test]
    fn bars_beats_grid_gets_finer_when_zoomed_in() {
        let ruler = TimeRuler {
            mode: TimeRulerMode::BarsBeats { beats_per_bar: 4 },
        };
        let wide = TimeViewport {
            rect: Rect {
                row: 0.0,
                col: 0.0,
                width: 128.0,
                height: 8.0,
            },
            header_height: 1.0,
            sidebar_width: 0.0,
            view_start: 0.0,
            view_duration: 4.0,
            zoom_min_duration: 0.25,
            zoom_max_duration: 128.0,
            grid_density: 1.0,
        };
        let closer = TimeViewport {
            view_duration: 1.0,
            ..wide
        };

        assert_eq!(wide.grid_step(Some(&ruler)), 0.5);
        assert_eq!(closer.grid_step(Some(&ruler)), 0.125);
    }

    #[test]
    fn seconds_ruler_prefers_ms_when_zoomed_in() {
        let viewport = TimeViewport {
            rect: Rect {
                row: 0.0,
                col: 0.0,
                width: 80.0,
                height: 8.0,
            },
            header_height: 1.0,
            sidebar_width: 0.0,
            view_start: 0.0,
            view_duration: 0.25,
            zoom_min_duration: 0.001,
            zoom_max_duration: 128.0,
            grid_density: 1.0,
        };
        let ruler = TimeRuler {
            mode: TimeRulerMode::Seconds,
        };
        let labels: Vec<String> = viewport
            .time_ruler_labels(Some(&ruler))
            .into_iter()
            .map(|(_, label)| label)
            .collect();
        assert!(labels.iter().any(|label| label.ends_with("ms")));
    }
}
