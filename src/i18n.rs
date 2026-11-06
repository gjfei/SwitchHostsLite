//! The UI language and user-facing errors.
//!
//! Texts live in `locales/app.yml`, each key with zh-CN, zh-TW and en, and are
//! looked up with `rust_i18n::t!`. The UI language is rust-i18n's global
//! locale, which gpui-component reads for its built-in texts as well.
//!
//! Wording follows SwitchHosts' own translations where the concept matches.

use std::fmt;

use rust_i18n::t;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Lang {
    ZhCn,
    ZhTw,
    #[default]
    En,
}

impl Lang {
    /// The language's name in that language.
    pub fn native_name(self) -> &'static str {
        match self {
            Lang::ZhCn => "简体中文",
            Lang::ZhTw => "繁體中文",
            Lang::En => "English",
        }
    }

    /// The locale code in `locales/app.yml`, which gpui-component's locale
    /// files use too.
    pub fn locale(self) -> &'static str {
        match self {
            Lang::ZhCn => "zh-CN",
            Lang::ZhTw => "zh-TW",
            Lang::En => "en",
        }
    }

    /// Separator between a message and its technical detail.
    fn detail_separator(self) -> &'static str {
        match self {
            Lang::ZhCn | Lang::ZhTw => "：",
            Lang::En => ": ",
        }
    }

    /// The language for a BCP 47 or POSIX locale such as `zh-Hans-CN` or
    /// `zh_TW.UTF-8`. Chinese other than Traditional is Simplified; any other
    /// language is English.
    pub fn from_locale(locale: &str) -> Self {
        let locale = locale.split(['.', '@']).next().unwrap_or_default().to_ascii_lowercase();
        let mut parts = locale.split(['-', '_']);
        if parts.next() != Some("zh") {
            return Lang::En;
        }
        if parts.any(|p| matches!(p, "hant" | "tw" | "hk" | "mo")) {
            Lang::ZhTw
        } else {
            Lang::ZhCn
        }
    }

    /// The language of the operating system's preferred locale.
    pub fn system() -> Self {
        sys_locale::get_locale().map_or(Lang::En, |l| Lang::from_locale(&l))
    }

    /// The current UI language.
    pub fn current() -> Self {
        Lang::from_locale(&rust_i18n::locale())
    }

    /// Makes this the UI language of the app and of the component library.
    pub fn apply(self) {
        rust_i18n::set_locale(self.locale());
    }
}

/// An error whose message is shown to the user in their language.
#[derive(Debug)]
pub struct LocalizedError {
    key: &'static str,
    args: Vec<(&'static str, String)>,
    /// Technical detail such as an OS error, shown as is.
    detail: Option<String>,
}

impl LocalizedError {
    /// An error with the message `key` of `locales/app.yml`.
    pub fn new(key: &'static str) -> Self {
        Self {
            key,
            args: Vec::new(),
            detail: None,
        }
    }

    /// Fills the `%{name}` placeholder of the message.
    pub fn arg(mut self, name: &'static str, value: impl fmt::Display) -> Self {
        self.args.push((name, value.to_string()));
        self
    }

    pub fn detail(mut self, detail: impl fmt::Display) -> Self {
        let detail = detail.to_string();
        self.detail = (!detail.trim().is_empty()).then_some(detail);
        self
    }

    /// The message in the current UI language.
    pub fn message(&self) -> String {
        self.message_in(Lang::current())
    }

    pub fn message_in(&self, lang: Lang) -> String {
        let (names, values): (Vec<&str>, Vec<String>) = self.args.iter().map(|(n, v)| (*n, v.clone())).unzip();
        let message = rust_i18n::replace_patterns(&t!(self.key, locale = lang.locale()), &names, &values);
        match &self.detail {
            Some(detail) => format!("{message}{}{detail}", lang.detail_separator()),
            None => message,
        }
    }
}

impl fmt::Display for LocalizedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message_in(Lang::En))
    }
}

impl std::error::Error for LocalizedError {}

