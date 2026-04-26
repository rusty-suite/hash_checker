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

const EN_EN_DEFAULT: &str = r#"
language_name = "English (EN)"
default_badge = "default"
"#;

const FR_FR: &str = r#"
language_name = "Français (FR)"
default_badge = "défaut"
"#;

const DE_DE: &str = r#"
language_name = "Deutsch (DE)"
default_badge = "Standard"
"#;

const IT_IT: &str = r#"
language_name = "Italiano (IT)"
default_badge = "predefinita"
"#;

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
        let (active_stem, active_name, default_badge) = selected.unwrap_or_else(|| {
            network_error = true;
            (
                "EN_en".to_string(),
                "English (EN)".to_string(),
                "default".to_string(),
            )
        });

        Self {
            work_dir,
            lang_dir,
            active_stem,
            active_name,
            default_badge,
            network_error,
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
            return Err("Ce programme a besoin d'un accès internet pour télécharger ses ressources linguistiques.".to_string());
        }

        let Some((stem, name, badge)) = read_language_file(&path) else {
            return Err("Fichier de langue invalide.".to_string());
        };

        fs::write(self.work_dir.join("lang_chosen.txt"), stem.as_bytes())
            .map_err(|e| e.to_string())?;
        self.active_stem = stem;
        self.active_name = name;
        self.default_badge = badge;
        Ok(())
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

    pub fn fetch_remote_languages() -> Result<Vec<String>, String> {
        let output = run_curl_text(GITHUB_LANG_INDEX)?;
        let value: serde_json::Value =
            serde_json::from_str(&output).map_err(|e| format!("Index GitHub invalide : {}", e))?;
        let Some(items) = value.as_array() else {
            return Err("Index GitHub invalide : dossier lang introuvable.".to_string());
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
            return Err("Aucune langue TOML trouvée sur le repo GitHub.".to_string());
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
        if !path.exists() {
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

fn choose_language(work_dir: &Path, lang_dir: &Path) -> Option<(String, String, String)> {
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

fn find_by_stem(lang_dir: &Path, stem: &str) -> Option<(String, String, String)> {
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

fn find_by_locale(lang_dir: &Path, locale: &str) -> Option<(String, String, String)> {
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

fn find_default(lang_dir: &Path) -> Option<(String, String, String)> {
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

fn read_language_file(path: &Path) -> Option<(String, String, String)> {
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
    Some((stem, name, badge))
}

fn pack_from_path(path: &Path, is_local: bool, is_remote: bool) -> Option<LanguagePack> {
    let file_name = path.file_name()?.to_string_lossy().to_string();
    let stem = lang_stem(path)?;
    let display_name = read_language_file(path)
        .map(|(_, name, _)| name)
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

fn run_curl_text(url: &str) -> Result<String, String> {
    #[cfg(windows)]
    let output = Command::new("curl.exe")
        .args(["-L", "--fail", "--silent", "--show-error", url])
        .output();

    #[cfg(not(windows))]
    let output = Command::new("curl")
        .args(["-fsSL", url])
        .output();

    let output = output.map_err(|e| format!("Impossible de lancer curl : {}", e))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            "Impossible de lire l'index GitHub.".to_string()
        } else {
            stderr
        });
    }

    String::from_utf8(output.stdout).map_err(|e| format!("Réponse GitHub non UTF-8 : {}", e))
}
