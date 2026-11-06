use std::borrow::Cow;

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Selectable as _, WindowExt as _,
    badge::Badge,
    button::{Button, ButtonVariants as _},
    menu::{DropdownMenu as _, PopupMenuItem},
    v_flex,
};
use gpui_kit::*;
use rust_i18n::t;

use super::{SwitchHostsApp, preferences};
use crate::settings::{Settings, ThemePref};
use crate::updater;

const REPO_URL: &str = "https://github.com/oldj/SwitchHosts";

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
        window.open_alert_dialog(cx, |alert, _, cx| {
            let muted = cx.theme().muted_foreground;
            alert.title("SwitchHosts Lite").description(
                v_flex()
                    .gap_1()
                    .child(t!("about_version", version = env!("CARGO_PKG_VERSION")))
                    .child(div().text_color(muted).child(t!("about_description")))
                    .child(
                        Button::new("about-link")
                            .link()
                            .label("github.com/oldj/SwitchHosts")
                            .on_click(|_, _, cx| cx.open_url(REPO_URL)),
                    ),
            )
        });
    }
}
