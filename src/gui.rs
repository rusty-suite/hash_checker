// =============================================================================
// gui.rs — Interface graphique de Hash Checker (charte Rusty Suite)
//
// Page « une tâche » de la charte : en-tête commun, contenu centré, barre d'état,
// notifications. Thème, composants, logo animé, langues, réglages et mise à jour
// viennent de rusty_kit (`rusty_ui` et `rusty_core`) : ce module n'assemble que le
// contenu propre à Hash Checker.
//
// Deux modes :
//   - fichier unique : dépôt d'un fichier, source du hash (automatique, fichier de
//     contrôle choisi, hash saisi), bouton d'action avec état, bannière de résultat ;
//   - plusieurs fichiers (clic droit sur N fichiers) : revue, saisie des hash
//     manquants, vérification, résultats.
// Les calculs tournent dans des fils séparés ; l'interface lit leur état à chaque image.
// =============================================================================

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;

use eframe::egui;
use rusty_core::lang::{self, Lang};
use rusty_core::settings::CommonSettings;
use rusty_core::update::{self, Release};
use rusty_ui::rusty_logo::{Logo, LogoAnim};
use rusty_ui::{self as ui, ActionButton, ActionLabels, ButtonKind, ChromeAction, Text, Tone};
use serde::{Deserialize, Serialize};

use crate::checksum::{find_checksum_file, find_entry_for_file, parse_checksum_file};
use crate::hasher::{compute_hash, Algorithm};
use crate::integration::{
    current_exe_path, install_kde, install_nautilus, install_thunar, install_windows, uninstall_kde,
    uninstall_nautilus, uninstall_thunar, uninstall_windows, IntegrationStatus,
};
use crate::language::{Msg, PACKS};

/// Identité de l'application (version et licence lues depuis Cargo.toml).
pub const APP_ID: &str = "hash_checker";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const LICENSE: &str = env!("CARGO_PKG_LICENSE");
const REPOSITORY: &str = "https://github.com/rusty-suite/hash_checker";
const AUTHORS: &str = "rusty-suite";

/// Agent HTTP générique (jamais d'information personnelle).
pub fn user_agent() -> String {
    format!("{APP_ID}/{VERSION}")
}

/// Réglages persistants de l'application.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub common: CommonSettings,
}

// =============================================================================
// États partagés avec les fils de calcul
// =============================================================================

#[derive(Debug, Clone, PartialEq)]
enum VerifyState {
    Idle,
    Computing,
    Success(String, String),
    Failure(String, String),
    Error(Msg),
}

#[derive(Debug, Clone)]
enum HashSource {
    AutoFound(PathBuf),
    ManualHash(String, Algorithm),
    NeedsInput,
    Skipped,
}

#[derive(Debug, Clone, PartialEq)]
enum EntryStatus {
    Pending,
    Computing,
    Success(String, String),
    Failure(String, String),
    Error(Msg),
    Skipped,
}

struct FileEntry {
    path: PathBuf,
    selected: bool,
    source: HashSource,
    status: Arc<Mutex<EntryStatus>>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum MultiPhase {
    Review,
    ManualInput,
    Verifying,
    Results,
}

/// Mode plusieurs fichiers : liste, file d'attente de saisie et champs de saisie.
struct MultiFileState {
    entries: Vec<FileEntry>,
    phase: MultiPhase,
    manual_queue: Vec<usize>,
    manual_pos: usize,
    input_hash: String,
    input_algo: usize,
}

/// Lit « algo:hash » (ex. « sha256:abc… ») ou un hash brut avec l'algorithme choisi.
fn parse_hash_input(raw: &str, fallback: Algorithm) -> (String, Algorithm) {
    let raw = raw.trim().to_lowercase();
    if let Some((prefix, hash)) = raw.split_once(':') {
        if let Some(detected) = Algorithm::from_str(prefix) {
            return (hash.to_string(), detected);
        }
    }
    (raw, fallback)
}

fn new_entry(path: PathBuf) -> FileEntry {
    let source = match find_checksum_file(&path) {
        Some(cs) => HashSource::AutoFound(cs),
        None => HashSource::NeedsInput,
    };
    FileEntry { path, selected: true, source, status: Arc::new(Mutex::new(EntryStatus::Pending)) }
}

impl MultiFileState {
    fn from_paths(paths: Vec<PathBuf>) -> Self {
        Self {
            entries: paths.into_iter().map(new_entry).collect(),
            phase: MultiPhase::Review,
            manual_queue: Vec::new(),
            manual_pos: 0,
            input_hash: String::new(),
            input_algo: default_algo_index(),
        }
    }

    /// Revue → saisie manuelle, ou directement vérification. `true` = lancer les calculs.
    fn advance_from_review(&mut self) -> bool {
        self.manual_queue = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.selected && matches!(e.source, HashSource::NeedsInput))
            .map(|(i, _)| i)
            .collect();
        self.manual_pos = 0;
        self.input_hash.clear();
        if self.manual_queue.is_empty() {
            self.phase = MultiPhase::Verifying;
            true
        } else {
            self.phase = MultiPhase::ManualInput;
            false
        }
    }

    fn validate_manual(&mut self) -> bool {
        if let Some(&idx) = self.manual_queue.get(self.manual_pos) {
            let (hash, algo) = parse_hash_input(&self.input_hash, algo_at(self.input_algo));
            self.entries[idx].source = HashSource::ManualHash(hash, algo);
        }
        self.next_manual()
    }

