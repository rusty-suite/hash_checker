// =============================================================================
// language.rs — Textes de Hash Checker
//
// Le chargement des langues (packs intégrés, fichiers locaux, mise à jour depuis
// GitHub, repli sur l'anglais) est fourni par `rusty_core::lang` (charte Rusty
// Suite, section 05). Ce module ne déclare que les packs de l'application, le type
// `Msg` des messages produits par les tâches de fond et une petite interface de
// lecture pour la ligne de commande et l'interface.
// =============================================================================

use rusty_core::Lang;

/// Packs de langue intégrés au binaire. Les textes de l'application sont dans la
/// section `[ui]` ; les textes communs (`[kit]`) viennent de rusty_core.
pub const PACKS: rusty_core::lang::Packs = &[
    ("EN_en", include_str!("../lang/EN_en.default.toml")),
    ("FR_fr", include_str!("../lang/FR_fr.toml")),
    ("CH_fr", include_str!("../lang/CH_fr.toml")),
    ("DE_de", include_str!("../lang/DE_de.toml")),
    ("CH_de", include_str!("../lang/CH_de.toml")),
    ("IT_it", include_str!("../lang/IT_it.toml")),
    ("CH_it", include_str!("../lang/CH_it.toml")),
];

/// Message destiné à l'utilisateur : clé de la section `[ui]` et valeurs de ses
/// `{variables}`. Les tâches de fond produisent des `Msg` ; l'interface et la ligne
/// de commande les traduisent à l'affichage (ils suivent un changement de langue).
#[derive(Debug, Clone, PartialEq)]
pub struct Msg {
    pub key: &'static str,
    pub args: Vec<(&'static str, String)>,
}

impl Msg {
    pub fn new(key: &'static str) -> Self {
        Self { key, args: Vec::new() }
    }

    pub fn arg(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.args.push((name, value.into()));
        self
    }

    /// Texte non traduisible (ex. message d'erreur du système).
    pub fn raw(text: impl Into<String>) -> Self {
        Self::new("raw_error").arg("text", text)
    }

    /// Traduction dans la langue active.
    pub fn text(&self, lang: &Lang) -> String {
        let args: Vec<(&str, &str)> = self.args.iter().map(|(k, v)| (*k, v.as_str())).collect();
        lang.tf(&format!("ui.{}", self.key), &args)
    }
}

/// Lecture des textes `[ui]` (ligne de commande et interface).
pub struct Texts<'a>(pub &'a Lang);

impl Texts<'_> {
    pub fn text(&self, key: &str) -> String {
        self.0.t(&format!("ui.{key}"))
    }

    pub fn text_replace(&self, key: &str, replacements: &[(&str, String)]) -> String {
        let args: Vec<(&str, &str)> = replacements.iter().map(|(k, v)| (*k, v.as_str())).collect();
        self.0.tf(&format!("ui.{key}"), &args)
    }

    pub fn msg(&self, msg: &Msg) -> String {
        msg.text(self.0)
    }
}

/// Valeurs d'une clé `[ui]` dans toutes les langues intégrées (pour reconnaître un
/// élément créé sous une autre langue, ex. le script Nautilus du menu contextuel).
#[cfg(unix)]
pub fn bundled_values(key: &str) -> Vec<String> {
    let mut out: Vec<String> = PACKS
        .iter()
        .filter_map(|(_, text)| {
            let table: toml::Table = toml::from_str(text).ok()?;
            table.get("ui")?.get(key)?.as_str().map(str::to_owned)
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn english() -> Lang {
        Lang::load(PACKS, &std::env::temp_dir().join("hash_checker_lang_test"), "EN_en")
    }

    /// Toutes les langues ont exactement les clés de l'anglais.
    #[test]
    fn all_languages_have_the_same_keys() {
        assert_eq!(rusty_core::lang::check_packs(PACKS), Vec::<String>::new());
    }

    /// Charte, 05 : pas de tiret cadratin ; orthographe suisse en CH-de.
    #[test]
    fn texts_follow_the_charter() {
        for (stem, text) in PACKS {
            assert!(!text.contains('—'), "{stem} contient un tiret cadratin");
        }
        let ch_de = PACKS.iter().find(|(s, _)| *s == "CH_de").unwrap().1;
        assert!(!ch_de.contains('ß'), "CH_de doit utiliser « ss »");
    }

    /// Chaque clé utilisée dans le code existe dans le pack anglais (`[ui]` ou `[kit]`).
    #[test]
    fn keys_used_in_code_exist() {
        let lang = english();
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut missing = Vec::new();
        for entry in std::fs::read_dir(src).unwrap().flatten() {
            let text = std::fs::read_to_string(entry.path()).unwrap();
            for (call, section) in [(".text(\"", "ui"), (".text_replace(\"", "ui"), ("Msg::new(\"", "ui"), (".ui(\"", "ui"), (".uif(\"", "ui"), (".kit(\"", "kit")] {
                for part in text.split(call).skip(1) {
                    let key = part.split('"').next().unwrap();
                    if !lang.has(&format!("{section}.{key}")) {
                        missing.push(format!("{}: {section}.{key}", entry.path().display()));
                    }
                }
            }
        }
        assert!(missing.is_empty(), "clés manquantes : {missing:#?}");
    }

    #[test]
    fn msg_is_translated_with_arguments() {
        let lang = Lang::load(PACKS, &std::env::temp_dir().join("hash_checker_lang_test"), "FR_fr");
        let text = Msg::new("file_not_in_checksum").arg("file", "a.iso").text(&lang);
        assert!(text.contains("a.iso"), "{text}");
        assert_eq!(Msg::raw("boom").text(&lang), "boom");
    }
}
