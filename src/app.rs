use std::{
    cell::Cell,
    collections::{HashMap, HashSet},
    ops::Range,
    time::Duration,
};

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, WindowExt as _, h_flex,
    input::{EditorState, InputEvent},
    notification::Notification,
    v_flex,
};
use gpui_kit::*;

use rust_i18n::t;

use crate::browser::{self, BrowserProfile};
use crate::i18n::error_text;
use crate::settings::{Settings, ThemePref};
use crate::store::{FolderMode, History, Item, ItemKind, Store, TrashItem, TrashedProfile};
use crate::{editor_ops, highlight, hosts, panels, remote, store, tile};

mod backup;
mod browser_panel;
mod edit_sheet;
mod history_sheet;
mod icon_sidebar;
mod list_panel;
mod main_panel;
mod preferences;
mod right_panel;
mod splitter;
mod tree_row;

use edit_sheet::EditDraft;
use splitter::PanelEdge;

actions!(
    switch_hosts,
    [ToggleComment, MoveLineUp, MoveLineDown, DuplicateLine, DeleteLine]
);

/// Key context of the code editor, excluding single-line inputs.
const EDITOR_CONTEXT: &str = "Input && mode == editor";
const AUTO_APPLY_DELAY: Duration = Duration::from_millis(1000);
/// Typing is written to disk once it pauses this long.
const SAVE_DELAY: Duration = Duration::from_millis(500);
/// How often the open browser view re-probes browser processes.
const RUNNING_POLL: Duration = Duration::from_secs(2);
/// How often remote items are scanned for a due refresh.
const REMOTE_POLL: Duration = Duration::from_secs(60);
/// Height of the window title bar, which doubles as the main toolbar.
pub(crate) const TOP_BAR_HEIGHT: Pixels = px(44.);

/// Probe which profiles have a browser open. Runs off the render path.
fn probe_running(profiles: Vec<BrowserProfile>) -> HashSet<String> {
    browser::running_profiles(&profiles, &store::data_dir(), &std::env::temp_dir())
}

/// Load the preferences and apply their language and theme.
pub fn init(cx: &mut App) {
    cx.set_global(Settings::load());
    #[cfg(any(target_os = "macos", windows))]
    if Settings::global(cx).launch_at_login {
        crate::autostart::repair();
    }
    preferences::apply_language(cx);
    let appearance = cx.window_appearance();
    preferences::apply_theme(appearance, cx);
}

pub fn bind_keys(cx: &mut App) {
    let editor = Some(EDITOR_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("ctrl-/", ToggleComment, editor),
        KeyBinding::new("alt-up", MoveLineUp, editor),
        KeyBinding::new("alt-down", MoveLineDown, editor),
    ]);
    #[cfg(target_os = "macos")]
    cx.bind_keys([
        KeyBinding::new("cmd-/", ToggleComment, editor),
        KeyBinding::new("cmd-shift-d", DuplicateLine, editor),
        KeyBinding::new("cmd-shift-k", DeleteLine, editor),
    ]);
    #[cfg(not(target_os = "macos"))]
    cx.bind_keys([
        KeyBinding::new("ctrl-shift-d", DuplicateLine, editor),
        KeyBinding::new("ctrl-shift-k", DeleteLine, editor),
    ]);
}

#[derive(Clone, PartialEq)]
enum Selection {
    System,
    Item(String),
    /// The browser profile view, with the selected profile.
    Browser(Option<String>),
    /// The trash view, with the selected trash entry.
    Trash(Option<String>),
}