    fn skip_manual(&mut self) -> bool {
        if let Some(&idx) = self.manual_queue.get(self.manual_pos) {
            self.entries[idx].source = HashSource::Skipped;
        }
        self.next_manual()
    }

    fn next_manual(&mut self) -> bool {
        self.manual_pos += 1;
        self.input_hash.clear();
        if self.manual_pos >= self.manual_queue.len() {
            self.phase = MultiPhase::Verifying;
            true
        } else {
            false
        }
    }

    fn all_done(&self) -> bool {
        self.entries.iter().all(|e| !matches!(*e.status.lock().unwrap(), EntryStatus::Pending | EntryStatus::Computing))
    }

    /// (vérifiés, intègres, différents, erreurs, ignorés).
    fn stats(&self) -> (usize, usize, usize, usize, usize) {
        let (mut ok, mut ko, mut err, mut skip) = (0, 0, 0, 0);
        for e in &self.entries {
            match *e.status.lock().unwrap() {
                EntryStatus::Success(..) => ok += 1,
                EntryStatus::Failure(..) => ko += 1,
                EntryStatus::Error(_) => err += 1,
                EntryStatus::Skipped => skip += 1,
                _ => {}
            }
        }
        (ok + ko + err, ok, ko, err, skip)
    }

    /// Ajoute un fichier pendant la revue (dépôt ou second lancement).
    fn add_if_new(&mut self, path: PathBuf) {
        if self.phase == MultiPhase::Review && !self.entries.iter().any(|e| e.path == path) {
            self.entries.push(new_entry(path));
        }
    }
}

fn algo_at(i: usize) -> Algorithm {
    Algorithm::all().get(i).cloned().unwrap_or(Algorithm::Sha256)
}

fn default_algo_index() -> usize {
    Algorithm::all().iter().position(|a| *a == Algorithm::Sha256).unwrap_or(0)
}

fn file_name(p: &Path) -> String {
    p.file_name().unwrap_or_default().to_string_lossy().into_owned()
}

// =============================================================================
// Application
// =============================================================================

pub struct HashCheckerApp {
    // Fichier unique
    target_file: Option<PathBuf>,
    /// Source du hash : 0 automatique, 1 fichier de contrôle, 2 hash saisi (slider 3 positions).
    input_mode: u8,
    checksum_path: String,
    auto_checksum_found: Option<PathBuf>,
    manual_hash: String,
    algo_index: usize,
    state: Arc<Mutex<VerifyState>>,
    action: ActionButton,
    multi: Option<MultiFileState>,
    multi_action: ActionButton,

    // Commun (charte)
    logo: Logo,
    lang: Lang,
    data_dir: PathBuf,
    log_path: PathBuf,
    settings: Settings,
    toasts: ui::Toasts,
    zones: ui::Zones,
    show_settings: bool,
    show_language: bool,
    show_about: bool,
    show_update: bool,
    online: Option<bool>,
    net_rx: Option<Receiver<bool>>,
    lang_rx: Option<Receiver<Result<usize, String>>>,
    update: Option<Release>,
    update_rx: Option<Receiver<Result<Option<Release>, String>>>,
    install_rx: Option<Receiver<Result<(), String>>>,
    args_rx: Receiver<Vec<String>>,
    integration_status: IntegrationStatus,
}

