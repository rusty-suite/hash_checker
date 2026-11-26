use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const APP_NAME: &str = "hash_checker";
const GITHUB_RAW_BASE: &str =
    "https://raw.githubusercontent.com/rusty-suite/hash_checker/main/lang";
const GITHUB_LANG_INDEX: &str =
    "https://api.github.com/repos/rusty-suite/hash_checker/contents/lang?ref=main";

const BUILTIN_LANGS: &[(&str, &str)] = &[
    ("EN_en.default.toml", EN_EN_DEFAULT),
    ("FR_fr.toml", FR_FR),
    ("CH_fr.toml", CH_FR),
    ("DE_de.toml", DE_DE),
    ("CH_de.toml", CH_DE),
    ("IT_it.toml", IT_IT),
    ("CH_it.toml", CH_IT),
];

const EN_EN_DEFAULT: &str = include_str!("../lang/EN_en.default.toml");
const FR_FR: &str = include_str!("../lang/FR_fr.toml");
const CH_FR: &str = include_str!("../lang/CH_fr.toml");
const DE_DE: &str = include_str!("../lang/DE_de.toml");
const CH_DE: &str = include_str!("../lang/CH_de.toml");
const IT_IT: &str = include_str!("../lang/IT_it.toml");
const CH_IT: &str = include_str!("../lang/CH_it.toml");

/// A user-facing message: a translation key of the `[ui]` section plus the
/// values of its `{placeholders}`. Background workers produce messages and the
/// GUI/CLI translate them when displaying, so they follow language changes.
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

    /// Untranslatable text (e.g. an OS error message).
    pub fn raw(text: impl Into<String>) -> Self {
        Self::new("raw_error").arg("text", text)
    }
}

#[derive(Debug, Clone)]
pub struct LanguagePack {
    pub stem: String,
    pub file_name: String,
    pub display_name: String,
    pub is_default: bool,
    pub is_local: bool,
    pub is_remote: bool,
}

#[derive(Debug, Clone)]
pub struct LanguageManager {
    pub work_dir: PathBuf,
    pub lang_dir: PathBuf,
    pub active_stem: String,
    pub active_name: String,
    pub default_badge: String,
    pub network_error: bool,
    ui: HashMap<String, String>,
    remote_files: Vec<String>,
}

impl LanguageManager {
    pub fn initialize() -> Self {
        let work_dir = resolve_work_dir();
        let lang_dir = work_dir.join("lang");
        let mut network_error = false;

        let _ = fs::create_dir_all(&lang_dir);
        seed_builtin_langs(&lang_dir);

        if !has_local_langs(&lang_dir) && !download_default_lang(&lang_dir) {
            network_error = true;
        }

        let selected = choose_language(&work_dir, &lang_dir);
        let (active_stem, active_name, default_badge, ui) = selected.unwrap_or_else(|| {
            network_error = true;
            (
                "EN_en".to_string(),
                "English (EN)".to_string(),
                "default".to_string(),
                default_ui_texts(),
            )
        });

        Self {
            work_dir,
            lang_dir,
            active_stem,
            active_name,
            default_badge,
            network_error,
            ui,
            remote_files: Vec::new(),
        }
    }

