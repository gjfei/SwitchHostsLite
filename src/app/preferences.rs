use std::{rc::Rc, time::Duration};

use gpui_kit::component::{
    ActiveTheme as _, IndexPath, StyledExt as _, Theme, ThemeMode, ThemeRegistry, WindowExt as _,
    button::{Toggle, ToggleGroup, ToggleVariants as _},
    checkbox::Checkbox,
    h_flex,
    highlighter::HighlightTheme,
    input::CursorBlink,
    radio::{Radio, RadioGroup},
    select::{SearchableVec, Select, SelectEvent, SelectState},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rust_i18n::t;

use super::{SwitchHostsApp, TOP_BAR_HEIGHT};
use crate::i18n::Lang;
use crate::settings::{LanguagePref, Settings, ThemePref};
use crate::{highlight, themes};

type ColorThemeSelect = Entity<SelectState<SearchableVec<SharedString>>>;

const CARET_IDLE_TIMEOUT: Duration = Duration::from_secs(10);

/// Switch to the preferred mode for `appearance`, the system's light or dark
/// mode, with the preferred color themes.
pub fn apply_theme(appearance: WindowAppearance, cx: &mut App) {
    let settings = Settings::global(cx);
    let mode = settings.theme.mode(appearance);
    let registry = ThemeRegistry::global(cx);
    let themes = registry.themes();
    let light = themes::pick(themes, settings.light_theme.as_deref(), ThemeMode::Light, registry.default_light_theme());
    let dark = themes::pick(themes, settings.dark_theme.as_deref(), ThemeMode::Dark, registry.default_dark_theme());

    let current = cx.theme();
    if current.mode != mode || !Rc::ptr_eq(&current.light_theme, &light) || !Rc::ptr_eq(&current.dark_theme, &dark) {
        let theme = Theme::global_mut(cx);
        reset_theme_file_fields(theme, mode);
        theme.light_theme = light;
        theme.dark_theme = dark;
        Theme::change(mode, None, cx);
    }
    // Sheets otherwise start at the component library's own, shorter title
    // bar height and cover the bottom of ours.
    Theme::update(cx, |theme| {
        theme.sheet.margin_top = TOP_BAR_HEIGHT;
        // A blinking caret repaints the window twice a second, also while the
        // window is in the background, so it stops once the user is idle.
        theme.cursor_blink = CursorBlink::default().with_idle_timeout(CARET_IDLE_TIMEOUT);
    });
    highlight::set_theme_colors(mode.is_dark(), cx.theme().danger);
}

/// A theme file only overrides what it names, so undo whatever the previous
/// one set before applying the next.
fn reset_theme_file_fields(theme: &mut Theme, mode: ThemeMode) {
    let defaults = Theme::default();
    theme.font_family = defaults.font_family;
    theme.font_size = defaults.font_size;
    theme.mono_font_family = defaults.mono_font_family;
    theme.mono_font_size = defaults.mono_font_size;
    theme.radius = defaults.radius;
    theme.radius_lg = defaults.radius_lg;
    theme.shadow = defaults.shadow;
    theme.highlight_theme = if mode.is_dark() {
        HighlightTheme::default_dark()
    } else {
        HighlightTheme::default_light()
    };
}

pub fn apply_language(cx: &mut App) {
    Settings::global(cx).language.lang().unwrap_or_else(Lang::system).apply();
    cx.refresh_windows();
}

pub fn set_theme(theme: ThemePref, window: &mut Window, cx: &mut App) {
    Settings::update(cx, |s| s.theme = theme);
    apply_theme(window.appearance(), cx);
}

fn default_color_theme(mode: ThemeMode, cx: &App) -> &Rc<gpui_kit::component::ThemeConfig> {
    let registry = ThemeRegistry::global(cx);
    if mode.is_dark() {
        registry.default_dark_theme()
    } else {
        registry.default_light_theme()
    }
}

fn set_color_theme(mode: ThemeMode, name: &SharedString, window: &mut Window, cx: &mut App) {
    let name = (*name != default_color_theme(mode, cx).name).then(|| name.to_string());
    Settings::update(cx, |s| match mode {
        ThemeMode::Light => s.light_theme = name,
        ThemeMode::Dark => s.dark_theme = name,
    });
    apply_theme(window.appearance(), cx);
}

fn set_language(language: LanguagePref, cx: &mut App) {
    Settings::update(cx, |s| s.language = language);
    apply_language(cx);
}

pub fn theme_label(theme: ThemePref) -> SharedString {
    match theme {
        ThemePref::System => t!("follow_system"),
        ThemePref::Light => t!("theme_light"),
        ThemePref::Dark => t!("theme_dark"),
    }
    .into()
}

fn language_label(language: LanguagePref) -> SharedString {
    language.lang().map_or_else(|| t!("follow_system").into(), |lang| lang.native_name().into())
}

fn section(label: impl Into<SharedString>, cx: &App) -> Div {
    v_flex()
        .gap_2()
        .child(div().text_sm().font_medium().text_color(cx.theme().foreground).child(label.into()))
}

fn color_theme_field(label: impl Into<SharedString>, select: &ColorThemeSelect, cx: &App) -> Div {
    v_flex()
        .flex_1()
        .min_w_0()
        .gap_1()
        .child(div().text_xs().text_color(cx.theme().muted_foreground).child(label.into()))
        .child(Select::new(select).menu_max_h(px(220.)).cursor_pointer())
}

impl SwitchHostsApp {
    /// A searchable select of the color themes for `mode` that applies the
    /// picked one right away.
    fn color_theme_select(mode: ThemeMode, window: &mut Window, cx: &mut Context<Self>) -> ColorThemeSelect {
        let current = match mode {
            ThemeMode::Light => cx.theme().light_theme.name.clone(),
            ThemeMode::Dark => cx.theme().dark_theme.name.clone(),
        };
        let default = default_color_theme(mode, cx).name.clone();
        let names = themes::names(ThemeRegistry::global(cx).themes(), mode, &default);
        let selected = names.iter().position(|name| *name == current).map(IndexPath::new);
        let select = cx.new(|cx| SelectState::new(SearchableVec::new(names), selected, window, cx).searchable(true));
        cx.subscribe_in(&select, window, move |_, _, event, window, cx| {
            if let SelectEvent::Confirm(Some(name)) = event {
                set_color_theme(mode, name, window, cx);
            }
        })
        .detach();
        select
    }

    pub(super) fn open_preferences(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.entity();
        let light_select = Self::color_theme_select(ThemeMode::Light, window, cx);
        let dark_select = Self::color_theme_select(ThemeMode::Dark, window, cx);
        window.open_dialog(cx, move |dialog, _, cx| {
            let settings = Settings::global(cx);
            let multi = view.read(cx).store.multi_select;
            let (primary, primary_fg) = (cx.theme().primary, cx.theme().primary_foreground);

            let language = RadioGroup::vertical("pref-language")
                .selected_index(LanguagePref::ALL.iter().position(|l| *l == settings.language))
                .children(LanguagePref::ALL.map(|l| {
                    Radio::new(ElementId::Name(format!("pref-language-{l:?}").into()))
                        .label(language_label(l))
                        .cursor_pointer()
                }))
                .on_click(|ix: &usize, _, cx| set_language(LanguagePref::ALL[*ix], cx));

            let current = settings.theme;
            let theme = ToggleGroup::new("pref-theme")
                .outline()
                .segmented()
                .w_full()
                .children(ThemePref::ALL.map(|p| {
                    let checked = p == current;
                    Toggle::new(ElementId::Name(format!("pref-theme-{p:?}").into()))
                        .flex_1()
                        .label(theme_label(p))
                        .checked(checked)
                        .cursor_pointer()
                        .when(checked, |this| this.bg(primary).text_color(primary_fg).border_color(primary))
                }))
                .on_click(move |checks: &Vec<bool>, window, cx| {
                    let picked = checks
                        .iter()
                        .zip(ThemePref::ALL)
                        .find(|(checked, p)| **checked && *p != current)
                        .map(|(_, p)| p);
                    if let Some(picked) = picked {
                        set_theme(picked, window, cx);
                    }
                });

            let color_themes = h_flex()
                .gap_3()
                .child(color_theme_field(t!("light_color_theme"), &light_select, cx))
                .child(color_theme_field(t!("dark_color_theme"), &dark_select, cx));

            let multi_view = view.clone();
            let multi_select = Checkbox::new("pref-multi")
                .label(SharedString::from(t!("multi_select")))
                .checked(multi)
                .cursor_pointer()
                .on_click(move |checked: &bool, window, cx| {
                    let checked = *checked;
                    multi_view.update(cx, |this, cx| this.set_multi_select(checked, window, cx));
                });

            let check_updates = Checkbox::new("pref-check-updates")
                .label(SharedString::from(t!("auto_check_updates")))
                .checked(settings.check_updates)
                .cursor_pointer()
                .on_click(|checked: &bool, _, cx| {
                    let checked = *checked;
                    Settings::update(cx, |s| s.check_updates = checked);
                });

            dialog.title(t!("preferences")).w(px(420.)).child(
                v_flex()
                    .gap_5()
                    .pb_2()
                    .child(section(t!("language"), cx).child(language))
                    .child(section(t!("theme"), cx).child(theme).child(color_themes))
                    .child(v_flex().gap_3().child(multi_select).child(check_updates)),
            )
        });
    }
}