impl HashCheckerApp {
    /// Construit l'application. `files` : fichiers passés en argument (clic droit) ;
    /// `args_rx` : arguments des lancements suivants (instance unique) ;
    /// `rolled_back` : version restaurée après une mise à jour qui a échoué.
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        files: Vec<PathBuf>,
        args_rx: Receiver<Vec<String>>,
        data_dir: PathBuf,
        log_path: PathBuf,
        settings: Settings,
        rolled_back: Option<String>,
    ) -> Self {
        ui::install(&cc.egui_ctx);
        let lang = Lang::load(PACKS, &data_dir, &settings.common.language);
        ui::set_texts(&cc.egui_ctx, ui::KitTexts::from_lookup(|k| lang.t(k)));
        let mut logo = Logo::new(LogoAnim::Orbit);
        logo.play_launch(0.0);
        let net_rx = Some(rusty_core::net::check_in_background(user_agent()));
        let mut app = Self {
            target_file: None,
            input_mode: 0,
            checksum_path: String::new(),
            auto_checksum_found: None,
            manual_hash: String::new(),
            algo_index: default_algo_index(),
            state: Arc::new(Mutex::new(VerifyState::Idle)),
            action: ActionButton::default(),
            multi: None,
            multi_action: ActionButton::default(),
            logo,
            lang,
            data_dir,
            log_path,
            settings,
            toasts: ui::Toasts::default(),
            zones: ui::Zones::default(),
            show_settings: false,
            show_language: false,
            show_about: false,
            show_update: false,
            online: None,
            net_rx,
            lang_rx: None,
            update: None,
            update_rx: None,
            install_rx: None,
            args_rx,
            integration_status: IntegrationStatus::detect(),
        };
        match files.len() {
            0 => {}
            1 => app.set_target_file(files.into_iter().next().unwrap()),
            _ => app.multi = Some(MultiFileState::from_paths(files)),
        }
        if let Some(v) = rolled_back {
            let text = app.lang.tf("kit.update_restored_text", &[("version", &v)]);
            app.toasts.push(Tone::Warning, app.kit("update_restored"), text);
        }
        app
    }

    // ── Textes ───────────────────────────────────────────────────────────────

    fn ui(&self, key: &str) -> String {
        self.lang.t(&format!("ui.{key}"))
    }

    fn uif(&self, key: &str, args: &[(&str, &str)]) -> String {
        self.lang.tf(&format!("ui.{key}"), args)
    }

    fn kit(&self, key: &str) -> String {
        self.lang.t(&format!("kit.{key}"))
    }

    fn size(&self, bytes: u64) -> String {
        let units = self.ui("size_units");
        let u: Vec<&str> = units.split(',').collect();
        let unit = |i: usize| u.get(i).copied().unwrap_or("?").trim().to_string();
        let b = bytes as f64;
        match bytes {
            0..=1023 => format!("{bytes} {}", unit(0)),
            1024..=1_048_575 => format!("{:.1} {}", b / 1024.0, unit(1)),
            1_048_576..=1_073_741_823 => format!("{:.1} {}", b / 1_048_576.0, unit(2)),
            _ => format!("{:.2} {}", b / 1_073_741_824.0, unit(3)),
        }
    }

    // ── Langue et réglages ──────────────────────────────────────────────────

    fn set_language(&mut self, ctx: &egui::Context, stem: String) {
        self.settings.common.language = stem;
        self.save_settings();
        self.reload_language(ctx);
    }

    fn reload_language(&mut self, ctx: &egui::Context) {
        self.lang = Lang::load(PACKS, &self.data_dir, &self.settings.common.language);
        let lang = &self.lang;
        ui::set_texts(ctx, ui::KitTexts::from_lookup(|k| lang.t(k)));
    }

    fn save_settings(&self) {
        if let Err(e) = rusty_core::settings::save(&self.data_dir, &self.settings) {
            log::warn!("Réglages non enregistrés : {e}");
        }
    }

    // ── Fichier unique ──────────────────────────────────────────────────────

    fn set_target_file(&mut self, path: PathBuf) {
        self.auto_checksum_found = find_checksum_file(&path);
        self.input_mode = if self.auto_checksum_found.is_some() { 0 } else { 1 };
        self.checksum_path.clear();
        self.manual_hash.clear();
        self.target_file = Some(path);
        *self.state.lock().unwrap() = VerifyState::Idle;
    }

    fn reset(&mut self) {
        self.multi = None;
        self.target_file = None;
        self.auto_checksum_found = None;
        self.checksum_path.clear();
        self.manual_hash.clear();
        *self.state.lock().unwrap() = VerifyState::Idle;
    }

    /// Fichiers reçus d'un second lancement ou déposés : liste multi si besoin.
    fn receive_files(&mut self, paths: Vec<PathBuf>) {
        let paths: Vec<PathBuf> = paths.into_iter().filter(|p| p.is_file()).collect();
        if paths.is_empty() {
            return;
        }
        if let Some(multi) = &mut self.multi {
            for p in paths {
                multi.add_if_new(p);
            }
        } else if self.target_file.is_none() && paths.len() == 1 {
            self.set_target_file(paths.into_iter().next().unwrap());
        } else {
            let mut all: Vec<PathBuf> = self.target_file.take().into_iter().collect();
            for p in paths {
                if !all_contains(&all, &p) {
                    all.push(p);
                }
            }
            self.reset();
            self.multi = Some(MultiFileState::from_paths(all));
        }
    }

    fn start_verification(&mut self) {
        let Some(target) = self.target_file.clone() else { return };
        let state = Arc::clone(&self.state);
        *state.lock().unwrap() = VerifyState::Computing;
        match self.input_mode {
            2 => {
                let (expected, algo) = parse_hash_input(&self.manual_hash, algo_at(self.algo_index));
                if expected.is_empty() {
                    *state.lock().unwrap() = VerifyState::Error(Msg::new("enter_hash_value"));
                    return;
                }
                thread::spawn(move || run_single(target, expected, algo, state));
            }
            mode => {
                let cs = if mode == 0 { self.auto_checksum_found.clone() } else { Some(PathBuf::from(&self.checksum_path)).filter(|p| p.is_file()) };
                let Some(cs) = cs else {
                    *state.lock().unwrap() = VerifyState::Error(Msg::new("no_checksum_selected"));
                    return;
                };
                let fallback = algo_at(self.algo_index);
                thread::spawn(move || {
                    let entries = match parse_checksum_file(&cs) {
                        Ok(e) => e,
                        Err(e) => {
                            *state.lock().unwrap() = VerifyState::Error(e);
                            return;
                        }
                    };
                    match find_entry_for_file(&entries, &target) {
                        Some(entry) => {
                            let algo = entry.algorithm.clone().unwrap_or(fallback);
                            run_single(target, entry.hash.clone(), algo, state);
                        }
                        None => {
                            *state.lock().unwrap() = VerifyState::Error(Msg::new("file_not_in_checksum").arg("file", file_name(&target)));
                        }
                    }
                });
            }
        }
    }

    fn start_multi_verification(&mut self) {
        let Some(multi) = &mut self.multi else { return };
        for entry in &mut multi.entries {
            let status = Arc::clone(&entry.status);
            if !entry.selected || matches!(entry.source, HashSource::Skipped | HashSource::NeedsInput) {
                *status.lock().unwrap() = EntryStatus::Skipped;
                continue;
            }
            *status.lock().unwrap() = EntryStatus::Computing;
            let target = entry.path.clone();
            match entry.source.clone() {
                HashSource::AutoFound(cs) => {
                    thread::spawn(move || run_entry_from_file(target, cs, Algorithm::Sha256, status));
                }
                HashSource::ManualHash(hash, algo) => {
                    thread::spawn(move || run_entry_direct(target, hash, algo, status));
                }
                HashSource::Skipped | HashSource::NeedsInput => {}
            }
        }
    }

    // ── Tâches de fond (réseau, langues, mise à jour) ───────────────────────

    fn poll_background(&mut self, ctx: &egui::Context) {
        if let Some(online) = self.net_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.net_rx = None;
            self.online = Some(online);
            if online {
                let (dir, ua) = (self.data_dir.clone(), user_agent());
                let (tx, rx) = mpsc::channel();
                thread::spawn(move || {
                    let _ = tx.send(lang::refresh_remote(APP_ID, PACKS, &dir, &ua));
                });
                self.lang_rx = Some(rx);
                if self.settings.common.check_updates {
                    let (tx, rx) = mpsc::channel();
                    thread::spawn(move || {
                        let _ = tx.send(update::check(APP_ID, APP_ID, VERSION, &user_agent()).map_err(|e| e.to_string()));
                    });
                    self.update_rx = Some(rx);
                }
            }
        }
        if let Some(result) = self.lang_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.lang_rx = None;
            match result {
                Ok(n) if n > 0 => {
                    log::info!("{n} pack(s) de langue mis à jour");
                    self.reload_language(ctx);
                }
                Ok(_) => {}
                Err(e) => log::warn!("Mise à jour des langues impossible : {e}"),
            }
        }
        if let Some(result) = self.update_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.update_rx = None;
            match result {
                Ok(Some(release)) => {
                    log::info!("Mise à jour disponible : {}", release.version);
                    self.update = Some(release);
                }
                Ok(None) => {}
                Err(e) => log::warn!("Vérification des mises à jour impossible : {e}"),
            }
        }
        if let Some(result) = self.install_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.install_rx = None;
            match result {
                Ok(()) => {
                    // Relance la nouvelle version puis ferme celle-ci.
                    if let Ok(exe) = std::env::current_exe() {
                        let _ = std::process::Command::new(exe).spawn();
                    }
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                Err(e) => {
                    self.show_update = false;
                    self.toasts.push(Tone::Error, self.kit("update_failed"), e);
                }
            }
        }
        // Arguments d'un second lancement (clic droit pendant que la fenêtre est ouverte).
        let incoming: Vec<PathBuf> = self.args_rx.try_iter().flatten().map(PathBuf::from).collect();
        if !incoming.is_empty() {
            self.receive_files(incoming);
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
    }

    fn start_install(&mut self) {
        let Some(release) = self.update.clone() else { return };
        let (dir, ua) = (self.data_dir.clone(), user_agent());
        let (tx, rx) = mpsc::channel();
        let checksum_text = self.kit("update_checksum");
        thread::spawn(move || {
            let result = update::install(&release, VERSION, &dir, &ua).map_err(|e| match e {
                update::UpdateError::Checksum => checksum_text,
                other => other.to_string(),
            });
            let _ = tx.send(result);
        });
        self.install_rx = Some(rx);
    }
}