/// Turns any error into a [`LocalizedError`] with the error as its detail.
pub trait Localize<T> {
    fn localize(self, key: &'static str) -> anyhow::Result<T>;
}

impl<T, E: fmt::Display> Localize<T> for Result<T, E> {
    fn localize(self, key: &'static str) -> anyhow::Result<T> {
        self.map_err(|err| LocalizedError::new(key).detail(err).into())
    }
}

/// The message of `err` in the UI language.
pub fn error_text(err: &anyhow::Error) -> String {
    match err.downcast_ref::<LocalizedError>() {
        Some(err) => err.message(),
        None => format!("{err:#}"),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::Path;

    use super::*;

    const LOCALES: [&str; 3] = ["en", "zh-CN", "zh-TW"];

    /// The messages of `locales/` by locale, then by key.
    fn messages() -> BTreeMap<&'static str, BTreeMap<String, String>> {
        let backend = crate::_rust_i18n_backend();
        LOCALES
            .into_iter()
            .map(|locale| {
                let messages = backend.messages_for_locale(locale).unwrap_or_default();
                (locale, messages.into_iter().map(|(k, v)| (k.into_owned(), v.into_owned())).collect())
            })
            .collect()
    }

    /// The `%{name}` placeholders of `text`.
    fn placeholders(text: &str) -> BTreeSet<&str> {
        text.split("%{").skip(1).filter_map(|rest| rest.split_once('}').map(|(name, _)| name)).collect()
    }

    /// The non-test source of every `.rs` file under `dir`.
    fn sources(dir: &Path, out: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                sources(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).unwrap().replace("\r\n", "\n");
                let code = text.split("#[cfg(test)]\nmod tests").next().unwrap_or_default().to_string();
                out.push((path.display().to_string(), code));
            }
        }
    }

    /// Keys named in `code`: the literal key of every `t!` and every string
    /// literal starting with `err_`, which is how error keys are written.
    fn used_keys(code: &str) -> Vec<String> {
        let literal = |rest: &str| rest.split_once('"').map(|(key, _)| key.to_string());
        let from_t = code
            .match_indices("t!(")
            .filter(|(i, _)| !code[..*i].ends_with(|c: char| c.is_alphanumeric() || c == '_'))
            .filter_map(|(i, m)| literal(code[i + m.len()..].trim_start().strip_prefix('"')?));
        let errors = code.split("\"err_").skip(1).filter_map(|rest| literal(rest).map(|key| format!("err_{key}")));
        from_t.chain(errors).collect()
    }

    #[test]
    fn every_key_has_all_languages() {
        let backend = crate::_rust_i18n_backend();
        let mut locales: Vec<_> = backend.available_locales().into_iter().map(|l| l.into_owned()).collect();
        locales.sort();
        assert_eq!(locales, LOCALES);

        let messages = messages();
        let keys: BTreeSet<_> = messages.values().flat_map(|m| m.keys()).collect();
        assert!(keys.len() > 100, "{} keys", keys.len());
        for (locale, texts) in &messages {
            for key in &keys {
                let text = texts.get(*key).map(String::as_str).unwrap_or_default();
                assert!(!text.trim().is_empty(), "{key} has no {locale} text");
            }
        }
    }

    #[test]
    fn placeholders_match_across_languages() {
        let messages = messages();
        for (key, en) in &messages["en"] {
            // Positional placeholders are not filled in by rust-i18n.
            assert!(!en.replace("%{", "").contains('{'), "{key}: {en}");
            for locale in ["zh-CN", "zh-TW"] {
                let text = &messages[locale][key];
                assert_eq!(placeholders(text), placeholders(en), "{key} in {locale}");
                assert!(!text.replace("%{", "").contains('{'), "{key}: {text}");
            }
        }
    }

    #[test]
    fn source_keys_exist() {
        let messages = messages();
        let mut files = Vec::new();
        sources(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
        let mut count = 0;
        for (path, code) in &files {
            for key in used_keys(code) {
                assert!(messages["en"].contains_key(&key), "{path}: `{key}` is not in locales/app.yml");
                count += 1;
            }
        }
        assert!(count > 100, "only {count} keys found in the source");
    }

    #[test]
    fn every_key_is_used() {
        let mut files = Vec::new();
        sources(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
        let used: BTreeSet<String> = files.iter().flat_map(|(_, code)| used_keys(code)).collect();
        for key in messages()["en"].keys() {
            assert!(used.contains(key), "`{key}` in locales/app.yml is unused");
        }
    }

    #[test]
    fn fills_named_placeholders() {
        assert_eq!(t!("about_version", locale = "zh-TW", version = "1.2.3"), "版本 1.2.3");
        assert_eq!(
            t!("history_entry_size", locale = "en", lines = 3, bytes = 42),
            "3 lines · 42 bytes"
        );
    }

    #[test]
    fn localized_error_messages() {
        let err = LocalizedError::new("err_duplicate_id").arg("id", "x").detail("io");
        assert_eq!(err.message_in(Lang::ZhCn), "备份中存在重复的 id：x：io");
        assert_eq!(err.to_string(), "The backup contains a duplicate id: x: io");
        let err: anyhow::Error = LocalizedError::new("err_too_deep").detail("  ").into();
        assert_eq!(err.downcast_ref::<LocalizedError>().unwrap().message_in(Lang::ZhTw), "備份層級過深");
    }

    #[test]
    fn maps_locales() {
        for (locale, lang) in [
            ("zh-Hans-CN", Lang::ZhCn),
            ("zh_CN.UTF-8", Lang::ZhCn),
            ("zh-SG", Lang::ZhCn),
            ("zh", Lang::ZhCn),
            ("zh-Hant-HK", Lang::ZhTw),
            ("zh-TW", Lang::ZhTw),
            ("zh_HK", Lang::ZhTw),
            ("zh-MO", Lang::ZhTw),
            ("ZH-hant", Lang::ZhTw),
            ("en-US", Lang::En),
            ("ja-JP", Lang::En),
            ("C", Lang::En),
            ("", Lang::En),
        ] {
            assert_eq!(Lang::from_locale(locale), lang, "{locale}");
        }
        for lang in [Lang::ZhCn, Lang::ZhTw, Lang::En] {
            assert_eq!(Lang::from_locale(lang.locale()), lang);
        }
    }
}
