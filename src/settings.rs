//! App preferences, stored as `settings.json` next to the data.

use anyhow::Result;
use gpui_kit::component::ThemeMode;
use gpui_kit::{App, Global, WindowAppearance};
use serde::{Deserialize, Serialize};

use crate::i18n::Lang;
use crate::store::{data_dir, load_json, write_json};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LanguagePref {
    #[default]
    System,
    ZhCn,
    ZhTw,
    En,
}

impl LanguagePref {
    pub const ALL: [LanguagePref; 4] = [Self::System, Self::ZhCn, Self::ZhTw, Self::En];

    /// The fixed language, `None` when following the system.
    pub fn lang(self) -> Option<Lang> {
        match self {
            Self::System => None,
            Self::ZhCn => Some(Lang::ZhCn),
            Self::ZhTw => Some(Lang::ZhTw),
            Self::En => Some(Lang::En),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemePref {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemePref {
    pub const ALL: [ThemePref; 3] = [Self::System, Self::Light, Self::Dark];

    pub fn mode(self, appearance: WindowAppearance) -> ThemeMode {
        match self {
            Self::System => appearance.into(),
            Self::Light => ThemeMode::Light,
            Self::Dark => ThemeMode::Dark,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub language: LanguagePref,
    pub theme: ThemePref,
    /// Name of the light color theme, `None` for the default one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub light_theme: Option<String>,
    /// Name of the dark color theme, `None` for the default one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dark_theme: Option<String>,
    /// Width of the hosts list panel, in logical pixels.
    #[serde(skip_serializing_if = "is_default_list_width")]
    pub list_width: u32,
    /// Width of the browser view's profile list panel, in logical pixels.
    #[serde(skip_serializing_if = "is_default_browser_list_width")]
    pub browser_list_width: u32,
    /// Width of the browser view's hosts tree panel, in logical pixels.
    #[serde(skip_serializing_if = "is_default_tree_width")]
    pub tree_width: u32,
    /// Width of the info panel, in logical pixels.
    #[serde(skip_serializing_if = "is_default_info_width")]
    pub info_width: u32,
    /// Look for a new release at startup, at most once a day.
    #[serde(skip_serializing_if = "is_true")]
    pub check_updates: bool,
    /// Tint the window and mark the Dock/taskbar of launched browsers.
    #[serde(skip_serializing_if = "is_true")]
    pub mark_launched: bool,
    /// Unix time of the last update check.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_update_check: Option<i64>,
}

/// Panel widths before any drag, and the values skipped when writing. The
/// profile rows carry a tile, a rule count and a subtitle, so their list
/// starts wider than the hosts list; the hosts tree only needs switch rows.
const DEFAULT_LIST_WIDTH: u32 = 240;
const DEFAULT_BROWSER_LIST_WIDTH: u32 = 260;
const DEFAULT_TREE_WIDTH: u32 = 300;
const DEFAULT_INFO_WIDTH: u32 = 240;

impl Default for Settings {
    fn default() -> Self {
        Self {
            language: LanguagePref::default(),
            theme: ThemePref::default(),
            light_theme: None,
            dark_theme: None,
            list_width: DEFAULT_LIST_WIDTH,
            browser_list_width: DEFAULT_BROWSER_LIST_WIDTH,
            tree_width: DEFAULT_TREE_WIDTH,
            info_width: DEFAULT_INFO_WIDTH,
            check_updates: true,
            mark_launched: true,
            last_update_check: None,
        }
    }
}

fn is_true(value: &bool) -> bool {
    *value
}

fn is_default_list_width(value: &u32) -> bool {
    *value == DEFAULT_LIST_WIDTH
}

fn is_default_browser_list_width(value: &u32) -> bool {
    *value == DEFAULT_BROWSER_LIST_WIDTH
}

fn is_default_tree_width(value: &u32) -> bool {
    *value == DEFAULT_TREE_WIDTH
}

fn is_default_info_width(value: &u32) -> bool {
    *value == DEFAULT_INFO_WIDTH
}

impl Global for Settings {}

impl Settings {
    pub fn load() -> Self {
        load_json(&data_dir().join("settings.json"), Self::default)
    }

    pub fn save(&self) -> Result<()> {
        write_json(&data_dir().join("settings.json"), self)
    }

    pub fn global(cx: &App) -> Self {
        cx.try_global::<Self>().cloned().unwrap_or_default()
    }

    /// Change the settings and write them to disk.
    pub fn update(cx: &mut App, f: impl FnOnce(&mut Settings)) {
        let mut settings = Self::global(cx);
        f(&mut settings);
        if let Err(err) = settings.save() {
            eprintln!("save settings failed: {err:#}");
        }
        cx.set_global(settings);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_partial_and_unknown_settings() {
        let s: Settings = serde_json::from_str(r#"{"theme":"dark","extra":1}"#).unwrap();
        assert_eq!(s, Settings { theme: ThemePref::Dark, ..Default::default() });
        let s: Settings = serde_json::from_str(r#"{"language":"zh_tw"}"#).unwrap();
        assert_eq!(s.language.lang(), Some(Lang::ZhTw));
        assert!(serde_json::from_str::<Settings>(r#"{"theme":"blue"}"#).is_err());
    }

    #[test]
    fn color_themes_are_optional_and_round_trip() {
        let old: Settings = serde_json::from_str(r#"{"language":"en","theme":"light"}"#).unwrap();
        assert_eq!((old.light_theme, old.dark_theme), (None, None));
        assert_eq!(serde_json::to_string(&Settings::default()).unwrap(), r#"{"language":"system","theme":"system"}"#);

        let s = Settings { dark_theme: Some("Ayu Dark".into()), ..Default::default() };
        let text = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<Settings>(&text).unwrap(), s);
        let s: Settings = serde_json::from_str(r#"{"light_theme":null,"dark_theme":"Ayu Dark"}"#).unwrap();
        assert_eq!((s.light_theme, s.dark_theme.as_deref()), (None, Some("Ayu Dark")));
    }

    #[test]
    fn update_settings_default_on_and_round_trip() {
        let old: Settings = serde_json::from_str(r#"{"language":"en"}"#).unwrap();
        assert!(old.check_updates);
        assert!(old.mark_launched);
        assert_eq!(old.last_update_check, None);
        let s = Settings { check_updates: false, mark_launched: false, last_update_check: Some(42), ..Default::default() };
        let text = serde_json::to_string(&s).unwrap();
        assert!(text.contains(r#""check_updates":false"#) && text.contains(r#""last_update_check":42"#));
        assert!(text.contains(r#""mark_launched":false"#));
        assert_eq!(serde_json::from_str::<Settings>(&text).unwrap(), s);
        assert!(!serde_json::to_string(&Settings::default()).unwrap().contains("mark_launched"));
    }

    #[test]
    fn panel_widths_default_and_round_trip() {
        let s: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!((s.list_width, s.browser_list_width, s.tree_width, s.info_width), (240, 260, 300, 240));
        // The default widths stay out of the file.
        assert!(!serde_json::to_string(&Settings::default()).unwrap().contains("width"));

        let s = Settings { list_width: 300, browser_list_width: 280, tree_width: 320, info_width: 260, ..Default::default() };
        let text = serde_json::to_string(&s).unwrap();
        assert!(text.contains(r#""list_width":300"#) && text.contains(r#""info_width":260"#));
        assert!(text.contains(r#""browser_list_width":280"#) && text.contains(r#""tree_width":320"#));
        assert_eq!(serde_json::from_str::<Settings>(&text).unwrap(), s);
    }

    #[test]
    fn theme_follows_appearance_only_for_system() {
        assert_eq!(ThemePref::System.mode(WindowAppearance::VibrantDark), ThemeMode::Dark);
        assert_eq!(ThemePref::System.mode(WindowAppearance::Light), ThemeMode::Light);
        assert_eq!(ThemePref::Light.mode(WindowAppearance::Dark), ThemeMode::Light);
        assert_eq!(ThemePref::Dark.mode(WindowAppearance::Light), ThemeMode::Dark);
    }
}