fn all_contains(list: &[PathBuf], p: &Path) -> bool {
    list.iter().any(|x| x == p)
}

// =============================================================================
// Boucle principale
// =============================================================================

impl eframe::App for HashCheckerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.zones.begin_frame(ctx);
        self.poll_background(ctx);

        // Fichiers déposés n'importe où sur la fenêtre (la zone de dépôt les gère elle-même).
        if self.target_file.is_some() || self.multi.is_some() {
            let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
            if !dropped.is_empty() {
                self.receive_files(dropped);
            }
        }

        let info = ui::AppInfo { name: "Hash Checker".into(), description: self.ui("app_description"), version: VERSION.into() };
        let mut action = ui::app_header(ctx, &mut self.logo, &info, &self.lang.code());
        let languages = self.lang.tf("kit.languages_count", &[("count", &lang::STEMS.len().to_string())]);
        let update_version = self.update.as_ref().map(|r| r.version.clone());
        let status = ui::StatusInfo { online: self.online, languages: &languages, update: update_version.as_deref(), version: VERSION };
        let a = ui::status_bar(ctx, &status);
        if a != ChromeAction::None {
            action = a;
        }
        match action {
            ChromeAction::Settings => self.show_settings = true,
            ChromeAction::Language => self.show_language = true,
            ChromeAction::About => self.show_about = true,
            ChromeAction::Update => self.show_update = true,
            ChromeAction::None => {}
        }

        ui::content(ctx, true, |ui| {
            if self.multi.is_some() {
                self.show_multi(ui);
            } else {
                self.show_single(ui);
            }
        });

        self.show_windows(ctx);
        self.toasts.show(ctx);
        self.zones.end_frame(ctx);

        // Calculs en cours ou messages d'autres instances : rafraîchir régulièrement.
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Fermeture propre : la version en cours est saine (charte, 06).
        update::mark_healthy(&self.data_dir);
    }
}

// =============================================================================
// Mode fichier unique
// =============================================================================

