//! Draggable dividers between the list panel, the main panel and the info panel.

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::*;

use super::SwitchHostsApp;
use crate::panels::{BAND, MIN_PANEL, max_width};
use crate::settings::Settings;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PanelEdge {
    List,
    Tree,
    Info,
}

pub(super) struct PanelResize {
    edge: PanelEdge,
    start_x: f32,
    start_width: f32,
}

impl SwitchHostsApp {
    pub(super) fn render_panel_divider(&self, edge: PanelEdge, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let hover_bg = theme.list_hover;
        let (band_bg, line) = match edge {
            PanelEdge::List => (theme.sidebar, theme.sidebar_border),
            PanelEdge::Tree | PanelEdge::Info => (theme.background, theme.border),
        };
        let band = div()
            .id(match edge {
                PanelEdge::List => "resize-list",
                PanelEdge::Tree => "resize-tree",
                PanelEdge::Info => "resize-info",
            })
            .flex_shrink_0()
            .h_full()
            .w(px(BAND))
            .bg(band_bg)
            .cursor_col_resize()
            .hover(move |this| this.bg(hover_bg))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    let start_width = match edge {
                        PanelEdge::List => {
                            if this.in_browser_view() { this.browser_list_width } else { this.list_width }
                        }
                        PanelEdge::Tree => this.tree_width,
                        PanelEdge::Info => this.info_width,
                    };
                    this.resize = Some(PanelResize {
                        edge,
                        start_x: f32::from(event.position.x),
                        start_width,
                    });
                    cx.notify();
                }),
            );
        // The seam lives on the main panel's edge, so the status bar's border runs into it.
        match edge {
            PanelEdge::List => band.border_r_1(),
            PanelEdge::Tree | PanelEdge::Info => band.border_l_1(),
        }
        .border_color(line)
        .into_any_element()
    }

    pub(super) fn resize_panels(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(resize) = &self.resize else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.finish_resize(cx);
            return;
        }
        let (edge, delta) = (resize.edge, f32::from(event.position.x) - resize.start_x);
        let start_width = resize.start_width;
        let total = f32::from(window.bounds().size.width);
        let browser = self.in_browser_view();
        let (other, width, current) = match edge {
            PanelEdge::List => {
                let other = if browser {
                    self.show_info.then_some(self.tree_width)
                } else {
                    self.show_info.then_some(self.info_width)
                };
                let current = if browser { &mut self.browser_list_width } else { &mut self.list_width };
                (other, start_width + delta, current)
            }
            PanelEdge::Tree => (
                self.show_list.then_some(self.browser_list_width),
                start_width + delta,
                &mut self.tree_width,
            ),
            PanelEdge::Info => (
                self.show_list.then_some(self.list_width),
                start_width - delta,
                &mut self.info_width,
            ),
        };
        let width = width.clamp(MIN_PANEL, max_width(total, other));
        if *current == width {
            return;
        }
        *current = width;
        cx.notify();
    }

    pub(super) fn finish_resize(&mut self, cx: &mut Context<Self>) {
        if self.resize.take().is_none() {
            return;
        }
        let (list, browser_list, tree, info) = (
            self.list_width.round() as u32,
            self.browser_list_width.round() as u32,
            self.tree_width.round() as u32,
            self.info_width.round() as u32,
        );
        Settings::update(cx, |settings| {
            settings.list_width = list;
            settings.browser_list_width = browser_list;
            settings.tree_width = tree;
            settings.info_width = info;
        });
    }
}
