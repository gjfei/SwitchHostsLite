use std::cell::Cell;

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Colorize as _, Disableable as _, Icon, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState},
    h_flex,
    input::{Input, InputEvent, InputState},
    menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenu, PopupMenuItem},
    radio::{Radio, RadioGroup},
    switch::Switch,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rust_i18n::t;

use super::right_panel::{field, heading};
use super::tree_row::{self, RowState, no_drag};
use super::{Selection, SwitchHostsApp, display_title};
use crate::browser::{self, BrowserKind, BrowserProfile, ProfileMode, ProfileSnapshot, RevKind};
use crate::settings::Settings;
use crate::store::{self, Item, ItemKind, TrashedProfile};

const MODES: [ProfileMode; 3] = [ProfileMode::Independent, ProfileMode::Ephemeral, ProfileMode::Default];

/// The two text lines' line boxes: (text_sm + text_xs) rems at the 16 px rem,
/// with gpui's default phi (golden ratio) line height. The tile matches them
/// edge to edge.
const TILE_SIZE: f32 = (0.875 + 0.75) * 16. * std::f32::consts::GOLDEN_RATIO;

fn color_u32(color: [u8; 3]) -> u32 {
    let [r, g, b] = color;
    (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)
}

/// The single-line fields of the selected profile. Rebuilt whenever the
/// selection changes, so they never show another profile's text.
pub(super) struct ProfileInputs {
    name: Entity<InputState>,
    path: Entity<InputState>,
    color: Entity<ColorPickerState>,
    /// A revision was recorded since this field gained focus, so further
    /// typing in the same edit session stays one revision.
    name_rev: bool,
    path_rev: bool,
    /// Same idea for the color picker: slider drags commit on every tick, so
    /// one popover session records one revision. Reset when it opens.
    color_rev: Cell<bool>,
    picker_open: Cell<bool>,
    /// Whether the pointer is on the profile icon tile. Group hover alone only
    /// repaints when something else in the tile redraws, which made the hover
    /// scrim appear late; a real hover listener notifies on the entry itself.
    #[cfg(any(target_os = "macos", windows))]
    tile_hovered: Cell<bool>,
    _subscriptions: Vec<Subscription>,
}

fn menu_entry(
    view: &Entity<SwitchHostsApp>,
    label: impl Into<SharedString>,
    f: impl Fn(&mut SwitchHostsApp, &mut Window, &mut Context<SwitchHostsApp>) + 'static,
) -> PopupMenuItem {
    let view = view.clone();
    PopupMenuItem::new(label).on_click(move |_, window, cx| view.update(cx, |this, cx| f(this, window, cx)))
}

fn labeled(label: impl Into<SharedString>, control: impl IntoElement, cx: &App) -> Div {
    v_flex()
        .gap_1()
        .child(div().text_xs().text_color(cx.theme().muted_foreground).child(label.into()))
        .child(control)
}

pub(super) fn mode_label(mode: ProfileMode) -> SharedString {
    match mode {
        ProfileMode::Independent => t!("mode_independent"),
        ProfileMode::Ephemeral => t!("mode_ephemeral"),
        ProfileMode::Default => t!("mode_default"),
    }
    .into()
}

pub(super) fn mode_short(mode: ProfileMode) -> SharedString {
    match mode {
        ProfileMode::Independent => t!("mode_independent_short"),
        ProfileMode::Ephemeral => t!("mode_ephemeral_short"),
        ProfileMode::Default => t!("mode_default_short"),
    }
    .into()
}

fn mode_hint(mode: ProfileMode, browser: BrowserKind) -> SharedString {
    match mode {
        ProfileMode::Independent => t!("mode_independent_hint"),
        ProfileMode::Ephemeral => t!("mode_ephemeral_hint"),
        ProfileMode::Default => t!("mode_default_hint", name = browser.short()),
    }
    .into()
}

pub(super) fn rev_kind_label(kind: RevKind) -> SharedString {
    match kind {
        RevKind::Mode => t!("rev_mode"),
        RevKind::Browser => t!("rev_browser"),
        RevKind::Name => t!("rev_name"),
        RevKind::Path => t!("rev_path"),
        RevKind::Color => t!("rev_color"),
        RevKind::Image => t!("rev_image"),
        RevKind::Selection => t!("rev_selection"),
        RevKind::Restore => t!("rev_restore"),
    }
    .into()
}

pub(super) fn block_label(block: browser::Block, browser: BrowserKind) -> SharedString {
    match block {
        browser::Block::NoMappings => t!("block_no_mappings"),
        browser::Block::Missing => t!("block_browser_missing", name = browser.name()),
        browser::Block::TooLong => t!("block_too_long", limit = browser::cmd_limit()),
        browser::Block::Running => t!("block_running", name = browser.name()),
    }
    .into()
}