impl HashCheckerApp {
    fn show_single(&mut self, ui: &mut egui::Ui) {
        let now = ui.input(|i| i.time);
        let Some(target) = self.target_file.clone() else {
            match ui::drop_zone(ui, 160.0) {
                ui::DropResult::Dropped(paths) => self.receive_files(paths),
                ui::DropResult::Browse => {
                    if let Some(p) = rfd::FileDialog::new().set_title(self.ui("select_file_title")).pick_file() {
                        self.set_target_file(p);
                    }
                }
                ui::DropResult::None => {}
            }
            return;
        };

        let meta = std::fs::metadata(&target).map(|m| self.size(m.len())).unwrap_or_default();
        let path_text = target.display().to_string();
        let removed = ui::file_card(ui, &file_name(&target), &meta, &path_text, |ui| {
            let modes = [self.ui("mode_auto"), self.ui("mode_checksum_file"), self.ui("mode_manual_hash")];
            let mut mode = self.input_mode;
            ui::setting_row(ui, &self.ui("source_label"), Some(&self.ui("source_help")), |ui| {
                let label = modes[mode as usize].clone();
                ui::slider3(ui, &mut mode, &label);
            });
            if mode != self.input_mode {
                self.input_mode = mode;
                *self.state.lock().unwrap() = VerifyState::Idle;
            }
            ui::row_separator(ui);
            match self.input_mode {
                0 => match self.auto_checksum_found.clone() {
                    Some(cs) => {
                        ui.label(Text::Label.rich(self.ui("checksum_detected")).color(ui::tokens::TEXT_SECONDARY));
                        ui::copy_value(ui, "auto_checksum", &cs.display().to_string());
                    }
                    None => ui::banner(ui, Tone::Warning, &self.ui("no_checksum_auto"), None),
                },
                1 => {
                    ui.label(Text::Label.rich(self.ui("checksum_file_label")).color(ui::tokens::TEXT_SECONDARY));
                    if ui::path_field(ui, "checksum_file", &mut self.checksum_path, false) == ui::PathAction::Browse {
                        if let Some(p) = rfd::FileDialog::new().set_title(self.ui("select_checksum_title")).pick_file() {
                            self.checksum_path = p.display().to_string();
                            *self.state.lock().unwrap() = VerifyState::Idle;
                        }
                    }
                }
                _ => {
                    let algos: Vec<String> = Algorithm::all().iter().map(|a| a.to_string()).collect();
                    let mut idx = self.algo_index;
                    ui::setting_row(ui, &self.ui("algorithm"), None, |ui| {
                        ui::select(ui, "algo", &mut idx, &algos);
                    });
                    self.algo_index = idx;
                    let hint = self.ui("manual_hash_hint");
                    let label = self.ui("expected_hash");
                    ui::TextField { label: Some(&label), hint: Some(&hint), mono: true, ..Default::default() }.show(ui, &mut self.manual_hash);
                }
            }
        });
        if removed {
            self.reset();
            return;
        }

        // Rangée d'actions alignée à droite : action principale avec état.
        let labels = (self.ui("verify_integrity"), self.ui("computing"), self.ui("result_ok"), self.ui("result_failed"));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let l = ActionLabels { idle: &labels.0, working: &labels.1, success: &labels.2, error: &labels.3 };
            if self.action.show(ui, &l) {
                self.logo.start(now);
                self.start_verification();
            }
        });

        let state = self.state.lock().unwrap().clone();
        if self.action.status() == ui::Status::Working && state != VerifyState::Computing && state != VerifyState::Idle {
            let ok = matches!(state, VerifyState::Success(..));
            self.action.finish(ok, now);
            self.logo.finish(ok, now);
        }
        match state {
            VerifyState::Idle | VerifyState::Computing => {}
            VerifyState::Success(exp, comp) => self.result_banner(ui, Tone::Success, "verification_success", "file_intact", &exp, &comp),
            VerifyState::Failure(exp, comp) => self.result_banner(ui, Tone::Error, "verification_failed", "file_modified", &exp, &comp),
            VerifyState::Error(msg) => ui::banner(ui, Tone::Error, &self.ui("result_error"), Some(&msg.text(&self.lang))),
        }
    }

    fn result_banner(&self, ui: &mut egui::Ui, tone: Tone, title: &str, text: &str, expected: &str, computed: &str) {
        ui::banner(ui, tone, &self.ui(title), Some(&self.ui(text)));
        ui::card(ui, |ui| {
            ui.label(Text::Overline.rich(self.ui("expected")).color(ui::tokens::TEXT_MUTED));
            ui::copy_value(ui, "expected", expected);
            ui.label(Text::Overline.rich(self.ui("computed")).color(ui::tokens::TEXT_MUTED));
            ui::copy_value(ui, "computed", computed);
        });
    }
}

// =============================================================================
// Mode plusieurs fichiers
// =============================================================================

impl HashCheckerApp {
    fn show_multi(&mut self, ui: &mut egui::Ui) {
        let phase = self.multi.as_ref().map(|m| m.phase);
        match phase {
            Some(MultiPhase::Review) => self.show_review(ui),
            Some(MultiPhase::ManualInput) => self.show_manual_input(ui),
            Some(MultiPhase::Verifying) => self.show_verifying(ui),
            Some(MultiPhase::Results) => self.show_results(ui),
            None => {}
        }
    }

