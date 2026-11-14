use std::{cell::Cell, ops::Range, time::Duration};

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, WindowExt as _, h_flex,
    input::{EditorState, InputEvent},
    notification::Notification,
    v_flex,
};
use gpui_kit::*;

use rust_i18n::t;

use crate::i18n::error_text;
use crate::settings::{Settings, ThemePref};
use crate::store::{FolderMode, History, Item, ItemKind, Store, TrashItem};
use crate::{editor_ops, highlight, hosts, store};

mod backup;
mod edit_sheet;
mod history_sheet;
mod icon_sidebar;
mod list_panel;
mod main_panel;
mod preferences;
mod right_panel;

use edit_sheet::EditDraft;

actions!(
    switch_hosts,
    [ApplyHosts, ToggleComment, MoveLineUp, MoveLineDown, DuplicateLine, DeleteLine]
);

/// Key context of the code editor, excluding single-line inputs.
const EDITOR_CONTEXT: &str = "Input && mode == editor";
const AUTO_APPLY_DELAY: Duration = Duration::from_millis(1000);
/// Typing is written to disk once it pauses this long.
const SAVE_DELAY: Duration = Duration::from_millis(500);
/// Height of the window title bar, which doubles as the main toolbar.
const TOP_BAR_HEIGHT: Pixels = px(44.);

/// Load the preferences and apply their language and theme.
pub fn init(cx: &mut App) {
    cx.set_global(Settings::load());
    preferences::apply_language(cx);
    let appearance = cx.window_appearance();
    preferences::apply_theme(appearance, cx);
}

pub fn bind_keys(cx: &mut App) {
    let editor = Some(EDITOR_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("cmd-s", ApplyHosts, None),
        KeyBinding::new("ctrl-s", ApplyHosts, None),
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
    /// The trash view, with the selected trash entry.
    Trash(Option<String>),
}

/// What the main panel currently shows.
enum Current<'a> {
    System,
    Item(&'a Item),
    Trash(&'a TrashItem),
    None,
}

pub struct SwitchHostsApp {
    store: Store,
    /// Loaded only while the history sheet is open.
    history: Option<History>,
    selected: Selection,
    /// Selection to return to when leaving the trash view.
    last_list_selection: Selection,
    show_list: bool,
    show_info: bool,
    history_selected: Option<String>,
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
                if window.is_window_active() && this.reload_system_hosts(false) {
                    if this.selected == Selection::System {
                        this.refresh_editor(window, cx);
                    }
                    cx.notify();
                }
            }),
            cx.on_app_quit(|this, _| {
                this.flush_save();
                async {}
            }),
        ];

        Self {
            store,
            history: None,
            selected: Selection::System,
            last_list_selection: Selection::System,
            show_list: true,
            show_info: false,
            history_selected: None,
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
        }
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

    fn current(&self) -> Current<'_> {
        match &self.selected {
            Selection::System => Current::System,
            Selection::Item(id) => self.store.find(id).map_or(Current::None, Current::Item),
            Selection::Trash(Some(id)) => self.trash_item(id).map_or(Current::None, Current::Trash),
            Selection::Trash(None) => Current::None,
        }
    }

    fn in_trash_view(&self) -> bool {
        matches!(self.selected, Selection::Trash(_))
    }

    /// Only local items are edited in the main editor.
    fn editor_readonly(&self) -> bool {
        !matches!(self.current(), Current::Item(item) if item.kind == ItemKind::Local)
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
            Current::Item(item) if item.kind == ItemKind::Local => store::count_rules(&item.content),
            Current::Item(item) => store::count_rules(&self.store.content_of(item, false)),
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
    /// away only when that needs no password prompt; otherwise it waits for
    /// an explicit apply.
    fn mutate(&mut self, window: &mut Window, cx: &mut Context<Self>, f: impl FnOnce(&mut Store)) {
        let before = self.store.enabled_content();
        f(&mut self.store);
        self.store_changed(window, cx);
        if self.store.enabled_content() != before && hosts::can_write_directly() {
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
        self.selected = selection;
        self.rules.set(None);
        self.refresh_editor(window, cx);
    }

    /// Fall back to a valid selection after items were removed.
    fn validate_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selection = match &self.selected {
            Selection::Item(id) if self.store.find(id).is_none() => Selection::System,
            Selection::Trash(Some(id)) if self.trash_item(id).is_none() => {
                Selection::Trash(self.store.trash.first().map(|t| t.item.id.clone()))
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
            let first = self.store.trash.first().map(|t| t.item.id.clone());
            self.select(Selection::Trash(first), window, cx);
        }
    }

    /// The text the editor should show for the current selection.
    fn editor_text(&self) -> String {
        match self.current() {
            Current::System => self.system_hosts.clone(),
            Current::Item(item) => self.store.content_of(item, false),
            Current::Trash(trash) => self.store.content_of(&trash.item, true),
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

    /// Apply edits of an effective item once typing pauses, but only when no
    /// password prompt is needed.
    fn schedule_auto_apply(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.edit_generation += 1;
        let generation = self.edit_generation;
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(AUTO_APPLY_DELAY).await;
            this.update_in(cx, |this, window, cx| {
                if this.edit_generation != generation
                    || !this.store.is_effective(&id)
                    || this.is_applied()
                    || !hosts::can_write_directly()
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

    fn on_apply(&mut self, _: &ApplyHosts, window: &mut Window, cx: &mut Context<Self>) {
        self.apply(None, false, window, cx);
    }
}

/// Label of the shortcut bound to `cmd-<key>` on macOS and `ctrl-<key>` elsewhere.
fn shortcut_label(key: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("⌘{key}")
    } else {
        format!("Ctrl+{key}")
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
        ItemKind::Folder if item.collapsed => IconName::Folder,
        ItemKind::Folder => IconName::FolderOpen,
        ItemKind::Group => IconName::Layers,
    }
}

fn kind_label(kind: ItemKind) -> SharedString {
    match kind {
        ItemKind::Local => t!("kind_local"),
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
        let list_panel = self.show_list.then(|| self.render_list_panel(cx).into_any_element());
        let right_panel = self.show_info.then(|| self.render_right_panel(cx).into_any_element());
        v_flex()
            .track_focus(&self.focus_handle)
            .key_context("SwitchHostsApp")
            .on_action(cx.listener(Self::on_apply))
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
            .child(self.render_top_bar(cx))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_icon_sidebar(cx))
                    .children(list_panel)
                    .child(self.render_main_panel(cx))
                    .children(right_panel),
            )
    }
}