/// The settings a revision stands for, one per line.
pub(super) fn profile_rev_text(snapshot: &ProfileSnapshot, mappings: usize) -> String {
    let mut lines = vec![
        format!("{}: {}", t!("profile_name"), snapshot.name),
        format!("{}: {}", t!("profile_browser"), snapshot.browser.name()),
        format!("{}: {}", t!("profile_mode"), mode_label(snapshot.mode)),
        format!("{}: {}", t!("profile_mappings"), t!("mappings_count", count = mappings)),
    ];
    if !snapshot.path.is_empty() {
        lines.push(format!("{}: {}", t!("profile_path"), snapshot.path));
    }
    if let Some([r, g, b]) = snapshot.tile_color {
        lines.push(format!("{}: #{r:02x}{g:02x}{b:02x}", t!("profile_color")));
    }
    if snapshot.custom_image {
        lines.push(format!("{}: {}", t!("profile_image"), t!("image_custom")));
    }
    lines.join("\n")
}

impl SwitchHostsApp {
    // -- inputs --------------------------------------------------------------

    /// Rebuild the name and path fields for the selected profile.
    pub(super) fn refresh_profile_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(profile) = self.selected_profile() else {
            self.profile_inputs = None;
            return;
        };
        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("profile_name"))
                .default_value(profile.name.clone())
        });
        let path = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("profile_path_placeholder"))
                .default_value(profile.path.clone())
        });
        let name_sub = cx.subscribe_in(&name, window, |this, input, event, _window, cx| match event {
            InputEvent::Change => {
                let value = input.read(cx).value().to_string();
                this.edit_profile_field(RevKind::Name, value, cx);
            }
            InputEvent::Blur => {
                if let Some(inputs) = &mut this.profile_inputs {
                    inputs.name_rev = false;
                }
                // The badge letter follows the name; recomposite once per
                // edit session instead of on every keystroke.
                this.refresh_profile_tile(cx);
            }
            _ => {}
        });
        let path_sub = cx.subscribe_in(&path, window, |this, input, event, _window, cx| match event {
            InputEvent::Change => {
                let value = input.read(cx).value().to_string();
                this.edit_profile_field(RevKind::Path, value, cx);
            }
            InputEvent::Blur => {
                if let Some(inputs) = &mut this.profile_inputs {
                    inputs.path_rev = false;
                }
                // The mark's browser icon follows the path.
                this.refresh_profile_tile(cx);
            }
            _ => {}
        });
        let color = cx.new(|cx| {
            let state = ColorPickerState::new(window, cx);
            match profile.tile_color {
                Some(color) => state.default_value(browser::color_to_hsla(color)),
                None => state,
            }
        });
        let color_sub = cx.subscribe_in(&color, window, |this, _, ColorPickerEvent::Change(value), _window, cx| {
            this.set_profile_color(value.map(browser::hsla_to_color), cx);
        });
        self.profile_inputs = Some(ProfileInputs {
            name,
            path,
            color,
            name_rev: false,
            path_rev: false,
            color_rev: Cell::new(false),
            picker_open: Cell::new(false),
            #[cfg(any(target_os = "macos", windows))]
            tile_hovered: Cell::new(false),
            _subscriptions: vec![name_sub, path_sub, color_sub],
        });
    }

    /// Focus the name field with its text selected, so typing replaces the
    /// default name of a freshly added profile.
    pub(super) fn focus_profile_name(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(inputs) = &self.profile_inputs {
            let name = inputs.name.clone();
            name.update(cx, |state, cx| state.select_all(window, cx));
            let handle = name.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        }
    }

    /// Write one field of the selected profile, recording the value it had
    /// before the first edit of this focus session as a revision.
    fn edit_profile_field(&mut self, kind: RevKind, value: String, cx: &mut Context<Self>) {
        let Some(profile) = self.selected_profile() else {
            return;
        };
        let current = match kind {
            RevKind::Path => &profile.path,
            _ => &profile.name,
        };
        if *current == value {
            return;
        }
        let recorded = match kind {
            RevKind::Path => self.profile_inputs.as_ref().is_some_and(|i| i.path_rev),
            _ => self.profile_inputs.as_ref().is_some_and(|i| i.name_rev),
        };
        let Some(profile) = self.selected_profile_mut() else {
            return;
        };
        if !recorded {
            let snapshot = profile.snapshot();
            profile.push_rev_snapshot(kind, snapshot);
        }
        match kind {
            RevKind::Path => profile.path = value,
            _ => profile.name = value,
        }
        if let Some(inputs) = &mut self.profile_inputs {
            match kind {
                RevKind::Path => inputs.path_rev = true,
                _ => inputs.name_rev = true,
            }
        }
        self.persist_later(cx);
    }

    fn set_profile_mode(&mut self, mode: ProfileMode, cx: &mut Context<Self>) {
        if self.selected_profile().is_some_and(|p| p.mode == mode) {
            return;
        }
        self.mutate_profile(cx, |profile| {
            profile.push_rev(RevKind::Mode);
            profile.mode = mode;
        });
        // The badge turns neutral for a default-mode profile, so recomposite.
        self.refresh_profile_tile(cx);
    }

    fn set_profile_browser(&mut self, browser: BrowserKind, cx: &mut Context<Self>) {
        if self.selected_profile().is_some_and(|p| p.browser == browser) {
            return;
        }
        self.mutate_profile(cx, |profile| {
            profile.push_rev(RevKind::Browser);
            profile.browser = browser;
        });
        self.refresh_profile_tile(cx);
    }

    fn set_profile_color(&mut self, color: Option<[u8; 3]>, cx: &mut Context<Self>) {
        if self.selected_profile().is_some_and(|p| p.tile_color == color) {
            return;
        }
        let recorded = self.profile_inputs.as_ref().is_some_and(|i| i.color_rev.get());
        self.mutate_profile(cx, |profile| {
            if !recorded {
                profile.push_rev(RevKind::Color);
            }
            profile.tile_color = color;
        });
        if let Some(inputs) = &self.profile_inputs {
            inputs.color_rev.set(true);
        }
        self.refresh_profile_tile(cx);
    }

    /// Recomposite the mark and drop the info panel's cached preview of it.
    pub(super) fn refresh_profile_tile(&mut self, cx: &mut Context<Self>) {
        let Some(profile) = self.selected_profile() else {
            return;
        };
        crate::tile::refresh(profile);
        crate::tile::invalidate_preview(&profile.id, cx);
    }

    /// Pick the browser executable; a macOS `.app` bundle resolves to the
    /// executable inside it, which is what `path` stores.
    fn choose_profile_path(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t!("choose_path").into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                let exe = if path.extension().is_some_and(|e| e == "app") {
                    match browser::app_executable(&path) {
                        Some(exe) => exe,
                        None => {
                            let err: anyhow::Error = crate::i18n::LocalizedError::new("err_app_executable")
                                .detail(path.display().to_string())
                                .into();
                            Self::notify_error(window, crate::i18n::error_text(&err), cx);
                            return;
                        }
                    }
                } else {
                    path
                };
                let value = exe.to_string_lossy().to_string();
                if let Some(inputs) = &this.profile_inputs {
                    let input = inputs.path.clone();
                    input.update(cx, |state, cx| state.set_value(value.clone(), window, cx));
                }
                this.edit_profile_field(RevKind::Path, value, cx);
                this.refresh_profile_tile(cx);
            })
            .ok();
        })
        .detach();
    }

    fn choose_profile_image(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selected_profile().map(|p| p.id.clone()) else {
            return;
        };
        // The panel offers images only, so an exotic file cannot even be
        // picked; `save_image` still validates that it decodes.
        let picked = crate::platform::prompt_for_image(&t!("choose_image"));
        cx.spawn(async move |this, cx| {
            let Some(source) = picked.await else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                // Decoding is cheap enough to stay on the UI thread.
                if crate::tile::save_image(&id, &source).is_none() {
                    let err: anyhow::Error = crate::i18n::LocalizedError::new("err_image_load")
                        .detail(source.display().to_string())
                        .into();
                    Self::notify_error(window, crate::i18n::error_text(&err), cx);
                    return;
                }
                this.mutate_profile(cx, |profile| {
                    profile.push_rev(RevKind::Image);
                    profile.custom_image = true;
                });
                this.refresh_profile_tile(cx);
            })
            .ok();
        })
        .detach();
    }

    fn reset_profile_image(&mut self, cx: &mut Context<Self>) {
        if self.selected_profile().is_some_and(|p| !p.custom_image) {
            return;
        }
        // The picture file survives, so restoring the revision brings it back;
        // only purging the profile deletes it.
        self.mutate_profile(cx, |profile| {
            profile.push_rev(RevKind::Image);
            profile.custom_image = false;
        });
        self.refresh_profile_tile(cx);
    }

    fn image_controls(&self, profile: &BrowserProfile, cx: &Context<Self>) -> AnyElement {
        use gpui_kit::component::attachment::{Attachment, AttachmentMedia};

        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let tile = crate::tile::ensure(profile);
        let hovered = self
            .profile_inputs
            .as_ref()
            .is_some_and(|inputs| inputs.tile_hovered.get());

        // A small Attachment is a 48px image tile. Its media slot holds the
        // composed picture — the app icon with a badge (the profile color as its
        // background), the user's own image, or a globe when AppKit yields no
        // icon.
        let media = AttachmentMedia::new()
            .when_some(tile.clone(), |this, tile| this.src(tile))
            .when_none(&tile, |this| {
                this.child(Icon::new(IconName::Globe).text_color(muted))
            });
        let attachment = Attachment::new()
            .with_size(gpui_kit::component::Size::Small)
            .axis(Axis::Vertical)
            .media(media);

        // The hover target is the 48 px tile plus a 4 px ring: the corner cross
        // overhangs into that ring, and keeping the cross inside the target
        // means reaching for it does not drop the hover scrim. The negative
        // margins cancel the ring on the leading edges, so the picture itself
        // still lines up with the label above it.
        const TILE: f32 = 48.;
        const RING: f32 = 4.;

        div()
            .id("profile-icon-tile")
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .flex_none()
            // The label column stretches its children, so an unsized wrapper
            // grew to the panel's width and pinned the corner cross to the
            // row's right edge.
            .size(px(TILE + RING * 2.))
            .ml(px(-RING))
            .mt(px(-RING))
            // A named group hover only repaints when something else inside the
            // tile happens to redraw, so the scrim used to show up late — after
            // the pointer reached the button in the middle. A hover listener
            // notifies on the entry itself, and its bounds are the target's.
            .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                let Some(inputs) = &this.profile_inputs else {
                    return;
                };
                if inputs.tile_hovered.replace(*hovered) != *hovered {
                    cx.notify();
                }
            }))
            .child(attachment)
            // The scrim exists only while the pointer is on the target: a dim
            // wash over the picture with the edit button, so nothing invisible
            // can take a click.
            .when(hovered, |this| {
                this.child(
                    div()
                        .absolute()
                        .inset(px(RING))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(10.))
                        .bg(gpui_kit::black().opacity(0.55))
                        // A ghost `Button` has no padding and its content is
                        // `size_full`, so it has no intrinsic width. The fixed
                        // 20px slot gives it a box to fill.
                        .child(
                            div().size(px(20.)).flex().items_center().justify_center().child(
                                Button::new("profile-icon-replace")
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::PencilLine)
                                    .tooltip(t!("choose_image"))
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.choose_profile_image(cx)
                                    })),
                            ),
                        ),
                )
            })
            // A custom picture is cleared from the tile's corner, so the way
            // back to the app icon is visible without hovering. Painted after
            // the scrim, so it stays on top and takes the click.
            .when(profile.custom_image, |this| {
                this.child(
                    div()
                        .id("profile-icon-clear")
                        .absolute()
                        // Flush with the target's corner, which the 4 px ring
                        // puts 4 px outside the picture — the badge hangs off
                        // the tile instead of sitting on top of it.
                        .top(px(0.))
                        .right(px(0.))
                        .size(px(16.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .bg(gpui_kit::black().opacity(0.65))
                        .hover(|this| this.bg(gpui_kit::black().opacity(0.85)))
                        .cursor_pointer()
                        .child(
                            Icon::new(IconName::Close)
                                .with_size(px(11.))
                                .text_color(gpui_kit::white()),
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.reset_profile_image(cx))),
                )
            })
            .into_any_element()
    }

    /// A Select-style color field drawn by hand: a compact `ColorPicker` (its
    /// swatch + hex are the whole popover trigger, so the palette still works)
    /// inside a framed field, with an in-field clear cross as a plain sibling.
    ///
    /// We cannot use `ColorSelect` here: its `render_field` trigger is a
    /// `w_full()` hitbox with a hardcoded swatch/hex/caret and no trailing
    /// slot, so any clear button placed over it gets swallowed. Drawing the
    /// field ourselves keeps the full palette (HSV + hex) and lets the cross
    /// live beside the trigger as an ordinary, independently-clickable child.
    fn color_controls(&self, cx: &Context<Self>) -> AnyElement {
        let Some(inputs) = &self.profile_inputs else {
            return div().into_any_element();
        };
        let open = inputs.color.read(cx).is_open();
        if !inputs.picker_open.replace(open) && open {
            inputs.color_rev.set(false);
        }
        let marked = inputs.color.read(cx).value().is_some();
        let theme = cx.theme();
        let (bg, fg) = (theme.input_background(), theme.foreground);

        h_flex()
            .id("profile-color-field")
            .w_full()
            .h(px(30.))
            .items_center()
            .gap_2()
            .px_2()
            .bg(bg)
            .text_color(fg)
            .border_1()
            .border_color(if open { theme.ring } else { theme.input })
            .rounded(theme.radius)
            .child(
                // Compact picker: the swatch + hex label form the popover
                // trigger, so clicking either opens the full color palette.
                // It takes the flexible slot so the trailing glyph stays
                // pinned to the right edge, like a `Select` trigger.
                div().flex_1().min_w_0().overflow_hidden().child(
                    ColorPicker::new(&inputs.color)
                        .featured_colors(browser::TILE_PALETTE.map(browser::color_to_hsla).to_vec())
                        .when(marked, |this| {
                            this.label(
                                inputs
                                    .color
                                    .read(cx)
                                    .value()
                                    .map(|c| c.to_hex())
                                    .unwrap_or_default(),
                            )
                        })
                        .when(!marked, |this| this.label(t!("profile_color_placeholder")))
                        .with_size(gpui_kit::component::Size::Small),
                ),
            )
            // The caret and the cross are mutually exclusive, the way
            // `Select::cleanable` swaps its clear button for the caret once a
            // value is set: the trailing slot holds one glyph, never two.
            .when(!marked, |this| {
                this.child(
                    Icon::new(IconName::ChevronDown)
                        .xsmall()
                        .flex_shrink_0()
                        .text_color(theme.muted_foreground),
                )
            })
            .when(marked, |this| {
                this.child(
                    div()
                        .id("tile-color-clear")
                        .flex_shrink_0()
                        .size(px(20.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .rounded(theme.radius / 2.)
                        .hover(|this| this.bg(theme.muted))
                        .child(Icon::new(IconName::Close).xsmall().text_color(theme.muted_foreground))
                        .on_click(cx.listener(|this, _, window, cx| {
                            if let Some(inputs) = &this.profile_inputs {
                                inputs.color_rev.set(false);
                                let color = inputs.color.clone();
                                color.update(cx, |state, cx| state.clear_value(window, cx));
                            }
                            this.set_profile_color(None, cx);
                        })),
                )
            })
            .into_any_element()
    }

    /// Why `profile` cannot launch, given the mappings it would use.
    pub(super) fn block_for(&self, profile: &BrowserProfile, maps: &[(String, String)], cx: &App) -> Option<browser::Block> {
        let mark = browser::mark_color(profile, Settings::global(cx).mark_launched);
        let cmd = browser::build_cmd(profile, maps, &store::data_dir(), &std::env::temp_dir(), mark);
        let running = self.profile_running(profile);
        browser::block_reason(profile, maps.len(), cmd.len(), running)
    }

    // -- list ----------------------------------------------------------------

    pub(super) fn render_profile_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        if self.store.profiles.is_empty() {
            return v_flex()
                .size_full()
                .child(
                    v_flex()
                        .flex_1()
                        .items_center()
                        .justify_center()
                        .px_4()
                        .gap_1()
                        .text_sm()
                        .text_color(muted)
                        .child(t!("profile_empty_title"))
                        .child(div().text_xs().text_center().child(t!("profile_empty_hint"))),
                )
                .into_any_element();
        }

        let rows: Vec<AnyElement> = self
            .store
            .profiles
            .iter()
            .map(|profile| self.render_profile_row(profile, cx).into_any_element())
            .collect();

        v_flex()
            .size_full()
            .child(
                v_flex()
                    .id("profile-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .py_1p5()
                    .gap_0p5()
                    .children(rows),
            )
            .into_any_element()
    }

    fn render_profile_row(&self, profile: &BrowserProfile, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, active_bg, hover_bg) = (theme.muted_foreground, theme.list_active, theme.list_hover);
        let success = theme.success;
        let id = profile.id.clone();
        let active = self.selected == Selection::Browser(Some(id.clone()));
        let maps = browser::mappings(&self.store.switched_content(&profile.switches));
        let blocked = self.block_for(profile, &maps, cx);
        let launchable = blocked.is_none() && !self.launching;
        let running = self.profile_running(profile);
        let stoppable = self.profile_stoppable(profile);
        let launch_id = id.clone();
        let stop_id = id.clone();
        let actionable = if running { stoppable && !self.stopping } else { launchable };
        let hint = if running {
            SharedString::from(t!("stop"))
        } else {
            blocked.map_or_else(|| SharedString::from(t!("launch")), |b| block_label(b, profile.browser))
        };
        let switch = Switch::new(ElementId::Name(format!("profile-switch-{id}").into()))
            .small()
            .checked(running)
            .when(running, |this| this.color(success))
            .disabled(!actionable)
            .tooltip(hint)
            .on_change(cx.listener(move |this, on: &bool, window, cx| {
                cx.stop_propagation();
                if *on {
                    this.launch_profile_by_id(launch_id.clone(), window, cx);
                } else {
                    this.stop_profile_by_id(stop_id.clone(), window, cx);
                }
            }));

        let select_id = id.clone();
        let icon = crate::tile::ensure(profile);
        // The badge already carries the color, so a dot would repeat it —
        // except on a custom picture (no badge) and where nothing composites an
        // icon at all.
        let composited = cfg!(any(target_os = "macos", windows));
        let dot = profile
            .tile_color
            .filter(|_| !composited || profile.custom_image);
        h_flex()
            .id(ElementId::Name(format!("profile-{id}").into()))
            .flex_shrink_0()
            .h(px(48.))
            .mx(px(6.))
            .px_1p5()
            .gap_1p5()
            .rounded(px(4.))
            .cursor_pointer()
            .when(active, |this| this.bg(active_bg))
            .when(!active, |this| this.hover(|s| s.bg(hover_bg)))
            .child(
                div()
                    .flex_shrink_0()
                    .size(px(TILE_SIZE))
                    .rounded(px(8.))
                    .overflow_hidden()
                    .when_none(&icon, |this| {
                        this.flex()
                            .items_center()
                            .justify_center()
                            .child(Icon::new(IconName::Globe).large().text_color(muted))
                    })
                    .when_some(icon, |this, icon| this.child(img(icon).size(px(TILE_SIZE)))),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .when_some(dot, |this, color| {
                                this.child(
                                    div()
                                        .flex_shrink_0()
                                        .size(px(8.))
                                        .rounded_full()
                                        .bg(rgb(color_u32(color))),
                                )
                            })
                            .child(div().flex_1().min_w_0().truncate().text_sm().child(display_title(&profile.name))),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .text_xs()
                            .text_color(muted)
                            .child(div().min_w_0().truncate().child(format!(
                                "{} · {}",
                                profile.browser.short(),
                                mode_short(profile.mode)
                            ))),
                    ),
            )
            .child(no_drag(switch))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.select(Selection::Browser(Some(select_id.clone())), window, cx)
            }))
            .context_menu(self.profile_menu(id, cx))
    }

    fn profile_menu(
        &self,
        id: String,
        cx: &mut Context<Self>,
    ) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
        let view = cx.entity();
        move |menu, _, cx| {
            let this = view.read(cx);
            let blocked = this
                .store
                .profile(&id)
                .and_then(|profile| {
                    let maps = browser::mappings(&this.store.switched_content(&profile.switches));
                    this.block_for(profile, &maps, cx)
                })
                .is_some();
            let launch_item = menu_entry(&view, t!("launch"), {
                let id = id.clone();
                move |this, window, cx| this.launch_profile_by_id(id.clone(), window, cx)
            })
            .disabled(blocked || this.launching);
            let action_item = if this.store.profile(&id).is_some_and(|profile| this.profile_stoppable(profile)) {
                menu_entry(&view, t!("stop"), {
                    let id = id.clone();
                    move |this, window, cx| this.stop_profile_by_id(id.clone(), window, cx)
                })
            } else {
                launch_item
            };
            let copy_id = id.clone();
            let dup_id = id.clone();
            let delete_id = id.clone();
            menu.item(action_item)
                .item(menu_entry(&view, t!("copy_command"), move |this, window, cx| {
                    this.select(Selection::Browser(Some(copy_id.clone())), window, cx);
                    this.copy_command(window, cx);
                }))
                .separator()
                .item(menu_entry(&view, t!("duplicate_profile"), move |this, window, cx| {
                    this.duplicate_profile(dup_id.clone(), window, cx)
                }))
                .separator()
                .item(menu_entry(&view, t!("move_to_trash"), move |this, window, cx| {
                    this.confirm_delete_profile(delete_id.clone(), window, cx)
                }))
        }
    }

    // -- main panel ----------------------------------------------------------

    pub(super) fn render_profile_empty(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, foreground) = (theme.muted_foreground, theme.foreground);
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_3()
            .child(Icon::new(IconName::Globe).with_size(px(40.)).text_color(muted))
            .child(div().text_sm().font_medium().text_color(foreground).child(t!("profile_empty_title")))
            .child(
                div()
                    .max_w(px(320.))
                    .text_xs()
                    .text_center()
                    .text_color(muted)
                    .child(t!("profile_empty_hint")),
            )
            .child(
                Button::new("profile-empty-add")
                    .primary()
                    .small()
                    .icon(IconName::Plus)
                    .label(t!("profile_add"))
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, window, cx| this.add_profile(window, cx))),
            )
    }

    // -- hosts selection -----------------------------------------------------

    /// The hosts tree the profile's rules come from. Its switches follow the
    /// hosts list rules but are saved per profile: they never touch the
    /// system switches or write the hosts file.
    pub(super) fn render_profile_tree(&self, profile: &BrowserProfile, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, border, warning, success) = (
            theme.muted_foreground,
            theme.border,
            theme.warning,
            theme.success,
        );
        let body = if self.store.items.is_empty() {
            div()
                .flex_1()
                .min_h_0()
                .p_4()
                .text_sm()
                .text_color(muted)
                .child(t!("profile_tree_empty"))
                .into_any_element()
        } else {
            let mut rows = Vec::new();
            self.render_switch_rows(&self.store.items, 0, profile, &mut rows, cx);
            v_flex()
                .id("profile-tree")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                // Don't let wheel events scroll the enclosing info-panel.
                .on_scroll(|_, _, cx| cx.stop_propagation())
                .py_1p5()
                .gap_0p5()
                .children(rows)
                .into_any_element()
        };
        let maps = browser::mappings(&self.store.switched_content(&profile.switches));
        let status = match self.block_for(profile, &maps, cx) {
            // The count on the left already says there are no mappings, so the
            // reason is not repeated on the right.
            Some(browser::Block::NoMappings) => None,
            Some(block) => Some(
                h_flex()
                    .child(div().text_color(warning).child(block_label(block, profile.browser)))
                    .into_any_element(),
            ),
            None if self.profile_running(profile) => Some(
                h_flex()
                    .child(div().text_color(success).child(t!("profile_running")))
                    .into_any_element(),
            ),
            None => Some(
                h_flex()
                    .child(div().text_color(success).child(t!("profile_ready")))
                    .into_any_element(),
            ),
        };
        v_flex()
            .flex_1()
            .h_full()
            .min_w_0()
            .child(body)
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
                    .child(t!("mappings_count", count = maps.len()))
                    .children(status),
            )
            .into_any_element()
    }

    fn render_switch_rows(
        &self,
        items: &[Item],
        depth: usize,
        profile: &BrowserProfile,
        out: &mut Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) {
        for item in items {
            let open = !profile.collapsed.contains(&item.id);
            let checked = profile.switches.contains(&item.id);
            let count = (item.kind == ItemKind::Local).then(|| {
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(t!("mappings_count", count = store::count_rules(&item.content)))
                    .into_any_element()
            });
            let collapse_id = item.id.clone();
            let switch_id = item.id.clone();
            let row = tree_row::item_row(
                item,
                depth,
                RowState {
                    open,
                    active: false,
                    checked,
                    switch_disabled: false,
                },
                count,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.toggle_profile_collapsed(&collapse_id, cx);
                }),
                cx.listener(move |this, on: &bool, _, cx| {
                    // Keep the row from toggling the switch a second time.
                    cx.stop_propagation();
                    this.switch_profile_item(switch_id.clone(), *on, cx);
                }),
                cx,
            );
            out.push(row.into_any_element());
            if item.kind == ItemKind::Folder && open {
                self.render_switch_rows(&item.children, depth + 1, profile, out, cx);
            }
        }
    }

    /// Switch `id` in the selected profile with the hosts list rules,
    /// recording the previous switches as a revision.
    fn switch_profile_item(&mut self, id: String, on: bool, cx: &mut Context<Self>) {
        let Some(profile) = self.selected_profile() else {
            return;
        };
        let switches = self.store.set_switched(&profile.switches, &id, on);
        let Some(profile) = self.selected_profile_mut() else {
            return;
        };
        if profile.switches == switches {
            return;
        }
        profile.push_rev(RevKind::Selection);
        profile.switches = switches;
        self.persist(cx);
    }

    fn toggle_profile_collapsed(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(profile) = self.selected_profile_mut() else {
            return;
        };
        match profile.collapsed.iter().position(|c| c == id) {
            Some(ix) => {
                profile.collapsed.remove(ix);
            }
            None => profile.collapsed.push(id.to_string()),
        }
        self.persist(cx);
    }

    // -- right panel ---------------------------------------------------------

    pub(super) fn render_profile_right(&self, profile: &BrowserProfile, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let name = self.profile_inputs.as_ref().map(|inputs| Input::new(&inputs.name));
        let path = self.profile_inputs.as_ref().map(|inputs| Input::new(&inputs.path));
        let color_controls = self.color_controls(cx);
        let image_controls = Some(self.image_controls(profile, cx));

        let browser_view = cx.entity();
        let browser_menu = Button::new("profile-browser")
            .outline()
            .small()
            .w_full()
            .label(profile.browser.name())
            .cursor_pointer()
            .dropdown_menu_with_anchor(Anchor::BottomLeft, move |menu, _, cx| {
                let current = browser_view.read(cx).selected_profile().map(|p| p.browser);
                let mut menu = menu;
                for kind in BrowserKind::ALL {
                    let view = browser_view.clone();
                    menu = menu.item(
                        PopupMenuItem::new(kind.name())
                            .checked(current == Some(kind))
                            .on_click(move |_, _, cx| view.update(cx, |this, cx| this.set_profile_browser(kind, cx))),
                    );
                }
                menu
            });

        let mode_view = cx.entity();
        let radios = RadioGroup::vertical("profile-mode")
            .selected_index(MODES.iter().position(|m| *m == profile.mode))
            .children(
                MODES.map(|mode| {
                    Radio::new(ElementId::Name(format!("profile-mode-{mode:?}").into()))
                        .label(mode_label(mode))
                        .cursor_pointer()
                }),
            )
            .on_click(move |ix: &usize, _, cx| {
                let mode = MODES[*ix];
                mode_view.update(cx, |this, cx| this.set_profile_mode(mode, cx));
            });

        v_flex()
            .gap_4()
            .child(heading(IconName::Globe, display_title(&profile.name)))
            .child(labeled(t!("profile_name"), div().children(name), cx))
            .child(labeled(t!("profile_browser"), browser_menu, cx))
            .child(
                labeled(
                    t!("profile_path"),
                    h_flex()
                        .gap_1p5()
                        .items_center()
                        .child(div().flex_1().min_w_0().children(path))
                        .child(
                            Button::new("profile-path-choose")
                                .outline()
                                .small()
                                .icon(IconName::FolderOpen)
                                .tooltip(t!("choose_path"))
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, _, cx| this.choose_profile_path(cx))),
                        ),
                    cx,
                )
                .child(div().text_xs().text_color(muted).child(t!("profile_path_hint"))),
            )
            .child(labeled(t!("profile_mode"), radios, cx))
            .child(div().text_xs().text_color(muted).child(mode_hint(profile.mode, profile.browser)))
            // A default-mode profile is the everyday browser: it gets no
            // window tint, no Dock mark and no custom icon, so both rows are
            // hidden rather than offering settings that would do nothing.
            .when(profile.mode != ProfileMode::Default, |this| {
                this.child(labeled(t!("profile_color"), color_controls, cx))
                    .child(div().text_xs().text_color(muted).child(t!("profile_color_hint")))
                    .when_some(image_controls, |this, controls| {
                        this.child(labeled(t!("profile_image"), controls, cx))
                            .child(div().text_xs().text_color(muted).child(t!("profile_image_hint")))
                    })
            })
            .child(
                Button::new("profile-delete")
                    .ghost()
                    .small()
                    .text_color(theme.danger)
                    .icon(IconName::Trash)
                    .label(t!("move_to_trash"))
                    .cursor_pointer()
                    .on_click(cx.listener({
                        let id = profile.id.clone();
                        move |this, _, window, cx| this.confirm_delete_profile(id.clone(), window, cx)
                    })),
            )
            .into_any_element()
    }

    pub(super) fn render_trash_profile_info(&self, trash: &TrashedProfile, cx: &mut Context<Self>) -> AnyElement {
        let profile = &trash.profile;
        let (restore_id, purge_id) = (profile.id.clone(), profile.id.clone());
        v_flex()
            .gap_3()
            .child(heading(IconName::Globe, display_title(&profile.name)))
            .child(field(t!("profile_browser"), profile.browser.name(), cx))
            .child(field(t!("profile_mode"), mode_label(profile.mode), cx))
            .child(field(t!("deleted_at"), store::format_time(trash.deleted_at), cx))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("trash-profile-restore")
                            .small()
                            .icon(IconName::RotateCcw)
                            .label(t!("restore"))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.restore_trash(restore_id.clone(), window, cx)
                            })),
                    )
                    .child(
                        Button::new("trash-profile-purge")
                            .danger()
                            .small()
                            .label(t!("delete_permanently"))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.purge_trash(Some(purge_id.clone()), window, cx)
                            })),
                    ),
            )
            .into_any_element()
    }
}
