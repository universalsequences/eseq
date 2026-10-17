//! The command palette (docs/command-palette-spec.md §5–§7): M-x with a
//! window-level search panel. It lives in the minibuffer slot, so it takes
//! the keyboard from any tile; the backends draw its [`PaletteFrame`].
//!
//! The first stage opens on a flat home list: recently run commands, then
//! the `:pinned` ones in definition order (every command when none is
//! pinned); while typing, `:group` sections appear, matches rank by title and name, sections ordered by their best
//! match, and anything else M-x can run follows under "Functions" (matched
//! as a substring of its name). A command with `:choices` opens a second
//! stage that picks its argument — the field shows the command's badge
//! (`Theme`) and Backspace on an empty field goes back.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crossterm::event::{KeyEvent, MouseButton, MouseEvent, MouseEventKind};

use super::minibuffer::{ReadKey, classify_read_key};
use super::{Editor, MinibufferMode};
use crate::backend::{PaletteFrame, PaletteRowFrame};
use crate::vm::Value;
use crate::widget_render::command_palette::{self, PaletteHit};

/// The command that opens the palette (bound to `/` in sequencer views).
pub(super) const COMMAND_PALETTE: &str = "command-palette";
/// Recently run commands kept for the empty-query list.
const HISTORY_LIMIT: usize = 30;
/// Recently run commands shown at the top of the empty-query list.
const RECENT_SHOWN: usize = 5;
/// Matches kept while typing; the list scrolls through these.
const MATCH_LIMIT: usize = 200;
/// The section uncurated M-x names list under while typing.
const FUNCTIONS_SECTION: &str = "Functions";
/// What a `:search :parent` leaf adds to its match score: more than any
/// plain leaf match scores, so instruments list above their presets.
const PARENT_LEAF_BOOST: i32 = 1100;
/// Choices one `:searchable` command contributes to search, at most.
const SEARCH_LEAF_LIMIT: usize = 20_000;
/// Live previews closer together than this wait for the highlight to
/// settle (a held arrow key over themes or presets).
const PREVIEW_SETTLE: std::time::Duration = std::time::Duration::from_millis(90);

#[derive(Debug, Clone)]
pub(super) struct PaletteState {
    /// The command list, then a choice stage on top while one is open.
    stages: Vec<Stage>,
    footer: Option<(String, String)>,
    /// Registered `command-group` icons, by group name.
    group_icons: HashMap<String, String>,
    /// Whether the `:searchable` commands' choices were added to the
    /// commands stage (on the first keystroke).
    leaves_loaded: bool,
    /// Fractional rows of trackpad/wheel scroll not yet applied.
    scroll_accum: f32,
    /// When the last live preview ran, and whether one is waiting for the
    /// highlight to settle.
    previewed_at: Option<std::time::Instant>,
    preview_due: bool,
    /// The last change was a pointer hover: it moves the highlight but
    /// does not preview (sweeping the list would apply every row).
    hovering: bool,
    /// The host republished data the palette lists (the selected track's
    /// sounds): `:current` values and the choice stage are re-read on the
    /// next timer tick.
    stale: bool,
}

/// The `:searchable` commands' final choices as commands-stage entries,
/// kept across palette sessions: walking them runs every choice source
/// (the instrument tree, each instrument's presets), too slow for the
/// first keystroke. Dropped when the searchable commands (names, titles,
/// icons) change or the host reports their sources' data moved
/// ([`Editor::invalidate_palette_search`]); a choice source redefined
/// from the REPL shows once one of those moves.
#[derive(Debug)]
pub(super) struct PaletteSearchCache {
    commands: Vec<(String, String, Option<String>)>,
    entries: Vec<PaletteEntry>,
    group_icons: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
struct Stage {
    kind: StageKind,
    entries: Vec<PaletteEntry>,
    input: String,
    rows: Vec<Row>,
    /// Index into `rows`; always an item while any exist.
    selected: usize,
    scroll: usize,
}

#[derive(Debug, Clone)]
enum StageKind {
    Commands,
    /// Picking `command`'s next argument, after the `picked` ones.
    Choices {
        command: String,
        prompt: String,
        /// The arguments earlier stages picked, and their labels (the
        /// badge's breadcrumb).
        picked: Vec<Value>,
        picked_labels: Vec<String>,
        values: Vec<Value>,
        /// Live preview (`:preview`): the value the stage opened on, to
        /// restore if the pick is abandoned, and the choice previewed last.
        preview: Option<Preview>,
    },
}

#[derive(Debug, Clone)]
struct Preview {
    original: Option<Value>,
    shown: Option<usize>,
}

#[derive(Debug, Clone)]
enum Row {
    Header(String),
    Item(usize),
}

#[derive(Debug, Clone)]
struct PaletteEntry {
    /// What the row shows: a command's title, a choice's label, else the
    /// symbol.
    label: String,
    label_lower: String,
    /// The symbol M-x runs (commands stage); empty for a choice.
    name: String,
    name_lower: String,
    icon: Option<String>,
    group: Option<String>,
    pinned: bool,
    /// `:pinned N`: in the home list's top block, by `N`.
    pin_rank: Option<u32>,
    detail: Option<String>,
    shortcut: Option<String>,
    submenu: bool,
    checked: bool,
    glyph: Option<String>,
    /// Curated: a `:command` or a choice, ranked above uncurated names and
    /// matched by subsequence too.
    curated: bool,
    /// A `:searchable` command's final choice: runs `name` with these
    /// arguments, and matches on its label.
    args: Option<Vec<Value>>,
    /// A final choice listed as the pick before it (`:search :parent`): it
    /// ranks above that pick's deeper choices.
    stands_for_parent: bool,
}

impl PaletteEntry {
    fn new(label: String, name: String, curated: bool) -> Self {
        Self {
            label_lower: label.to_lowercase(),
            // Match the symbol, not its module: `eseq.application-menus/…`
            // would otherwise match "pat" for every menu command.
            name_lower: name.rsplit('/').next().unwrap_or(&name).to_lowercase(),
            label,
            name,
            icon: None,
            group: None,
            pinned: false,
            pin_rank: None,
            detail: None,
            shortcut: None,
            submenu: false,
            checked: false,
            glyph: None,
            curated,
            args: None,
            stands_for_parent: false,
        }
    }

    /// Shows the command's `:current` value: a check, or its label.
    fn set_current(&mut self, display: CurrentDisplay) {
        self.checked = matches!(display, CurrentDisplay::On);
        self.detail = match display {
            CurrentDisplay::Label(label) => Some(label),
            _ => None,
        };
    }
}

impl Stage {
    fn new(kind: StageKind, entries: Vec<PaletteEntry>) -> Self {
        Self { kind, entries, input: String::new(), rows: Vec::new(), selected: 0, scroll: 0 }
    }

    /// Rebuilds `rows` for `input` and highlights the first item.
    fn refilter(&mut self, history: &[String]) {
        self.rows = match (&self.kind, self.input.is_empty()) {
            (StageKind::Choices { .. }, true) => (0..self.entries.len()).map(Row::Item).collect(),
            (StageKind::Commands, true) => self.browse_rows(history),
            (_, false) => self.search_rows(),
        };
        self.scroll = 0;
        self.selected = self.first_item_from(0, 1).unwrap_or(0);
    }

    /// The empty-query home list, flat: the `:pinned N` commands by rank,
    /// recently run commands, then the other pinned ones in definition
    /// order — or every command when none is pinned.
    fn browse_rows(&self, history: &[String]) -> Vec<Row> {
        let curated: Vec<usize> = (0..self.entries.len()).filter(|&idx| self.entries[idx].curated).collect();
        let mut top: Vec<usize> = curated.iter().copied().filter(|&idx| self.entries[idx].pin_rank.is_some()).collect();
        top.sort_by_key(|&idx| self.entries[idx].pin_rank);
        let recent: Vec<usize> = history
            .iter()
            .filter_map(|name| curated.iter().copied().find(|&idx| &self.entries[idx].name == name))
            .filter(|idx| !top.contains(idx))
            .take(RECENT_SHOWN)
            .collect();
        let any_pinned = curated.iter().any(|&idx| self.entries[idx].pinned);
        let home = curated.iter().copied().filter(|idx| {
            !top.contains(idx) && !recent.contains(idx) && (!any_pinned || self.entries[*idx].pinned)
        });
        top.iter().chain(&recent).copied().chain(home).map(Row::Item).collect()
    }