    pub fn available_languages(&self) -> Vec<LanguagePack> {
        let mut by_file: HashMap<String, LanguagePack> = HashMap::new();

        if let Ok(entries) = fs::read_dir(&self.lang_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                    continue;
                }
                if let Some(pack) = pack_from_path(&path, true, false) {
                    by_file.insert(pack.file_name.clone(), pack);
                }
            }
        }

        for file_name in &self.remote_files {
            by_file
                .entry(file_name.clone())
                .and_modify(|pack| pack.is_remote = true)
                .or_insert_with(|| pack_from_file_name(file_name, false, true));
        }

        let mut packs: Vec<_> = by_file.into_values().collect();
        packs.sort_by(|a, b| {
            a.is_default
                .cmp(&b.is_default)
                .then_with(|| a.stem.to_lowercase().cmp(&b.stem.to_lowercase()))
        });
        packs
    }

    pub fn select_language(&mut self, pack: &LanguagePack) -> Result<(), String> {
        let path = self.lang_dir.join(&pack.file_name);
        if !path.exists() && !download_language(&self.lang_dir, &pack.file_name) {
            return Err(self.text("network_error"));
        }
        refresh_builtin_if_stale(&path, &pack.file_name);

        let Some((stem, name, badge, ui)) = read_language_file(&path) else {
            return Err(self.text("language_file_invalid"));
        };

        fs::write(self.work_dir.join("lang_chosen.txt"), stem.as_bytes())
            .map_err(|e| e.to_string())?;
        self.active_stem = stem;
        self.active_name = name;
        self.default_badge = badge;
        self.ui = ui;
        Ok(())
    }

    pub fn text(&self, key: &str) -> String {
        self.ui
            .get(key)
            .cloned()
            .or_else(|| default_ui_texts().remove(key))
            .unwrap_or_else(|| key.to_string())
    }

    /// Translates a [`Msg`].
    pub fn msg(&self, msg: &Msg) -> String {
        let args: Vec<(&str, String)> = msg.args.iter().map(|(k, v)| (*k, v.clone())).collect();
        self.text_replace(msg.key, &args)
    }

    pub fn text_replace(&self, key: &str, replacements: &[(&str, String)]) -> String {
        let mut text = self.text(key);
        for (placeholder, value) in replacements {
            text = text.replace(&format!("{{{}}}", placeholder), value);
        }
        text
    }

    pub fn open_lang_folder(&self) {
        #[cfg(windows)]
        {
            let _ = Command::new("explorer").arg(&self.lang_dir).spawn();
        }
        #[cfg(target_os = "macos")]
        {
            let _ = Command::new("open").arg(&self.lang_dir).spawn();
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            let _ = Command::new("xdg-open").arg(&self.lang_dir).spawn();
        }
    }

    pub fn set_remote_files(&mut self, mut remote_files: Vec<String>) {
        remote_files.sort();
        remote_files.dedup();
        self.remote_files = remote_files;
    }

    pub fn ui_texts(&self) -> HashMap<String, String> {
        self.ui.clone()
    }

    pub fn fetch_remote_languages(ui: HashMap<String, String>) -> Result<Vec<String>, String> {
        let output = run_curl_text(GITHUB_LANG_INDEX, &ui)?;
        let value: serde_json::Value =
            serde_json::from_str(&output).map_err(|e| {
                text_from_map(&ui, "github_index_parse_error")
                    .replace("{error}", &e.to_string())
            })?;
        let Some(items) = value.as_array() else {
            return Err(text_from_map(&ui, "github_index_invalid"));
        };

        let mut files = Vec::new();
        for item in items {
            let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let kind = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
            if kind == "file" && name.ends_with(".toml") {
                files.push(name.to_string());
            }
        }

        if files.is_empty() {
            return Err(text_from_map(&ui, "no_remote_languages"));
        }

        Ok(files)
    }
}

fn resolve_work_dir() -> PathBuf {
    let appdata_suite = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .map(|p| p.join("rusty-suite"));

    if let Some(suite_dir) = appdata_suite {
        if suite_dir.exists() {
            return suite_dir.join(APP_NAME);
        }
    }

    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        .join(APP_NAME)
}

fn seed_builtin_langs(lang_dir: &Path) {
    for (file_name, content) in BUILTIN_LANGS {
        let path = lang_dir.join(file_name);
        refresh_builtin_file(&path, content);
    }
}

fn refresh_builtin_if_stale(path: &Path, file_name: &str) {
    if let Some((_, content)) = BUILTIN_LANGS.iter().find(|(name, _)| *name == file_name) {
        refresh_builtin_file(path, content);
    }
}

fn refresh_builtin_file(path: &Path, content: &str) {
    let needs_refresh = fs::read_to_string(path)
        .map(|existing| {
            !existing.contains("[ui]")
                || !existing.contains("verify_integrity")
                || !existing.contains("repo_status_line")
        })
        .unwrap_or(true);
    if needs_refresh {
        let _ = fs::write(path, content.trim_start());
    }
}

fn has_local_langs(lang_dir: &Path) -> bool {
    fs::read_dir(lang_dir)
        .map(|entries| {
            entries
                .flatten()
                .any(|e| e.path().extension().and_then(|ext| ext.to_str()) == Some("toml"))
        })
        .unwrap_or(false)
}

fn choose_language(
    work_dir: &Path,
    lang_dir: &Path,
) -> Option<(String, String, String, HashMap<String, String>)> {
    if let Ok(chosen) = fs::read_to_string(work_dir.join("lang_chosen.txt")) {
        if let Some(found) = find_by_stem(lang_dir, chosen.trim()) {
            return Some(found);
        }
    }

    for locale in system_locale_candidates() {
        if let Some(found) = find_by_locale(lang_dir, &locale) {
            return Some(found);
        }
    }

    find_default(lang_dir)
}

fn system_locale_candidates() -> Vec<String> {
    ["LC_ALL", "LANGUAGE", "LANG"]
        .iter()
        .filter_map(|key| std::env::var(key).ok())
        .flat_map(|value| value.split(':').map(str::to_string).collect::<Vec<_>>())
        .map(|value| {
            value
                .split('.')
                .next()
                .unwrap_or("")
                .replace('-', "_")
                .trim()
                .to_string()
        })
        .filter(|value| !value.is_empty() && value != "C")
        .collect()
}