    fn show_review(&mut self, ui: &mut egui::Ui) {
        let (total, selected) = self.multi.as_ref().map(|m| (m.entries.len(), m.entries.iter().filter(|e| e.selected).count())).unwrap_or((0, 0));
        ui.label(Text::Heading.rich(self.uif("multi_review_title", &[("total", &total.to_string()), ("selected", &selected.to_string())])).color(ui::tokens::TEXT_PRIMARY));
        ui.label(Text::Body.rich(self.ui("multi_review_help")).color(ui::tokens::TEXT_SECONDARY));
        let (on, off, needs) = (self.ui("value_on"), self.ui("value_off"), self.ui("input_required"));
        ui::card(ui, |ui| {
            let Some(multi) = &mut self.multi else { return };
            let n = multi.entries.len();
            for (i, entry) in multi.entries.iter_mut().enumerate() {
                let source = match &entry.source {
                    HashSource::AutoFound(cs) => file_name(cs),
                    _ => needs.clone(),
                };
                let name = file_name(&entry.path);
                ui::setting_row(ui, &name, Some(&source), |ui| {
                    let label = if entry.selected { on.clone() } else { off.clone() };
                    ui::slider2(ui, &mut entry.selected, &label);
                });
                if i + 1 < n {
                    ui::row_separator(ui);
                }
            }
        });
        let any = selected > 0;
        ui.horizontal(|ui| {
            if ui::button(ui, &self.ui("select_all"), ButtonKind::Secondary).clicked() {
                self.set_all_selected(true);
            }
            if ui::button(ui, &self.ui("unselect_all"), ButtonKind::Secondary).clicked() {
                self.set_all_selected(false);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui::button_enabled(ui, any, &self.ui("continue"), ButtonKind::Primary).clicked() {
                    self.continue_from_review(ui.input(|i| i.time));
                }
                if ui::button(ui, &self.ui("cancel"), ButtonKind::Quiet).clicked() {
                    self.reset();
                }
            });
        });
    }

    fn set_all_selected(&mut self, value: bool) {
        if let Some(m) = &mut self.multi {
            m.entries.iter_mut().for_each(|e| e.selected = value);
        }
    }

    fn continue_from_review(&mut self, now: f64) {
        let start = self.multi.as_mut().map(|m| m.advance_from_review()).unwrap_or(false);
        if start {
            self.begin_multi(now);
        }
    }

    fn begin_multi(&mut self, now: f64) {
        self.logo.start(now);
        self.multi_action.start();
        self.start_multi_verification();
    }

    fn show_manual_input(&mut self, ui: &mut egui::Ui) {
        let now = ui.input(|i| i.time);
        let Some(multi) = &self.multi else { return };
        let idx = multi.manual_queue.get(multi.manual_pos).copied().unwrap_or(0);
        let name = multi.entries.get(idx).map(|e| file_name(&e.path)).unwrap_or_default();
        let (pos, len) = (multi.manual_pos + 1, multi.manual_queue.len());
        let algos: Vec<String> = Algorithm::all().iter().map(|a| a.to_string()).collect();
        let title = self.uif("manual_input_title", &[("current", &pos.to_string()), ("total", &len.to_string())]);
        let (algo_label, hash_label, hint, warn) = (self.ui("algorithm"), self.ui("expected_hash"), self.ui("manual_hash_hint_short"), self.ui("no_checksum_for_file"));
        ui.label(Text::Heading.rich(title).color(ui::tokens::TEXT_PRIMARY));
        ui::card(ui, |ui| {
            ui.label(Text::Mono.rich(&name).color(ui::tokens::TEXT_PRIMARY));
            ui.label(Text::Caption.rich(&warn).color(ui::tokens::WARNING));
            ui::row_separator(ui);
            if let Some(multi) = &mut self.multi {
                ui::setting_row(ui, &algo_label, None, |ui| {
                    ui::select(ui, "multi_algo", &mut multi.input_algo, &algos);
                });
                ui::TextField { label: Some(&hash_label), hint: Some(&hint), mono: true, ..Default::default() }.show(ui, &mut multi.input_hash);
            }
        });
        let empty = self.multi.as_ref().map(|m| m.input_hash.trim().is_empty()).unwrap_or(true);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui::button_enabled(ui, !empty, &self.ui("validate"), ButtonKind::Primary).clicked() {
                if self.multi.as_mut().map(|m| m.validate_manual()).unwrap_or(false) {
                    self.begin_multi(now);
                }
            }
            if ui::button(ui, &self.ui("skip"), ButtonKind::Secondary).clicked() {
                if self.multi.as_mut().map(|m| m.skip_manual()).unwrap_or(false) {
                    self.begin_multi(now);
                }
            }
        });
    }

    fn status_row(&self, ui: &mut egui::Ui, entry: &FileEntry, status: &EntryStatus) {
        let (icon, color, text) = match status {
            EntryStatus::Success(..) => (ui::icons::CHECK, ui::tokens::SUCCESS, self.ui("status_success")),
            EntryStatus::Failure(..) => (ui::icons::ERROR, ui::tokens::DANGER, self.ui("status_failed")),
            EntryStatus::Error(msg) => (ui::icons::WARNING, ui::tokens::WARNING, msg.text(&self.lang)),
            EntryStatus::Skipped => (ui::icons::CLOSE, ui::tokens::TEXT_MUTED, self.ui("status_skipped")),
            EntryStatus::Computing => (ui::icons::REFRESH, ui::tokens::PROGRESS, self.ui("status_computing")),
            EntryStatus::Pending => (ui::icons::INFO, ui::tokens::TEXT_MUTED, self.ui("status_pending")),
        };
        ui.horizontal(|ui| {
            ui.label(ui::icons::rich(icon, 16.0).color(color));
            ui.label(Text::BodyStrong.rich(file_name(&entry.path)).color(ui::tokens::TEXT_PRIMARY));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Statut en texte en plus de la couleur (charte, Accessibilité).
                ui.label(Text::Label.rich(text).color(color));
            });
        });
    }

    fn show_verifying(&mut self, ui: &mut egui::Ui) {
        let now = ui.input(|i| i.time);
        let done = self.multi.as_ref().map(|m| m.all_done()).unwrap_or(true);
        if done {
            if let Some(m) = &mut self.multi {
                m.phase = MultiPhase::Results;
            }
            let (_, _, ko, err, _) = self.multi.as_ref().map(|m| m.stats()).unwrap_or_default();
            let ok = ko == 0 && err == 0;
            self.multi_action.finish(ok, now);
            self.logo.finish(ok, now);
            return;
        }
        let Some(multi) = &self.multi else { return };
        let active: Vec<&FileEntry> = multi.entries.iter().filter(|e| !matches!(*e.status.lock().unwrap(), EntryStatus::Skipped)).collect();
        let finished = active.iter().filter(|e| !matches!(*e.status.lock().unwrap(), EntryStatus::Pending | EntryStatus::Computing)).count();
        let total = active.len().max(1);
        ui.label(Text::Heading.rich(self.uif("verifying_title", &[("count", &active.len().to_string())])).color(ui::tokens::TEXT_PRIMARY));
        ui::progress_bar(ui, "multi_progress", finished as f32 / total as f32, &self.ui("computing"), &format!("{finished}/{total}"), &[]);
        ui::card(ui, |ui| {
            for entry in &multi.entries {
                let status = entry.status.lock().unwrap().clone();
                self.status_row(ui, entry, &status);
            }
        });
    }

    fn show_results(&mut self, ui: &mut egui::Ui) {
        let Some(multi) = &self.multi else { return };
        let (total, ok, ko, err, skipped) = multi.stats();
        let (tone, title) = if ko == 0 && err == 0 && ok > 0 {
            (Tone::Success, self.ui("all_files_intact"))
        } else if ko > 0 {
            (Tone::Error, self.ui("multi_failed"))
        } else if err > 0 {
            (Tone::Warning, self.ui("multi_errors"))
        } else {
            (Tone::Info, self.ui("verification_finished"))
        };
        let stats = self.uif("multi_stats", &[
            ("total", &total.to_string()),
            ("success", &ok.to_string()),
            ("failure", &ko.to_string()),
            ("error", &err.to_string()),
            ("skipped", &skipped.to_string()),
        ]);
        ui::banner(ui, tone, &title, Some(&stats));
        ui::card(ui, |ui| {
            for (i, entry) in multi.entries.iter().enumerate() {
                let status = entry.status.lock().unwrap().clone();
                self.status_row(ui, entry, &status);
                if let EntryStatus::Failure(expected, computed) = &status {
                    ui.label(Text::Overline.rich(self.ui("expected")).color(ui::tokens::TEXT_MUTED));
                    ui::copy_value(ui, ("exp", i), expected);
                    ui.label(Text::Overline.rich(self.ui("computed")).color(ui::tokens::TEXT_MUTED));
                    ui::copy_value(ui, ("comp", i), computed);
                }
                ui::row_separator(ui);
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui::button(ui, &self.ui("new_verification"), ButtonKind::Secondary).clicked() {
                self.reset();
            }
        });
    }
}