    /// Ranked matches for `input`, sectioned: ungrouped and each group
    /// ordered by their best match, uncurated names last.
    fn search_rows(&self) -> Vec<Row> {
        let query = self.input.to_lowercase();
        let mut scored: Vec<(i32, usize)> = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(idx, entry)| {
                let score = if entry.curated {
                    let by_label = match_score(&entry.label_lower, &query);
                    let by_name = match_score(&entry.name_lower, &query).map(|score| score - 50);
                    by_label.max(by_name).map(|score| score + 2000)
                } else if entry.args.is_some() {
                    // A searchable choice: by its label, as a substring; one
                    // standing for its parent pick (an instrument) above
                    // every deeper choice (its presets).
                    let boost = if entry.stands_for_parent { PARENT_LEAF_BOOST } else { 0 };
                    substring_score(&entry.label_lower, &query).map(|score| score + boost)
                } else {
                    // A subsequence over thousands of names is mostly
                    // noise: uncurated names match as substrings only.
                    substring_score(&entry.name_lower, &query)
                };
                score.map(|score| (score, idx))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        scored.truncate(MATCH_LIMIT);

        // Sections in order of their best match (`scored` is best first).
        let section = |idx: usize| -> Option<&str> {
            let entry = &self.entries[idx];
            if entry.curated || entry.args.is_some() { entry.group.as_deref() } else { Some(FUNCTIONS_SECTION) }
        };
        let mut sections: Vec<Option<&str>> = Vec::new();
        for (_, idx) in &scored {
            let key = section(*idx);
            if !sections.contains(&key) {
                sections.push(key);
            }
        }
        // Ungrouped commands lead (they have no header, so after another
        // section they would read as its members); uncurated names close.
        sections.sort_by_key(|key| (key.is_some(), *key == Some(FUNCTIONS_SECTION)));
        let mut rows = Vec::with_capacity(scored.len() + sections.len());
        for key in sections {
            if let Some(name) = key {
                rows.push(Row::Header(name.to_string()));
            }
            rows.extend(scored.iter().filter(|(_, idx)| section(*idx) == key).map(|(_, idx)| Row::Item(*idx)));
        }
        rows
    }

    /// The first item row from `from` stepping by `step`, if any.
    fn first_item_from(&self, from: usize, step: isize) -> Option<usize> {
        let mut at = from as isize;
        while (0..self.rows.len() as isize).contains(&at) {
            if matches!(self.rows[at as usize], Row::Item(_)) {
                return Some(at as usize);
            }
            at += step;
        }
        None
    }

    fn selected_entry(&self) -> Option<&PaletteEntry> {
        self.selected_index().map(|entry| &self.entries[entry])
    }

    /// Rows this stage shows at once: what fit the window when the palette
    /// was last drawn, at this stage's row height (glyph rows are taller).
    fn page(&self) -> usize {
        command_palette::page_size(self.has_glyphs())
    }

    fn has_glyphs(&self) -> bool {
        self.entries.iter().any(|entry| entry.glyph.is_some())
    }

    /// Whether live host data keeps this stage current: the home list (its
    /// `:current` values) or a glyph list (Apply Sound's sounds), which may
    /// open empty before the host publishes. Other pick stages read their
    /// choices once.
    fn is_live(&self) -> bool {
        matches!(self.kind, StageKind::Commands) || self.entries.is_empty() || self.has_glyphs()
    }

    fn selected_index(&self) -> Option<usize> {
        match self.rows.get(self.selected)? {
            Row::Item(entry) => Some(*entry),
            Row::Header(_) => None,
        }
    }

    /// Moves the highlight over items; ±1 steps wrap, page moves clamp.
    /// Keeps the highlight inside the `page`-row scroll window.
    fn move_selection(&mut self, key: ReadKey, page: usize) {
        let items: Vec<usize> =
            (0..self.rows.len()).filter(|&at| matches!(self.rows[at], Row::Item(_))).collect();
        let Some(current) = items.iter().position(|&at| at == self.selected) else {
            return;
        };
        let count = items.len() as isize;
        let target = match key {
            ReadKey::Step(delta) => (current as isize + delta).rem_euclid(count),
            ReadKey::Page(pages) => {
                // A page is `page` rows; land on the item nearest that far.
                let goal = self.selected as isize + pages * page as isize;
                let nearest = items.iter().position(|&at| at as isize >= goal).unwrap_or(items.len() - 1);
                if pages < 0 { nearest.min(current) as isize } else { nearest.max(current) as isize }
            }
            _ => current as isize,
        };
        self.selected = items[target as usize];
        self.scroll_to_selection(page);
    }

    /// Scrolls the list `delta` rows (positive: down), keeping the
    /// highlight on an item inside the visible window. False when nothing
    /// moved.
    fn scroll_by(&mut self, delta: isize, page: usize) -> bool {
        let max = self.rows.len().saturating_sub(page) as isize;
        let scroll = (self.scroll as isize + delta).clamp(0, max) as usize;
        if scroll == self.scroll {
            return false;
        }
        self.scroll = scroll;
        let window = scroll..scroll + page;
        if !window.contains(&self.selected) {
            let inside = if self.selected < scroll {
                self.first_item_from(scroll, 1)
            } else {
                self.first_item_from((scroll + page).min(self.rows.len()).saturating_sub(1), -1)
            };
            if let Some(at) = inside.filter(|at| window.contains(at)) {
                self.selected = at;
            }
        }
        true
    }

    fn scroll_to_selection(&mut self, page: usize) {
        // Keep a section's header in view with its first item.
        let top = if self.selected > 0 && matches!(self.rows[self.selected - 1], Row::Header(_)) {
            self.selected - 1
        } else {
            self.selected
        };
        if top < self.scroll {
            self.scroll = top;
        } else if self.selected >= self.scroll + page {
            self.scroll = self.selected + 1 - page;
        }
    }

    /// Highlights the `row`th visible row. False when it is a header, past
    /// the end, or already highlighted.
    fn select_visible_row(&mut self, row: usize) -> bool {
        let at = self.scroll + row;
        if at == self.selected || !matches!(self.rows.get(at), Some(Row::Item(_))) {
            return false;
        }
        self.selected = at;
        true
    }
}

impl PaletteState {
    fn top(&self) -> &Stage {
        self.stages.last().expect("the commands stage is never popped")
    }

    fn top_mut(&mut self) -> &mut Stage {
        self.stages.last_mut().expect("the commands stage is never popped")
    }

