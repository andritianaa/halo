//! Localisation: one TOML file per language in `crates/halo-core/locales/`, embedded at
//! build time. English is the reference; a missing key falls back to English, then to
//! the key itself (so gaps are visible). A test checks every locale has every key.
//!
//! Placeholders use `{name}` and are filled with [`tf`].

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Lang {
    #[default]
    En,
    Fr,
    Es,
    De,
    It,
    Ru,
    #[serde(rename = "zh-cn")]
    ZhCn,
    Ja,
    Pt,
}

impl Lang {
    pub const ALL: [Lang; 9] = [Lang::En, Lang::Fr, Lang::Es, Lang::De, Lang::It, Lang::Ru, Lang::ZhCn, Lang::Ja, Lang::Pt];

    /// Name of the language in itself (for the language picker).
    pub fn native_name(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::Fr => "Français",
            Lang::Es => "Español",
            Lang::De => "Deutsch",
            Lang::It => "Italiano",
            Lang::Ru => "Русский",
            Lang::ZhCn => "简体中文",
            Lang::Ja => "日本語",
            Lang::Pt => "Português",
        }
    }

    /// chrono locale identifier for localized dates.
    pub fn locale_code(self) -> &'static str {
        match self {
            Lang::En => "en_US",
            Lang::Fr => "fr_FR",
            Lang::Es => "es_ES",
            Lang::De => "de_DE",
            Lang::It => "it_IT",
            Lang::Ru => "ru_RU",
            Lang::ZhCn => "zh_CN",
            Lang::Ja => "ja_JP",
            Lang::Pt => "pt_BR",
        }
    }

    /// From a BCP-47-ish locale string ("fr-FR", "zh-Hans-CN", "pt-BR"…); English otherwise.
    pub fn from_locale(locale: &str) -> Self {
        let l = locale.to_ascii_lowercase();
        let primary = l.split(['-', '_']).next().unwrap_or("");
        match primary {
            "fr" => Lang::Fr,
            "es" => Lang::Es,
            "de" => Lang::De,
            "it" => Lang::It,
            "ru" => Lang::Ru,
            "zh" => Lang::ZhCn,
            "ja" => Lang::Ja,
            "pt" => Lang::Pt,
            _ => Lang::En,
        }
    }

    fn source(self) -> &'static str {
        match self {
            Lang::En => include_str!("../locales/en.toml"),
            Lang::Fr => include_str!("../locales/fr.toml"),
            Lang::Es => include_str!("../locales/es.toml"),
            Lang::De => include_str!("../locales/de.toml"),
            Lang::It => include_str!("../locales/it.toml"),
            Lang::Ru => include_str!("../locales/ru.toml"),
            Lang::ZhCn => include_str!("../locales/zh-cn.toml"),
            Lang::Ja => include_str!("../locales/ja.toml"),
            Lang::Pt => include_str!("../locales/pt.toml"),
        }
    }
}

type Table = HashMap<String, String>;

fn tables() -> &'static HashMap<Lang, Table> {
    static T: OnceLock<HashMap<Lang, Table>> = OnceLock::new();
    T.get_or_init(|| {
        Lang::ALL
            .iter()
            .map(|&l| {
                let t: Table = toml::from_str(l.source()).unwrap_or_else(|e| {
                    log::error!("locale {l:?} is invalid: {e}");
                    Table::new()
                });
                (l, t)
            })
            .collect()
    })
}

/// Translate `key`; falls back to English, then to the key itself.
pub fn t(lang: Lang, key: &str) -> &'static str {
    let tb = tables();
    if let Some(v) = tb.get(&lang).and_then(|t| t.get(key)) {
        return v.as_str();
    }
    if let Some(v) = tb.get(&Lang::En).and_then(|t| t.get(key)) {
        return v.as_str();
    }
    // Leak once per unknown key so the signature can stay `&'static str`.
    static MISSING: OnceLock<std::sync::Mutex<HashMap<String, &'static str>>> = OnceLock::new();
    let mut m = MISSING.get_or_init(Default::default).lock().unwrap();
    m.entry(key.to_string()).or_insert_with(|| Box::leak(key.to_string().into_boxed_str()))
}

/// Translate and fill `{name}` placeholders.
pub fn tf(lang: Lang, key: &str, args: &[(&str, &str)]) -> String {
    let mut s = t(lang, key).to_string();
    for (k, v) in args {
        s = s.replace(&format!("{{{k}}}"), v);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_locale_parses_and_has_every_key() {
        let en = &tables()[&Lang::En];
        assert!(en.len() > 150);
        for l in Lang::ALL {
            let tb: Table = toml::from_str(l.source()).unwrap_or_else(|e| panic!("{l:?}: {e}"));
            let missing: Vec<_> = en.keys().filter(|k| !tb.contains_key(*k)).collect();
            assert!(missing.is_empty(), "{l:?} missing {} keys, e.g. {:?}", missing.len(), missing.iter().take(5).collect::<Vec<_>>());
            for (k, v) in &tb {
                assert!(!v.trim().is_empty() || en[k].trim().is_empty(), "{l:?}: empty {k}");
                // Placeholders must survive translation.
                for ph in ["{n}", "{q}", "{app}", "{i}", "{v}"] {
                    if en[k].contains(ph) {
                        assert!(v.contains(ph), "{l:?}: {k} lost {ph}");
                    }
                }
            }
        }
    }

    #[test]
    fn fallback_and_placeholders() {
        assert_eq!(t(Lang::Fr, "notes.new"), "Nouvelle note");
        assert_eq!(t(Lang::En, "no.such.key"), "no.such.key");
        assert_eq!(tf(Lang::En, "dash.pending", &[("n", "3")]), "3 pending");
    }

    #[test]
    fn every_anchor_has_a_label() {
        for a in crate::overlay::Anchor::ALL {
            assert_ne!(t(Lang::Fr, a.label_key()), a.label_key());
        }
    }

    #[test]
    fn locale_detection() {
        assert_eq!(Lang::from_locale("fr-CA"), Lang::Fr);
        assert_eq!(Lang::from_locale("zh-Hans-CN"), Lang::ZhCn);
        assert_eq!(Lang::from_locale("pt_BR"), Lang::Pt);
        assert_eq!(Lang::from_locale("nl-NL"), Lang::En);
    }
}