fn find_by_stem(lang_dir: &Path, stem: &str) -> Option<(String, String, String, HashMap<String, String>)> {
    let target = stem.trim_end_matches(".default");
    local_lang_files(lang_dir).into_iter().find_map(|path| {
        let file_stem = lang_stem(&path)?;
        if file_stem == target {
            read_language_file(&path)
        } else {
            None
        }
    })
}

fn find_by_locale(
    lang_dir: &Path,
    locale: &str,
) -> Option<(String, String, String, HashMap<String, String>)> {
    let normalized = locale.to_lowercase();
    let parts: Vec<&str> = normalized.split('_').collect();
    let exact = if parts.len() >= 2 {
        Some(format!("{}_{}", parts[1].to_uppercase(), parts[0]))
    } else {
        None
    };

    if let Some(exact) = exact {
        if let Some(found) = find_by_stem(lang_dir, &exact) {
            return Some(found);
        }
    }

    let lang = parts.first().copied().unwrap_or("");
    local_lang_files(lang_dir).into_iter().find_map(|path| {
        let stem = lang_stem(&path)?;
        if stem.to_lowercase().ends_with(&format!("_{}", lang)) {
            read_language_file(&path)
        } else {
            None
        }
    })
}

fn find_default(lang_dir: &Path) -> Option<(String, String, String, HashMap<String, String>)> {
    local_lang_files(lang_dir).into_iter().find_map(|path| {
        let name = path.file_name()?.to_string_lossy();
        if name.ends_with(".default.toml") {
            read_language_file(&path)
        } else {
            None
        }
    })
}

fn local_lang_files(lang_dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<_> = fs::read_dir(lang_dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("toml"))
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    files
}

fn read_language_file(path: &Path) -> Option<(String, String, String, HashMap<String, String>)> {
    let content = fs::read_to_string(path).ok()?;
    let table: toml::Table = toml::from_str(&content).ok()?;
    let stem = lang_stem(path)?;
    let name = table
        .get("language_name")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| display_name_from_stem(&stem));
    let badge = table
        .get("default_badge")
        .and_then(|v| v.as_str())
        .unwrap_or("default")
        .to_string();
    // Layers: bundled English < bundled version of this language < local file.
    // A stale local file therefore never lacks a key.
    let mut ui = default_ui_texts();
    if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
        ui.extend(bundled_ui_texts(file_name));
    }
    if let Some(toml::Value::Table(ui_table)) = table.get("ui") {
        for (key, value) in ui_table {
            if let Some(text) = value.as_str() {
                ui.insert(key.clone(), text.to_string());
            }
        }
    }
    Some((stem, name, badge, ui))
}

fn pack_from_path(path: &Path, is_local: bool, is_remote: bool) -> Option<LanguagePack> {
    let file_name = path.file_name()?.to_string_lossy().to_string();
    let stem = lang_stem(path)?;
    let display_name = read_language_file(path)
        .map(|(_, name, _, _)| name)
        .unwrap_or_else(|| display_name_from_stem(&stem));
    Some(LanguagePack {
        stem,
        file_name: file_name.clone(),
        display_name,
        is_default: file_name.ends_with(".default.toml"),
        is_local,
        is_remote,
    })
}

fn pack_from_file_name(file_name: &str, is_local: bool, is_remote: bool) -> LanguagePack {
    let stem = file_name
        .strip_suffix(".toml")
        .unwrap_or(file_name)
        .trim_end_matches(".default")
        .to_string();
    LanguagePack {
        stem: stem.clone(),
        file_name: file_name.to_string(),
        display_name: display_name_from_stem(&stem),
        is_default: file_name.ends_with(".default.toml"),
        is_local,
        is_remote,
    }
}

fn lang_stem(path: &Path) -> Option<String> {
    Some(
        path.file_name()?
            .to_string_lossy()
            .strip_suffix(".toml")?
            .trim_end_matches(".default")
            .to_string(),
    )
}

fn display_name_from_stem(stem: &str) -> String {
    match stem {
        "EN_en" => "English (EN)",
        "FR_fr" => "Français (FR)",
        "CH_fr" => "Français (CH)",
        "DE_de" => "Deutsch (DE)",
        "CH_de" => "Deutsch (CH)",
        "IT_it" => "Italiano (IT)",
        "CH_it" => "Italiano (CH)",
        _ => stem,
    }
    .to_string()
}

/// `[ui]` strings of a bundled language file (empty if not bundled).
fn bundled_ui_texts(file_name: &str) -> HashMap<String, String> {
    BUILTIN_LANGS
        .iter()
        .find(|(name, _)| *name == file_name)
        .and_then(|(_, content)| toml::from_str::<toml::Table>(content).ok())
        .and_then(|table| match table.get("ui") {
            Some(toml::Value::Table(ui)) => Some(
                ui.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect(),
            ),
            _ => None,
        })
        .unwrap_or_default()
}

