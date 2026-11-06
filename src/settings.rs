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
    /// Look for a new release at startup, at most once a day.
    #[serde(skip_serializing_if = "is_true")]
    pub check_updates: bool,
    /// Unix time of the last update check.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_update_check: Option<i64>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            language: LanguagePref::default(),
            theme: ThemePref::default(),
            light_theme: None,
            dark_theme: None,
            check_updates: true,
            last_update_check: None,
        }
    }
}

fn is_true(value: &bool) -> bool {
    *value
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
        assert_eq!(old.last_update_check, None);
        let s = Settings { check_updates: false, last_update_check: Some(42), ..Default::default() };
        let text = serde_json::to_string(&s).unwrap();
        assert!(text.contains(r#""check_updates":false"#) && text.contains(r#""last_update_check":42"#));
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
