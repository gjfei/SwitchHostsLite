use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem},
    notification::Notification,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rust_i18n::t;

use super::tree_row::{self, RowState, list_row, no_drag};
use super::{Selection, SwitchHostsApp, TrashRow, display_title, item_icon};
use crate::store::{self, DropPosition, Item, ItemKind};

/// How long a drag has to rest on a collapsed folder before it opens.
const EXPAND_DELAY: Duration = Duration::from_millis(1000);

/// The value of an item row drag: the item's id.
struct DraggedItem(String);

struct DragPreview {
    icon: IconName,
    title: SharedString,
}

impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        h_flex()
            .gap_1p5()
            .px_2()
            .py_1()
            .rounded(px(4.))
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .shadow_md()
            .text_sm()
            .child(Icon::new(self.icon).small().text_color(theme.muted_foreground))
            .child(self.title.clone())
    }
}

pub(super) struct DragState {
    source: String,
    /// Rows scrolled out of view still report their bounds, so a row only
    /// becomes the target while the pointer is over the visible list.
    in_list: bool,
    target: Option<(String, DropPosition)>,
    /// Bumped whenever the target row changes, so only an uninterrupted
    /// hover opens a collapsed folder.
    hover_generation: u64,
}

fn menu_entry(
    view: &Entity<SwitchHostsApp>,
    label: impl Into<SharedString>,
    f: impl Fn(&mut SwitchHostsApp, &mut Window, &mut Context<SwitchHostsApp>) + 'static,
) -> PopupMenuItem {
    let view = view.clone();
    PopupMenuItem::new(label).on_click(move |_, window, cx| view.update(cx, |this, cx| f(this, window, cx)))
}

