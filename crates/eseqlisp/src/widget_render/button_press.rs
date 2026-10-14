//! Pointer-press feedback for `button` widgets, like a native macOS push
//! button: the bezel shows a pressed fill while the left button is held on
//! it, un-presses when the pointer drags off (and re-presses on drag back),
//! and eases back to normal over [`RELEASE_SECONDS`] once let go.
//!
//! The state is render-only and never changes click semantics: `on-click`
//! still fires on mouse-down and `on-release` keeps its gesture routing.
//!
//! Identity: the press is keyed by the button's state key
//! (`stable_widget_id`, else `widget_id`) plus its layout rect. `on-click`
//! usually re-renders the enclosing subtree, and replacement subtrees mint
//! fresh widget ids for unkeyed nodes, so a node at the identical rect is
//! accepted as the same button — otherwise the pressed fill would vanish on
//! the very frame the click is handled.
//!
//! Frames: holding a press is static (one repaint via the widget's state
//! revision); only the release fade asks for animation frames, and only
//! until a frame has painted the settled (unpressed) look.

use std::cell::RefCell;

use super::bump_widget_state_revision;
use crate::layout::{LayoutNode, Rect};

/// Pressed → normal fade duration.
pub const RELEASE_SECONDS: f32 = 0.1;
/// A fade that no frame painted to completion (the button was culled or its
/// tile hidden) stops requesting frames after this extra margin.
const SETTLE_GRACE_SECONDS: f32 = 0.25;

#[derive(Clone, Copy, Debug, PartialEq)]
struct PressTarget {
    key: u64,
    rect: Rect,
}

#[derive(Clone, Copy, Debug)]
struct HeldPress {
    target: PressTarget,
    inside: bool,
}

#[derive(Clone, Copy, Debug)]
struct Release {
    target: PressTarget,
    started_at: f32,
    from: f32,
}

#[derive(Default)]
struct PressState {
    held: Option<HeldPress>,
    releases: Vec<Release>,
}

thread_local! {
    static PRESS_STATE: RefCell<PressState> = RefCell::new(PressState::default());
}

/// The widget-state key for a button: survives relayout when the node is keyed.
pub fn press_key(node: &LayoutNode) -> u64 {
    node.stable_widget_id.unwrap_or(node.widget_id)
}

fn target_for(node: &LayoutNode) -> PressTarget {
    PressTarget {
        key: press_key(node),
        rect: node.rect,
    }
}

fn target_matches(target: PressTarget, node: &LayoutNode) -> bool {
    let key = press_key(node);
    key == target.key || (node.stable_widget_id.is_none() && node.rect == target.rect)
}

fn ease_out_cubic(value: f32) -> f32 {
    let remaining = 1.0 - value.clamp(0.0, 1.0);
    1.0 - remaining * remaining * remaining
}

fn release_amount(release: &Release, now: f32) -> f32 {
    let age = (now - release.started_at).max(0.0);
    if age >= RELEASE_SECONDS {
        0.0
    } else {
        release.from * (1.0 - ease_out_cubic(age / RELEASE_SECONDS))
    }
}

fn prune_expired(state: &mut PressState, now: f32) {
    state
        .releases
        .retain(|release| now - release.started_at < RELEASE_SECONDS + SETTLE_GRACE_SECONDS);
}

fn start_release(state: &mut PressState, target: PressTarget, now: f32) {
    state.releases.retain(|release| release.target != target);
    state.releases.push(Release {
        target,
        started_at: now,
        from: 1.0,
    });
}

fn forget_release(state: &mut PressState, matches_target: impl Fn(&PressTarget) -> bool) {
    state
        .releases
        .retain(|release| !matches_target(&release.target));
}