/// What the main panel currently shows.
enum Current<'a> {
    System,
    Item(&'a Item),
    Browser(&'a BrowserProfile),
    Trash(&'a TrashItem),
    TrashProfile(&'a TrashedProfile),
    None,
}

/// Which revisions the history sheet lists.
#[derive(Clone, PartialEq)]
enum HistoryScope {
    Hosts,
    Profile(String),
}

/// One row of the unified trash list, newest first.
pub(super) struct TrashRow {
    pub id: String,
    pub profile: bool,
    pub icon: IconName,
    pub title: String,
    pub deleted_at: i64,
}

pub struct SwitchHostsApp {
    store: Store,
    /// Loaded only while the history sheet is open.
    history: Option<History>,
    selected: Selection,
    /// Selection to return to when leaving the trash view.
    last_list_selection: Selection,
    /// Selection to return to when leaving the browser or trash view.
    last_hosts_selection: Selection,
    show_list: bool,
    show_info: bool,
    list_width: f32,
    browser_list_width: f32,
    tree_width: f32,
    info_width: f32,
    /// The divider being dragged, if any.
    resize: Option<splitter::PanelResize>,
    history_selected: Option<String>,
    history_scope: HistoryScope,
    /// Profiles this app launched, by id, with the browser's pid.
    launched: HashMap<String, u32>,
    /// Profiles whose own browser instance is detected by process command
    /// line, including instances started before this app process.
    profile_procs: HashSet<String>,
    /// Bumped when the browser view opens, so older polling loops stop.
    running_generation: u64,
    /// Remote items with a fetch in flight.
    refreshing: HashSet<String>,
    launching: bool,
    stopping: bool,
    profile_inputs: Option<browser_panel::ProfileInputs>,
    system_hosts: String,
    /// Modification stamp of the system hosts when it was last read.
    system_stamp: Option<hosts::Stamp>,
    editor: Entity<EditorState>,
    /// Exists only while the history sheet is open.
    history_editor: Option<Entity<EditorState>>,
    edit_draft: Option<EditDraft>,
    /// The item being dragged in the list; stale once no drag is active.
    drag: Option<list_panel::DragState>,
    applying: bool,
    /// Whether the system hosts reflects the enabled items; `None` once
    /// either changed. Rendering reads it on every frame.
    applied: Cell<Option<bool>>,
    /// Rule count of the current selection, `None` once it may have changed.
    rules: Cell<Option<usize>>,
    /// Whether the store has edits not yet written to disk.
    save_pending: bool,
    /// Bumped on every deferred save so only the last timer writes.
    save_generation: u64,
    /// Bumped on every edit so stale auto-apply timers do nothing.
    edit_generation: u64,
    /// Bumped on every store change, so a failed write only rolls back when
    /// nothing changed meanwhile.
    revision: u64,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl SwitchHostsApp {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = Store::load();
        let system_stamp = hosts::system_stamp();
        let system_hosts = hosts::read_system();

        let editor = Self::new_editor(system_hosts.clone(), window, cx);

        // Re-apply once the window exists, since it knows the current appearance.
        if Settings::global(cx).theme == ThemePref::System {
            preferences::apply_theme(window.appearance(), cx);
        }

        let subscriptions = vec![
            cx.subscribe_in(&editor, window, |this, editor, event, window, cx| {
                if let InputEvent::Change = event
                    && let Some(item) = this.selected_item_mut()
                    && item.kind == ItemKind::Local
                {
                    item.content = editor.read(cx).text().to_string();
                    let id = item.id.clone();
                    this.revision += 1;
                    this.rules.set(None);
                    if this.store.is_effective(&id) {
                        this.applied.set(None);
                    }
                    this.persist_later(cx);
                    this.schedule_auto_apply(id, window, cx);
                }
            }),
            cx.observe_window_appearance(window, |_, window, cx| {
                if Settings::global(cx).theme == ThemePref::System {
                    preferences::apply_theme(window.appearance(), cx);
                }
            }),
            // Pick up changes other programs made to the system hosts.
            cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() {
                    if this.in_browser_view() {
                        this.refresh_running(cx);
                    }
                    if this.reload_system_hosts(false) {
                        if this.selected == Selection::System {
                            this.refresh_editor(window, cx);
                        }
                        cx.notify();
                    }
                }
            }),
            cx.on_app_quit(|this, _| {
                this.flush_save();
                async {}
            }),
        ];

        let settings = Settings::global(cx);

        let mut app = Self {
            store,
            history: None,
            selected: Selection::System,
            last_list_selection: Selection::System,
            last_hosts_selection: Selection::System,
            show_list: true,
            show_info: false,
            list_width: panels::initial_width(settings.list_width),
            browser_list_width: panels::initial_width(settings.browser_list_width),
            tree_width: panels::initial_width(settings.tree_width),
            info_width: panels::initial_width(settings.info_width),
            resize: None,
            history_selected: None,
            history_scope: HistoryScope::Hosts,
            launched: HashMap::new(),
            profile_procs: HashSet::new(),
            running_generation: 0,
            refreshing: HashSet::new(),
            launching: false,
            stopping: false,
            profile_inputs: None,
            system_hosts,
            system_stamp,
            editor,
            history_editor: None,
            edit_draft: None,
            drag: None,
            applying: false,
            applied: Cell::new(None),
            rules: Cell::new(None),
            save_pending: false,
            save_generation: 0,
            edit_generation: 0,
            revision: 0,
            focus_handle: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        app.start_remote_poll(window, cx);
        app
    }

    fn new_editor(value: String, window: &mut Window, cx: &mut Context<Self>) -> Entity<EditorState> {
        cx.new(|cx| {
            let mut state = EditorState::new(window, cx)
                .language("hosts")
                .soft_wrap(false)
                .default_value(value);
            state.set_highlighter_factory(highlight::factory(), cx);
            state
        })
    }

    // -- state helpers -------------------------------------------------------

    fn selected_id(&self) -> Option<&str> {
        match &self.selected {
            Selection::Item(id) => Some(id),
            _ => None,
        }
    }

    fn selected_item(&self) -> Option<&Item> {
        self.selected_id().and_then(|id| self.store.find(id))
    }

    fn selected_item_mut(&mut self) -> Option<&mut Item> {
        let id = self.selected_id()?.to_string();
        self.store.find_mut(&id)
    }

    fn trash_item(&self, id: &str) -> Option<&TrashItem> {
        self.store.trash.iter().find(|t| t.item.id == id)
    }

    fn selected_profile(&self) -> Option<&BrowserProfile> {
        match &self.selected {
            Selection::Browser(Some(id)) => self.store.profile(id),
            _ => None,
        }
    }

    fn selected_profile_mut(&mut self) -> Option<&mut BrowserProfile> {
        let id = match &self.selected {
            Selection::Browser(Some(id)) => id.clone(),
            _ => return None,
        };
        self.store.profile_mut(&id)
    }

    fn in_browser_view(&self) -> bool {
        matches!(self.selected, Selection::Browser(_))
    }

    /// Both kinds of deleted entries, newest first.
    fn trash_rows(&self) -> Vec<TrashRow> {
        let mut rows: Vec<TrashRow> = self
            .store
            .trash
            .iter()
            .map(|t| TrashRow {
                id: t.item.id.clone(),
                profile: false,
                icon: item_icon(&t.item),
                title: t.item.title.clone(),
                deleted_at: t.deleted_at,
            })
            .chain(self.store.trash_profiles.iter().map(|t| TrashRow {
                id: t.profile.id.clone(),
                profile: true,
                icon: IconName::Globe,
                title: t.profile.name.clone(),
                deleted_at: t.deleted_at,
            }))
            .collect();
        rows.sort_by_key(|row| std::cmp::Reverse(row.deleted_at));
        rows
    }

    fn trash_first_id(&self) -> Option<String> {
        self.trash_rows().first().map(|row| row.id.clone())
    }

    fn current(&self) -> Current<'_> {
        match &self.selected {
            Selection::System => Current::System,
            Selection::Item(id) => self.store.find(id).map_or(Current::None, Current::Item),
            Selection::Browser(Some(id)) => self.store.profile(id).map_or(Current::None, Current::Browser),
            Selection::Browser(None) => Current::None,
            Selection::Trash(Some(id)) => match self.trash_item(id) {
                Some(trash) => Current::Trash(trash),
                None => self
                    .store
                    .trashed_profile(id)
                    .map_or(Current::None, Current::TrashProfile),
            },
            Selection::Trash(None) => Current::None,
        }
    }

    fn in_trash_view(&self) -> bool {
        matches!(self.selected, Selection::Trash(_))
    }

    /// Only local items are edited in the main editor.
    fn editor_readonly(&self) -> bool {
        self.in_browser_view()
            || !matches!(self.current(), Current::Item(item) if item.kind == ItemKind::Local)
    }

    fn is_applied(&self) -> bool {
        if let Some(applied) = self.applied.get() {
            return applied;
        }
        let applied = hosts::is_applied(&self.system_hosts, &self.store.enabled_content());
        self.applied.set(Some(applied));
        applied
    }

    /// Number of rules in what the current selection stands for.
    fn rule_count(&self) -> usize {
        if let Some(rules) = self.rules.get() {
            return rules;
        }
        let rules = match self.current() {
            Current::System => store::count_rules(&self.system_hosts),
            Current::Item(item) if item.kind.has_content() => store::count_rules(&item.content),
            Current::Item(item) => store::count_rules(&self.store.content_of(item, false)),
            Current::Browser(profile) => {
                browser::mappings(&self.store.switched_content(&profile.switches)).len()
            }
            Current::TrashProfile(trash) => {
                browser::mappings(&self.store.switched_content(&trash.profile.switches)).len()
            }
            Current::Trash(_) | Current::None => 0,
        };
        self.rules.set(Some(rules));
        rules
    }

    /// Forget everything derived from the store or the system hosts.
    fn invalidate(&self) {
        self.applied.set(None);
        self.rules.set(None);
    }

    /// Write the store to disk now.
    fn persist(&mut self, cx: &mut Context<Self>) {
        self.save_pending = false;
        if let Err(err) = self.store.save() {
            eprintln!("save failed: {err:#}");
        }
        self.invalidate();
        cx.notify();
    }

    /// Write the store to disk once edits pause.
    fn persist_later(&mut self, cx: &mut Context<Self>) {
        self.save_pending = true;
        self.save_generation += 1;
        let generation = self.save_generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_DELAY).await;
            this.update(cx, |this, _| {
                if this.save_generation == generation {
                    this.flush_save();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn flush_save(&mut self) {
        if !self.save_pending {
            return;
        }
        self.save_pending = false;
        if let Err(err) = self.store.save() {
            eprintln!("save failed: {err:#}");
        }
    }

    /// Re-read the system hosts when `force` is set or its modification
    /// stamp changed. Returns whether its content changed.
    fn reload_system_hosts(&mut self, force: bool) -> bool {
        let stamp = hosts::system_stamp();
        if !force && stamp.is_some() && stamp == self.system_stamp {
            return false;
        }
        self.system_stamp = stamp;
        let text = hosts::read_system();
        if text == self.system_hosts {
            return false;
        }
        self.system_hosts = text;
        self.invalidate();
        true
    }

    fn notify_error(window: &mut Window, message: impl Into<SharedString>, cx: &mut App) {
        window.push_notification(Notification::error(message.into()), cx);
    }

    /// Mutate the store. A change of the enabled content is written right
    /// away, asking for privileges when needed.
    fn mutate(&mut self, window: &mut Window, cx: &mut Context<Self>, f: impl FnOnce(&mut Store)) {
        let before = self.store.enabled_content();
        f(&mut self.store);
        self.store_changed(window, cx);
        if self.store.enabled_content() != before {
            self.apply(None, true, window, cx);
        }
    }

    fn store_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.revision += 1;
        self.persist(cx);
        self.validate_selection(window, cx);
        self.refresh_preview(window, cx);
    }

    // -- selection -----------------------------------------------------------

    fn select(&mut self, selection: Selection, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected == selection {
            return;
        }
        if !matches!(selection, Selection::Trash(_)) {
            self.last_list_selection = selection.clone();
        }
        if matches!(selection, Selection::System | Selection::Item(_)) {
            self.last_hosts_selection = selection.clone();
        }
        let was_browser = self.in_browser_view();
        self.selected = selection;
        self.rules.set(None);
        if self.in_browser_view() {
            self.refresh_profile_inputs(window, cx);
            if !was_browser {
                self.start_running_poll(cx);
            }
        } else {
            self.profile_inputs = None;
        }
        self.refresh_editor(window, cx);
    }

    /// Fall back to a valid selection after items were removed.
    fn validate_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selection = match &self.selected {
            Selection::Item(id) if self.store.find(id).is_none() => Selection::System,
            Selection::Browser(Some(id)) if self.store.profile(id).is_none() => {
                Selection::Browser(self.store.profiles.first().map(|p| p.id.clone()))
            }
            Selection::Trash(Some(id))
                if self.trash_item(id).is_none() && self.store.trashed_profile(id).is_none() =>
            {
                Selection::Trash(self.trash_first_id())
            }
            _ => return,
        };
        self.select(selection, window, cx);
    }

    fn leave_trash_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.select(self.last_list_selection.clone(), window, cx);
        self.validate_selection(window, cx);
    }

    /// Show the item list, or hide it when it is already shown.
    fn toggle_list_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.in_trash_view() {
            self.show_list = true;
            self.leave_trash_view(window, cx);
        } else {
            self.show_list = !self.show_list;
            cx.notify();
        }
    }

    fn toggle_trash_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.in_trash_view() {
            self.leave_trash_view(window, cx);
        } else {
            self.show_list = true;
            let first = self.trash_first_id();
            self.select(Selection::Trash(first), window, cx);
        }
    }

    fn enter_hosts_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_list = true;
        self.select(self.last_hosts_selection.clone(), window, cx);
        self.validate_selection(window, cx);
    }

    fn enter_browser_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_list = true;
        if self.in_browser_view() {
            cx.notify();
            return;
        }
        let first = self.store.profiles.first().map(|p| p.id.clone());
        self.select(Selection::Browser(first), window, cx);
    }

    /// The text the editor should show for the current selection.
    fn editor_text(&self) -> String {
        match self.current() {
            Current::System => self.system_hosts.clone(),
            Current::Item(item) => self.store.content_of(item, false),
            Current::Trash(trash) => self.store.content_of(&trash.item, true),
            // Profiles inject the hosts items they check themselves; this
            // preview is only used outside the browser view.
            Current::Browser(profile) => self.store.switched_content(&profile.switches),
            Current::TrashProfile(trash) => self.store.switched_content(&trash.profile.switches),
            Current::None => String::new(),
        }
    }

    /// Load the editor for the current selection, resetting cursor and undo
    /// history so they never carry over to another item.
    fn refresh_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected == Selection::System {
            self.reload_system_hosts(false);
        }
        let content = self.editor_text();
        self.editor
            .update(cx, |state, cx| state.set_value(content, window, cx));
        cx.notify();
    }

    /// Reload a read-only preview (folder, group, …) whose text depends on
    /// other items. Editable items are the editor's own text and stay as is.
    fn refresh_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.editor_readonly() {
            return;
        }
        let content = self.editor_text();
        if *self.editor.read(cx).text() != *content {
            self.editor
                .update(cx, |state, cx| state.set_value(content, window, cx));
        }
    }

    // -- editor commands -----------------------------------------------------

    fn run_editor_op(
        &mut self,
        op: fn(&str, Range<usize>) -> Option<editor_ops::Edit>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = self.editor.read(cx);
        if self.editor_readonly()
            || !state.is_editable()
            || !state.focus_handle(cx).is_focused(window)
        {
            cx.propagate();
            return;
        }
        self.editor.update(cx, |state, cx| {
            let text = state.text().to_string();
            let Some(edit) = op(&text, state.selected_range()) else {
                return;
            };
            let reversed = state.is_selection_reversed();
            let utf16 = |byte: usize| text[..byte].encode_utf16().count();
            let range = utf16(edit.range.start)..utf16(edit.range.end);
            state.replace_text_in_range(Some(range), &edit.new_text, window, cx);
            state.set_selected_range_with_direction(edit.selection, reversed, cx);
        });
    }

    // -- item operations -----------------------------------------------------

    /// Switch an item and write the hosts right away, asking for privileges
    /// when needed. The switch is reverted when writing fails.
    fn toggle(&mut self, id: String, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        let snapshot = self.store.clone();
        self.store.set_on(&id, on);
        self.store_changed(window, cx);
        if self.store.enabled_content() != snapshot.enabled_content() {
            self.apply(Some(snapshot), false, window, cx);
        }
    }

    fn toggle_collapsed(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(item) = self.store.find_mut(id) {
            item.collapsed = !item.collapsed;
            self.persist(cx);
        }
    }

    fn move_item(&mut self, id: String, parent: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.mutate(window, cx, |store| store.move_to(&id, parent.as_deref()));
    }

    fn set_multi_select(&mut self, multi: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.mutate(window, cx, |store| store.multi_select = multi);
    }

    /// Ask, then move the item to the trash. Closes the edit sheet when asked.
    fn confirm_delete(&mut self, id: String, close_sheet: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.store.find(&id) else {
            return;
        };
        let title = display_title(&item.title);
        let view = cx.entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (view, id) = (view.clone(), id.clone());
            alert
                .confirm()
                .title(t!("move_to_trash"))
                .description(t!("move_to_trash_confirm", title = title))
                .on_ok(move |_, window, cx| {
                    let id = id.clone();
                    view.update(cx, |this, cx| {
                        this.mutate(window, cx, |store| store.delete(&id));
                        if close_sheet {
                            this.edit_draft = None;
                            window.close_sheet(cx);
                        }
                    });
                    true
                })
        });
    }

    // -- browser profiles ----------------------------------------------------

    /// The `(host, ip)` mappings the selected profile injects.
    fn profile_maps(&self) -> Vec<(String, String)> {
        self.selected_profile().map_or_else(Vec::new, |profile| {
            browser::mappings(&self.store.switched_content(&profile.switches))
        })
    }

    fn profile_block(&self) -> Option<browser::Block> {
        let profile = self.selected_profile()?;
        let maps = self.profile_maps();
        let mark = browser::mark_color(profile);
        let cmd = browser::build_cmd(profile, &maps, &store::data_dir(), &std::env::temp_dir(), mark);
        browser::block_reason(profile, cmd.len())
    }

    /// Whether the profile's browser is open, as far as this app can tell.
    /// `Default` profiles are the everyday instance, found by process name;
    /// independent and ephemeral ones by their own `--user-data-dir`, so
    /// instances started outside this app or in an earlier session still count.
    fn profile_running(&self, profile: &BrowserProfile) -> bool {
        self.launched.contains_key(&profile.id) || self.profile_procs.contains(&profile.id)
    }

    /// Probe browser processes off the render path; results are cached.
    fn refresh_running(&mut self, cx: &mut Context<Self>) {
        let profiles = self.store.profiles.clone();
        cx.spawn(async move |this, cx| {
            let procs = cx
                .background_executor()
                .spawn(async move { probe_running(profiles) })
                .await;
            this.update(cx, |this, cx| {
                this.profile_procs = procs;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Keep probing while the browser view is open, so quitting a browser
    /// outside the app shows up without switching windows first.
    fn start_running_poll(&mut self, cx: &mut Context<Self>) {
        self.refresh_running(cx);
        self.running_generation += 1;
        let generation = self.running_generation;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(RUNNING_POLL).await;
                let Ok((active, profiles)) = this.update(cx, |this, _| {
                    (this.in_browser_view() && this.running_generation == generation, this.store.profiles.clone())
                }) else {
                    break;
                };
                if !active {
                    break;
                }
                let procs = cx
                    .background_executor()
                    .spawn(async move { probe_running(profiles) })
                    .await;
                let Ok(still_active) = this.update(cx, |this, cx| {
                    let active = this.in_browser_view() && this.running_generation == generation;
                    if active {
                        this.profile_procs = procs;
                        cx.notify();
                    }
                    active
                }) else {
                    break;
                };
                if !still_active {
                    break;
                }
            }
        })
        .detach();
    }

    fn mutate_profile(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut BrowserProfile)) {
        if let Some(profile) = self.selected_profile_mut() {
            f(profile);
        }
        self.persist(cx);
    }

    /// The title bar's add button: a hosts item, or a profile in the browser view.
    fn add_current(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.in_browser_view() {
            self.add_profile(window, cx);
        } else if !self.in_trash_view() {
            self.open_add_sheet(window, cx);
        }
    }

    fn add_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.store.add_profile(t!("profile_default_name"));
        if let Some(profile) = self.store.profile(&id) {
            tile::refresh(profile);
        }
        self.store_changed(window, cx);
        self.select(Selection::Browser(Some(id)), window, cx);
        // Like the list's other add flows, the new name is visible and ready to
        // be typed over.
        self.show_info = true;
        self.focus_profile_name(window, cx);
        cx.notify();
    }

    /// Launch from a list row, selecting it first.
    fn launch_profile_by_id(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.select(Selection::Browser(Some(id)), window, cx);
        self.launch_profile(window, cx);
    }

    fn duplicate_profile(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(copy) = self.store.duplicate_profile(&id) {
            self.store_changed(window, cx);
            self.select(Selection::Browser(Some(copy)), window, cx);
        }
    }

    fn confirm_delete_profile(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(profile) = self.store.profile(&id) else {
            return;
        };
        let title = display_title(&profile.name);
        let view = cx.entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (view, id) = (view.clone(), id.clone());
            alert
                .confirm()
                .title(t!("move_to_trash"))
                .description(t!("move_to_trash_confirm", title = title))
                .on_ok(move |_, window, cx| {
                    let id = id.clone();
                    view.update(cx, |this, cx| {
                        this.store.delete_profile(&id);
                        this.store_changed(window, cx);
                    });
                    true
                })
        });
    }

    fn launch_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(profile) = self.selected_profile().cloned() else {
            return;
        };
        if self.launching || self.profile_block().is_some() {
            return;
        }
        self.launching = true;
        cx.notify();
        let maps = self.profile_maps();
        let mark = browser::mark_color(&profile);
        // Composite on the main thread; launching only reads the cached PNG.
        let tile = mark.and_then(|_| tile::ensure(&profile));
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    // Re-probe: the cached state may predate a quit or start.
                    let data_dir = store::data_dir();
                    let tmp = std::env::temp_dir();
                    let busy = match profile.mode {
                        browser::ProfileMode::Default => browser::default_running(profile.browser),
                        _ => !browser::running_profiles(std::slice::from_ref(&profile), &data_dir, &tmp).is_empty(),
                    };
                    if busy {
                        return Err(crate::i18n::LocalizedError::new("err_browser_running")
                            .arg("name", profile.browser.name())
                            .into());
                    }
                    #[cfg(any(target_os = "macos", windows))]
                    if let (Some(color), Some(udd)) = (mark, browser::user_data_dir(&profile, &data_dir, &tmp)) {
                        let _ = browser::sync_prefs(&profile, color, &udd);
                    }
                    let (child, mut pipe) = browser::spawn(&profile, &maps, &data_dir, &tmp, mark)
                        .map_err(|err| crate::i18n::LocalizedError::new("err_launch_browser").detail(err))?;
                    if let (Some(pipe), Some(tile)) = (pipe.as_mut(), tile.as_ref())
                        && let Ok(png) = std::fs::read(tile)
                    {
                        pipe.set_tile(&png);
                    }
                    // Windows has no debugging pipe: the picture is handed to
                    // the browser window's icon instead, which the taskbar
                    // draws from. Best effort — Chrome may repaint over it.
                    #[cfg(windows)]
                    if let Some(tile) = tile.as_ref() {
                        crate::windows::hand_off_window_icon(child.id(), tile);
                    }
                    let cleanup = (profile.mode == browser::ProfileMode::Ephemeral)
                        .then(|| browser::ephemeral_dir(&profile, &tmp));
                    Ok::<_, anyhow::Error>((profile, child, cleanup, pipe))
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.launching = false;
                match &result {
                    Ok((profile, child, _, _)) => {
                        window.push_notification(
                            Notification::success(t!(
                                "launched",
                                name = profile.browser.name(),
                                profile = profile.name
                            )),
                            cx,
                        );
                        this.launched.insert(profile.id.clone(), child.id());
                        // Remember what was pushed, so the exit read-back can
                        // tell a browser-side edit from the app's own.
                        #[cfg(any(target_os = "macos", windows))]
                        if let Some(color) = mark {
                            if let Some(p) = this.store.profile_mut(&profile.id) {
                                p.last_synced_name = Some(profile.name.clone());
                                p.last_synced_color = Some(color);
                            }
                            this.persist(cx);
                        }
                        this.refresh_running(cx);
                    }
                    Err(err) => Self::notify_error(window, error_text(err), cx),
                }
                cx.notify();
            })
            .ok();
            // Wait for the browser itself, so the running mark clears on exit
            // and an ephemeral profile's directory is removed afterwards.
            if let Ok((profile, mut child, cleanup, pipe)) = result {
                let id = profile.id.clone();
                let independent = profile.mode == browser::ProfileMode::Independent;
                let identity = cx
                    .background_executor()
                    .spawn(async move {
                        // Chrome quits when the debugging pipe reaches EOF.
                        let _pipe = pipe;
                        let _ = child.wait();
                        // Read back the identity the browser now holds, so a
                        // rename or recolor done inside it can be adopted. Only
                        // an independent profile keeps a directory to read; an
                        // ephemeral one is removed below.
                        let identity = if independent {
                            browser::user_data_dir(&profile, &store::data_dir(), &std::env::temp_dir())
                                .map(|dir| browser::read_identity(&dir))
                                .unwrap_or((None, None))
                        } else {
                            (None, None)
                        };
                        if let Some(dir) = cleanup {
                            let _ = std::fs::remove_dir_all(dir);
                        }
                        identity
                    })
                    .await;
                this.update_in(cx, |this, window, cx| {
                    this.launched.remove(&id);
                    let (name, color) = identity;
                    if name.is_some() || color.is_some() {
                        this.adopt_browser_identity(window, &id, name, color, cx);
                    }
                    this.refresh_running(cx);
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    /// Whether the profile's browser is running, so it can be stopped here; for
    /// `Default` that is the everyday instance.
    fn profile_stoppable(&self, profile: &BrowserProfile) -> bool {
        self.profile_running(profile)
    }

    fn stop_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(profile) = self.selected_profile() else {
            return;
        };
        let id = profile.id.clone();
        self.stop_profile_by_id(id, window, cx);
    }

    fn stop_profile_by_id(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(profile) = self.store.profile(&id).cloned() else {
            return;
        };
        let pid = self.launched.get(&id).copied();
        if self.stopping {
            return;
        }
        self.stopping = true;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let name = profile.browser.name();
            let stopped = cx
                .background_executor()
                .spawn(async move {
                    browser::stop(&profile, &store::data_dir(), &std::env::temp_dir(), pid)
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.stopping = false;
                if stopped {
                    window.push_notification(
                        Notification::success(t!("browser_stopped", name = name)),
                        cx,
                    );
                    this.refresh_running(cx);
                } else {
                    Self::notify_error(window, t!("err_stop_browser", name = name), cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // -- applying ------------------------------------------------------------

    /// Write the enabled items to the system hosts. `quiet` suppresses the
    /// success notification.
    fn apply(&mut self, rollback: Option<Store>, quiet: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.applying {
            return;
        }
        self.applying = true;
        self.flush_save();
        cx.notify();

        let content = self.store.enabled_content();
        let revision = self.revision;
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { hosts::apply(&content) })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.applying = false;
                match result {
                    Ok((previous, written)) => {
                        this.record_history(&previous, &written);
                        if !quiet {
                            window.push_notification(Notification::success(t!("hosts_updated")), cx);
                        }
                    }
                    Err(err) => {
                        if let Some(snapshot) = rollback
                            && this.revision == revision
                        {
                            this.store = snapshot;
                            this.store_changed(window, cx);
                        }
                        Self::notify_error(window, error_text(&err), cx);
                    }
                }
                this.after_write(window, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Apply edits of an effective item once typing pauses.
    fn schedule_auto_apply(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.edit_generation += 1;
        let generation = self.edit_generation;
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(AUTO_APPLY_DELAY).await;
            this.update_in(cx, |this, window, cx| {
                if this.edit_generation != generation
                    || !this.store.is_effective(&id)
                    || this.is_applied()
                {
                    return;
                }
                if this.applying {
                    this.schedule_auto_apply(id, window, cx);
                } else {
                    this.apply(None, true, window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Fetch a remote item's content in the background. A manual refresh
    /// reports the outcome; an automatic one stays silent unless it fails to
    /// even start. The attempt stamps `last_refresh` either way, so a failing
    /// URL is retried once per interval, not once per scan.
    fn refresh_remote(&mut self, id: String, manual: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.refreshing.contains(&id) {
            return;
        }
        let Some(item) = self.store.find(&id) else { return };
        if item.kind != ItemKind::Remote {
            return;
        }
        if !remote::valid_url(&item.url) {
            if manual {
                Self::notify_error(window, t!("err_remote_url"), cx);
            }
            return;
        }
        let url = item.url.clone();
        self.refreshing.insert(id.clone());
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let fetch_url = url.clone();
            let result = cx
                .background_executor()
                .spawn(async move { remote::fetch(&fetch_url) })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.refreshing.remove(&id);
                let now = store::now();
                match result {
                    Ok(content) => {
                        // The item may have been deleted or its URL edited
                        // while the fetch ran; such a result is discarded.
                        let unchanged = this
                            .store
                            .find(&id)
                            .is_some_and(|i| i.kind == ItemKind::Remote && i.url == url);
                        if unchanged {
                            this.mutate(window, cx, |store| {
                                if let Some(item) = store.find_mut(&id) {
                                    item.content = content;
                                    item.last_refresh = Some(now);
                                }
                            });
                            if manual {
                                window.push_notification(Notification::success(t!("remote_refreshed")), cx);
                            }
                        }
                    }
                    Err(err) => {
                        if let Some(item) = this.store.find_mut(&id) {
                            item.last_refresh = Some(now);
                        }
                        this.persist(cx);
                        if manual {
                            Self::notify_error(window, error_text(&err), cx);
                        } else {
                            eprintln!("remote refresh failed: {err:#}");
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Refresh due remote items every minute, starting right away so items
    /// overdue while the app was closed catch up at startup. The loop ends
    /// when the view is gone (app quit).
    fn start_remote_poll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            loop {
                let ok = this
                    .update_in(cx, |this, window, cx| {
                        for (id, _) in this.store.due_remotes(store::now()) {
                            this.refresh_remote(id, false, window, cx);
                        }
                    })
                    .is_ok();
                if !ok {
                    break;
                }
                cx.background_executor().timer(REMOTE_POLL).await;
            }
        })
        .detach();
    }

    fn record_history(&mut self, previous: &str, next: &str) {
        if previous == next {
            return;
        }
        let mut loaded = None;
        let history = match &mut self.history {
            Some(history) => history,
            None => loaded.insert(History::load()),
        };
        if history.record_change(previous, next)
            && let Err(err) = history.save()
        {
            eprintln!("save history failed: {err:#}");
        }
    }

    fn after_write(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reload_system_hosts(true);
        if self.selected == Selection::System {
            self.refresh_editor(window, cx);
        }
        cx.notify();
    }
}

fn display_title(title: &str) -> SharedString {
    if title.trim().is_empty() {
        t!("untitled").into()
    } else {
        title.to_string().into()
    }
}

/// Width of a sheet: `preferred`, but leaving part of a narrow window uncovered.
fn sheet_width(preferred: Pixels, window: &Window) -> Pixels {
    preferred.min(window.viewport_size().width * 0.9)
}

/// Footer row of a sheet, spanning the full sheet width below a divider.
fn sheet_footer(cx: &App) -> Div {
    // `Sheet` wraps its footer in 16px / 12px padding; cancel it so the divider reaches both edges.
    h_flex()
        .flex_1()
        .mx(px(-16.))
        .my(px(-12.))
        .px_4()
        .py_3()
        .justify_between()
        .border_t_1()
        .border_color(cx.theme().border)
}

fn item_icon(item: &Item) -> IconName {
    match item.kind {
        ItemKind::Local => IconName::FileText,
        ItemKind::Remote => IconName::Globe,
        ItemKind::Folder if item.collapsed => IconName::Folder,
        ItemKind::Folder => IconName::FolderOpen,
        ItemKind::Group => IconName::Layers,
    }
}

fn kind_label(kind: ItemKind) -> SharedString {
    match kind {
        ItemKind::Local => t!("kind_local"),
        ItemKind::Remote => t!("kind_remote"),
        ItemKind::Group => t!("kind_group"),
        ItemKind::Folder => t!("kind_folder"),
    }
    .into()
}

fn folder_mode_label(mode: FolderMode) -> SharedString {
    match mode {
        FolderMode::Default => t!("choice_default"),
        FolderMode::Single => t!("choice_single"),
        FolderMode::Multi => t!("choice_multi"),
    }
    .into()
}

impl Render for SwitchHostsApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Without any focus, key bindings dispatch from the window root and
        // never reach the actions handled here.
        if window.focused(cx).is_none() {
            self.focus_handle.focus(window, cx);
        }
        let total = f32::from(window.bounds().size.width);
        let browser = self.in_browser_view();
        if browser {
            // With the info panel open the tree keeps its own width and the
            // info panel takes the rest; closed, the tree fills like an editor.
            let (list, tree) = panels::fit_widths(
                total,
                self.show_list.then_some(self.browser_list_width),
                self.show_info.then_some(self.tree_width),
            );
            if let Some(width) = list {
                self.browser_list_width = width;
            }
            if let Some(width) = tree {
                self.tree_width = width;
            }
        } else {
            let (list, info) = panels::fit_widths(
                total,
                self.show_list.then_some(self.list_width),
                self.show_info.then_some(self.info_width),
            );
            if let Some(width) = list {
                self.list_width = width;
            }
            if let Some(width) = info {
                self.info_width = width;
            }
        }
        let list_panel = self.show_list.then(|| self.render_list_panel(cx).into_any_element());
        let list_divider = self.show_list.then(|| self.render_panel_divider(PanelEdge::List, cx));
        let info_divider = (self.show_info && !browser).then(|| self.render_panel_divider(PanelEdge::Info, cx));
        let right_panel = (self.show_info && !browser).then(|| self.render_right_panel(false, cx).into_any_element());
        v_flex()
            .track_focus(&self.focus_handle)
            .key_context("SwitchHostsApp")
            .on_action(cx.listener(|this, _: &ToggleComment, window, cx| {
                this.run_editor_op(editor_ops::toggle_comment, window, cx)
            }))
            .on_action(cx.listener(|this, _: &MoveLineUp, window, cx| {
                this.run_editor_op(editor_ops::move_line_up, window, cx)
            }))
            .on_action(cx.listener(|this, _: &MoveLineDown, window, cx| {
                this.run_editor_op(editor_ops::move_line_down, window, cx)
            }))
            .on_action(cx.listener(|this, _: &DuplicateLine, window, cx| {
                this.run_editor_op(editor_ops::duplicate_line, window, cx)
            }))
            .on_action(cx.listener(|this, _: &DeleteLine, window, cx| {
                this.run_editor_op(editor_ops::delete_line, window, cx)
            }))
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .on_mouse_move(cx.listener(Self::resize_panels))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, cx| this.finish_resize(cx)))
            .child(self.render_top_bar(cx))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_icon_sidebar(cx))
                    .children(list_panel)
                    .children(list_divider)
                    .child(self.render_main_panel(cx))
                    .children(info_divider)
                    .children(right_panel),
            )
    }
}
