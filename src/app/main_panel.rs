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
use super::browser_panel::{block_label, mode_short};
use super::splitter::PanelEdge;

/// Keeps a press on a toolbar control from becoming a Windows title-bar drag.
/// The bar's drag area makes the system swallow clicks on its children
/// (HTCAPTION), so controls need their own occluding hitbox; stopping the
/// press also keeps a one-pixel hand movement from starting a window move.
fn no_drag(control: impl IntoElement) -> Div {
    div()
        .occlude()
        .flex_shrink_0()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(control)
}

impl SwitchHostsApp {
    /// The window title bar, which doubles as the main toolbar.
    pub(super) fn render_top_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, border, warning) = (theme.muted_foreground, theme.border, theme.warning);
        let applied = self.is_applied();
        let in_browser = self.in_browser_view();

        let (icon, title) = match self.current() {
            Current::System => (Some(IconName::Monitor), t!("system_hosts").into()),
            Current::Item(item) => (Some(item_icon(item)), display_title(&item.title)),
            Current::Browser(profile) => (Some(IconName::Globe), display_title(&profile.name)),
            Current::Trash(trash) => (Some(item_icon(&trash.item)), display_title(&trash.item.title)),
            Current::TrashProfile(trash) => (Some(IconName::Globe), display_title(&trash.profile.name)),
            Current::None => (None, SharedString::default()),
        };
        let badge = self
            .selected_profile()
            .map(|profile| format!("{} · {}", profile.browser.short(), mode_short(profile.mode)));
        let readonly = !in_browser && icon.is_some() && self.editor_readonly();
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

        // Windows: the bar's first control sits above the 40px icon sidebar,
        // so 8px padding puts the 24px button's centre on its 20px centre line.
        TitleBar::new().h(TOP_BAR_HEIGHT).when(cfg!(windows), |this| this.pl(px(8.))).child(
            h_flex()
                .flex_1()
                .h_full()
                .min_w_0()
                .pr_2()
                .gap_1()
                .child(no_drag(
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
                ))
                .child(no_drag(
                    Button::new("add")
                        .ghost()
                        .small()
                        .icon(IconName::Plus)
                        .tooltip(if in_browser { t!("profile_add") } else { t!("hosts_add") })
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, window, cx| this.add_current(window, cx))),
                ))
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .justify_center()
                        .gap_2()
                        .children(icon.map(|icon| Icon::new(icon).small().text_color(muted)))
                        .child(div().min_w_0().truncate().text_sm().font_semibold().child(title))
                        .when_some(badge, |this, badge| {
                            this.child(
                                div()
                                    .flex_shrink_0()
                                    .px_1p5()
                                    .rounded(px(4.))
                                    .border_1()
                                    .border_color(border)
                                    .text_xs()
                                    .text_color(muted)
                                    .child(badge),
                            )
                        })
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
                        .children(switch.map(no_drag)),
                )
                .when(!applied && !in_browser, |this| {
                    this.child(
                        div()
                            .flex_shrink_0()
                            .text_xs()
                            .text_color(warning)
                            .child(t!("unapplied_changes")),
                    )
                })
                .child(no_drag(
                    Button::new("history")
                        .ghost()
                        .small()
                        .icon(IconName::Clock)
                        .tooltip(t!("show_history"))
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, window, cx| this.open_history(window, cx))),
                ))
                .map(|this| {
                    if in_browser {
                        let stoppable =
                            self.selected_profile().is_some_and(|profile| self.profile_stoppable(profile));
                        if stoppable {
                            return this.child(no_drag(
                                Button::new("stop-profile")
                                    .danger()
                                    .small()
                                    .icon(IconName::Square)
                                    .label(t!("stop"))
                                    .loading(self.stopping)
                                    .tooltip(t!("stop"))
                                    .when(!self.stopping, |this| this.cursor_pointer())
                                    .on_click(cx.listener(|this, _, window, cx| this.stop_profile(window, cx))),
                            ));
                        }
                        let blocked = self.profile_block();
                        let launchable = blocked.is_none();
                        let tooltip = match (blocked, self.selected_profile()) {
                            (Some(block), Some(profile)) => block_label(block, profile.browser),
                            _ => t!("launch").into(),
                        };
                        this.child(no_drag(
                            Button::new("launch-profile")
                                .primary()
                                .small()
                                .icon(IconName::Play)
                                .label(t!("launch"))
                                .loading(self.launching)
                                .disabled(!launchable)
                                .tooltip(tooltip)
                                .when(launchable, |this| this.cursor_pointer())
                                .on_click(cx.listener(|this, _, window, cx| this.launch_profile(window, cx))),
                        ))
                    } else {
                        this.child(no_drag(
                            Button::new("apply")
                                .primary()
                                .small()
                                .icon(IconName::Save)
                                .label(t!("apply"))
                                .loading(self.applying)
                                .tooltip(t!("apply_tooltip", shortcut = shortcut_label("S")))
                                .when(!self.applying, |this| this.cursor_pointer())
                                .on_click(cx.listener(|this, _, window, cx| this.apply(None, false, window, cx))),
                        ))
                    }
                })
                .child(no_drag(
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
                )),
        )
    }

    pub(super) fn render_main_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, border, warning) = (theme.muted_foreground, theme.border, theme.warning);
        if self.in_browser_view() {
            let content = match self.selected_profile() {
                Some(profile) => self.render_profile_tree(profile, cx),
                None => self.render_profile_empty(cx).into_any_element(),
            };
            if !self.show_info {
                return content;
            }
            return h_flex()
                .flex_1()
                .h_full()
                .min_w_0()
                .child(
                    v_flex()
                        .w(px(self.tree_width))
                        .h_full()
                        .flex_shrink_0()
                        .min_w_0()
                        .child(content),
                )
                .child(self.render_panel_divider(PanelEdge::Tree, cx))
                .child(self.render_right_panel(true, cx))
                .into_any_element();
        }
        let readonly = self.editor_readonly();
        let applied = self.is_applied();
        let (lines, bytes) = {
            let text = self.editor.read(cx).text();
            (text.lines_len(), text.len())
        };

        let counts = h_flex()
            .min_w_0()
            .gap_3()
            .overflow_hidden()
            .child(t!("line_count", count = lines))
            .child(t!("byte_count", count = bytes))
            .into_any_element();
        let status = h_flex()
            .flex_shrink_0()
            .gap_3()
            .when(readonly, |this| this.child(t!("read_only")))
            .map(|this| {
                if applied {
                    this.child(t!("applied"))
                } else {
                    this.child(div().text_color(warning).child(t!("unapplied_changes")))
                }
            })
            .into_any_element();

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
                    .child(counts)
                    .child(status),
            )
            .into_any_element()
    }
}