impl SwitchHostsApp {
    pub(super) fn render_list_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let sidebar = theme.sidebar;
        let body = if self.in_trash_view() {
            self.render_trash_list(cx).into_any_element()
        } else if self.in_browser_view() {
            self.render_profile_list(cx)
        } else {
            self.render_hosts_list(cx).into_any_element()
        };
        let width = if self.in_browser_view() { self.browser_list_width } else { self.list_width };
        v_flex()
            .w(px(width))
            .h_full()
            .flex_shrink_0()
            .bg(sidebar)
            .child(body)
    }

    fn render_hosts_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let system = list_row("system".into(), self.selected == Selection::System, 0, cx)
            .child(Icon::new(IconName::Monitor).small().text_color(muted))
            .child(div().flex_1().truncate().text_sm().child(t!("system_hosts")))
            .on_click(cx.listener(|this, _, window, cx| this.select(Selection::System, window, cx)))
            .on_drag_move(cx.listener(Self::drag_off_rows));

        let drag = self.drag.as_ref().filter(|_| cx.has_active_drag());
        let mut rows = Vec::new();
        self.render_tree(&self.store.items, 0, false, drag, &mut rows, cx);

        let view = cx.entity();
        let empty_area = div()
            .id("list-empty-area")
            .flex_1()
            .min_h(px(48.))
            .on_drag_move(cx.listener(Self::drag_off_rows))
            .context_menu(move |menu, _, _| {
                let view = view.clone();
                menu.item(PopupMenuItem::new(t!("hosts_add")).on_click(move |_, window, cx| {
                    view.update(cx, |this, cx| this.open_add_sheet(window, cx))
                }))
            });

        v_flex()
            .id("hosts-list")
            .size_full()
            .overflow_y_scroll()
            .py_1p5()
            .gap_0p5()
            .child(system)
            .children(rows)
            .child(empty_area)
            // Capture-phase drag listeners run parent first, so this sees the
            // pointer before the rows do.
            .on_drag_move(cx.listener(|this, event: &DragMoveEvent<DraggedItem>, _, cx| {
                let inside = event.bounds.contains(&event.event.position);
                if let Some(drag) = &mut this.drag {
                    drag.in_list = inside;
                }
                if !inside {
                    this.set_drop_target(None, cx);
                }
            }))
            // Dropping between rows uses the row the pointer was last over.
            .on_drop(cx.listener(|this, dragged: &DraggedItem, window, cx| {
                let target = this.drag.take().and_then(|drag| drag.target);
                if let Some((target, position)) = target {
                    this.mutate(window, cx, |store| {
                        store.move_item(&dragged.0, &target, position);
                    });
                }
                cx.notify();
            }))
    }

    fn render_tree(
        &self,
        items: &[Item],
        depth: usize,
        dimmed: bool,
        drag: Option<&DragState>,
        out: &mut Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) {
        for item in items {
            let dimmed = dimmed || drag.is_some_and(|drag| drag.source == item.id);
            let drop = drag
                .and_then(|drag| drag.target.as_ref())
                .and_then(|(id, position)| (*id == item.id).then_some(*position));
            out.push(self.render_item_row(item, depth, dimmed, drop, cx).into_any_element());
            if item.kind == ItemKind::Folder && !item.collapsed {
                self.render_tree(&item.children, depth + 1, dimmed, drag, out, cx);
            }
        }
    }

    /// Rows other than items are never drop targets.
    fn drag_off_rows(&mut self, event: &DragMoveEvent<DraggedItem>, _: &mut Window, cx: &mut Context<Self>) {
        if event.bounds.contains(&event.event.position) {
            self.set_drop_target(None, cx);
        }
    }

    fn set_drop_target(&mut self, target: Option<(String, DropPosition)>, cx: &mut Context<Self>) {
        let Some(drag) = &mut self.drag else {
            return;
        };
        let target = target
            .filter(|(id, position)| drag.in_list && self.store.can_drop(&drag.source, id, *position));
        if drag.target == target {
            return;
        }
        let row_changed = drag.target.as_ref().map(|t| &t.0) != target.as_ref().map(|t| &t.0);
        drag.target = target;
        cx.notify();
        if !row_changed {
            return;
        }
        drag.hover_generation += 1;
        let Some((id, _)) = &drag.target else {
            return;
        };
        let closed = |f: &Item| f.kind == ItemKind::Folder && f.collapsed && !f.children.is_empty();
        if !self.store.find(id).is_some_and(closed) {
            return;
        }
        let (folder, generation) = (id.clone(), drag.hover_generation);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(EXPAND_DELAY).await;
            this.update(cx, |this, cx| {
                let hovering = cx.has_active_drag()
                    && this.drag.as_ref().is_some_and(|d| d.hover_generation == generation);
                if hovering && this.store.find(&folder).is_some_and(|f| f.collapsed) {
                    this.toggle_collapsed(&folder, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn render_item_row(
        &self,
        item: &Item,
        depth: usize,
        dimmed: bool,
        drop: Option<DropPosition>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let (primary, drop_target) = (theme.primary, theme.drop_target);
        let id = item.id.clone();
        let active = self.selected_id() == Some(id.as_str());
        let group: SharedString = format!("row-{id}").into();
        let is_folder = item.kind == ItemKind::Folder;

        let edit_id = id.clone();
        let edit_button = no_drag(

            Button::new(ElementId::Name(format!("edit-{id}").into()))
                .ghost()
                .xsmall()
                .icon(IconName::Pencil)
                .tooltip(t!("edit"))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.open_edit_sheet(edit_id.clone(), window, cx)
                })),
        )
        .invisible()
        .group_hover(group.clone(), |s| s.visible());

        let indicator = drop.filter(|p| *p != DropPosition::Inside).map(|position| {
            let line = div()
                .absolute()
                .left(px(8. + depth as f32 * 14.))
                .right_0()
                .h(px(2.))
                .rounded(px(1.))
                .bg(primary);
            // Rows are 2px apart, so the line sits in the gap.
            match position {
                DropPosition::Before => line.top(px(-2.)),
                _ => line.bottom(px(-2.)),
            }
        });

        let collapse_id = id.clone();
        let toggle_id = id.clone();
        let click_id = id.clone();
        let (drag_view, drag_icon, drag_title) = (cx.entity(), item_icon(item), display_title(&item.title));
        let move_id = id.clone();
        tree_row::item_row(
            item,
            depth,
            RowState {
                open: !item.collapsed,
                active,
                checked: item.on,
                switch_disabled: self.applying,
            },
            Some(edit_button.into_any_element()),
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.toggle_collapsed(&collapse_id, cx);
            }),
            cx.listener(move |this, on: &bool, window, cx| {
                // Keep the row from also selecting or opening the item.
                cx.stop_propagation();
                this.toggle(toggle_id.clone(), *on, window, cx)
            }),
            cx,
        )
            .group(group)
            .when(dimmed, |this| this.opacity(0.5))
            .when(drop == Some(DropPosition::Inside), |this| this.bg(drop_target))
            .children(indicator)
            .on_drag(DraggedItem(id.clone()), move |dragged, _, _, cx| {
                drag_view.update(cx, |this, _| {
                    let hover_generation = this.drag.as_ref().map_or(0, |d| d.hover_generation);
                    this.drag = Some(DragState {
                        source: dragged.0.clone(),
                        in_list: true,
                        target: None,
                        hover_generation,
                    });
                });
                let title = drag_title.clone();
                cx.new(|_| DragPreview { icon: drag_icon, title })
            })
            .on_drag_move(cx.listener(move |this, event: &DragMoveEvent<DraggedItem>, _, cx| {
                let (bounds, pointer) = (event.bounds, event.event.position);
                if bounds.contains(&pointer) {
                    let y = f32::from(pointer.y - bounds.top());
                    let position = DropPosition::at(y, f32::from(bounds.size.height), is_folder);
                    this.set_drop_target(Some((move_id.clone(), position)), cx);
                }
            }))
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                if event.click_count() == 2 {
                    this.open_edit_sheet(click_id.clone(), window, cx);
                } else {
                    this.select(Selection::Item(click_id.clone()), window, cx);
                }
            }))
            .context_menu(self.item_menu(id, cx))
    }

    /// Context menu builder for an item row.
    fn item_menu(
        &self,
        id: String,
        cx: &mut Context<Self>,
    ) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
        let view = cx.entity();
        move |menu, _, cx| {
            // Built when the menu opens rather than for every row on every frame.
            let store = &view.read(cx).store;
            let folders = store.folders(&id);
            let current = store.parent_of(&id).flatten();
            let (edit_id, delete_id) = (id.clone(), id.clone());
            let mut menu = menu
                .item(menu_entry(&view, t!("edit"), move |this, window, cx| {
                    this.open_edit_sheet(edit_id.clone(), window, cx)
                }))
                .separator()
                .item(menu_entry(&view, t!("move_to_trash"), move |this, window, cx| {
                    this.confirm_delete(delete_id.clone(), false, window, cx)
                }));
            if !folders.is_empty() {
                let target = |label: SharedString, parent: Option<String>| {
                    let checked = current == parent;
                    let id = id.clone();
                    menu_entry(&view, label, move |this, window, cx| {
                        this.move_item(id.clone(), parent.clone(), window, cx)
                    })
                    .checked(checked)
                };
                menu = menu
                    .separator()
                    .label(t!("move_to"))
                    .item(target(t!("top_level").into(), None));
                for (fid, title, depth) in &folders {
                    let label = format!("{}{}", "    ".repeat(depth + 1), display_title(title));
                    menu = menu.item(target(label.into(), Some(fid.clone())));
                }
            }
            menu
        }
    }

    /// Restoring can change the enabled content through a group including it.
    pub(super) fn restore_trash(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(profile_id) = self.store.restore_profile(&id) {
            self.store_changed(window, cx);
            self.select(Selection::Browser(Some(profile_id)), window, cx);
        } else {
            self.mutate(window, cx, |store| {
                store.restore(&id);
            });
        }
        window.push_notification(Notification::success(t!("restored")), cx);
    }

    pub(super) fn purge_trash(&mut self, id: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (view, id) = (view.clone(), id.clone());
            let message = match &id {
                Some(_) => t!("trash_delete_confirm"),
                None => t!("trash_clear_confirm"),
            };
            alert
                .confirm()
                .title(t!("delete_permanently"))
                .description(message)
                .on_ok(move |_, window, cx| {
                    view.update(cx, |this, cx| {
                        match &id {
                            Some(id) => {
                                this.store.purge(id);
                                this.store.purge_profile(id);
                            }
                            None => {
                                this.store.trash.clear();
                                this.store.trash_profiles.clear();
                            }
                        }
                        this.store_changed(window, cx);
                    });
                    true
                })
        });
    }

    fn render_trash_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, border) = (theme.muted_foreground, theme.sidebar_border);
        let rows = self.trash_rows();
        let header = h_flex()
            .h(px(36.))
            .flex_shrink_0()
            .pl_3()
            .pr_1p5()
            .justify_between()
            .border_b_1()
            .border_color(border)
            .child(div().text_sm().font_semibold().child(t!("trashcan")))
            .child(
                Button::new("empty-trash")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Trash)
                    .tooltip(t!("trash_clear"))
                    .disabled(rows.is_empty())
                    .when(!rows.is_empty(), |this| this.cursor_pointer())
                    .on_click(cx.listener(|this, _, window, cx| this.purge_trash(None, window, cx))),
            );

        if rows.is_empty() {
            return v_flex()
                .size_full()
                .child(header)
                .child(
                    v_flex()
                        .flex_1()
                        .items_center()
                        .justify_center()
                        .text_sm()
                        .text_color(muted)
                        .child(t!("trash_empty")),
                )
                .into_any_element();
        }

        let view = cx.entity();
        let rows = rows.into_iter().map(|row| {
            let TrashRow { id, profile, icon, title, deleted_at } = row;
            let active = self.selected == Selection::Trash(Some(id.clone()));
            let select_id = id.clone();
            let view = view.clone();
            let kind = if profile {
                SharedString::from(t!("browser_profiles"))
            } else {
                SharedString::from(t!("hosts_entry"))
            };
            list_row(ElementId::Name(format!("trash-{id}").into()), active, 0, cx)
                .h(px(44.))
                .child(Icon::new(icon).small().text_color(muted))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(div().text_sm().truncate().child(display_title(&title)))
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .truncate()
                                .child(format!("{kind} · {}", store::format_time(deleted_at))),
                        ),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.select(Selection::Trash(Some(select_id.clone())), window, cx)
                }))
                .context_menu(move |menu, _, _| {
                    let (v1, v2, id1, id2) = (view.clone(), view.clone(), id.clone(), id.clone());
                    menu.item(PopupMenuItem::new(t!("restore")).on_click(move |_, window, cx| {
                        v1.update(cx, |this, cx| this.restore_trash(id1.clone(), window, cx))
                    }))
                    .item(PopupMenuItem::new(t!("hosts_delete")).on_click(move |_, window, cx| {
                        v2.update(cx, |this, cx| this.purge_trash(Some(id2.clone()), window, cx))
                    }))
                })
        });

        v_flex()
            .size_full()
            .child(header)
            .child(
                v_flex()
                    .id("trash-list")
                    .flex_1()
                    .overflow_y_scroll()
                    .py_1p5()
                    .gap_0p5()
                    .children(rows.collect::<Vec<_>>()),
            )
            .into_any_element()
    }
}