// =============================================================================
// Fenêtres : paramètres, langue, à propos, mise à jour
// =============================================================================

impl HashCheckerApp {
    fn show_windows(&mut self, ctx: &egui::Context) {
        self.settings_window(ctx);

        let languages: Vec<(String, String)> = lang::STEMS.iter().map(|s| (s.to_string(), lang::language_name(PACKS, s))).collect();
        let folder = self.data_dir.join("lang").display().to_string();
        let chosen = {
            let lang = &self.lang;
            ui::language_window(ctx, &mut self.show_language, &|k| lang.t(k), &self.lang.stem.clone(), &languages, &folder)
        };
        if let Some(stem) = chosen {
            self.set_language(ctx, stem);
        }

        let info = ui::AppInfo { name: "Hash Checker".into(), description: self.ui("app_description"), version: VERSION.into() };
        {
            let lang = &self.lang;
            ui::about_window(ctx, &mut self.show_about, &|k| lang.t(k), &info, LICENSE, REPOSITORY, AUTHORS);
        }

        if let Some(version) = self.update.as_ref().map(|r| r.version.clone()) {
            let installing = self.install_rx.is_some();
            let choice = {
                let lang = &self.lang;
                ui::update_window(ctx, &mut self.show_update, &|k| lang.t(k), &version, installing)
            };
            if choice == ui::UpdateChoice::Install && !installing {
                self.start_install();
            }
        }
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_settings;
        let title = self.ui("settings_title");
        let (on, off) = (self.ui("value_on"), self.ui("value_off"));
        let mut check_updates = self.settings.common.check_updates;
        let mut toggles: Vec<(&'static str, bool)> = Vec::new();
        let rows: Vec<(&'static str, String, Option<bool>)> = vec![
            ("windows", self.ui("integration_windows"), self.integration_status.windows_registry),
            ("nautilus", self.ui("integration_nautilus"), self.integration_status.linux_nautilus),
            ("kde", self.ui("integration_kde"), self.integration_status.linux_kde),
            ("thunar", self.ui("integration_thunar"), self.integration_status.linux_thunar),
        ];
        let (general, check_label, ctx_title, ctx_help, algos_label) =
            (self.ui("general"), self.ui("check_updates"), self.ui("context_integration"), self.ui("context_integration_help"), self.ui("algorithms"));
        let (about_label, log_label) = (self.kit("about"), self.kit("open_log"));
        let mut open_about = false;
        let mut open_log = false;
        ui::dialog(ctx, "settings", ui::icons::GEAR, &title, &mut open, |ui| {
            ui.label(Text::Overline.rich(&general).color(ui::tokens::TEXT_MUTED));
            ui::setting_row(ui, &check_label, None, |ui| {
                let l = if check_updates { on.clone() } else { off.clone() };
                ui::slider2(ui, &mut check_updates, &l);
            });
            ui.add_space(ui::tokens::SPACE_2);
            ui.label(Text::Overline.rich(&ctx_title).color(ui::tokens::TEXT_MUTED));
            ui.label(Text::Caption.rich(&ctx_help).color(ui::tokens::TEXT_SECONDARY));
            for (id, label, installed) in &rows {
                let Some(installed) = *installed else { continue };
                let mut value = installed;
                ui::setting_row(ui, label, None, |ui| {
                    let l = if value { on.clone() } else { off.clone() };
                    ui::slider2(ui, &mut value, &l);
                });
                if value != installed {
                    toggles.push((id, value));
                }
            }
            ui.add_space(ui::tokens::SPACE_2);
            ui.label(Text::Overline.rich(&algos_label).color(ui::tokens::TEXT_MUTED));
            ui.label(Text::Mono.rich(Algorithm::all().iter().map(|a| a.to_string()).collect::<Vec<_>>().join("  ")).color(ui::tokens::TEXT_SECONDARY));
        }, |ui| {
            if ui::button(ui, &about_label, ButtonKind::Secondary).clicked() {
                open_about = true;
            }
            if ui::button(ui, &log_label, ButtonKind::Quiet).clicked() {
                open_log = true;
            }
        });
        self.show_settings = open;
        if check_updates != self.settings.common.check_updates {
            self.settings.common.check_updates = check_updates;
            self.save_settings();
        }
        for (id, enable) in toggles {
            self.toggle_integration(id, enable);
        }
        if open_about {
            self.show_about = true;
        }
        if open_log {
            ui::open_path(&self.log_path.display().to_string());
        }
    }

    fn toggle_integration(&mut self, target: &str, enable: bool) {
        let exe = current_exe_path();
        let label = self.ui("context_menu_label");
        let result = match (target, enable) {
            ("windows", true) => install_windows(&exe, &label),
            ("windows", false) => uninstall_windows(),
            ("nautilus", true) => install_nautilus(&exe, &self.ui("nautilus_script_name")),
            ("nautilus", false) => uninstall_nautilus(),
            ("kde", true) => install_kde(&exe, &label),
            ("kde", false) => uninstall_kde(),
            ("thunar", true) => install_thunar(&exe, &label, &self.ui("context_menu_description")),
            ("thunar", false) => uninstall_thunar(),
            _ => Err("integration_unknown".to_string()),
        };
        match result {
            Ok(()) => {
                let key = if enable { "enabled_success" } else { "disabled_success" };
                log::info!("Intégration {target} : {}", if enable { "activée" } else { "désactivée" });
                self.toasts.push(Tone::Success, self.ui(key), "");
            }
            // `e` est une clé de langue (ex. integration_unsupported) ou un message du système.
            Err(e) => {
                let text = if self.lang.has(&format!("ui.{e}")) { self.ui(&e) } else { e.clone() };
                log::warn!("Intégration {target} : {e}");
                self.toasts.push(Tone::Error, self.ui("result_error"), text);
            }
        }
        self.integration_status = IntegrationStatus::detect();
    }
}

// =============================================================================
// Calculs (fils séparés)
// =============================================================================

fn run_single(target: PathBuf, expected: String, algo: Algorithm, state: Arc<Mutex<VerifyState>>) {
    *state.lock().unwrap() = match compute_hash(&target, &algo) {
        Ok(computed) => {
            let (exp, comp) = (expected.to_lowercase(), computed.to_lowercase());
            if exp == comp { VerifyState::Success(exp, comp) } else { VerifyState::Failure(exp, comp) }
        }
        Err(e) => VerifyState::Error(Msg::raw(e.to_string())),
    };
}

fn run_entry_direct(target: PathBuf, expected: String, algo: Algorithm, status: Arc<Mutex<EntryStatus>>) {
    *status.lock().unwrap() = match compute_hash(&target, &algo) {
        Ok(computed) => {
            let (exp, comp) = (expected.to_lowercase(), computed.to_lowercase());
            if exp == comp { EntryStatus::Success(exp, comp) } else { EntryStatus::Failure(exp, comp) }
        }
        Err(e) => EntryStatus::Error(Msg::raw(e.to_string())),
    };
}

fn run_entry_from_file(target: PathBuf, cs_path: PathBuf, fallback: Algorithm, status: Arc<Mutex<EntryStatus>>) {
    let entries = match parse_checksum_file(&cs_path) {
        Ok(e) => e,
        Err(e) => {
            *status.lock().unwrap() = EntryStatus::Error(e);
            return;
        }
    };
    match find_entry_for_file(&entries, &target) {
        Some(entry) => {
            let algo = entry.algorithm.clone().unwrap_or(fallback);
            run_entry_direct(target, entry.hash.clone(), algo, status);
        }
        None => {
            *status.lock().unwrap() = EntryStatus::Error(Msg::new("file_not_in_checksum").arg("file", file_name(&target)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_input_with_prefix_selects_algorithm() {
        let (hash, algo) = parse_hash_input(" SHA512:ABC ", Algorithm::Md5);
        assert_eq!(hash, "abc");
        assert_eq!(algo, Algorithm::Sha512);
        let (hash, algo) = parse_hash_input("def", Algorithm::Md5);
        assert_eq!((hash.as_str(), algo), ("def", Algorithm::Md5));
    }
}
