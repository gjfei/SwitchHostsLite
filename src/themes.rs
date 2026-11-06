//! Color themes: the bundled gpui-component themes plus the user's own
//! `*.json` files in `<data dir>/themes`.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    rc::Rc,
};

use gpui_kit::component::{ThemeConfig, ThemeMode, ThemeRegistry};
use gpui_kit::{App, SharedString};

use crate::store::data_dir;

macro_rules! bundled {
    ($($file:literal),* $(,)?) => {
        &[$(($file, include_str!(concat!("../assets/themes/", $file)))),*]
    };
}

const BUNDLED: &[(&str, &str)] = bundled![
    "adventure.json",
    "alduin.json",
    "asciinema.json",
    "aurora.json",
    "ayu.json",
    "catppuccin.json",
    "everforest.json",
    "fahrenheit.json",
    "flexoki.json",
    "gruvbox.json",
    "harper.json",
    "hybrid.json",
    "jellybeans.json",
    "kibble.json",
    "macos-classic.json",
    "mellifluous.json",
    "molokai.json",
    "solarized.json",
    "spaceduck.json",
    "tokyonight.json",
    "twilight.json",
];

/// Where users drop their own theme files.
pub fn user_dir() -> PathBuf {
    data_dir().join("themes")
}

/// Register the user's and the bundled themes. A file that does not parse is
/// skipped.
pub fn load(cx: &mut App) {
    let user = user_theme_files(&user_dir());
    let registry = ThemeRegistry::global_mut(cx);
    // The registry keeps the first theme of a name, so user themes go first
    // and can replace a bundled one.
    for (path, content) in &user {
        if let Err(err) = registry.load_themes_from_str(content) {
            eprintln!("skipped theme file {}: {err:#}", path.display());
        }
    }
    for (file, content) in BUNDLED {
        if let Err(err) = registry.load_themes_from_str(content) {
            eprintln!("skipped bundled theme {file}: {err:#}");
        }
    }
}

/// The `*.json` files in `dir` with their content, in file name order.
fn user_theme_files(dir: &Path) -> Vec<(PathBuf, String)> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.is_file() && path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("json")))
        .collect();
    paths.sort();
    paths
        .into_iter()
        .filter_map(|path| match fs::read_to_string(&path) {
            Ok(content) => Some((path, content)),
            Err(err) => {
                eprintln!("skipped theme file {}: {err}", path.display());
                None
            }
        })
        .collect()
}

/// The theme for `mode`: the saved one while it still exists with that mode,
/// otherwise `default`.
pub fn pick(
    themes: &HashMap<SharedString, Rc<ThemeConfig>>,
    saved: Option<&str>,
    mode: ThemeMode,
    default: &Rc<ThemeConfig>,
) -> Rc<ThemeConfig> {
    saved
        .and_then(|name| themes.get(name))
        .filter(|theme| theme.mode == mode)
        .unwrap_or(default)
        .clone()
}

/// Names of the themes for `mode`: `default` first, then alphabetically.
pub fn names(themes: &HashMap<SharedString, Rc<ThemeConfig>>, mode: ThemeMode, default: &str) -> Vec<SharedString> {
    let mut names: Vec<SharedString> = themes
        .values()
        .filter(|theme| theme.mode == mode)
        .map(|theme| theme.name.clone())
        .collect();
    names.sort_by_cached_key(|name| (name.as_ref() != default, name.to_lowercase()));
    names
}

#[cfg(test)]
mod tests {
    use gpui_kit::component::ThemeSet;

    use super::*;

    fn config(name: &str, mode: &str) -> Rc<ThemeConfig> {
        Rc::new(serde_json::from_value(serde_json::json!({ "name": name, "mode": mode })).unwrap())
    }

    fn registry(configs: &[Rc<ThemeConfig>]) -> HashMap<SharedString, Rc<ThemeConfig>> {
        configs.iter().map(|c| (c.name.clone(), c.clone())).collect()
    }

    #[test]
    fn bundled_themes_parse_with_unique_names() {
        let mut seen = std::collections::HashSet::new();
        let (mut light, mut dark) = (0, 0);
        for (file, content) in BUNDLED {
            let set: ThemeSet = serde_json::from_str(content).unwrap_or_else(|e| panic!("{file}: {e}"));
            for theme in set.themes {
                assert!(seen.insert(theme.name.clone()), "duplicate {}", theme.name);
                assert!(theme.highlight.is_some(), "{} has no highlight", theme.name);
                if theme.mode.is_dark() { dark += 1 } else { light += 1 }
            }
        }
        assert_eq!((light, dark), (11, 25));
    }

    #[test]
    fn picks_the_saved_theme_only_when_it_exists_with_that_mode() {
        let default = config("Default Light", "light");
        let themes = registry(&[default.clone(), config("Ayu Light", "light"), config("Ayu Dark", "dark")]);
        let pick = |saved| pick(&themes, saved, ThemeMode::Light, &default).name.clone();
        assert_eq!(pick(Some("Ayu Light")), "Ayu Light");
        assert_eq!(pick(None), "Default Light");
        assert_eq!(pick(Some("Removed Theme")), "Default Light");
        assert_eq!(pick(Some("Ayu Dark")), "Default Light");
    }

    #[test]
    fn lists_the_default_first_then_by_name() {
        let themes = registry(&[
            config("solarized Light", "light"),
            config("Default Light", "light"),
            config("Ayu Light", "light"),
            config("Ayu Dark", "dark"),
            config("Catppuccin Latte", "light"),
        ]);
        assert_eq!(
            names(&themes, ThemeMode::Light, "Default Light"),
            ["Default Light", "Ayu Light", "Catppuccin Latte", "solarized Light"]
        );
        assert_eq!(names(&themes, ThemeMode::Dark, "Default Dark"), ["Ayu Dark"]);
    }

    #[test]
    fn reads_only_json_files_and_tolerates_a_missing_dir() {
        let dir = std::env::temp_dir().join(format!("shl-themes-{}", std::process::id()));
        assert!(user_theme_files(&dir).is_empty());
        fs::create_dir_all(dir.join("nested.json")).unwrap();
        fs::write(dir.join("b.json"), "{}").unwrap();
        fs::write(dir.join("a.JSON"), "x").unwrap();
        fs::write(dir.join("notes.txt"), "").unwrap();
        let files: Vec<_> = user_theme_files(&dir)
            .into_iter()
            .map(|(p, _)| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(files, ["a.JSON", "b.json"]);
    }
}
