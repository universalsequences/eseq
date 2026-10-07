//! Event streams (spec §14.2s, stage 7g-4): what the event-view widget
//! draws, as positional rows ([`EventRow`]): a graph's fired events and each
//! node's latest (`graph.events`, `graph.node-events`, read with the graph's
//! playback in `graphs.rs`) and the tracks' output notes
//! (`transport.track-events`, `track-events-beat`).
//!
//! Feeds: live, observed only. Each history carries a revision that moves
//! with every change (a graph snapshot's `history_stamp` and
//! `node_events_stamp`,
//! `SequencerState::track_output_events_revision`), so an idle tick compares
//! one number and copies nothing; a moved one copies the history into a
//! buffer of rows (its capacity kept) and pushes it, building cells for the
//! new rows only ([`RowHistory`]).

use super::*;
use eseqlisp::widget_render::event_view::ROW_FIELDS;
use sequencer::graph::GraphVisualizationEvent;
use sequencer::sequencer::TrackOutputEvent;

/// One event as the event-view's positional row, in [`ROW_FIELDS`] order:
/// `(node track beat transpose velocity)`, -1 for no node or track.
pub(super) type EventRow = [f64; ROW_FIELDS.len()];

fn index_or_none(index: Option<usize>) -> f64 {
    index.map_or(-1.0, |index| index as f64)
}

/// A graph's history event, raw (the legacy `event-history` entry).
pub(super) fn graph_event_row(event: &GraphVisualizationEvent) -> EventRow {
    [
        event.node_index as f64,
        index_or_none(event.track),
        event.beat,
        event.transpose as f64,
        event.velocity as f64,
    ]
}

/// A node's latest event with the legacy `node-events` display transforms
/// (transpose to 0.01, velocity clamped to 0–1).
pub(super) fn graph_node_event_row(event: &GraphVisualizationEvent) -> EventRow {
    [
        event.node_index as f64,
        index_or_none(event.track),
        event.beat,
        graph_weight_display_value(event.transpose as f64),
        neural_trigger_display_value(event.velocity),
    ]
}

/// A track's output event as a row: no node.
pub(super) fn track_event_row(event: &TrackOutputEvent) -> EventRow {
    [
        -1.0,
        event.track as f64,
        event.beat,
        event.transpose as f64,
        event.velocity as f64,
    ]
}

/// An event history as pushed: its rows and their list cells, kept so the
/// next push builds cells for the new rows only (a history drops from the
/// front and grows at the end). The store still compares the whole list
/// before it pushes (number by number, nothing allocated) and the list
/// itself is one vector of cell pointers.
#[derive(Default)]
pub(super) struct RowHistory {
    rows: Vec<EventRow>,
    cells: Vec<Rc<RefCell<Value>>>,
    next: Vec<EventRow>,
}

impl RowHistory {
    /// Replace the rows with the ones `fill` appends (to an empty buffer),
    /// keeping the cells of the rows that remain: the longest tail of the
    /// old rows the new ones start with (none after a reset).
    pub(super) fn update<R>(&mut self, fill: impl FnOnce(&mut Vec<EventRow>) -> R) -> R {
        self.next.clear();
        let result = fill(&mut self.next);
        let next = &self.next;
        let dropped = (0..self.rows.len())
            .find(|&dropped| next.starts_with(&self.rows[dropped..]))
            .unwrap_or(self.rows.len());
        self.cells.drain(..dropped);
        let kept = self.cells.len();
        let fresh = next[kept..]
            .iter()
            .map(|row| Rc::new(RefCell::new(numbers(row))));
        self.cells.extend(fresh);
        std::mem::swap(&mut self.rows, &mut self.next);
        result
    }

    pub(super) fn value(&self) -> Value {
        Value::List(self.cells.clone())
    }
}

/// The track output history, as rows into `rows`; returns its revision.
fn read_track_events(sources: &KindsHandles, rows: &mut Vec<EventRow>) -> u64 {
    sources.state.with_track_output_events(|revision, events| {
        rows.extend(events.iter().map(track_event_row));
        revision
    })
}

/// The track output history as rows (the reader hook's cold read).
pub(super) fn track_events_value(sources: &KindsHandles) -> Value {
    let mut history = RowHistory::default();
    history.update(|rows| read_track_events(sources, rows));
    history.value()
}

/// What the transport's event stream keeps across ticks.
#[derive(Default)]
pub(crate) struct TrackEventsState {
    /// The transport instance and the history revision last pushed to it;
    /// `None` while unobserved (reopening pushes the current history).
    pushed: Option<(InstanceId, u64)>,
    history: RowHistory,
}

impl TrackEventsState {
    pub(super) fn invalidate(&mut self) {
        self.pushed = None;
    }
}

impl HostKinds {
    /// The transport's live fields: `track-events` only when observed and
    /// its revision moved since the last push; the rest computed and
    /// compared as any live field.
    pub(super) fn sync_transport_live(&mut self, pusher: &mut Pusher<'_>) {
        let Some(id) = pusher.singleton(TRANSPORT) else {
            return;
        };
        let events_bit = TRANSPORT_LIVE.bit(f::TRANSPORT_TRACK_EVENTS);
        let mask = pusher.push_live_except(id, &TRANSPORT_LIVE, events_bit);
        let stream = &mut self.track_events;
        if mask & events_bit == 0 {
            stream.pushed = None;
            return;
        }
        let sources = pusher.sources;
        let mut revision = sources.state.track_output_events_revision();
        let changed = stream.pushed != Some((id, revision));
        let history = &mut stream.history;
        pusher.push_computed_if(id, f::TRANSPORT_TRACK_EVENTS, changed, || {
            revision = history.update(|rows| read_track_events(sources, rows));
            history.value()
        });
        stream.pushed = Some((id, revision));
    }
}