/// Left button went down on `node` (an enabled, non-badge button).
pub fn begin_press(node: &LayoutNode, now: f32) {
    let target = target_for(node);
    PRESS_STATE.with(|state| {
        let mut state = state.borrow_mut();
        prune_expired(&mut state, now);
        // A press whose mouse-up never arrived (lost to another window, a
        // modal grab) fades out instead of staying stuck down.
        if let Some(previous) = state.held.take()
            && previous.inside
            && previous.target != target
        {
            start_release(&mut state, previous.target, now);
            bump_widget_state_revision(previous.target.key);
        }
        forget_release(&mut state, |release| *release == target);
        state.held = Some(HeldPress {
            target,
            inside: true,
        });
    });
    bump_widget_state_revision(target.key);
}

/// Whether a press is being held (cheap; gates the drag hit test).
pub fn press_held() -> bool {
    PRESS_STATE.with(|state| state.borrow().held.is_some())
}

/// The pointer moved while the button is held; `hit` is the widget now
/// under it. Returns whether the pressed look changed (drag off / back on).
pub fn pointer_over(hit: Option<&LayoutNode>, now: f32) -> bool {
    PRESS_STATE.with(|state| {
        let mut state = state.borrow_mut();
        let Some(mut held) = state.held else {
            return false;
        };
        let inside = hit.is_some_and(|node| target_matches(held.target, node));
        if let Some(node) = hit.filter(|_| inside) {
            // Follow a relayout's new id so later revision bumps land on it.
            held.target = target_for(node);
        }
        let changed = inside != held.inside;
        if changed {
            if inside {
                let target = held.target;
                forget_release(&mut state, |release| *release == target);
            } else {
                start_release(&mut state, held.target, now);
            }
            bump_widget_state_revision(held.target.key);
        }
        held.inside = inside;
        state.held = Some(held);
        changed
    })
}

/// The left button came up anywhere. Returns whether the pressed look
/// changed (a release fade started).
pub fn end_press(now: f32) -> bool {
    PRESS_STATE.with(|state| {
        let mut state = state.borrow_mut();
        let Some(held) = state.held.take() else {
            return false;
        };
        if held.inside {
            start_release(&mut state, held.target, now);
            bump_widget_state_revision(held.target.key);
        }
        held.inside
    })
}

/// How pressed `node` looks at `now`: 1.0 while held on it, fading to 0.0
/// over [`RELEASE_SECONDS`] after release or drag-off.
pub fn press_amount(node: &LayoutNode, now: f32) -> f32 {
    PRESS_STATE.with(|state| {
        let state = state.borrow();
        if state.held.is_none() && state.releases.is_empty() {
            return 0.0;
        }
        if state
            .held
            .is_some_and(|held| held.inside && target_matches(held.target, node))
        {
            return 1.0;
        }
        state
            .releases
            .iter()
            .rev()
            .find(|release| target_matches(release.target, node))
            .map_or(0.0, |release| release_amount(release, now))
    })
}

/// [`press_amount`] for the paint pass: once a frame paints a finished fade
/// its entry is dropped, so the animation-frame request ends only after the
/// settled look is on screen.
pub fn paint_press_amount(node: &LayoutNode, now: f32) -> f32 {
    let amount = press_amount(node, now);
    if amount <= 0.0 {
        PRESS_STATE.with(|state| {
            let mut state = state.borrow_mut();
            if !state.releases.is_empty() {
                state.releases.retain(|release| {
                    !(target_matches(release.target, node) && release_amount(release, now) <= 0.0)
                });
            }
        });
    }
    amount
}

/// Whether `node` is mid-fade and needs animation frames. Idle and held
/// buttons never do.
pub fn press_animating(node: &LayoutNode, now: f32) -> bool {
    PRESS_STATE.with(|state| {
        let state = state.borrow();
        !state.releases.is_empty()
            && state.releases.iter().any(|release| {
                target_matches(release.target, node)
                    && now - release.started_at < RELEASE_SECONDS + SETTLE_GRACE_SECONDS
            })
    })
}

#[cfg(test)]
pub(crate) fn reset_press_state() {
    PRESS_STATE.with(|state| *state.borrow_mut() = PressState::default());
}
