//! `(dyn SOURCE)` word sources (docs/sexp-slot-spec.md §4, §7): a string atom
//! whose completions and validity come from the host, not the schema.
//!
//! The host registers a source by name; the widget hands it the slot's
//! `:dyn-context` (any Lisp value, untouched) and gets back grouped words.
//! Sources are only asked lazily — when a popup or field opens on a `dyn`
//! atom, or to revalidate after the context or the source's epoch changed —
//! and each widget caches the answer per (source, context, epoch).
//!
//! An item whose `word` is empty is a **hint**: its `detail` is shown as a
//! dim, non-selectable row above the completions (e.g. "route this row to see
//! its parameters") and it never makes a word valid.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use super::schema::NumSpec;
use crate::vm::{Value, format_lisp_source};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DynItem {
    /// What is inserted (stored as a string atom). Empty: a hint row.
    pub word: String,
    /// Secondary text: shown dim beside the word, and fuzzy-matched too.
    pub detail: String,
    /// The rails of a number that belongs to this word: a `(dyn-num SOURCE
    /// …)` next to it scrubs, snaps, clamps and formats with these instead
    /// of its fallback (`None`: the fallback).
    pub num: Option<NumSpec>,
}

impl DynItem {
    pub fn new(word: impl Into<String>, detail: impl Into<String>) -> Self {
        Self { word: word.into(), detail: detail.into(), num: None }
    }

    pub fn with_num(self, num: NumSpec) -> Self {
        Self { num: Some(num), ..self }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DynGroup {
    pub group: String,
    pub items: Vec<DynItem>,
}

/// One answer from a source for one context.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DynWords {
    /// The host data epoch this answer reflects. The registry adopts it when
    /// it is newer than the source's registered epoch, so a host may either
    /// report its epoch here or call `set_dyn_word_epoch` / `bump_dyn_word_epoch`.
    pub epoch: u64,
    pub groups: Vec<DynGroup>,
    /// Further valid spellings that are never offered as completions (e.g.
    /// slot-free aliases of listed names): they make a name valid without
    /// doubling the popup. Only `word` and `num` matter; `detail` is unused.
    pub aliases: Vec<DynItem>,
}

impl DynWords {
    /// Whether `word` is one of the (non-hint) words or an alias.
    pub fn contains(&self, word: &str) -> bool {
        !word.is_empty()
            && (self.items().any(|(_, item)| item.word == word)
                || self.aliases.iter().any(|alias| alias.word == word))
    }

    /// The number rails `word` (a listed word or an alias) carries.
    pub fn num_spec(&self, word: &str) -> Option<NumSpec> {
        if word.is_empty() {
            return None;
        }
        self.items()
            .map(|(_, item)| item)
            .chain(self.aliases.iter())
            .find(|item| item.word == word)
            .and_then(|item| item.num)
    }

    /// Every item with its group, in source order.
    pub fn items(&self) -> impl Iterator<Item = (&str, &DynItem)> {
        self.groups.iter().flat_map(|group| {
            group
                .items
                .iter()
                .map(move |item| (group.group.as_str(), item))
        })
    }

