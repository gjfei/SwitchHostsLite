use std::borrow::Cow;

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Selectable as _, Sizable as _, StyledExt as _, WindowExt as _,
    badge::Badge,
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::{DropdownMenu as _, PopupMenuItem},
    v_flex,
};
use gpui_kit::*;
use rust_i18n::t;

use super::{SwitchHostsApp, preferences};
use crate::settings::{Settings, ThemePref};
use crate::updater;

const REPO_URL: &str = env!("CARGO_PKG_REPOSITORY");

fn nav_button(id: &'static str, icon: IconName, tooltip: impl Into<SharedString>) -> Button {
    Button::new(id).ghost().icon(icon).tooltip(tooltip).cursor_pointer()
}

impl SwitchHostsApp {
    pub(super) fn render_icon_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (sidebar, border, danger) = (theme.sidebar, theme.sidebar_border, theme.danger);
        let in_trash = self.in_trash_view();

        let trash = nav_button("nav-trash", IconName::Trash, t!("trashcan"))
            .selected(in_trash)
            .on_click(cx.listener(|this, _, window, cx| this.toggle_trash_view(window, cx)));
        let trash = if self.store.trash.is_empty() {
            trash.into_any_element()
        } else {
            Badge::new().dot().color(danger).child(trash).into_any_element()
        };

        v_flex()
            .w(px(40.))
            .h_full()
            .flex_shrink_0()
            .items_center()
            .py_2()
            .gap_1()
            .bg(sidebar)
            .border_r_1()
            .border_color(border)
            .child(
                nav_button("nav-list", IconName::List, t!("hosts_list"))
                    .selected(self.show_list && !in_trash)
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_list_view(window, cx))),
            )
            .child(
                nav_button("nav-history", IconName::Clock, t!("show_history"))
                    .on_click(cx.listener(|this, _, window, cx| this.open_history(window, cx))),
            )
            .child(div().flex_1())
            .child(trash)
            .child(self.render_settings_menu(cx))
    }

    fn render_settings_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let multi = self.store.multi_select;
        nav_button("nav-settings", IconName::Settings, t!("settings")).dropdown_menu_with_anchor(
            Anchor::BottomLeft,
            move |menu, window, cx| {
                let action = |label: Cow<'static, str>, f: fn(&mut Self, &mut Window, &mut Context<Self>)| {
                    let view = view.clone();
                    PopupMenuItem::new(label)
                        .on_click(move |_, window, cx| view.update(cx, |this, cx| f(this, window, cx)))
                };
                let current = Settings::global(cx).theme;
                let submenu_view = view.clone();
                menu.item(action(t!("menu_import"), Self::import))
                    .item(action(t!("menu_export"), Self::export))
                    .separator()
                    .item(action(t!("menu_preferences"), Self::open_preferences))
                    .submenu(t!("theme"), window, cx, move |mut menu, _, _| {
                        for theme in ThemePref::ALL {
                            menu = menu.item(
                                PopupMenuItem::new(preferences::theme_label(theme))
                                    .checked(theme == current)
                                    .on_click(move |_, window, cx| preferences::set_theme(theme, window, cx)),
                            );
                        }
                        let view = submenu_view.clone();
                        menu.separator().item(
                            PopupMenuItem::new(t!("more_color_themes")).on_click(move |_, window, cx| {
                                view.update(cx, |this, cx| this.open_preferences(window, cx))
                            }),
                        )
                    })
                    .item(
                        action(t!("multi_select"), |this, window, cx| {
                            let multi = !this.store.multi_select;
                            this.set_multi_select(multi, window, cx)
                        })
                        .checked(multi),
                    )
                    .separator()
                    .item(if updater::is_configured() {
                        action(t!("check_for_updates"), |_, _, cx| updater::check(true, cx))
                    } else {
                        PopupMenuItem::new(t!("check_for_updates_unconfigured")).disabled(true)
                    })
                    .item(action(t!("about"), |_, window, cx| Self::show_about(window, cx)))
            },
        )
    }

    fn show_about(window: &mut Window, cx: &mut App) {
        window.open_dialog(cx, |dialog, _, cx| {
            let theme = cx.theme();
            let muted = theme.muted_foreground;
            let repo = REPO_URL.strip_prefix("https://").unwrap_or(REPO_URL);
            dialog.w(px(360.)).centered().child(
                v_flex()
                    .items_center()
                    .gap_3()
                    .pt_2()
                    .child(img(crate::ABOUT_ICON).w(px(104.)).h(px(104.)))
                    .child(
                        div()
                            .text_xl()
                            .font_semibold()
                            .child("SwitchHosts Lite"),
                    )
                    .child(
                        div()
                            .px_2()
                            .py_0p5()
                            .rounded_full()
                            .bg(theme.muted)
                            .text_xs()
                            .text_color(muted)
                            .child(t!("about_version", version = env!("CARGO_PKG_VERSION"))),
                    )
                    .child(
                        div()
                            .max_w(px(260.))
                            .text_sm()
                            .text_center()
                            .text_color(muted)
                            .child(t!("about_description")),
                    )
                    .child(div().w_full().h(px(1.)).bg(theme.border))
                    .child(
                        h_flex()
                            .id("about-home-link")
                            .w_full()
                            .justify_center()
                            .gap_1p5()
                            .py_2()
                            .rounded_md()
                            .cursor_pointer()
                            .text_sm()
                            .text_color(theme.primary)
                            .hover(|this| this.bg(theme.list_hover))
                            .child(Icon::new(IconName::ExternalLink).xsmall())
                            .child(repo)
                            .on_click(|_, _, cx| cx.open_url(REPO_URL)),
                    ),
            )
        });
    }
}