/// English strings compiled into the binary: the single fallback source.
fn default_ui_texts() -> HashMap<String, String> {
    bundled_ui_texts("EN_en.default.toml")
}

/// Values of one `[ui]` key in every bundled language (used to recognise
/// artefacts created under another language, e.g. a context-menu script).
#[cfg(unix)]
pub fn bundled_values(key: &str) -> Vec<String> {
    let mut out: Vec<String> = BUILTIN_LANGS
        .iter()
        .filter_map(|(name, _)| bundled_ui_texts(name).remove(key))
        .collect();
    out.sort();
    out.dedup();
    out
}

fn text_from_map(ui: &HashMap<String, String>, key: &str) -> String {
    ui.get(key)
        .cloned()
        .or_else(|| default_ui_texts().remove(key))
        .unwrap_or_else(|| key.to_string())
}

fn download_default_lang(lang_dir: &Path) -> bool {
    download_language(lang_dir, "EN_en.default.toml")
}

fn download_language(lang_dir: &Path, file_name: &str) -> bool {
    let url = format!("{}/{}", GITHUB_RAW_BASE, file_name);
    let target = lang_dir.join(file_name);

    #[cfg(windows)]
    let status = Command::new("curl.exe")
        .args(["-L", "--fail", "--silent", "--show-error", "-o"])
        .arg(&target)
        .arg(&url)
        .status();

    #[cfg(not(windows))]
    let status = Command::new("curl")
        .args(["-fsSL", "-o"])
        .arg(&target)
        .arg(&url)
        .status();

    let ok = status.map(|s| s.success()).unwrap_or(false) && target.exists();
    if !ok {
        let _ = fs::remove_file(&target);
    }
    ok
}

fn run_curl_text(url: &str, ui: &HashMap<String, String>) -> Result<String, String> {
    #[cfg(windows)]
    let output = Command::new("curl.exe")
        .args(["-L", "--fail", "--silent", "--show-error", url])
        .output();

    #[cfg(not(windows))]
    let output = Command::new("curl")
        .args(["-fsSL", url])
        .output();

    let output = output.map_err(|e| {
        text_from_map(ui, "curl_start_error").replace("{error}", &e.to_string())
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            text_from_map(ui, "github_index_read_error")
        } else {
            stderr
        });
    }

    String::from_utf8(output.stdout)
        .map_err(|e| text_from_map(ui, "github_utf8_error").replace("{error}", &e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn ui_keys(file: &str) -> BTreeSet<String> {
        bundled_ui_texts(file).into_keys().collect()
    }

    /// Every bundled language defines exactly the keys of the English file.
    #[test]
    fn all_languages_have_the_same_keys() {
        let reference = ui_keys("EN_en.default.toml");
        assert!(reference.len() > 50);
        for (file, _) in BUILTIN_LANGS {
            let k = ui_keys(file);
            let missing: Vec<_> = reference.difference(&k).collect();
            let extra: Vec<_> = k.difference(&reference).collect();
            assert!(missing.is_empty() && extra.is_empty(), "{file}: missing {missing:?}, extra {extra:?}");
        }
    }

    /// Every key used through t(), tr(), text(), text_replace() or Msg::new() exists.
    #[test]
    fn keys_used_in_code_exist() {
        let reference = ui_keys("EN_en.default.toml");
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut missing = Vec::new();
        for entry in fs::read_dir(src).unwrap().flatten() {
            let text = fs::read_to_string(entry.path()).unwrap();
            for call in [".t(\"", ".tr(\"", ".text(\"", ".text_replace(\"", "Msg::new(\""] {
                for part in text.split(call).skip(1) {
                    let key = part.split('"').next().unwrap();
                    if !reference.contains(key) {
                        missing.push(format!("{}: {key}", entry.path().display()));
                    }
                }
            }
        }
        assert!(missing.is_empty(), "missing keys: {missing:#?}");
    }

    #[test]
    fn language_window_title_is_localized() {
        for (file, _) in BUILTIN_LANGS {
            let title = bundled_ui_texts(file).remove("language_window_title").unwrap();
            assert!(!title.contains('/'), "{file}: {title}");
        }
    }

    #[test]
    fn msg_is_translated_with_arguments() {
        let ui = default_ui_texts();
        let text = text_from_map(&ui, "file_not_in_checksum").replace("{file}", "a.iso");
        assert!(text.contains("a.iso"));
        assert_eq!(text_from_map(&ui, "raw_error"), "{text}");
    }
}