    /// Hint rows (items with an empty word): their detail text.
    pub fn hints(&self) -> impl Iterator<Item = &str> {
        self.items()
            .filter(|(_, item)| item.word.is_empty())
            .map(|(_, item)| item.detail.as_str())
    }
}

pub type DynWordSource = Box<dyn Fn(&Value) -> DynWords>;

struct Source {
    query: Rc<dyn Fn(&Value) -> DynWords>,
    epoch: u64,
}

thread_local! {
    static SOURCES: RefCell<HashMap<String, Source>> = RefCell::new(HashMap::new());
}

/// Register (or replace) the source `(dyn NAME)` atoms ask. Replacing a
/// source bumps its epoch, so every cached answer is re-asked.
pub fn register_dyn_word_source(name: &str, source: DynWordSource) {
    SOURCES.with(|sources| {
        let mut sources = sources.borrow_mut();
        let epoch = sources.get(name).map_or(0, |old| old.epoch + 1);
        sources.insert(
            name.to_string(),
            Source {
                query: Rc::from(source),
                epoch,
            },
        );
    });
    crate::widget_render::bump_widget_state_generation();
}

pub fn unregister_dyn_word_source(name: &str) {
    SOURCES.with(|sources| sources.borrow_mut().remove(name));
    crate::widget_render::bump_widget_state_generation();
}

/// The source's current epoch (`None`: not registered). A cached answer is
/// fresh while this is unchanged.
pub fn dyn_word_epoch(name: &str) -> Option<u64> {
    SOURCES.with(|sources| sources.borrow().get(name).map(|source| source.epoch))
}

/// The host's data changed: every cached answer from `name` goes stale.
pub fn bump_dyn_word_epoch(name: &str) {
    let bumped = SOURCES.with(|sources| {
        sources
            .borrow_mut()
            .get_mut(name)
            .map(|source| source.epoch += 1)
            .is_some()
    });
    if bumped {
        crate::widget_render::bump_widget_state_generation();
    }
}

/// Set the source's epoch to the host's own counter (no-op when unchanged).
pub fn set_dyn_word_epoch(name: &str, epoch: u64) {
    let changed = SOURCES.with(|sources| match sources.borrow_mut().get_mut(name) {
        Some(source) if source.epoch != epoch => {
            source.epoch = epoch;
            true
        }
        _ => false,
    });
    if changed {
        crate::widget_render::bump_widget_state_generation();
    }
}

/// Ask `name` for `context`, uncached (for a host that must judge a name
/// outside a widget, e.g. an `:on-hover` reason). `None`: no such source.
pub fn query_dyn_words(name: &str, context: &Value) -> Option<Rc<DynWords>> {
    query(name, context).map(|(words, _)| words)
}

/// Ask `name` for `context`, uncached: the words and the epoch they are valid
/// for. `None` when no such source is registered.
fn query(name: &str, context: &Value) -> Option<(Rc<DynWords>, u64)> {
    // Call outside the borrow: a source may itself touch the registry.
    let (query, epoch) = SOURCES.with(|sources| {
        sources
            .borrow()
            .get(name)
            .map(|source| (source.query.clone(), source.epoch))
    })?;
    let words = query(context);
    let epoch = if words.epoch > epoch {
        SOURCES.with(|sources| {
            if let Some(source) = sources.borrow_mut().get_mut(name) {
                source.epoch = source.epoch.max(words.epoch);
            }
        });
        words.epoch
    } else {
        epoch
    };
    Some((Rc::new(words), epoch))
}

#[derive(Clone, Debug)]
struct CacheEntry {
    source: String,
    context: String,
    epoch: u64,
    words: Rc<DynWords>,
}

/// One widget's answers: at most one per source (a new context replaces it).
#[derive(Clone, Debug, Default)]
pub struct DynCache {
    entries: Vec<CacheEntry>,
}

impl DynCache {
    /// The words of `source` for `context`, from the cache while the context
    /// and the source's epoch are unchanged, else freshly asked.
    pub fn words(&mut self, source: &str, context: &Value) -> Option<Rc<DynWords>> {
        let epoch = dyn_word_epoch(source)?;
        let context_key = format_lisp_source(context);
        if let Some(entry) = self.entries.iter().find(|entry| {
            entry.source == source && entry.context == context_key && entry.epoch == epoch
        }) {
            return Some(entry.words.clone());
        }
        let (words, epoch) = query(source, context)?;
        self.entries.retain(|entry| entry.source != source);
        self.entries.push(CacheEntry {
            source: source.to_string(),
            context: context_key,
            epoch,
            words: words.clone(),
        });
        Some(words)
    }
}

/// What a slot needs to resolve its `dyn` atoms: the widget's
/// `:dyn-context` and its cache (shared, so a `Slot` stays `Copy`).
#[derive(Clone, Debug)]
pub struct DynLookup {
    pub context: Value,
    pub cache: Rc<RefCell<DynCache>>,
}

impl DynLookup {
    pub fn new(context: Value, cache: Rc<RefCell<DynCache>>) -> Self {
        Self { context, cache }
    }

    pub fn words(&self, source: &str) -> Option<Rc<DynWords>> {
        self.cache.borrow_mut().words(source, &self.context)
    }

    /// Whether atoms are validated at all: a `nil` context is not.
    pub fn validates(&self) -> bool {
        !matches!(self.context, Value::Nil)
    }
}

/// Case-insensitive subsequence match of every whitespace-separated term of
/// `query` against `word` or `detail` (each term may match either).
pub fn fuzzy_matches(query: &str, word: &str, detail: &str) -> bool {
    fn subsequence(needle: &str, haystack: &str) -> bool {
        let mut hay = haystack.chars().flat_map(char::to_lowercase);
        needle
            .chars()
            .flat_map(char::to_lowercase)
            .all(|ch| hay.any(|h| h == ch))
    }
    query
        .split_whitespace()
        .all(|term| subsequence(term, word) || subsequence(term, detail))
}
