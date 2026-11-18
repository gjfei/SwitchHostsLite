use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::Editor,
    notification::Notification,
    sheet::Sheet,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rust_i18n::t;

use super::browser_panel::{profile_rev_text, rev_kind_label};
use super::{HistoryScope, Selection, SwitchHostsApp, display_title, sheet_footer, sheet_width};
use crate::browser::{self, RevKind};
use crate::i18n::error_text;
use crate::store::{self, History, HistoryEntry};
use crate::hosts;

/// One entry of the sheet's list, from either the hosts history or the
/// selected profile's revisions.
struct HistoryRow {
    id: String,
    created_at: i64,
    meta: SharedString,
    text: String,
}

impl SwitchHostsApp {
    pub(super) fn open_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.history_scope = match &self.selected {
            Selection::Browser(Some(id)) => HistoryScope::Profile(id.clone()),
            _ => HistoryScope::Hosts,
        };
        self.history = matches!(self.history_scope, HistoryScope::Hosts).then(History::load);
        self.history_selected = self.history_rows(false).first().map(|row| row.id.clone());
        self.history_editor = Some(Self::new_editor(String::new(), window, cx));
        self.refresh_history_editor(window, cx);
        let view = cx.entity();
        window.open_sheet(cx, move |sheet, window, cx| {
            Self::render_history_sheet(&view, sheet, window, cx)
        });
    }

    /// Drop the history and its editor, which are only needed while the sheet is open.
    fn close_history(&mut self) {
        self.history = None;
        self.history_editor = None;
        self.history_selected = None;
    }

    fn history_entries(&self) -> &[HistoryEntry] {
        self.history.as_ref().map_or(&[], |h| &h.entries)
    }

    /// `with_text` is off while rendering: hosts entries can be large, and the
    /// editor only needs the selected one.
    fn history_rows(&self, with_text: bool) -> Vec<HistoryRow> {
        match &self.history_scope {
            HistoryScope::Hosts => self
                .history_entries()
                .iter()
                .map(|entry| HistoryRow {
                    id: entry.id.clone(),
                    created_at: entry.created_at,
                    meta: t!(
                        "history_entry_size",
                        lines = entry.line_count(),
                        bytes = entry.content.len()
                    )
                    .into(),
                    text: if with_text { entry.content.clone() } else { String::new() },
                })
                .collect(),
            HistoryScope::Profile(id) => {
                let Some(profile) = self.store.profile(id) else {
                    return Vec::new();
                };
                profile
                    .revs
                    .iter()
                    .map(|rev| {
                        HistoryRow {
                            id: rev.id.clone(),
                            created_at: rev.created_at,
                            meta: rev_kind_label(rev.kind),
                            text: if with_text {
                                let mappings = browser::mappings(
                                    &self.store.switched_content(&rev.snapshot.switches),
                                )
                                .len();
                                profile_rev_text(&rev.snapshot, mappings)
                            } else {
                                String::new()
                            },
                        }
                    })
                    .collect()
            }
        }
    }

    fn selected_history_row(&self) -> Option<HistoryRow> {
        let id = self.history_selected.as_ref()?;
        self.history_rows(true).into_iter().find(|row| &row.id == id)
    }

    fn refresh_history_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let content = self.selected_history_row().map(|row| row.text).unwrap_or_default();
        if let Some(editor) = &self.history_editor {
            editor.update(cx, |state, cx| state.set_value(content, window, cx));
        }
        cx.notify();
    }

    fn select_history(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.history_selected = Some(id);
        self.refresh_history_editor(window, cx);
    }

    fn restore_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.selected_history_row() else {
            return;
        };
        if let HistoryScope::Profile(profile_id) = self.history_scope.clone() {
            self.restore_profile_rev(&profile_id, &row.id, window, cx);
            return;
        }
        let content = row.text;
        let time = store::format_time(row.created_at);
        let view = cx.entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (view, content) = (view.clone(), content.clone());
            alert
                .confirm()
                .title(t!("restore_history_title"))
                .description(t!("restore_history_confirm", time = time))
                .on_ok(move |_, window, cx| {
                    let content = content.clone();
                    view.update(cx, |this, cx| this.write_history(content, window, cx));
                    true
                })
        });
    }

    /// Restore an older settings snapshot as the current state of the profile,
    /// first recording the state being replaced so the restore is undoable.
    fn restore_profile_rev(
        &mut self,
        profile_id: &str,
        rev_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(profile) = self.store.profile_mut(profile_id) else {
            return;
        };
        let Some(rev) = profile.revs.iter().find(|rev| rev.id == rev_id) else {
            return;
        };
        let snapshot = rev.snapshot.clone();
        profile.push_rev(RevKind::Restore);
        profile.apply_snapshot(&snapshot);
        crate::tile::refresh(profile);
        crate::tile::invalidate_preview(profile_id, cx);
        self.persist(cx);
        self.refresh_profile_inputs(window, cx);
        self.history_selected = self.history_rows(false).first().map(|row| row.id.clone());
        self.refresh_history_editor(window, cx);
        window.push_notification(Notification::success(t!("profile_restored")), cx);
    }

    fn write_history(&mut self, content: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.applying {
            return;
        }
        self.applying = true;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let to_write = content.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    let previous = hosts::read_system();
                    hosts::write_raw(&to_write).map(|()| previous)
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.applying = false;
                match result {
                    Ok(previous) => {
                        this.record_history(&previous, &content);
                        this.history_selected = this.history_entries().first().map(|e| e.id.clone());
                        this.refresh_history_editor(window, cx);
                        window.push_notification(Notification::success(t!("history_restored")), cx);
                    }
                    Err(err) => Self::notify_error(window, error_text(&err), cx),
                }
                this.after_write(window, cx);
            })
            .ok();
        })
        .detach();
    }

    fn delete_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.history_selected.take() else {
            return;
        };
        match self.history_scope.clone() {
            HistoryScope::Hosts => {
                if let Some(history) = &mut self.history {
                    history.remove(&id);
                    history.save().ok();
                }
            }
            HistoryScope::Profile(profile_id) => {
                if let Some(profile) = self.store.profile_mut(&profile_id) {
                    profile.revs.retain(|rev| rev.id != id);
                }
                self.persist(cx);
            }
        }
        self.history_selected = self.history_rows(false).first().map(|row| row.id.clone());
        self.refresh_history_editor(window, cx);
    }

    fn clear_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let view = view.clone();
            alert
                .confirm()
                .title(t!("clear_history"))
                .description(t!("history_clear_confirm"))
                .on_ok(move |_, window, cx| {
                    view.update(cx, |this, cx| {
                        match this.history_scope.clone() {
                            HistoryScope::Hosts => {
                                if let Some(history) = &mut this.history {
                                    history.entries.clear();
                                    history.save().ok();
                                }
                            }
                            HistoryScope::Profile(profile_id) => {
                                if let Some(profile) = this.store.profile_mut(&profile_id) {
                                    profile.revs.clear();
                                }
                                this.persist(cx);
                            }
                        }
                        this.history_selected = None;
                        this.refresh_history_editor(window, cx);
                    });
                    true
                })
        });
    }

    fn render_history_sheet(view: &Entity<Self>, sheet: Sheet, window: &Window, cx: &App) -> Sheet {
        let this = view.read(cx);
        let theme = cx.theme();
        let (muted, border, danger) = (theme.muted_foreground, theme.border, theme.danger);
        let (active_bg, hover_bg) = (theme.list_active, theme.list_hover);
        let has_selection = this.history_selected.is_some();
        let rows = this.history_rows(false);
        let Some(history_editor) = &this.history_editor else {
            return sheet;
        };

        let body = if rows.is_empty() {
            v_flex()
                .size_full()
                .min_h(px(240.))
                .items_center()
                .justify_center()
                .text_color(muted)
                .child(t!("no_record"))
                .into_any_element()
        } else {
            let entries = rows.iter().map(|row| {
                let id = row.id.clone();
                let active = this.history_selected.as_ref() == Some(&row.id);
                let view = view.clone();
                v_flex()
                    .id(ElementId::Name(format!("history-{id}").into()))
                    .flex_shrink_0()
                    .px_2()
                    .py_1()
                    .rounded(px(4.))
                    .cursor_pointer()
                    .when(active, |this| this.bg(active_bg))
                    .when(!active, |this| this.hover(|s| s.bg(hover_bg)))
                    .child(div().text_sm().child(store::format_time(row.created_at)))
                    .child(div().text_xs().text_color(muted).child(row.meta.clone()))
                    .on_click(move |_, window, cx| {
                        let id = id.clone();
                        view.update(cx, |this, cx| this.select_history(id, window, cx))
                    })
            });
            h_flex()
                .size_full()
                .min_h(px(240.))
                .py_2()
                .gap_3()
                .items_start()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .border_1()
                        .border_color(border)
                        .rounded(px(4.))
                        .overflow_hidden()
                        .child(Editor::new(history_editor).h_full().bordered(false).readonly(true)),
                )
                .child(
                    v_flex()
                        .id("history-list")
                        .w(px(220.))
                        .h_full()
                        .flex_shrink_0()
                        .gap_0p5()
                        .overflow_y_scroll()
                        .children(entries.collect::<Vec<_>>()),
                )
                .into_any_element()
        };

        let action = |f: fn(&mut Self, &mut Window, &mut Context<Self>)| {
            let view = view.clone();
            move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                view.update(cx, |this, cx| f(this, window, cx))
            }
        };
        let footer = sheet_footer(cx)
            .child(
                Button::new("history-clear")
                    .ghost()
                    .small()
                    .text_color(danger)
                    .icon(IconName::Trash)
                    .label(t!("clear_history"))
                    .disabled(rows.is_empty())
                    .when(!rows.is_empty(), |this| this.cursor_pointer())
                    .on_click(action(Self::clear_history)),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("history-delete")
                            .ghost()
                            .small()
                            .label(t!("delete_record"))
                            .disabled(!has_selection)
                            .when(has_selection, |this| this.cursor_pointer())
                            .on_click(action(Self::delete_history)),
                    )
                    .child(
                        Button::new("history-restore")
                            .primary()
                            .small()
                            .icon(IconName::RotateCcw)
                            .label(t!("restore_this_version"))
                            .disabled(!has_selection || this.applying)
                            .when(has_selection && !this.applying, |this| this.cursor_pointer())
                            .on_click(action(Self::restore_history)),
                    )
                    .child(Button::new("history-close").small().label(t!("close")).cursor_pointer().on_click({
                        let view = view.clone();
                        move |_, window, cx| {
                            view.update(cx, |this, _| this.close_history());
                            window.close_sheet(cx);
                        }
                    })),
            );

        let close_view = view.clone();
        let title = match &this.history_scope {
            HistoryScope::Hosts => SharedString::from(t!("system_hosts_history")),
            HistoryScope::Profile(id) => {
                let name = this
                    .store
                    .profile(id)
                    .map_or_else(|| SharedString::from(t!("profile_default_name")), |p| display_title(&p.name));
                t!("profile_history", name = name).into()
            }
        };
        sheet
            .title(title)
            .size(sheet_width(px(720.), window))
            .child(body)
            .footer(footer)
            // Esc, the close button and the overlay close the sheet without the footer.
            .on_close(move |_, _, cx| close_view.update(cx, |this, _| this.close_history()))
    }
}
