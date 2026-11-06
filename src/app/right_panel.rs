use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::*;
use rust_i18n::t;

use super::{Current, SwitchHostsApp, display_title, folder_mode_label, item_icon, kind_label};
use crate::store::{self, Item, ItemKind, TrashItem};
use crate::hosts;

fn heading(icon: IconName, title: SharedString) -> impl IntoElement {
    h_flex()
        .gap_2()
        .child(Icon::new(icon).large())
        .child(div().flex_1().min_w_0().truncate().font_semibold().child(title))
}

fn field(label: impl Into<SharedString>, value: impl Into<SharedString>, cx: &App) -> impl IntoElement {
    h_flex()
        .gap_2()
        .justify_between()
        .text_sm()
        .child(div().flex_shrink_0().text_color(cx.theme().muted_foreground).child(label.into()))
        .child(div().min_w_0().truncate().child(value.into()))
}

impl SwitchHostsApp {
    pub(super) fn render_right_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted) = (theme.border, theme.muted_foreground);
        let body = match self.current() {
            Current::System => self.render_system_info(cx).into_any_element(),
            Current::Item(item) => self.render_item_info(item, cx).into_any_element(),
            Current::Trash(trash) => self.render_trash_info(trash, cx).into_any_element(),
            Current::None => v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(muted)
                .child(t!("nothing_selected"))
                .into_any_element(),
        };
        v_flex()
            .id("info-panel")
            .w(px(240.))
            .h_full()
            .flex_shrink_0()
            .overflow_y_scroll()
            .p_3()
            .gap_3()
            .border_l_1()
            .border_color(border)
            .child(body)
    }

    fn render_system_info(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let path = match hosts::hosts_path() {
            Ok(path) => path.display().to_string(),
            Err(err) => err.message(),
        };
        v_flex()
            .gap_3()
            .child(heading(IconName::Monitor, t!("system_hosts").into()))
            .child(field(t!("path"), path, cx))
            .child(field(t!("rules"), self.rule_count().to_string(), cx))
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(t!("system_hosts_hint")),
            )
    }

    fn render_item_info(&self, item: &Item, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, danger) = (theme.muted_foreground, theme.danger);
        let rules = self.rule_count();
        let (edit_id, delete_id) = (item.id.clone(), item.id.clone());

        let mut fields = v_flex()
            .gap_2()
            .child(field(t!("hosts_type"), kind_label(item.kind), cx))
            .child(field(t!("rules"), rules.to_string(), cx));
        match item.kind {
            ItemKind::Folder => {
                fields = fields
                    .child(field(t!("choice_mode"), folder_mode_label(item.folder_mode), cx))
                    .child(field(t!("child_count"), item.children.len().to_string(), cx));
            }
            ItemKind::Group => {
                let titles: Vec<SharedString> = item
                    .include
                    .iter()
                    .filter_map(|id| self.store.find(id))
                    .map(|i| display_title(&i.title))
                    .collect();
                fields = fields
                    .child(div().text_sm().text_color(muted).child(t!("included_items")))
                    .child(if titles.is_empty() {
                        div().text_sm().text_color(muted).child(t!("none_included")).into_any_element()
                    } else {
                        v_flex()
                            .gap_1()
                            .text_sm()
                            .children(titles.into_iter().map(|t| div().truncate().child(t)))
                            .into_any_element()
                    });
            }
            ItemKind::Local => {}
        }

        v_flex()
            .flex_1()
            .gap_3()
            .child(heading(item_icon(item), display_title(&item.title)))
            .child(
                Button::new("info-edit")
                    .outline()
                    .small()
                    .label(t!("edit"))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_edit_sheet(edit_id.clone(), window, cx)
                    })),
            )
            .child(fields)
            .child(div().flex_1())
            .child(
                Button::new("info-delete")
                    .ghost()
                    .small()
                    .text_color(danger)
                    .icon(IconName::Trash)
                    .label(t!("move_to_trash"))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.confirm_delete(delete_id.clone(), false, window, cx)
                    })),
            )
    }

    fn render_trash_info(&self, trash: &TrashItem, cx: &mut Context<Self>) -> impl IntoElement {
        let (restore_id, purge_id) = (trash.item.id.clone(), trash.item.id.clone());
        v_flex()
            .gap_3()
            .child(heading(item_icon(&trash.item), display_title(&trash.item.title)))
            .child(field(t!("hosts_type"), kind_label(trash.item.kind), cx))
            .child(field(t!("deleted_at"), store::format_time(trash.deleted_at), cx))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("info-restore")
                            .small()
                            .icon(IconName::RotateCcw)
                            .label(t!("restore"))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.restore_trash(restore_id.clone(), window, cx)
                            })),
                    )
                    .child(
                        Button::new("info-purge")
                            .danger()
                            .small()
                            .label(t!("delete_permanently"))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.purge_trash(Some(purge_id.clone()), window, cx)
                            })),
                    ),
            )
    }
}
