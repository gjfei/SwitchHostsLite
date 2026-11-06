use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, RopeExt as _, Selectable as _, Sizable as _,
    StyledExt as _, TitleBar,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::Editor,
    switch::Switch,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rust_i18n::t;

use super::{Current, SwitchHostsApp, TOP_BAR_HEIGHT, display_title, item_icon, shortcut_label};

impl SwitchHostsApp {
    /// The window title bar, which doubles as the main toolbar.
    pub(super) fn render_top_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, border, warning) = (theme.muted_foreground, theme.border, theme.warning);
        let applied = self.is_applied();

        let (icon, title) = match self.current() {
            Current::System => (Some(IconName::Monitor), t!("system_hosts").into()),
            Current::Item(item) => (Some(item_icon(item)), display_title(&item.title)),
            Current::Trash(trash) => (Some(item_icon(&trash.item)), display_title(&trash.item.title)),
            Current::None => (None, SharedString::default()),
        };
        let readonly = icon.is_some() && self.editor_readonly();
        let switch = match self.current() {
            Current::Item(item) if !self.show_list => {
                let id = item.id.clone();
                Some(
                    Switch::new("top-switch")
                        .small()
                        .checked(item.on)
                        .disabled(self.applying)
                        .when(!self.applying, |this| this.cursor_pointer())
                        .on_change(cx.listener(move |this, on: &bool, window, cx| {
                            this.toggle(id.clone(), *on, window, cx)
                        })),
                )
            }
            _ => None,
        };

        TitleBar::new().h(TOP_BAR_HEIGHT).child(
            h_flex()
                .flex_1()
                .h_full()
                .min_w_0()
                .pr_2()
                .gap_1()
                .child(
                    Button::new("toggle-list")
                        .ghost()
                        .small()
                        .icon(IconName::PanelLeft)
                        .tooltip(t!("toggle_list"))
                        .selected(self.show_list)
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_list = !this.show_list;
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("add")
                        .ghost()
                        .small()
                        .icon(IconName::Plus)
                        .tooltip(t!("hosts_add"))
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, window, cx| this.open_add_sheet(window, cx))),
                )
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .justify_center()
                        .gap_2()
                        .children(icon.map(|icon| Icon::new(icon).small().text_color(muted)))
                        .child(div().min_w_0().truncate().text_sm().font_semibold().child(title))
                        .when(readonly, |this| {
                            this.child(
                                div()
                                    .flex_shrink_0()
                                    .px_1p5()
                                    .rounded(px(4.))
                                    .border_1()
                                    .border_color(border)
                                    .text_xs()
                                    .text_color(muted)
                                    .child(t!("read_only")),
                            )
                        })
                        .children(switch),
                )
                .when(!applied, |this| {
                    this.child(
                        div()
                            .flex_shrink_0()
                            .text_xs()
                            .text_color(warning)
                            .child(t!("unapplied_changes")),
                    )
                })
                .child(
                    Button::new("apply")
                        .primary()
                        .small()
                        .icon(IconName::Save)
                        .label(t!("apply"))
                        .loading(self.applying)
                        .tooltip(t!("apply_tooltip", shortcut = shortcut_label("S")))
                        .when(!self.applying, |this| this.cursor_pointer())
                        .on_click(cx.listener(|this, _, window, cx| this.apply(None, false, window, cx))),
                )
                .child(
                    Button::new("toggle-info")
                        .ghost()
                        .small()
                        .icon(IconName::PanelRight)
                        .tooltip(t!("toggle_info"))
                        .selected(self.show_info)
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_info = !this.show_info;
                            cx.notify();
                        })),
                ),
        )
    }

    pub(super) fn render_main_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, border, warning) = (theme.muted_foreground, theme.border, theme.warning);
        let readonly = self.editor_readonly();
        let applied = self.is_applied();
        let (lines, bytes) = {
            let text = self.editor.read(cx).text();
            (text.lines_len(), text.len())
        };

        v_flex()
            .flex_1()
            .h_full()
            .min_w_0()
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(Editor::new(&self.editor).h_full().bordered(false).readonly(readonly)),
            )
            .child(
                h_flex()
                    .h(px(24.))
                    .flex_shrink_0()
                    .px_3()
                    .gap_3()
                    .justify_between()
                    .whitespace_nowrap()
                    .border_t_1()
                    .border_color(border)
                    .text_xs()
                    .text_color(muted)
                    // The counts give way first when the panel is narrow.
                    .child(
                        h_flex()
                            .min_w_0()
                            .gap_3()
                            .overflow_hidden()
                            .child(t!("line_count", count = lines))
                            .child(t!("byte_count", count = bytes)),
                    )
                    .child(
                        h_flex()
                            .flex_shrink_0()
                            .gap_3()
                            .when(readonly, |this| this.child(t!("read_only")))
                            .map(|this| {
                                if applied {
                                    this.child(t!("applied"))
                                } else {
                                    this.child(div().text_color(warning).child(t!("unapplied_changes")))
                                }
                            }),
                    ),
            )
    }
}
