use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, h_flex, switch::Switch,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::display_title;
use crate::store::{Item, ItemKind};

/// Keeps pressing a control from starting a drag of its row.
pub(super) fn no_drag(control: impl IntoElement) -> Div {
    div()
        .flex_shrink_0()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(control)
}

/// A selectable list row: 32px high with a primary indicator bar when active.
pub(super) fn list_row(id: ElementId, active: bool, depth: usize, cx: &App) -> Stateful<Div> {
    let theme = cx.theme();
    let (active_bg, hover_bg, primary) = (theme.list_active, theme.list_hover, theme.primary);
    h_flex()
        .id(id)
        .relative()
        .flex_shrink_0()
        .h(px(32.))
        .mx(px(6.))
        .pl(px(8. + depth as f32 * 14.))
        .pr_1()
        .gap_1p5()
        .rounded(px(4.))
        .cursor_pointer()
        .when(active, |this| {
            this.bg(active_bg).child(
                div()
                    .absolute()
                    .left_0()
                    .top(px(6.))
                    .bottom(px(6.))
                    .w(px(3.))
                    .rounded(px(2.))
                    .bg(primary),
            )
        })
        .when(!active, |this| this.hover(|s| s.bg(hover_bg)))
}

pub(super) struct RowState {
    /// The row's folder is expanded (affects the icons).
    pub open: bool,
    pub active: bool,
    pub checked: bool,
    pub switch_disabled: bool,
}

/// The 32px tree row shared by the hosts list and the browser profile tree:
/// chevron, icon, title, an optional trailing element and the switch. Callers
/// add their own row events to the result.
pub(super) fn item_row(
    item: &Item,
    depth: usize,
    state: RowState,
    trailing: Option<AnyElement>,
    on_collapse: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_switch: impl Fn(&bool, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Stateful<Div> {
    let muted = cx.theme().muted_foreground;
    let is_folder = item.kind == ItemKind::Folder;
    let icon = match item.kind {
        ItemKind::Local => IconName::FileText,
        ItemKind::Group => IconName::Layers,
        ItemKind::Folder if state.open => IconName::FolderOpen,
        ItemKind::Folder => IconName::Folder,
    };
    let id = item.id.clone();

    let chevron = div()
        .id(ElementId::Name(format!("chevron-{id}").into()))
        .size_4()
        .flex_shrink_0()
        .when(is_folder, |this| {
            let icon = if state.open { IconName::ChevronDown } else { IconName::ChevronRight };
            this.child(Icon::new(icon).xsmall().text_color(muted))
                .cursor_pointer()
                .on_click(on_collapse)
        });
    let chevron = div()
        .flex_shrink_0()
        .when(is_folder, |this| {
            this.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        })
        .child(chevron);

    let switch = Switch::new(ElementId::Name(format!("switch-{id}").into()))
        .small()
        .checked(state.checked)
        .disabled(state.switch_disabled)
        .on_change(on_switch);

    list_row(ElementId::Name(format!("item-{id}").into()), state.active, depth, cx)
        .child(chevron)
        .child(Icon::new(icon).small().text_color(muted))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_sm()
                .child(display_title(&item.title)),
        )
        .children(trailing)
        .child(no_drag(switch))
}
