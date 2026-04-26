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
    ("DE_de.toml", DE_DE),
    ("IT_it.toml", IT_IT),
];

const EN_EN_DEFAULT: &str = include_str!("../lang/EN_en.default.toml");
const FR_FR: &str = include_str!("../lang/FR_fr.toml");
const DE_DE: &str = include_str!("../lang/DE_de.toml");
const IT_IT: &str = include_str!("../lang/IT_it.toml");

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
        let needs_refresh = fs::read_to_string(&path)
            .map(|existing| !existing.contains("[ui]") || !existing.contains("verify_integrity"))
            .unwrap_or(true);
        if needs_refresh {
            let _ = fs::write(path, content.trim_start());
        }
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
    let mut ui = default_ui_texts();
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

fn default_ui_texts() -> HashMap<String, String> {
    [
        ("language_window_title", "Languages"),
        ("active", "Active"),
        ("repo_not_refreshed", "GitHub repo not refreshed."),
        ("repo_loading", "Reading GitHub repo..."),
        ("repo_available", "GitHub repo available: {count} language(s)."),
        ("repo_unavailable", "Offline or repo unavailable: {error}"),
        ("repo_status_line", "{dot} {status}"),
        ("local_folder", "Local folder: {path}"),
        ("open", "Open"),
        ("refresh", "Refresh"),
        ("close", "Close"),
        ("language_loaded", "Language loaded."),
        (
            "network_error",
            "This program needs internet access to download its language resources.",
        ),
        ("language_file_invalid", "Invalid language file."),
        ("no_remote_languages", "No TOML language found on the GitHub repo."),
        (
            "github_index_invalid",
            "Invalid GitHub index: lang folder not found.",
        ),
        ("github_index_parse_error", "Invalid GitHub index: {error}"),
        ("curl_start_error", "Unable to start curl: {error}"),
        ("github_index_read_error", "Unable to read GitHub index."),
        ("github_utf8_error", "GitHub response is not UTF-8: {error}"),
        ("settings", "Settings"),
        ("close_settings", "Close"),
        ("language_tooltip", "Language settings [{lang}]"),
        ("drop_file", "Drop a file here or click to select"),
        ("select_file_title", "Select the file to verify"),
        ("mode_auto", "Automatic"),
        ("mode_checksum_file", "Checksum file"),
        ("mode_manual_hash", "Manual hash"),
        ("checksum_detected", "Checksum detected:"),
        ("no_checksum_auto", "No checksum file found automatically."),
        ("no_file_selected", "No file selected"),
        ("choose", "Choose..."),
        ("select_checksum_title", "Select checksum file"),
        ("algorithm", "Algorithm:"),
        ("expected_hash", "Expected hash:"),
        ("manual_hash_hint", "ex: sha256:abc123... or simply the hash value"),
        ("manual_hash_hint_short", "ex: sha256:abc123... or raw value"),
        ("computing", "Computing..."),
        ("verify_integrity", "Verify integrity"),
        ("verification_success", "VERIFICATION SUCCESSFUL"),
        ("file_intact", "The file is intact and unmodified."),
        ("verification_failed", "VERIFICATION FAILED"),
        ("file_modified", "The file is corrupted or has been modified!"),
        ("error_prefix", "Error: {error}"),
        ("multi_review_title", "{total} file(s) to verify — {selected} selected"),
        ("multi_review_help", "Uncheck files to exclude, then click Continue."),
        ("input_required", "Input required"),
        ("select_all", "Select all"),
        ("unselect_all", "Unselect all"),
        ("continue", "Continue →"),
        ("cancel", "Cancel"),
        ("manual_input_title", "Hash input — file {current}/{total}"),
        ("no_checksum_for_file", "No checksum file was found automatically for this file."),
        ("validate", "Validate →"),
        ("skip", "← Skip"),
        ("all_files_intact", "ALL FILES ARE INTACT"),
        ("multi_failed", "VERIFICATION FAILED — CORRUPTED FILE(S)"),
        ("multi_errors", "ERROR(S) DURING VERIFICATION"),
        ("verification_finished", "Verification finished"),
        ("multi_stats", "{total} verified · {success} successful · {failure} failed · {error} error(s) · {skipped} skipped"),
        ("status_success", "SUCCESS"),
        ("status_failed", "FAILED"),
        ("status_skipped", "Skipped"),
        ("status_pending", "Pending"),
        ("expected", "Expected:"),
        ("computed", "Computed:"),
        ("new_verification", "New verification"),
        ("about", "About"),
        ("version", "Version:"),
        ("author", "Author:"),
        ("product_name", "Product name:"),
        ("algorithms", "Algorithms:"),
        ("license", "License:"),
        ("context_integration", "Context menu integration"),
        ("context_integration_help", "Allows verifying a file from the explorer right-click menu."),
        ("active_status", "Active"),
        ("inactive_status", "Inactive"),
        ("enable", "Enable"),
        ("disable", "Disable"),
        ("enabled_success", "Enabled successfully."),
        ("disabled_success", "Disabled successfully."),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect()
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