    fn frame(&self, page: usize) -> PaletteFrame {
        let stage = self.top();
        let rows = stage
            .rows
            .iter()
            .skip(stage.scroll)
            .take(page)
            .map(|row| match row {
                Row::Header(name) => PaletteRowFrame {
                    header: true,
                    label: name.clone(),
                    icon: self.group_icons.get(name).cloned(),
                    detail: None,
                    shortcut: None,
                    submenu: false,
                    checked: false,
                    glyph: None,
                },
                Row::Item(entry) => {
                    let entry = &stage.entries[*entry];
                    PaletteRowFrame {
                        header: false,
                        label: entry.label.clone(),
                        icon: entry.icon.clone(),
                        detail: entry.detail.clone(),
                        shortcut: entry.shortcut.clone(),
                        submenu: entry.submenu,
                        checked: entry.checked,
                        glyph: entry.glyph.clone(),
                    }
                }
            })
            .collect();
        let (badge, placeholder, empty) = match &stage.kind {
            StageKind::Commands => (None, "Search commands…".to_string(), "No matching commands"),
            StageKind::Choices { prompt, picked_labels, .. } => {
                let mut crumbs = picked_labels.clone();
                crumbs.push(prompt.clone());
                (Some(crumbs.join(" › ")), format!("Select {prompt}…"), "No matches")
            }
        };
        let has_items = stage.selected_entry().is_some();
        PaletteFrame {
            badge,
            placeholder,
            input: stage.input.clone(),
            rows,
            selected: stage
                .selected
                .checked_sub(stage.scroll)
                .filter(|&row| has_items && row < page),
            empty_text: (!has_items).then(|| empty.to_string()),
            footer: self.footer.clone(),
            scroll: (stage.rows.len() > page).then_some((stage.scroll, stage.rows.len())),
        }
    }
}

/// The lowest score a substring match earns.
const SUBSTRING_SCORE: i32 = 600;

/// How well `query` matches `text` as a substring (both lowercase): a
/// prefix beats a word start beats any other position.
fn substring_score(text: &str, query: &str) -> Option<i32> {
    let shortness = 100 - (text.len().min(100) as i32);
    let at = text.find(query)?;
    let score = if at == 0 {
        1000
    } else if text[..at].ends_with([' ', '-', '/', '.']) {
        800
    } else {
        SUBSTRING_SCORE
    };
    Some(score + shortness)
}

/// [`substring_score`], falling back to an in-order subsequence that
/// rewards characters starting a word ("ct" → "change theme"). `None` when
/// the query's characters don't all appear in order.
fn match_score(text: &str, query: &str) -> Option<i32> {
    if let Some(score) = substring_score(text, query) {
        return Some(score);
    }
    let mut chars = text.chars();
    let mut score = 0;
    let mut prev: Option<char> = None;
    for wanted in query.chars() {
        loop {
            let ch = chars.next()?;
            let at_word_start = prev.is_none_or(|p| matches!(p, ' ' | '-' | '/' | '.'));
            prev = Some(ch);
            if ch == wanted {
                score += if at_word_start { 20 } else { 5 };
                break;
            }
        }
    }
    Some(score)
}

/// A `key_str` binding as the palette's shortcut column shows it, in the
/// platform's notation (`S-s-p` → `⇧⌘P` on macOS); chords stay
/// space-separated (`C-x 2` → `⌃X 2`).
pub(super) fn binding_label(binding: &str) -> String {
    binding
        .split(' ')
        .map(|part| {
            let mut key = part;
            while key.len() > 2 && matches!(key.get(..2), Some("C-" | "M-" | "S-" | "s-")) {
                key = &key[2..];
            }
            let modifiers = &part[..part.len() - key.len()];
            // `key_str` spells keys it has no name for with `{:?}`.
            let key = match key {
                "Tab" => "⇥".to_string(),
                "BackTab" => "⇤".to_string(),
                "Home" => "↖".to_string(),
                "End" => "↘".to_string(),
                "PageUp" => "⇞".to_string(),
                "PageDown" => "⇟".to_string(),
                key if key.starts_with("F(") => key.replace(['(', ')'], ""),
                key => key.to_string(),
            };
            crate::widget_render::patcher::patcher_key_label(&format!("{modifiers}{key}"))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

impl Editor {
    /// Opens the palette on its commands stage (`/`, `(command-palette)`;
    /// M-x stays the minibuffer prompt).
    pub(super) fn open_command_palette(&mut self) {
        self.dismiss_palette();
        self.completion = None;
        self.minibuffer = None;
        command_palette::clear_hit_map();
        let bindings = self.binding_labels_by_handler();
        struct Command {
            name: String,
            title: String,
            icon: Option<String>,
            group: Option<String>,
            pinned: bool,
            pin_rank: Option<u32>,
            shortcut: Option<String>,
            has_choices: bool,
            has_current: bool,
        }
        let commands: Vec<Command> = self
            .runtime
            .commands()
            .into_iter()
            .map(|(name, header)| Command {
                title: header.command.clone().unwrap_or_else(|| name.clone()),
                icon: header.icon.clone(),
                group: header.group.clone(),
                pinned: header.pinned,
                pin_rank: header.pin_rank,
                shortcut: header.shortcut.as_deref().map(binding_label),
                has_choices: !header.choices.is_empty(),
                has_current: header.current.is_some(),
                name,
            })
            .collect();
        let mut entries = Vec::new();
        for command in &commands {
            let mut entry = PaletteEntry::new(command.title.clone(), command.name.clone(), true);
            entry.icon = command.icon.clone();
            entry.group = command.group.clone();
            entry.pinned = command.pinned;
            entry.pin_rank = command.pin_rank;
            entry.submenu = command.has_choices;
            entry.shortcut = command.shortcut.clone().or_else(|| bindings.get(&command.name).cloned());
            if command.has_current {
                entry.set_current(self.command_current_display(&command.name, command.has_choices));
            }
            entries.push(entry);
        }
        let command_set: HashSet<&str> = commands.iter().map(|command| command.name.as_str()).collect();
        for name in self.non_command_mx_names(&command_set) {
            let mut entry = PaletteEntry::new(name.clone(), name.clone(), false);
            entry.shortcut = bindings.get(&name).cloned();
            entries.push(entry);
        }
        let group_icons =
            self.runtime.command_groups().into_iter().filter_map(|group| Some((group.name, group.icon?))).collect();
        let mut stage = Stage::new(StageKind::Commands, entries);
        stage.refilter(&self.palette_history);
        self.store_palette(Box::new(PaletteState {
            stages: vec![stage],
            footer: None,
            group_icons,
            leaves_loaded: false,
            scroll_accum: 0.0,
            previewed_at: None,
            preview_due: false,
            hovering: false,
            stale: false,
        }));
    }

    /// Moves an open palette onto `command`'s choice stage, with the
    /// commands stage beneath it for Backspace.
    pub(super) fn open_palette_choices(&mut self, command: &str) {
        let Some(MinibufferMode::Palette(mut state)) = self.minibuffer_input.take() else {
            return;
        };
        if self.runtime.definition_header(command).is_some_and(|header| !header.choices.is_empty()) {
            self.push_choice_stage(&mut state, command.to_string(), Vec::new(), Vec::new());
        } else {
            self.show_transient_message(format!("No command with :choices named {command}"));
        }
        self.store_palette(state);
    }

    /// Adds every `:searchable` command's final choices to the commands
    /// stage, each under a section named for the command with the picks
    /// before it as its value (`808 Kick` · `Kicks › Digi Kick`). Walked
    /// on the first keystroke after they changed (see
    /// [`PaletteSearchCache`]), else copied from the last walk.
    fn load_search_leaves(&mut self, state: &mut PaletteState) {
        state.leaves_loaded = true;
        let commands = self.searchable_commands();
        if self.palette_search_cache.as_ref().is_none_or(|cache| cache.commands != commands) {
            let mut entries = Vec::new();
            let mut group_icons = Vec::new();
            for leaf in self.walk_search_leaves(commands.clone()) {
                let (label, crumbs) = leaf.labels.split_last().expect("a leaf has its own label");
                let mut entry = PaletteEntry::new(label.clone(), leaf.command.clone(), false);
                entry.group = Some(leaf.title.clone());
                entry.detail = leaf.detail.clone().or_else(|| (!crumbs.is_empty()).then(|| crumbs.join(" › ")));
                entry.args = Some(leaf.args);
                entry.stands_for_parent = leaf.stands_for_parent;
                entries.push(entry);
                if let Some(icon) = leaf.icon {
                    group_icons.push((leaf.title, icon));
                }
            }
            self.palette_search_cache = Some(PaletteSearchCache { commands, entries, group_icons });
        }
        let cache = self.palette_search_cache.as_ref().expect("filled above");
        state.stages[0].entries.extend(cache.entries.iter().cloned());
        for (title, icon) in &cache.group_icons {
            state.group_icons.entry(title.clone()).or_insert_with(|| icon.clone());
        }
    }

    /// The host's data behind the `:searchable` commands' choices moved
    /// (the instrument library, a preset bank): the next palette search
    /// walks them again.
    pub fn invalidate_palette_search(&mut self) {
        self.palette_search_cache = None;
    }

    /// The `:searchable` commands with choices: name, title and icon.
    fn searchable_commands(&self) -> Vec<(String, String, Option<String>)> {
        self.runtime
            .commands()
            .into_iter()
            .filter(|(_, header)| header.searchable && !header.choices.is_empty())
            .map(|(name, header)| (name, header.command.clone().unwrap_or_default(), header.icon.clone()))
            .collect()
    }

    /// Every final choice of the `searchable` commands.
    fn walk_search_leaves(&mut self, searchable: Vec<(String, String, Option<String>)>) -> Vec<SearchLeaf> {
        let mut leaves = Vec::new();
        for (name, title, icon) in searchable {
            let Ok(sources) = self.runtime.command_choice_sources(&name) else {
                continue;
            };
            let leaf = SearchLeaf {
                command: name,
                title,
                icon,
                args: Vec::new(),
                labels: Vec::new(),
                details: Vec::new(),
                detail: None,
                stands_for_parent: false,
            };
            let mut own = Vec::new();
            self.collect_search_leaves(&sources, leaf, &mut own);
            leaves.extend(own);
        }
        // A choice reachable two ways (an instrument listed under two ids)
        // would show twice with the same breadcrumb: keep the first.
        let mut seen = HashSet::new();
        leaves.retain(|leaf| seen.insert((leaf.command.clone(), leaf.labels.clone(), leaf.details.clone())));
        leaves
    }

    /// Walks a command's choice steps from `at` (its picks so far),
    /// collecting each final choice with the arguments and labels leading
    /// to it. `sources` are the steps' resolved sources.
    fn collect_search_leaves(&mut self, sources: &[Value], at: SearchLeaf, out: &mut Vec<SearchLeaf>) {
        let Some(source) = sources.get(at.args.len()) else {
            return;
        };
        let Ok(choices) = self.runtime.choices_from_source(&at.command, source, &at.args) else {
            return;
        };
        for choice in choices.into_iter().filter(|choice| choice.searchable) {
            if out.len() >= SEARCH_LEAF_LIMIT {
                return;
            }
            let mut next = at.clone();
            next.args.push(choice.value);
            if choice.stands_for_parent {
                // Listed as the pick before it, with that pick's detail.
                if at.labels.is_empty() || next.args.len() < sources.len() {
                    continue;
                }
                next.detail = at.details.last().cloned().flatten();
                next.stands_for_parent = true;
                out.push(next);
                continue;
            }
            next.labels.push(choice.label);
            next.details.push(choice.detail.clone());
            if next.args.len() < sources.len() {
                self.collect_search_leaves(sources, next, out);
            } else {
                next.detail = if sources.len() == 1 { choice.detail } else { None };
                out.push(next);
            }
        }
    }

    /// How a command row shows its `:current` value: a choice's label (or
    /// the value) for a command with choices or a non-boolean value; a
    /// check for a true state; nothing for a false one.
    fn command_current_display(&mut self, command: &str, has_choices: bool) -> CurrentDisplay {
        let Some(current) = self.runtime.command_current(command) else {
            return CurrentDisplay::Nothing;
        };
        if !has_choices {
            match current {
                Value::Bool(true) => return CurrentDisplay::On,
                Value::Bool(false) | Value::Nil => return CurrentDisplay::Nothing,
                _ => {}
            }
        }
        let choices = self.runtime.command_choices(command, &[]).unwrap_or_default();
        CurrentDisplay::Label(match choices.into_iter().find(|choice| choice.value == current) {
            Some(choice) => choice.label,
            None => match &current {
                Value::String(text) => text.clone(),
                other => self.runtime.format_value(other),
            },
        })
    }

    /// Opens `command`'s choice stage, highlighting its current value.
    /// Opens the stage that picks `command`'s argument after `picked` (its
    /// first when `picked` is empty). `:current` and `:preview` belong to
    /// one-step commands.
    fn push_choice_stage(
        &mut self,
        state: &mut PaletteState,
        command: String,
        picked: Vec<Value>,
        picked_labels: Vec<String>,
    ) {
        let choices = match self.runtime.command_choices(&command, &picked) {
            Ok(choices) => choices,
            Err(error) => {
                self.show_transient_message(format!("Error: {error}"));
                return;
            }
        };
        let one_step = self.runtime.definition_header(&command).is_some_and(|header| header.choices.len() == 1);
        let current = if one_step { self.runtime.command_current(&command) } else { None };
        let prompt = self
            .runtime
            .definition_header(&command)
            .and_then(|header| header.step_prompt(picked.len()))
            .unwrap_or_else(|| "Value".to_string());
        let previews =
            one_step && self.runtime.definition_header(&command).is_some_and(|header| header.preview.is_some());
        let original = current.clone();
        let mut values = Vec::with_capacity(choices.len());
        let entries = choices
            .into_iter()
            .map(|choice| {
                let mut entry = PaletteEntry::new(choice.label, String::new(), true);
                entry.detail = choice.detail;
                entry.glyph = choice.glyph;
                entry.checked = current.as_ref() == Some(&choice.value);
                values.push(choice.value);
                entry
            })
            .collect();
        // Without a current value there is nothing to put back on Esc, so
        // browsing would leave the last previewed choice applied.
        let restorable = original.as_ref().is_some_and(|value| match value {
            Value::Nil => false,
            Value::String(text) => !text.is_empty(),
            _ => true,
        });
        let preview = (previews && restorable).then_some(Preview { original, shown: None });
        let kind = StageKind::Choices { command, prompt, picked, picked_labels, values, preview };
        let mut stage = Stage::new(kind, entries);
        stage.refilter(&[]);
        if let Some(at) =
            stage.rows.iter().position(|row| matches!(row, Row::Item(entry) if stage.entries[*entry].checked))
        {
            stage.selected = at;
            stage.scroll_to_selection(stage.page());
        }
        // Opening on the current value previews nothing new.
        let opened_on = stage.selected_index();
        if let StageKind::Choices { preview: Some(preview), .. } = &mut stage.kind {
            preview.shown = opened_on;
        }
        state.stages.push(stage);
    }

    pub(super) fn handle_palette_key(&mut self, key: KeyEvent, mut state: Box<PaletteState>) {
        match classify_read_key(key) {
            ReadKey::Cancel => {
                for stage in state.stages.iter().rev() {
                    self.revert_palette_preview(stage);
                }
                self.close_palette();
                return;
            }
            ReadKey::Accept => {
                self.accept_palette_selection(state);
                return;
            }
            read @ (ReadKey::Step(_) | ReadKey::Page(_)) => {
                let stage = state.top_mut();
                stage.move_selection(read, stage.page());
            }
            ReadKey::Backspace => {
                let stage = state.top_mut();
                if stage.input.pop().is_some() {
                    stage.refilter(&self.palette_history);
                } else if state.stages.len() > 1
                    && let Some(popped) = state.stages.pop()
                {
                    self.revert_palette_preview(&popped);
                }
            }
            ReadKey::Insert(c) => {
                if state.stages.len() == 1 && !state.leaves_loaded {
                    self.load_search_leaves(&mut state);
                }
                let stage = state.top_mut();
                stage.input.push(c);
                stage.refilter(&self.palette_history);
            }
            ReadKey::Ignore => {}
        }
        self.store_palette(state);
    }

    /// Puts `state` back in the minibuffer slot with its footer refreshed
    /// and, on a live-preview stage, the highlighted choice applied.
    fn store_palette(&mut self, mut state: Box<PaletteState>) {
        if !std::mem::take(&mut state.hovering) {
            self.sync_palette_preview(&mut state, false);
        }
        state.footer = self.palette_footer(&state);
        self.minibuffer_input = Some(MinibufferMode::Palette(state));
        self.mark_needs_redraw();
    }

    /// On a live-preview stage, applies the highlighted choice through the
    /// command's `:preview` when it differs from the one shown last.
    fn sync_palette_preview(&mut self, state: &mut PaletteState, settled: bool) {
        let now = std::time::Instant::now();
        let too_soon = state.previewed_at.is_some_and(|at| now.duration_since(at) < PREVIEW_SETTLE);
        let stage = &mut state.stages.last_mut().expect("the commands stage is never popped");
        let selected = stage.selected_index();
        let StageKind::Choices { command, values, preview: Some(preview), .. } = &mut stage.kind else {
            return;
        };
        let Some(idx) = selected.filter(|&idx| preview.shown != Some(idx)) else {
            state.preview_due = false;
            return;
        };
        if too_soon && !settled {
            // `update_palette_preview` applies it once the highlight rests.
            state.preview_due = true;
            return;
        }
        preview.shown = Some(idx);
        state.preview_due = false;
        state.previewed_at = Some(now);
        let (command, value) = (command.clone(), values[idx].clone());
        if let Err(error) = self.runtime.command_preview(&command, value) {
            self.show_transient_message(format!("Error: {error}"));
        }
        self.refresh_runtime_side_effects();
    }

    /// The host republished data an open palette may list; see
    /// [`PaletteState::stale`].
    pub fn mark_palette_choices_stale(&mut self) {
        if let Some(MinibufferMode::Palette(state)) = &mut self.minibuffer_input {
            state.stale = true;
            self.mark_needs_redraw();
        }
    }

    /// Re-reads a stale palette: the commands' `:current` values, and the
    /// choice stage on top (keeping its input and highlighted value). A
    /// live-preview stage is left alone; its original value is in play.
    fn refresh_stale_palette(&mut self) {
        if !matches!(&self.minibuffer_input, Some(MinibufferMode::Palette(state)) if state.stale) {
            return;
        }
        let Some(MinibufferMode::Palette(mut state)) = self.minibuffer_input.take() else {
            return;
        };
        state.stale = false;
        let with_current: Vec<(usize, String, bool)> = state.stages[0]
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.curated && entry.args.is_none())
            .filter_map(|(at, entry)| {
                let header = self.runtime.definition_header(&entry.name)?;
                header.current.is_some().then(|| (at, entry.name.clone(), !header.choices.is_empty()))
            })
            .collect();
        for (at, name, has_choices) in with_current {
            let display = self.command_current_display(&name, has_choices);
            state.stages[0].entries[at].set_current(display);
        }
        // Only a live list re-reads: a stale mark must not rerun another
        // stage's choice source.
        let refreshable = state.stages.len() > 1
            && state.top().is_live()
            && matches!(&state.top().kind, StageKind::Choices { preview: None, .. });
        if refreshable && let Some(old) = state.stages.pop() {
            let StageKind::Choices { command, picked, picked_labels, values, .. } = &old.kind else {
                unreachable!("checked above");
            };
            let was = old.selected_index().map(|entry| values[entry].clone());
            let depth = state.stages.len();
            self.push_choice_stage(&mut state, command.clone(), picked.clone(), picked_labels.clone());
            if state.stages.len() == depth {
                state.stages.push(old);
            } else {
                let stage = state.top_mut();
                let page = stage.page();
                if !old.input.is_empty() {
                    stage.input = old.input.clone();
                    stage.refilter(&[]);
                }
                // The value highlighted before, else the current one.
                let StageKind::Choices { values, .. } = &stage.kind else { unreachable!("just pushed") };
                let at = was
                    .and_then(|was| values.iter().position(|value| *value == was))
                    .or_else(|| stage.entries.iter().position(|entry| entry.checked));
                if let Some(row) =
                    at.and_then(|at| stage.rows.iter().position(|row| matches!(row, Row::Item(entry) if *entry == at)))
                {
                    stage.selected = row;
                }
                stage.scroll = old.scroll.min(stage.rows.len().saturating_sub(page));
                stage.scroll_to_selection(page);
            }
        }
        self.store_palette(state);
    }

    /// Timer tick: applies a live preview that was waiting for the
    /// highlight to settle.
    pub(super) fn update_palette_preview(&mut self) {
        self.refresh_stale_palette();
        let due = match &self.minibuffer_input {
            Some(MinibufferMode::Palette(state)) => {
                state.preview_due
                    && state.previewed_at.is_none_or(|at| at.elapsed() >= PREVIEW_SETTLE)
            }
            _ => false,
        };
        if due && let Some(MinibufferMode::Palette(mut state)) = self.minibuffer_input.take() {
            self.sync_palette_preview(&mut state, true);
            self.minibuffer_input = Some(MinibufferMode::Palette(state));
            self.mark_needs_redraw();
        }
    }

    /// An abandoned live-preview stage: puts back the value it opened on.
    fn revert_palette_preview(&mut self, stage: &Stage) {
        let StageKind::Choices { command, values, preview: Some(preview), .. } = &stage.kind else {
            return;
        };
        let Some(original) = preview.original.clone() else {
            return;
        };
        if preview.shown.is_some_and(|shown| values[shown] == original) {
            return;
        }
        if let Err(error) = self.runtime.command_preview(command, original) {
            self.show_transient_message(format!("Error: {error}"));
        }
        self.refresh_runtime_side_effects();
    }

    /// Closes an open palette as Esc would (reverting live previews), for
    /// whatever replaces it in the minibuffer slot.
    pub(super) fn dismiss_palette(&mut self) {
        if let Some(MinibufferMode::Palette(state)) = self.minibuffer_input.take() {
            for stage in state.stages.iter().rev() {
                self.revert_palette_preview(stage);
            }
            self.close_palette();
        }
    }

    /// The palette is already out of the minibuffer slot: forget its drawn
    /// geometry and redraw without it.
    fn close_palette(&mut self) {
        self.minibuffer = None;
        command_palette::clear_hit_map();
        self.mark_needs_redraw();
    }

    /// Enter (or a click) on the highlighted row: a command with
    /// `:choices` opens its choice stage, any other command runs, and a
    /// choice runs its command with the chosen value. With nothing
    /// highlighted on the commands stage, the raw input runs as a name.
    fn accept_palette_selection(&mut self, mut state: Box<PaletteState>) {
        let stage = state.top();
        let selected = stage.selected_entry().cloned();
        match (&stage.kind, selected) {
            (StageKind::Commands, Some(entry)) if entry.submenu => {
                self.push_choice_stage(&mut state, entry.name, Vec::new(), Vec::new());
                self.store_palette(state);
            }
            (StageKind::Commands, Some(entry)) => {
                self.close_palette();
                if entry.curated || entry.args.is_some() {
                    self.record_palette_use(&entry.name);
                }
                match &entry.args {
                    Some(args) => {
                        self.call_lisp_handler_with_args(&entry.name, args);
                    }
                    None => self.execute_mx_command(&entry.name),
                }
            }
            (StageKind::Commands, None) => {
                let input = stage.input.clone();
                self.close_palette();
                if !input.is_empty() {
                    self.execute_mx_command(&input);
                }
            }
            (StageKind::Choices { command, picked, picked_labels, values, .. }, Some(entry)) => {
                let command = command.clone();
                let mut args = picked.clone();
                args.push(values[stage.selected_index().expect("an item is selected")].clone());
                let steps = self.runtime.definition_header(&command).map_or(1, |header| header.choices.len());
                if args.len() < steps {
                    // More to pick: the next stage, with this pick in the
                    // breadcrumb.
                    let mut labels = picked_labels.clone();
                    labels.push(entry.label);
                    self.push_choice_stage(&mut state, command, args, labels);
                    self.store_palette(state);
                    return;
                }
                // A live preview already shows the pick; running the command
                // makes it stick (and records it).
                self.close_palette();
                self.record_palette_use(&command);
                self.call_lisp_handler_with_args(&command, &args);
            }
            (StageKind::Choices { .. }, None) => self.store_palette(state),
        }
    }

    /// Pointer input while the palette is open: hover highlights a row, a
    /// click on it accepts it, the field's clear button empties it, the
    /// wheel moves the highlight, and a press outside closes the palette.
    /// Returns true when the event was consumed.
    pub(super) fn handle_palette_mouse(&mut self, mouse: MouseEvent, col: f32, row: f32) -> bool {
        if !self.palette_is_open() {
            return false;
        }
        let Some(hit) = command_palette::hit_map().map(|map| map.hit(col, row)) else {
            return false;
        };
        let page = self.palette_page();
        let changed = match (mouse.kind, hit) {
            (MouseEventKind::Down(_), PaletteHit::Outside) => {
                if let Some(MinibufferMode::Palette(state)) = self.minibuffer_input.take() {
                    for stage in state.stages.iter().rev() {
                        self.revert_palette_preview(stage);
                    }
                }
                self.close_palette();
                return true;
            }
            (_, PaletteHit::Outside) => return false,
            (MouseEventKind::Up(MouseButton::Left), PaletteHit::Row(row)) => {
                let Some(MinibufferMode::Palette(mut state)) = self.minibuffer_input.take() else {
                    return false;
                };
                let stage = state.top_mut();
                stage.select_visible_row(row);
                if stage.scroll + row == stage.selected && stage.selected_entry().is_some() {
                    self.accept_palette_selection(state);
                } else {
                    self.minibuffer_input = Some(MinibufferMode::Palette(state));
                }
                return true;
            }
            (MouseEventKind::Up(MouseButton::Left), PaletteHit::Clear) => {
                let history = self.palette_history.clone();
                let Some(MinibufferMode::Palette(state)) = &mut self.minibuffer_input else {
                    return false;
                };
                let stage = state.top_mut();
                let had_input = !stage.input.is_empty();
                stage.input.clear();
                stage.refilter(&history);
                had_input
            }
            (kind, hit) => {
                let Some(MinibufferMode::Palette(state)) = &mut self.minibuffer_input else {
                    return false;
                };
                let stage = state.top_mut();
                let before = stage.selected;
                let mut hovered = false;
                let mut scrolled = false;
                match (kind, hit) {
                    (MouseEventKind::Moved, PaletteHit::Row(row)) => {
                        hovered = stage.select_visible_row(row);
                    }
                    (MouseEventKind::Down(_), PaletteHit::Row(row)) => {
                        stage.select_visible_row(row);
                    }
                    (MouseEventKind::ScrollDown, _) => scrolled = stage.scroll_by(1, page),
                    (MouseEventKind::ScrollUp, _) => scrolled = stage.scroll_by(-1, page),
                    _ => {}
                }
                let changed = scrolled || stage.selected != before;
                state.hovering = hovered;
                changed
            }
        };
        if changed && let Some(MinibufferMode::Palette(state)) = self.minibuffer_input.take() {
            self.store_palette(state);
        }
        true
    }

    /// Trackpad and smooth-wheel scrolling (pixel deltas, positive toward
    /// the top) over the panel scroll its list; over anything else it falls
    /// through. Returns true when consumed.
    pub(super) fn handle_palette_touchpad_scroll(&mut self, col: f32, row: f32, delta_y: f32) -> bool {
        if !self.palette_is_open() {
            return false;
        }
        let Some(map) = command_palette::hit_map() else {
            return false;
        };
        if map.hit(col, row) == PaletteHit::Outside {
            return false;
        }
        let page = self.palette_page();
        let Some(MinibufferMode::Palette(state)) = &mut self.minibuffer_input else {
            return false;
        };
        // The tile scroll's pixel-to-cell rate, in palette rows.
        state.scroll_accum += -delta_y * 0.05 / map.row_height.max(0.1);
        let whole = state.scroll_accum.trunc();
        state.scroll_accum -= whole;
        let moved = whole != 0.0 && state.top_mut().scroll_by(whole as isize, page);
        if moved && let Some(MinibufferMode::Palette(state)) = self.minibuffer_input.take() {
            self.store_palette(state);
        }
        true
    }

    #[cfg(test)]
    fn palette_scroll_consumed_at(&mut self, col: f32, row: f32) -> bool {
        self.handle_palette_touchpad_scroll(col, row, -10.0)
    }

    /// Rows the open palette's top stage shows at once.
    fn palette_page(&self) -> usize {
        match &self.minibuffer_input {
            Some(MinibufferMode::Palette(state)) => state.top().page(),
            _ => command_palette::page_size(false),
        }
    }

    pub fn palette_is_open(&self) -> bool {
        matches!(self.minibuffer_input, Some(MinibufferMode::Palette(_)))
    }

    /// Whether the open palette shows a stage live host data keeps current
    /// (see `Stage::is_live`).
    pub fn palette_shows_live_choices(&self) -> bool {
        match &self.minibuffer_input {
            Some(MinibufferMode::Palette(state)) => state.top().is_live(),
            _ => false,
        }
    }

    /// The palette as the backends draw it, while open.
    pub fn palette_frame(&self) -> Option<PaletteFrame> {
        match &self.minibuffer_input {
            Some(MinibufferMode::Palette(state)) => Some(state.frame(state.top().page())),
            _ => None,
        }
    }

    /// The status-line echo of an open palette (the TUI's only view of it).
    pub(super) fn palette_prompt(state: &PaletteState) -> String {
        let stage = state.top();
        let prefix = match &stage.kind {
            StageKind::Commands => "M-x".to_string(),
            StageKind::Choices { prompt, .. } => format!("{prompt}:"),
        };
        match stage.selected_entry() {
            Some(entry) if !stage.input.is_empty() => format!("{prefix} {}  [{}]", stage.input, entry.label),
            _ => format!("{prefix} {}", stage.input),
        }
    }

    /// Where the palette keeps its recently-run list (one name per line);
    /// `None` keeps it in memory only. Reads the list now.
    pub fn set_palette_history_path(&mut self, path: Option<PathBuf>) {
        self.palette_history = path
            .as_ref()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .map(|text| {
                text.lines().filter(|line| !line.is_empty()).take(HISTORY_LIMIT).map(str::to_string).collect()
            })
            .unwrap_or_default();
        self.palette_history_path = path;
    }

    fn record_palette_use(&mut self, name: &str) {
        if self.palette_history.first().is_some_and(|recent| recent == name) {
            return;
        }
        self.palette_history.retain(|recent| recent != name);
        self.palette_history.insert(0, name.to_string());
        self.palette_history.truncate(HISTORY_LIMIT);
        let Some(path) = &self.palette_history_path else {
            return;
        };
        let mut text = self.palette_history.join("\n");
        text.push('\n');
        // Write beside, then rename: a crash mid-write never truncates it.
        let temp = path.with_extension("tmp");
        let written = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&temp, text))
            .and_then(|()| std::fs::rename(&temp, path));
        if let Err(error) = written {
            eprintln!("command palette: could not save history to {}: {error}", path.display());
        }
    }

    /// Display label of each handler's shortest key binding (ties broken
    /// by spelling, so the label is stable).
    fn binding_labels_by_handler(&self) -> HashMap<String, String> {
        let mut shortest: HashMap<String, String> = HashMap::new();
        let builtin_keys = self.builtins.iter().map(|(key, name)| (super::commands::key_str(*key), name));
        let lisp_keys = self.lisp_bindings.iter().map(|(key, name)| (key.clone(), name));
        for (key, handler) in builtin_keys.chain(lisp_keys) {
            let handler = crate::modules::strip_implicit(handler).to_string();
            let keep = shortest.get(&handler).is_none_or(|current| (key.len(), &key) < (current.len(), current));
            if keep {
                shortest.insert(handler, key);
            }
        }
        shortest.into_iter().map(|(handler, key)| (handler, binding_label(&key))).collect()
    }

    /// Signature and first doc line of the highlighted command (commands
    /// stage), or of the command a choice stage picks for.
    fn palette_footer(&mut self, state: &PaletteState) -> Option<(String, String)> {
        let stage = state.top();
        let name = match &stage.kind {
            StageKind::Commands => stage.selected_entry()?.name.clone(),
            StageKind::Choices { command, .. } => command.clone(),
        };
        if let Some(header) = self.runtime.definition_header(&name) {
            return Some((header.signature.clone(), header.summary().unwrap_or_default().to_string()));
        }
        let metadata = self.runtime.completion_metadata_for(&name)?;
        let summary = crate::lang::def_header::first_doc_line(&metadata.docs).unwrap_or_default().to_string();
        Some((metadata.signature, summary))
    }
}

/// A `:searchable` command's final choice: the command to run with `args`,
/// the labels of each pick (the last is the row's title), and the section it
/// lists under.
#[derive(Debug, Clone)]
pub(super) struct SearchLeaf {
    command: String,
    title: String,
    icon: Option<String>,
    args: Vec<Value>,
    labels: Vec<String>,
    /// Each pick's own detail (an instrument's folder): two picks with the
    /// same label from different folders are different leaves.
    details: Vec<Option<String>>,
    detail: Option<String>,
    /// A `:search :parent` choice: `labels` end at the pick it stands for.
    stands_for_parent: bool,
}

enum CurrentDisplay {
    Label(String),
    On,
    Nothing,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EditorConfig, Runtime};
    use crossterm::event::{KeyCode, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn type_text(editor: &mut Editor, text: &str) {
        for ch in text.chars() {
            editor.handle_key(key(KeyCode::Char(ch)));
        }
    }

    fn palette_editor() -> Editor {
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor
            .runtime_mut()
            .eval_str(
                r#"(def ran nil)
                   (def previewed (list))
                   (def pal-preview (value) (set! previewed (append previewed (list value))))
                   (def tabs-horizontal true)
                   (def pal-current-theme "b")
                   (def pal-themes () (list (dict :label "Alpha" :value "a" :detail "light")
                                            (dict :label "Beta" :value "b")))
                   (command-group "Look" :icon :swatch)
                   (def pal-theme (theme) :doc "Switch the theme." :command "Change Theme"
                     :prompt "Theme" :choices pal-themes :current pal-current-theme :icon :swatch
                     :preview pal-preview
                     (set! ran theme))
                   (def pal-tabs () :command "Horizontal Tabs" :current tabs-horizontal :group "Look"
                     (set! ran "tabs"))
                   (def pal-new-track () :doc "Add a track." :command "New Track" :group "Tracks"
                     :shortcut "S-s-t" (set! ran "track"))
                   (def pal-export () :command "Export Song" (set! ran "export"))
                   (def theme-helper () 1)"#,
            )
            .unwrap();
        editor
    }

    fn rows(editor: &Editor) -> Vec<String> {
        editor
            .palette_frame()
            .expect("open")
            .rows
            .into_iter()
            .map(|row| if row.header { format!("# {}", row.label) } else { row.label })
            .collect()
    }

    fn labels(editor: &Editor) -> Vec<String> {
        rows(editor).into_iter().filter(|row| !row.starts_with("# ")).collect()
    }

    fn ran(editor: &Editor) -> Option<Value> {
        editor.runtime().global_value("ran")
    }

    #[test]
    fn prefix_beats_word_start_beats_substring_beats_subsequence() {
        let prefix = match_score("change theme", "chan").unwrap();
        let word = match_score("change theme", "them").unwrap();
        let substring = match_score("change theme", "hang").unwrap();
        let subsequence = match_score("change theme", "ct").unwrap();
        assert!(prefix > word && word > substring && substring > subsequence);
        assert!(match_score("change theme", "tc").is_none());
        assert!(substring_score("change theme", "ct").is_none());
    }

    #[test]
    fn bindings_render_in_platform_notation() {
        let macos = crate::ui::platform::CURRENT_SHORTCUT_PLATFORM
            == crate::ui::platform::ShortcutPlatform::MacOS;
        if macos {
            assert_eq!(binding_label("S-s-p"), "⇧⌘P");
            assert_eq!(binding_label("C-x 2"), "⌃X 2");
            assert_eq!(binding_label("C-Tab"), "⌃⇥");
            assert_eq!(binding_label("F(5)"), "F5");
        } else {
            assert_eq!(binding_label("C-x 2"), "Ctrl+X 2");
        }
    }

    #[test]
    fn empty_query_lists_every_command_flat_without_pins() {
        let mut editor = palette_editor();
        editor.run_command(COMMAND_PALETTE);
        assert_eq!(rows(&editor), ["Change Theme", "Horizontal Tabs", "New Track", "Export Song"]);
        let frame = editor.palette_frame().unwrap();
        assert!(frame.rows[0].submenu, "a :choices command shows ›");
        assert_eq!(frame.rows[0].icon.as_deref(), Some("swatch"));
        assert_eq!(frame.rows[0].detail.as_deref(), Some("Beta"), ":current shows its choice's label");
        assert!(frame.rows[1].checked, "a true :current state shows ✓");
        let new_track = frame.rows[2].shortcut.as_deref().unwrap();
        assert!(new_track.contains('T'), "{new_track}");
        assert_eq!(frame.footer, Some(("(pal-theme theme)".into(), "Switch the theme.".into())));
    }

    #[test]
    fn pinned_commands_make_the_home_list_and_sections_appear_while_typing() {
        let mut editor = palette_editor();
        editor
            .runtime_mut()
            .eval_str(r#"(def pal-export () :command "Export Song" :pinned true (set! ran "export"))
                         (def pal-new-track () :command "New Track" :group "Tracks" :pinned true (set! ran "track"))"#)
            .unwrap();
        editor.run_command(COMMAND_PALETTE);
        assert_eq!(rows(&editor), ["New Track", "Export Song"], "pinned only, flat, definition order");
        editor.handle_key(key(KeyCode::Esc));
        editor.runtime_mut().eval_str(r#"(def pal-top () :command "Top Thing" :pinned 1 (set! ran "top"))"#).unwrap();
        editor.runtime_mut().eval_str("(command-palette)").unwrap();
        editor.refresh_runtime_side_effects();
        assert_eq!(rows(&editor), ["Top Thing", "New Track", "Export Song"], "a ranked pin leads");
        type_text(&mut editor, "t");
        assert!(rows(&editor).iter().any(|row| row == "# Look"), "{:?}", rows(&editor));
        let frame = editor.palette_frame().unwrap();
        let look = frame.rows.iter().find(|row| row.header && row.label == "Look").unwrap();
        assert_eq!(look.icon.as_deref(), Some("swatch"), "registered group icon");
    }

    #[test]
    fn typing_ranks_sections_by_best_match() {
        let mut editor = palette_editor();
        editor.run_command(COMMAND_PALETTE);
        type_text(&mut editor, "theme");
        let all = rows(&editor);
        assert_eq!(all[0], "Change Theme");
        let functions = all.iter().position(|row| row == "# Functions").expect("functions section");
        assert!(all[functions + 1..].iter().any(|row| row == "theme-helper"), "{all:?}");

        assert!(!all.iter().any(|row| row == "New Track"), "{all:?}");
        assert_eq!(editor.palette_frame().unwrap().selected, Some(0));

        editor.handle_key(key(KeyCode::Esc));
        editor.run_command(COMMAND_PALETTE);
        type_text(&mut editor, "ran");
        assert!(!labels(&editor).iter().any(|row| row == "ran"), "plain variables are not commands");
    }

    #[test]
    fn the_highlight_skips_section_headers() {
        let mut editor = palette_editor();
        editor.run_command(COMMAND_PALETTE);
        type_text(&mut editor, "t");
        let all = rows(&editor);
        // A header with items above it: moving down from the item before it
        // lands past it, and back up skips it again.
        let header = (1..all.len()).find(|&at| all[at].starts_with("# ")).expect("a later section header");
        let before = header - 1;
        let first = editor.palette_frame().unwrap().selected.unwrap();
        for _ in first..before {
            editor.handle_key(key(KeyCode::Down));
        }
        assert_eq!(editor.palette_frame().unwrap().selected, Some(before), "{all:?}");
        editor.handle_key(key(KeyCode::Down));
        assert_eq!(editor.palette_frame().unwrap().selected, Some(header + 1), "jumps over {}", all[header]);
        editor.handle_key(key(KeyCode::Up));
        assert_eq!(editor.palette_frame().unwrap().selected, Some(before));
    }

    #[test]
    fn a_choices_command_opens_a_badged_stage_and_runs_with_the_pick() {
        let mut editor = palette_editor();
        editor.run_command(COMMAND_PALETTE);
        editor.handle_key(key(KeyCode::Enter));
        let frame = editor.palette_frame().expect("still open on the choice stage");
        assert_eq!(frame.badge.as_deref(), Some("Theme"));
        assert_eq!(frame.placeholder, "Select Theme…");
        assert_eq!(labels(&editor), ["Alpha", "Beta"]);
        assert_eq!(frame.selected, Some(1), "opens on the current choice");
        assert!(frame.rows[1].checked && !frame.rows[0].checked);
        assert_eq!(frame.rows[0].detail.as_deref(), Some("light"));

        // Backspace on an empty field goes back to the commands.
        editor.handle_key(key(KeyCode::Backspace));
        assert_eq!(editor.palette_frame().unwrap().badge, None);
        editor.handle_key(key(KeyCode::Enter));

        type_text(&mut editor, "alp");
        editor.handle_key(key(KeyCode::Enter));
        assert!(editor.palette_frame().is_none());
        assert_eq!(ran(&editor), Some(Value::String("a".into())));
    }

    fn previewed(editor: &Editor) -> Vec<String> {
        match editor.runtime().global_value("previewed") {
            Some(Value::List(items)) => items
                .iter()
                .map(|item| match &*item.borrow() {
                    Value::String(text) => text.clone(),
                    other => format!("{other:?}"),
                })
                .collect(),
            other => panic!("previewed: {other:?}"),
        }
    }

    #[test]
    fn a_preview_stage_applies_the_highlight_and_reverts_when_abandoned() {
        let mut editor = palette_editor();
        editor.runtime_mut().eval_str(r#"(command-palette "pal-theme")"#).unwrap();
        editor.refresh_runtime_side_effects();
        assert!(previewed(&editor).is_empty(), "opening on the current choice previews nothing");
        editor.handle_key(key(KeyCode::Up));
        assert_eq!(previewed(&editor), ["a"]);
        editor.handle_key(key(KeyCode::Up));
        editor.handle_key(key(KeyCode::Esc));
        assert_eq!(previewed(&editor), ["a", "b"], "wraps back to b; Esc then has nothing to revert");

        editor.runtime_mut().eval_str("(set! previewed (list))").unwrap();
        editor.runtime_mut().eval_str(r#"(command-palette "pal-theme")"#).unwrap();
        editor.refresh_runtime_side_effects();
        editor.handle_key(key(KeyCode::Up));
        editor.handle_key(key(KeyCode::Esc));
        assert_eq!(previewed(&editor), ["a", "b"], "Esc restores the value it opened on");

        editor.runtime_mut().eval_str("(set! previewed (list))").unwrap();
        editor.runtime_mut().eval_str(r#"(command-palette "pal-theme")"#).unwrap();
        editor.refresh_runtime_side_effects();
        editor.handle_key(key(KeyCode::Up));
        editor.handle_key(key(KeyCode::Enter));
        assert_eq!(previewed(&editor), ["a"], "a kept pick is not reverted");
        assert_eq!(ran(&editor), Some(Value::String("a".into())));
    }

    fn two_step_editor() -> Editor {
        let mut editor = palette_editor();
        editor
            .runtime_mut()
            .eval_str(
                r#"(def pal-kits () (list (dict :label "Kicks" :value "kicks") (dict :label "Snares" :value "snares")))
                   (def pal-sounds (kit)
                     (if (= kit "kicks")
                       (list (dict :label "Default" :value "" :search false) "808 Kick" "909 Kick")
                       (list "Clap Snare")))
                   (def pal-add (kit sound) :command "Add Sound" :choices (kit pal-kits sound pal-sounds)
                     :searchable true :icon :plus
                     (set! ran (str kit "/" sound)))"#,
            )
            .unwrap();
        editor
    }

    #[test]
    fn a_two_step_command_picks_each_argument_with_a_breadcrumb() {
        let mut editor = two_step_editor();
        editor.run_command(COMMAND_PALETTE);
        type_text(&mut editor, "add sound");
        editor.handle_key(key(KeyCode::Enter));
        assert_eq!(editor.palette_frame().unwrap().badge.as_deref(), Some("Kit"));
        assert_eq!(labels(&editor), ["Kicks", "Snares"]);
        editor.handle_key(key(KeyCode::Enter));
        let frame = editor.palette_frame().unwrap();
        assert_eq!(frame.badge.as_deref(), Some("Kicks › Sound"));
        assert_eq!(frame.placeholder, "Select Sound…");
        assert_eq!(labels(&editor), ["Default", "808 Kick", "909 Kick"], "the second source sees the first pick");

        editor.handle_key(key(KeyCode::Backspace));
        assert_eq!(editor.palette_frame().unwrap().badge.as_deref(), Some("Kit"), "back one step");
        editor.handle_key(key(KeyCode::Enter));
        type_text(&mut editor, "909");
        editor.handle_key(key(KeyCode::Enter));
        assert!(editor.palette_frame().is_none());
        assert_eq!(ran(&editor), Some(Value::String("kicks/909 Kick".into())));
    }

    #[test]
    fn searching_finds_a_searchable_commands_final_choices() {
        let mut editor = two_step_editor();
        editor.run_command(COMMAND_PALETTE);
        type_text(&mut editor, "clap");
        let all = rows(&editor);
        let section = all.iter().position(|row| row == "# Add Sound").expect("a section for the command");
        assert_eq!(all[section + 1], "Clap Snare");
        let frame = editor.palette_frame().unwrap();
        let clap = frame.rows.iter().find(|row| row.label == "Clap Snare").unwrap();
        assert_eq!(clap.detail.as_deref(), Some("Snares"), "the picks before it");
        assert_eq!(frame.rows[section].icon.as_deref(), Some("plus"), "the command's icon heads its section");

        editor.handle_key(key(KeyCode::Esc));
        editor.run_command(COMMAND_PALETTE);
        type_text(&mut editor, "default");
        assert!(!labels(&editor).iter().any(|row| row == "Default"), ":search false keeps a choice out");

        editor.handle_key(key(KeyCode::Esc));
        editor.run_command(COMMAND_PALETTE);
        type_text(&mut editor, "clap");
        let at = rows(&editor).iter().position(|row| row == "Clap Snare").unwrap();
        let selected = editor.palette_frame().unwrap().selected.unwrap();
        for _ in selected..at {
            editor.handle_key(key(KeyCode::Down));
        }
        editor.handle_key(key(KeyCode::Enter));
        assert_eq!(ran(&editor), Some(Value::String("snares/Clap Snare".into())));
    }

    #[test]
    fn search_choices_are_kept_across_sessions_until_invalidated() {
        let mut editor = two_step_editor();
        let search = |editor: &mut Editor, text: &str| {
            editor.run_command(COMMAND_PALETTE);
            type_text(editor, text);
            let found = labels(editor);
            editor.handle_key(key(KeyCode::Esc));
            found
        };
        assert!(search(&mut editor, "clap").iter().any(|row| row == "Clap Snare"));

        // A source's data changing is invisible until the host says so.
        editor.runtime_mut().eval_str(r#"(def pal-sounds (kit) (list "Rim Snare"))"#).unwrap();
        assert!(search(&mut editor, "clap").iter().any(|row| row == "Clap Snare"), "the last walk is reused");
        editor.invalidate_palette_search();
        let found = search(&mut editor, "snare");
        assert!(found.iter().any(|row| row == "Rim Snare") && !found.iter().any(|row| row == "Clap Snare"));

        // A new searchable command re-walks on its own.
        editor
            .runtime_mut()
            .eval_str(r#"(def pal-fx (fx) :command "Add FX" :choices pal-kits :searchable true (set! ran fx))"#)
            .unwrap();
        assert!(search(&mut editor, "kicks").iter().any(|row| row == "Kicks"));
    }

    #[test]
    fn a_parent_standin_lists_the_first_pick_above_deeper_choices() {
        let mut editor = palette_editor();
        editor
            .runtime_mut()
            .eval_str(
                r#"(def pal-instruments () (list (dict :label "Bass Synth" :value "bass" :detail "Factory")
                                                 (dict :label "Keys" :value "keys")))
                   (def pal-presets (instrument)
                     (if (= instrument "keys")
                       (list (dict :label "Default" :value "" :search :parent) "Synth Bass Pluck")
                       (list (dict :label "Default" :value "" :search :parent) "Warm")))
                   (def pal-add-inst (instrument preset) :command "Add Instrument"
                     :choices (instrument pal-instruments preset pal-presets) :searchable true
                     (set! ran (str instrument "/" preset)))"#,
            )
            .unwrap();
        editor.run_command(COMMAND_PALETTE);
        type_text(&mut editor, "synth");
        let all = rows(&editor);
        let section = all.iter().position(|row| row == "# Add Instrument").expect("a section");
        assert_eq!(
            all[section + 1..section + 3],
            ["Bass Synth", "Synth Bass Pluck"],
            "the instrument (a mid-word match) ranks above a preset's prefix match"
        );
        let frame = editor.palette_frame().unwrap();
        let bass = frame.rows.iter().find(|row| row.label == "Bass Synth").unwrap();
        assert_eq!(bass.detail.as_deref(), Some("Factory"), "the stand-in shows its pick's detail");

        editor.handle_key(key(KeyCode::Esc));
        editor.run_command(COMMAND_PALETTE);
        type_text(&mut editor, "default");
        assert!(!labels(&editor).iter().any(|row| row == "Default"), "a stand-in is not listed by its own label");

        editor.handle_key(key(KeyCode::Esc));
        editor.run_command(COMMAND_PALETTE);
        type_text(&mut editor, "bass synth");
        let at = rows(&editor).iter().position(|row| row == "Bass Synth").unwrap();
        let selected = editor.palette_frame().unwrap().selected.unwrap();
        for _ in selected..at {
            editor.handle_key(key(KeyCode::Down));
        }
        editor.handle_key(key(KeyCode::Enter));
        assert_eq!(ran(&editor), Some(Value::String("bass/".into())), "runs with the stand-in's own value");
    }

    #[test]
    fn enter_runs_the_selection_closes_and_floats_it_to_the_top() {
        let mut editor = palette_editor();
        let history = std::env::temp_dir().join(format!("palette-history-{}", std::process::id()));
        let _ = std::fs::remove_file(&history);
        editor.set_palette_history_path(Some(history.clone()));
        editor.run_command(COMMAND_PALETTE);
        type_text(&mut editor, "new");
        editor.handle_key(key(KeyCode::Enter));
        assert!(editor.palette_frame().is_none());
        assert_eq!(ran(&editor), Some(Value::String("track".into())));
        assert_eq!(std::fs::read_to_string(&history).unwrap(), "pal-new-track\n");

        // An uncurated name runs but stays out of the history.
        editor.run_command(COMMAND_PALETTE);
        type_text(&mut editor, "theme-helper");
        editor.handle_key(key(KeyCode::Enter));
        assert_eq!(std::fs::read_to_string(&history).unwrap(), "pal-new-track\n");

        editor.run_command(COMMAND_PALETTE);
        assert_eq!(rows(&editor)[0], "New Track", "recent first, out of its section");
        editor.handle_key(key(KeyCode::Esc));
        assert!(editor.palette_frame().is_none());

        // A fresh editor reads the history back.
        let mut fresh = palette_editor();
        fresh.set_palette_history_path(Some(history.clone()));
        fresh.run_command(COMMAND_PALETTE);
        assert_eq!(rows(&fresh)[0], "New Track");
        let _ = std::fs::remove_file(&history);
    }

    #[test]
    fn pages_clamp_and_c_g_closes() {
        let mut editor = palette_editor();
        editor.run_command(COMMAND_PALETTE);
        editor.handle_key(key(KeyCode::PageDown));
        assert_eq!(editor.palette_frame().unwrap().selected, Some(3));
        editor.handle_key(key(KeyCode::PageDown));
        assert_eq!(editor.palette_frame().unwrap().selected, Some(3));
        editor.handle_key(key(KeyCode::PageUp));
        assert_eq!(editor.palette_frame().unwrap().selected, Some(0));
        editor.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
        assert!(editor.palette_frame().is_none());
    }

    #[test]
    fn pointer_runs_a_clicked_row_clears_the_field_and_a_press_outside_closes() {
        let mut editor = palette_editor();
        editor.run_command(COMMAND_PALETTE);
        let viewport = crate::widget_render::WidgetViewport::window(10.0, 20.0, 1400.0, 900.0, 0.0);
        command_palette::primitives(&editor.palette_frame().unwrap(), viewport);
        let map = command_palette::hit_map().expect("drawn");
        let at = |kind: MouseEventKind, col: f32, row: f32| MouseEvent {
            kind,
            column: col as u16,
            row: row as u16,
            modifiers: KeyModifiers::NONE,
        };
        let row_1 = map.list_top + map.row_height * 1.5;
        let col = map.panel.col + 5.0;
        editor.handle_tiled_mouse_precise(at(MouseEventKind::Moved, col, row_1), col, row_1, 0);
        assert_eq!(editor.palette_frame().unwrap().selected, Some(1));
        editor.handle_tiled_mouse_precise(at(MouseEventKind::Up(MouseButton::Left), col, row_1), col, row_1, 0);
        assert_eq!(ran(&editor), Some(Value::String("tabs".into())));
        assert!(editor.palette_frame().is_none());

        editor.run_command(COMMAND_PALETTE);
        type_text(&mut editor, "zz");
        command_palette::primitives(&editor.palette_frame().unwrap(), viewport);
        let clear = command_palette::hit_map().unwrap().clear.expect("clear button with input");
        let (col, row) = (clear.col + clear.width * 0.5, clear.row + clear.height * 0.5);
        editor.handle_tiled_mouse_precise(at(MouseEventKind::Up(MouseButton::Left), col, row), col, row, 0);
        assert_eq!(editor.palette_frame().unwrap().input, "");

        command_palette::primitives(&editor.palette_frame().unwrap(), viewport);
        editor.handle_tiled_mouse_precise(at(MouseEventKind::Down(MouseButton::Left), 1.0, 1.0), 1.0, 1.0, 0);
        assert!(editor.palette_frame().is_none());
    }

    #[test]
    fn the_palette_does_not_force_a_ui_tiles_status_row_open() {
        let mut editor = palette_editor();
        let tile = editor.active_tile;
        editor.active_leaf_mut().show_status = false;
        editor.active_buffer_mut().view_mode = crate::editor::ViewMode::UiOnly;
        editor.run_command(COMMAND_PALETTE);
        assert_eq!(editor.tile_effective_show_status(tile), Some(false));
        editor.handle_key(key(KeyCode::Esc));
        editor.run_command("execute-extended-command");
        assert_eq!(editor.tile_effective_show_status(tile), Some(true), "M-x still shows its prompt");
    }

    #[test]
    fn scrolling_over_the_panel_scrolls_its_list_not_the_tile_behind() {
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        let defs: String = (0..40).map(|i| format!("(def pal-cmd-{i:02} () :command \"Command {i:02}\" {i})\n")).collect();
        editor.runtime_mut().eval_str(&defs).unwrap();
        editor.run_command(COMMAND_PALETTE);
        let viewport = crate::widget_render::WidgetViewport::window(10.0, 20.0, 1400.0, 700.0, 0.0);
        command_palette::primitives(&editor.palette_frame().unwrap(), viewport);
        let map = command_palette::hit_map().unwrap();
        let (col, row) = (map.panel.col + 5.0, map.list_top + 1.0);

        // Swipe up (negative delta): the list scrolls down.
        assert!(editor.handle_tiled_touchpad_scroll(col, row, 0, 0.0, -300.0));
        let frame = editor.palette_frame().unwrap();
        let (first, total) = frame.scroll.expect("a scrolling list");
        assert!(first > 0, "scrolled");
        assert_eq!(total, 40);
        assert!(frame.selected.is_some(), "the highlight stays in view");
        assert_eq!(frame.rows[0].label, format!("Command {first:02}"));

        // Back up past the top clamps at the first row.
        editor.handle_tiled_touchpad_scroll(col, row, 0, 0.0, 5000.0);
        assert_eq!(editor.palette_frame().unwrap().scroll.unwrap().0, 0);

        // Outside the panel it falls through to the tile.
        assert!(!editor.palette_scroll_consumed_at(1.0, 1.0));
    }

    #[test]
    fn the_command_palette_native_opens_it_optionally_on_a_choice_stage() {
        let mut editor = palette_editor();
        editor.runtime_mut().eval_str("(command-palette)").unwrap();
        editor.refresh_runtime_side_effects();
        assert_eq!(editor.palette_frame().unwrap().badge, None);
        editor.handle_key(key(KeyCode::Esc));

        editor.runtime_mut().eval_str(r#"(command-palette "pal-theme")"#).unwrap();
        editor.refresh_runtime_side_effects();
        assert_eq!(editor.palette_frame().unwrap().badge.as_deref(), Some("Theme"));
        editor.handle_key(key(KeyCode::Backspace));
        assert_eq!(editor.palette_frame().unwrap().badge, None, "the commands stage is beneath");
    }
    #[test]
    fn a_stale_choice_stage_rereads_its_source_and_keeps_the_highlight() {
        let mut editor = palette_editor();
        editor
            .runtime_mut()
            .eval_str(
                r#"(def pal-sounds (list))
                   (def pal-current-sound nil)
                   (def pal-sound-choices ()
                     (map (lambda (name) (dict :label name :value name :glyph (str "glyph:" name))) pal-sounds))
                   (def pal-apply-sound (sound) :command "Apply Sound" :choices pal-sound-choices
                     :current pal-current-sound (set! ran sound))"#,
            )
            .unwrap();
        editor.runtime_mut().eval_str(r#"(command-palette "pal-apply-sound")"#).unwrap();
        editor.refresh_runtime_side_effects();
        assert!(labels(&editor).is_empty(), "the host has not published yet");

        editor.runtime_mut().eval_str(r#"(set! pal-sounds (list "one" "two" "three"))"#).unwrap();
        editor.runtime_mut().eval_str(r#"(set! pal-current-sound "two")"#).unwrap();
        editor.mark_palette_choices_stale();
        editor.update_palette_preview();
        let frame = editor.palette_frame().unwrap();
        assert_eq!(labels(&editor), ["one", "two", "three"]);
        assert_eq!(frame.rows[1].glyph.as_deref(), Some("glyph:two"));
        assert_eq!(frame.selected, Some(1), "opens on the current sound");

        editor.handle_key(key(KeyCode::Down));
        editor.runtime_mut().eval_str(r#"(set! pal-sounds (list "zero" "one" "two" "three"))"#).unwrap();
        editor.mark_palette_choices_stale();
        editor.update_palette_preview();
        assert_eq!(labels(&editor), ["zero", "one", "two", "three"]);
        assert_eq!(editor.palette_frame().unwrap().selected, Some(3), "the highlight follows its value");
        editor.handle_key(key(KeyCode::Enter));
        assert_eq!(ran(&editor), Some(Value::String("three".into())));
    }
}
