//! Server-sent events as the Messages API streams them: `event:` and
//! `data:` lines, a blank line ending each event. Every event's JSON carries
//! its own `type`, so only the data matters.

#[derive(Default)]
pub struct SseParser {
    data: String,
}

impl SseParser {
    /// Feed one line (its newline removed); the data of the event it
    /// completes, if any.
    pub fn line(&mut self, line: &str) -> Option<String> {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            return (!self.data.is_empty()).then(|| std::mem::take(&mut self.data));
        }
        if let Some(data) = line.strip_prefix("data:") {
            if !self.data.is_empty() {
                self.data.push('\n');
            }
            self.data.push_str(data.strip_prefix(' ').unwrap_or(data));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_end_at_a_blank_line_and_only_their_data_is_kept() {
        let mut parser = SseParser::default();
        let mut events = Vec::new();
        for line in "event: ping\ndata: {\"type\":\"ping\"}\n\n: comment\nevent: x\r\ndata: a\ndata:b\n\n\n".lines() {
            events.extend(parser.line(line));
        }
        assert_eq!(events, vec!["{\"type\":\"ping\"}".to_string(), "a\nb".to_string()]);
    }
}
