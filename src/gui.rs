// =============================================================================
// gui.rs — Interface graphique (egui / eframe)
//
// Ce module dessine toute l'interface utilisateur avec la bibliothèque egui
// (mode immédiat : l'interface est redessinée entièrement à chaque frame).
//
// Deux modes d'interface coexistent :
//
//   Mode fichier unique (drag & drop, lancement sans argument)
//   ┌─────────────────────────────────────────┐
//   │  Hash Checker              [Paramètres] │
//   ├─────────────────────────────────────────┤
//   │  [Zone de dépôt / sélection de fichier] │
//   │  [Automatique] [Fichier] [Hash manuel]  │
//   │  [Vérifier l'intégrité]                 │
//   │  ┌ Résultat ────────────────────────┐   │
//   │  │ VERIFICATION REUSSIE / ECHOUEE  │   │
//   │  └──────────────────────────────────┘   │
//   └─────────────────────────────────────────┘
//
//   Mode multi-fichiers (clic droit sur plusieurs fichiers)
//   4 phases successives :
//     1. Review      — liste des fichiers, cases à cocher
//     2. ManualInput — saisie du hash pour les fichiers sans checksum auto
//     3. Verifying   — calcul en cours (threads séparés)
//     4. Results     — tableau récapitulatif coloré
//
// L'IPC (src/ipc.rs) envoie les fichiers supplémentaires depuis les processus
// suivants via un channel mpsc. La GUI les récupère à chaque frame.
// =============================================================================

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;

use eframe::egui::{self, Color32, FontId, RichText, ScrollArea, Stroke, Vec2};

use crate::checksum::{find_checksum_file, find_entry_for_file, parse_checksum_file};
use crate::hasher::{Algorithm, compute_hash};
use crate::integration::{
    IntegrationStatus, current_exe_path, install_kde, install_nautilus, install_thunar,
    install_windows, uninstall_kde, uninstall_nautilus, uninstall_thunar, uninstall_windows,
};
use crate::language::LanguageManager;

// Informations affichées dans le panneau "À propos"
const VERSION: &str = env!("CARGO_PKG_VERSION");
const AUTHORS: &str = "Rusty-Suite.com";
const PRODUCTNAME: &str = "Hash Checker";
const GITHUB: &str = "https://github.com/rusty-suite/hash_checker";
const STATUS_DOT: &str = "";

// =============================================================================
// Types partagés — mode fichier unique
// =============================================================================

/// État de la vérification — partagé entre le thread de calcul et l'UI.
#[derive(Debug, Clone, PartialEq)]
enum VerifyState {
    Idle,                    // Aucune vérification en cours ou demandée
    Computing,               // Calcul en cours dans un thread séparé
    Success(String, String), // (hash_attendu, hash_calculé) — identiques
    Failure(String, String), // (hash_attendu, hash_calculé) — différents
    Error(String),           // Message d'erreur (fichier illisible, etc.)
}

/// Mode de saisie du hash de référence (onglets du mode fichier unique).
#[derive(Debug, Clone, PartialEq)]
enum InputMode {
    Auto,       // Recherche automatique d'un fichier checksum dans le répertoire
    FileManual, // L'utilisateur sélectionne manuellement le fichier checksum
    HashManual, // L'utilisateur saisit directement la valeur du hash
}

/// Panneau affiché dans la fenêtre principale.
#[derive(Debug, Clone, PartialEq)]
enum Panel {
    Main,     // Interface principale de vérification
    Settings, // Panneau paramètres (à propos + intégration OS)
}

// =============================================================================
// Types exclusifs au mode multi-fichiers
// =============================================================================

/// Origine du hash de référence pour un fichier dans la liste multi.
#[derive(Debug, Clone)]
enum HashSource {
    /// Fichier checksum détecté automatiquement dans le même répertoire.
    AutoFound(PathBuf),
    /// Fichier checksum sélectionné manuellement par l'utilisateur.
    ManualFile(PathBuf),
    /// Hash saisi directement par l'utilisateur (format brut ou "algo:hash").
    ManualHash(String, Algorithm),
    /// Aucun checksum trouvé : l'utilisateur doit saisir le hash manuellement.
    NeedsInput,
    /// L'utilisateur a choisi de passer ce fichier.
    Skipped,
}

/// État de vérification d'un fichier individuel dans la liste multi.
/// Partagé entre le thread de calcul et l'UI via Arc<Mutex<>>.
#[derive(Debug, Clone, PartialEq)]
enum EntryStatus {
    Pending,                 // Pas encore traité
    Computing,               // Calcul en cours dans un thread séparé
    Success(String, String), // (hash_attendu, hash_calculé) — identiques
    Failure(String, String), // (hash_attendu, hash_calculé) — différents
    Error(String),           // Erreur (fichier illisible, checksum introuvable, etc.)
    Skipped,                 // Ignoré (décoché ou passé par l'utilisateur)
}

/// Une entrée dans la liste de vérification multi-fichiers.
struct FileEntry {
    path: PathBuf,
    /// Coché = sera vérifié, décoché = sera ignoré (phase Review).
    selected: bool,
    /// D'où vient le hash de référence pour ce fichier.
    source: HashSource,
    /// Résultat de la vérification, partagé avec le thread de calcul.
    status: Arc<Mutex<EntryStatus>>,
}

/// Phase actuelle du workflow multi-fichiers.
#[derive(Debug, Clone, PartialEq)]
enum MultiPhase {
    /// L'utilisateur examine la liste et coche/décoche les fichiers.
    Review,
    /// Saisie du hash pour les fichiers sans checksum auto, un par un.
    ManualInput,
    /// Calcul des empreintes en cours (un thread par fichier).
    Verifying,
    /// Tous les calculs terminés — affichage du tableau récapitulatif.
    Results,
}

// -----------------------------------------------------------------------------
// État complet du mode multi-fichiers
// -----------------------------------------------------------------------------
struct MultiFileState {
    entries: Vec<FileEntry>,
    phase: MultiPhase,

    // File d'attente des indices (dans `entries`) nécessitant une saisie manuelle.
    // Calculée lors de la transition Review → ManualInput.
    manual_queue: Vec<usize>,
    // Position courante dans manual_queue (quel fichier on demande en ce moment).
    manual_pos: usize,

    // Champs temporaires pour l'UI de saisie manuelle (réinitialisés à chaque fichier).
    input_hash: String,
    input_algo: Algorithm,
}

impl MultiFileState {
    // -------------------------------------------------------------------------
    // Construction
    // -------------------------------------------------------------------------

    /// Crée un état multi-fichiers depuis une liste de chemins.
    /// Auto-détecte les fichiers checksum pour chaque chemin.
    fn from_paths(paths: Vec<PathBuf>) -> Self {
        let entries = paths
            .into_iter()
            .map(|path| {
                // Cherche un fichier checksum dans le même répertoire
                let source = match find_checksum_file(&path) {
                    Some(cs) => HashSource::AutoFound(cs),
                    None => HashSource::NeedsInput, // L'utilisateur devra saisir le hash
                };
                FileEntry {
                    path,
                    selected: true, // Tous cochés par défaut
                    source,
                    status: Arc::new(Mutex::new(EntryStatus::Pending)),
                }
            })
            .collect();

        Self {
            entries,
            phase: MultiPhase::Review,
            manual_queue: Vec::new(),
            manual_pos: 0,
            input_hash: String::new(),
            input_algo: Algorithm::Sha256,
        }
    }

    // -------------------------------------------------------------------------
    // Transitions de phase
    // -------------------------------------------------------------------------

    /// Passe de Review à ManualInput (si des fichiers nécessitent une saisie)
    /// ou directement à Verifying (si tout est déjà résolu automatiquement).
    /// Retourne true si la phase est maintenant Verifying (déclenche les threads).
    fn advance_from_review(&mut self) -> bool {
        // Collecte les indices des fichiers sélectionnés qui n'ont pas de checksum auto
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
            // Tout est prêt : pas besoin de saisie manuelle
            self.phase = MultiPhase::Verifying;
            true
        } else {
            self.phase = MultiPhase::ManualInput;
            false
        }
    }

    /// Valide la saisie manuelle pour le fichier courant et avance au suivant.
    /// Retourne true si tous les fichiers manuels ont été traités → Verifying.
    fn validate_manual(&mut self) -> bool {
        if self.manual_pos < self.manual_queue.len() {
            let idx = self.manual_queue[self.manual_pos];
            // Parse le format "algo:hash" ou hash brut (même logique que mode single)
            let raw = self.input_hash.trim().to_lowercase();
            let (hash, algo) = if let Some(pos) = raw.find(':') {
                let prefix = &raw[..pos];
                let h = raw[pos + 1..].to_string();
                if let Some(detected) = Algorithm::from_str(prefix) {
                    (h, detected)
                } else {
                    (raw.clone(), self.input_algo.clone())
                }
            } else {
                (raw.clone(), self.input_algo.clone())
            };
            self.entries[idx].source = HashSource::ManualHash(hash, algo);
        }
        self.next_manual()
    }

    /// Marque le fichier courant comme ignoré et avance.
    /// Retourne true si tous les fichiers manuels ont été traités → Verifying.
    fn skip_manual(&mut self) -> bool {
        if self.manual_pos < self.manual_queue.len() {
            let idx = self.manual_queue[self.manual_pos];
            self.entries[idx].source = HashSource::Skipped;
        }
        self.next_manual()
    }

    /// Avance la position manuelle. Passe en Verifying si la queue est épuisée.
    /// Retourne true si on passe en Verifying.
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

    // -------------------------------------------------------------------------
    // Helpers pour l'affichage
    // -------------------------------------------------------------------------

    /// True quand tous les threads de vérification ont terminé leur travail.
    fn all_done(&self) -> bool {
        self.entries.iter().all(|e| {
            !matches!(
                *e.status.lock().unwrap(),
                EntryStatus::Pending | EntryStatus::Computing
            )
        })
    }

    /// Compte les résultats pour l'affichage du résumé.
    /// Retourne (total_vérifiés, succès, échecs, erreurs, ignorés).
    fn stats(&self) -> (usize, usize, usize, usize, usize) {
        let mut success = 0usize;
        let mut failure = 0usize;
        let mut error = 0usize;
        let mut skipped = 0usize;
        for e in &self.entries {
            match *e.status.lock().unwrap() {
                EntryStatus::Success(_, _) => success += 1,
                EntryStatus::Failure(_, _) => failure += 1,
                EntryStatus::Error(_) => error += 1,
                EntryStatus::Skipped => skipped += 1,
                _ => {}
            }
        }
        (success + failure + error, success, failure, error, skipped)
    }

    /// Ajoute un fichier à la liste si pas déjà présent (IPC, phase Review uniquement).
    /// Retourne true si le fichier a été ajouté.
    fn add_if_new(&mut self, path: PathBuf) -> bool {
        if self.phase != MultiPhase::Review {
            return false; // Trop tard pour ajouter des fichiers
        }
        if self.entries.iter().any(|e| e.path == path) {
            return false; // Doublon
        }
        let source = match find_checksum_file(&path) {
            Some(cs) => HashSource::AutoFound(cs),
            None => HashSource::NeedsInput,
        };
        self.entries.push(FileEntry {
            path,
            selected: true,
            source,
            status: Arc::new(Mutex::new(EntryStatus::Pending)),
        });
        true
    }
}

// =============================================================================
// Structure principale de l'application
// =============================================================================

pub struct HashCheckerApp {
    // ── Champs du mode fichier unique ─────────────────────────────────────────
    target_file: Option<PathBuf>,
    input_mode: InputMode,
    checksum_file: Option<PathBuf>,
    auto_checksum_found: Option<PathBuf>,
    manual_hash: String,
    selected_algo: Algorithm,
    state: Arc<Mutex<VerifyState>>,
    drag_hover: bool,

    // ── Panneau actif (main ou paramètres) ───────────────────────────────────
    active_panel: Panel,

    // ── Langues Rusty Suite ──────────────────────────────────────────────────
    language: LanguageManager,
    show_language_window: bool,
    language_message: Option<(String, bool)>,
    language_repo_rx: Option<Receiver<Result<Vec<String>, String>>>,
    language_repo_loading: bool,
    language_repo_status: String,
    language_repo_ok: Option<bool>,
    language_repo_loaded_once: bool,

    // ── Intégration OS ────────────────────────────────────────────────────────
    integration_status: IntegrationStatus,
    integration_message: Option<(String, bool)>,

    // ── IPC : reçoit les chemins envoyés par d'autres instances ──────────────
    // None si le serveur IPC n'a pas pu démarrer (cas rare).
    ipc_rx: Option<Receiver<PathBuf>>,

    // ── Mode multi-fichiers ───────────────────────────────────────────────────
    // Some(_) = mode multi actif, None = mode fichier unique.
    multi: Option<MultiFileState>,
}

impl Default for HashCheckerApp {
    fn default() -> Self {
        Self {
            target_file: None,
            input_mode: InputMode::Auto,
            checksum_file: None,
            auto_checksum_found: None,
            manual_hash: String::new(),
            selected_algo: Algorithm::Sha256,
            state: Arc::new(Mutex::new(VerifyState::Idle)),
            drag_hover: false,
            active_panel: Panel::Main,
            language: LanguageManager::initialize(),
            show_language_window: false,
            language_message: None,
            language_repo_rx: None,
            language_repo_loading: false,
            language_repo_status: String::new(),
            language_repo_ok: None,
            language_repo_loaded_once: false,
            integration_status: IntegrationStatus::detect(),
            integration_message: None,
            ipc_rx: None,
            multi: None,
        }
    }
}

impl HashCheckerApp {
    fn t(&self, key: &str) -> String {
        self.language.text(key)
    }

    fn tr(&self, key: &str, replacements: &[(&str, String)]) -> String {
        self.language.text_replace(key, replacements)
    }

    /// Lancement sans fichier (GUI vide, drag & drop).
    pub fn new_with_ipc(
        _cc: &eframe::CreationContext<'_>,
        ipc_rx: Option<Receiver<PathBuf>>,
    ) -> Self {
        Self {
            ipc_rx,
            ..Self::default()
        }
    }

    /// Rétro-compatibilité : sans IPC (ne devrait plus être appelé directement).
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        Self::default()
    }

    /// Lancement depuis le clic droit avec une liste de fichiers.
    /// Démarre directement en mode multi-fichiers.
    pub fn with_files(paths: Vec<PathBuf>, ipc_rx: Receiver<PathBuf>) -> Self {
        Self {
            multi: Some(MultiFileState::from_paths(paths)),
            ipc_rx: Some(ipc_rx),
            ..Self::default()
        }
    }

    /// Rétro-compatibilité : un seul fichier sans IPC.
    pub fn with_file(path: PathBuf) -> Self {
        let mut app = Self::default();
        app.set_target_file(path);
        app
    }

    // ── Gestion du fichier cible (mode single) ────────────────────────────────

    fn set_target_file(&mut self, path: PathBuf) {
        self.auto_checksum_found = find_checksum_file(&path);
        self.target_file = Some(path);
        self.checksum_file = None;
        self.manual_hash.clear();
        self.input_mode = if self.auto_checksum_found.is_some() {
            InputMode::Auto
        } else {
            InputMode::FileManual
        };
        *self.state.lock().unwrap() = VerifyState::Idle;
    }

    // ── Gestion de l'arrivée d'un fichier via IPC ─────────────────────────────

    /// Ajoute un fichier reçu via IPC à l'état approprié.
    ///
    /// - Si le mode multi est actif en phase Review : ajoute à la liste.
    /// - Si le mode fichier unique est actif : bascule en multi avec les 2 fichiers.
    /// - Si la fenêtre est vide : ouvre directement en multi avec ce fichier.
    fn receive_ipc_file(&mut self, path: PathBuf) {
        if let Some(multi) = &mut self.multi {
            // Multi déjà actif : tente d'ajouter (ignoré si pas en Review)
            multi.add_if_new(path);
        } else {
            // Bascule en mode multi en portant l'éventuel fichier déjà chargé
            let mut paths = Vec::new();
            if let Some(existing) = self.target_file.take() {
                paths.push(existing);
            }
            paths.push(path);
            self.multi = Some(MultiFileState::from_paths(paths));
            // Réinitialise l'état single-file
            self.input_mode = InputMode::Auto;
            self.auto_checksum_found = None;
            self.checksum_file = None;
            self.manual_hash.clear();
            *self.state.lock().unwrap() = VerifyState::Idle;
        }
    }

    // ── Vérification mode fichier unique ──────────────────────────────────────

    fn start_verification(&mut self) {
        let Some(target) = self.target_file.clone() else {
            return;
        };
        *self.state.lock().unwrap() = VerifyState::Computing;
        let state = Arc::clone(&self.state);

        match &self.input_mode {
            InputMode::HashManual => {
                let raw = self.manual_hash.trim().to_lowercase();
                // Accepte les formats "algo:hash" comme "sha256:abc123..."
                let (expected, algo) = if let Some(pos) = raw.find(':') {
                    let prefix = &raw[..pos];
                    let hash = raw[pos + 1..].to_string();
                    if let Some(detected) = Algorithm::from_str(prefix) {
                        (hash, detected)
                    } else {
                        (raw.clone(), self.selected_algo.clone())
                    }
                } else {
                    (raw.clone(), self.selected_algo.clone())
                };
                if expected.is_empty() {
                    *state.lock().unwrap() =
                        VerifyState::Error("Veuillez entrer une valeur de hash.".to_string());
                    return;
                }
                thread::spawn(move || run_single_verification(target, expected, algo, state));
            }
            InputMode::Auto | InputMode::FileManual => {
                let checksum_path = match &self.input_mode {
                    InputMode::Auto => self.auto_checksum_found.clone(),
                    InputMode::FileManual => self.checksum_file.clone(),
                    _ => unreachable!(),
                };
                let Some(cs_path) = checksum_path else {
                    *state.lock().unwrap() =
                        VerifyState::Error("Aucun fichier checksum sélectionné.".to_string());
                    return;
                };
                let algo_fallback = self.selected_algo.clone();
                thread::spawn(move || {
                    let entries = match parse_checksum_file(&cs_path) {
                        Ok(e) => e,
                        Err(e) => {
                            *state.lock().unwrap() = VerifyState::Error(e);
                            return;
                        }
                    };
                    let entry = match find_entry_for_file(&entries, &target) {
                        Some(e) => e.clone(),
                        None => {
                            *state.lock().unwrap() = VerifyState::Error(format!(
                                "Fichier '{}' non trouvé dans le checksum.",
                                target.file_name().unwrap_or_default().to_string_lossy()
                            ));
                            return;
                        }
                    };
                    let algo = entry.algorithm.unwrap_or(algo_fallback);
                    run_single_verification(target, entry.hash, algo, state);
                });
            }
        }
    }

    // ── Vérification mode multi-fichiers ──────────────────────────────────────

    /// Lance les threads de vérification pour tous les fichiers sélectionnés.
    /// Appelé lors du passage en phase Verifying.
    fn start_multi_verification(&mut self) {
        let Some(multi) = &mut self.multi else {
            return;
        };

        for entry in &mut multi.entries {
            let status_arc = Arc::clone(&entry.status);

            // Fichiers non sélectionnés ou explicitement ignorés → Skipped direct
            if !entry.selected
                || matches!(entry.source, HashSource::Skipped | HashSource::NeedsInput)
            {
                *status_arc.lock().unwrap() = EntryStatus::Skipped;
                continue;
            }

            match &entry.source {
                HashSource::AutoFound(cs) | HashSource::ManualFile(cs) => {
                    *status_arc.lock().unwrap() = EntryStatus::Computing;
                    let target = entry.path.clone();
                    let cs_path = cs.clone();
                    let algo_fallback = Algorithm::Sha256;
                    thread::spawn(move || {
                        run_entry_from_file(target, cs_path, algo_fallback, status_arc);
                    });
                }
                HashSource::ManualHash(hash, algo) => {
                    *status_arc.lock().unwrap() = EntryStatus::Computing;
                    let target = entry.path.clone();
                    let expected = hash.clone();
                    let algo = algo.clone();
                    thread::spawn(move || {
                        run_entry_direct(target, expected, algo, status_arc);
                    });
                }
                // Cas déjà traités ci-dessus
                HashSource::Skipped | HashSource::NeedsInput => {}
            }
        }
    }
}

// =============================================================================
// Boucle principale egui
// =============================================================================

impl eframe::App for HashCheckerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // ── 1. Drag & drop ────────────────────────────────────────────────────
        ctx.input(|i| {
            self.drag_hover = !i.raw.hovered_files.is_empty();
            if !i.raw.dropped_files.is_empty() {
                if let Some(path) = i.raw.dropped_files[0].path.clone() {
                    if self.multi.is_some() {
                        // Multi actif : ajoute à la liste (ignoré si pas en Review)
                        if let Some(multi) = &mut self.multi {
                            multi.add_if_new(path);
                        }
                    } else {
                        // Mode single : charge le fichier normalement
                        self.set_target_file(path);
                        self.active_panel = Panel::Main;
                    }
                }
            }
        });

        // ── 2. Réception des fichiers IPC ─────────────────────────────────────
        // On vide entièrement le channel à chaque frame pour ne manquer aucun
        // fichier arrivé pendant le calcul ou la saisie manuelle.
        let ipc_paths: Vec<PathBuf> = if let Some(rx) = &self.ipc_rx {
            std::iter::from_fn(|| rx.try_recv().ok()).collect()
        } else {
            vec![]
        };
        for path in ipc_paths {
            self.receive_ipc_file(path);
        }

        // ── 3. Style global ───────────────────────────────────────────────────
        let mut style = (*ctx.style()).clone();
        style.spacing.item_spacing = Vec2::new(8.0, 8.0);
        style.spacing.button_padding = Vec2::new(14.0, 7.0);
        ctx.set_style(style);

        // ── 4. Rendu ──────────────────────────────────────────────────────────
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(10.0);

            // Barre de titre
            ui.horizontal(|ui| {
                ui.add_space(4.0);
                ui.label(
                    RichText::new("Hash Checker")
                        .font(FontId::proportional(26.0))
                        .strong()
                        .color(Color32::from_rgb(100, 180, 255)),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let settings_btn = egui::Button::new(
                        RichText::new(if self.active_panel == Panel::Settings {
                            self.t("close_settings")
                        } else {
                            self.t("settings")
                        })
                        .font(FontId::proportional(13.0))
                        .color(Color32::from_rgb(180, 180, 200)),
                    )
                    .fill(Color32::from_rgba_premultiplied(50, 50, 70, 180));

                    if ui.add(settings_btn).clicked() {
                        self.active_panel = if self.active_panel == Panel::Settings {
                            Panel::Main
                        } else {
                            Panel::Settings
                        };
                    }

                    let lang_btn = egui::Button::new(
                        RichText::new("🌐")
                            .font(FontId::proportional(14.0))
                            .color(Color32::from_rgb(180, 180, 200)),
                    )
                    .fill(Color32::from_rgba_premultiplied(50, 50, 70, 180))
                    .min_size(Vec2::new(28.0, 26.0));
                    if ui
                        .add(lang_btn)
                        .on_hover_text(self.tr(
                            "language_tooltip",
                            &[("lang", self.language.active_stem.clone())],
                        ))
                        .clicked()
                    {
                        self.show_language_window = true;
                        if !self.language_repo_loaded_once {
                            self.refresh_language_repo();
                        }
                    }
                });
            });

            ui.separator();
            ui.add_space(6.0);

            match self.active_panel {
                Panel::Main => self.show_main(ui),
                Panel::Settings => self.show_settings(ui),
            }

            // Overlay de survol drag & drop
            if self.drag_hover {
                ui.painter().rect_filled(
                    ui.clip_rect(),
                    8.0,
                    Color32::from_rgba_premultiplied(100, 180, 255, 30),
                );
            }
        });

        self.poll_language_repo();
        self.show_language_dialog(ctx);
        self.show_network_error_dialog(ctx);

        // Demande un redessin si un calcul est en cours
        let needs_repaint = if let Some(multi) = &self.multi {
            multi.phase == MultiPhase::Verifying
        } else {
            *self.state.lock().unwrap() == VerifyState::Computing
        };
        if needs_repaint {
            ctx.request_repaint();
        }
    }
}

// =============================================================================
// Fenêtres langues et erreurs réseau
// =============================================================================

impl HashCheckerApp {
    fn refresh_language_repo(&mut self) {
        if self.language_repo_loading {
            return;
        }

        let (tx, rx) = mpsc::channel();
        self.language_repo_rx = Some(rx);
        self.language_repo_loading = true;
        self.language_repo_status = self.language.text("repo_loading");
        self.language_repo_ok = None;
        let ui_texts = self.language.ui_texts();
        thread::spawn(move || {
            let result = LanguageManager::fetch_remote_languages(ui_texts);
            let _ = tx.send(result);
        });
    }

    fn poll_language_repo(&mut self) {
        let result = self
            .language_repo_rx
            .as_ref()
            .and_then(|rx| rx.try_recv().ok());

        if let Some(result) = result {
            self.language_repo_rx = None;
            self.language_repo_loading = false;
            self.language_repo_loaded_once = true;
            match result {
                Ok(files) => {
                    let count = files.len();
                    self.language.set_remote_files(files);
                    self.language_repo_status =
                        self.language
                            .text_replace("repo_available", &[("count", count.to_string())]);
                    self.language_repo_ok = Some(true);
                    self.language.network_error = false;
                }
                Err(e) => {
                    self.language_repo_status =
                        self.language.text_replace("repo_unavailable", &[("error", e)]);
                    self.language_repo_ok = Some(false);
                }
            }
        }
    }

    fn show_language_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_language_window {
            return;
        }

        let mut open = self.show_language_window;
        egui::Window::new("Language / Langue")
            .collapsible(false)
            .resizable(false)
            .fixed_size(Vec2::new(660.0, 500.0))
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing = Vec2::new(6.0, 7.0);

                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("{}:", self.language.text("active")))
                            .color(Color32::from_rgb(125, 125, 135)),
                    );
                    ui.label(
                        RichText::new(&self.language.active_name)
                        .color(Color32::WHITE)
                        .strong(),
                    );
                });

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);

                ui.horizontal(|ui| {
                    let (dot, color, text) = if self.language_repo_loading {
                        (STATUS_DOT, Color32::from_rgb(120, 180, 255), self.language.text("repo_loading"))
                    } else if self.language_repo_ok == Some(false) || self.language_repo_ok.is_none() {
                        (
                            STATUS_DOT,
                            if self.language_repo_ok.is_none() {
                                Color32::from_rgb(150, 150, 155)
                            } else {
                                Color32::from_rgb(160, 130, 70)
                            },
                            if self.language_repo_status.is_empty() {
                                self.language.text("repo_not_refreshed")
                            } else {
                                self.language_repo_status.clone()
                            },
                        )
                    } else {
                        (
                            STATUS_DOT,
                            Color32::from_rgb(35, 140, 55),
                            if self.language_repo_status.is_empty() {
                                self.language.text("repo_not_refreshed")
                            } else {
                                self.language_repo_status.clone()
                            },
                        )
                    };

                    let status_line = self.tr(
                        "repo_status_line",
                        &[("dot", dot.to_string()), ("status", text)],
                    );
                    let (dot_rect, _) =
                        ui.allocate_exact_size(Vec2::new(14.0, 18.0), egui::Sense::hover());
                    ui.painter()
                        .circle_filled(dot_rect.center(), 7.0, color);
                    ui.label(
                        RichText::new(status_line.trim_start())
                            .color(color)
                            .font(FontId::proportional(16.0)),
                    );
                });

                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Available files:")
                            .color(Color32::from_rgb(105, 105, 112))
                            .font(FontId::proportional(12.0)),
                    );
                    if ui
                        .add_enabled(
                            !self.language_repo_loading,
                            egui::Button::new(self.language.text("refresh"))
                                .min_size(Vec2::new(66.0, 20.0)),
                        )
                        .clicked()
                    {
                        self.refresh_language_repo();
                    }
                });

                ui.add_space(4.0);

                ScrollArea::vertical().max_height(245.0).show(ui, |ui| {
                    for pack in self.language.available_languages() {
                        let selected = pack.stem == self.language.active_stem;
                        let mut label = pack.display_name.clone();
                        let code = language_display_code(&pack.stem);
                        if pack.is_default {
                            label.push_str(&format!(" [{}]", self.language.default_badge));
                        }
                        label.push_str(if pack.is_local { " [local]" } else { " [repo]" });
                        if pack.is_remote && pack.is_local {
                            label.push_str(" [git]");
                        } else if pack.is_remote {
                            label = label.replace("[repo]", "[git]");
                        }

                        let row_height = 30.0;
                        let (rect, response) = ui.allocate_exact_size(
                            Vec2::new(ui.available_width(), row_height),
                            egui::Sense::click(),
                        );
                        let bg = if selected {
                            Color32::from_rgb(0, 112, 145)
                        } else {
                            Color32::from_rgb(58, 58, 58)
                        };
                        ui.painter().rect_filled(rect, 2.0, bg);

                        let text_color = if selected {
                            Color32::WHITE
                        } else {
                            Color32::from_rgb(172, 172, 172)
                        };
                        ui.painter().text(
                            rect.left_center() + Vec2::new(8.0, 0.0),
                            egui::Align2::LEFT_CENTER,
                            label,
                            FontId::proportional(15.0),
                            text_color,
                        );
                        ui.painter().text(
                            rect.right_center() - Vec2::new(10.0, 0.0),
                            egui::Align2::RIGHT_CENTER,
                            code,
                            FontId::monospace(14.0),
                            if selected {
                                Color32::from_rgb(220, 245, 255)
                            } else {
                                Color32::from_rgb(145, 145, 145)
                            },
                        );

                        if response.clicked() {
                            self.language_message =
                                Some(match self.language.select_language(&pack) {
                                    Ok(_) => {
                                        self.language.network_error = false;
                                        let msg = self.language.text("language_loaded");
                                        ctx.request_repaint();
                                        (msg, true)
                                    }
                                    Err(e) => (e, false),
                                });
                        }
                    }
                });

                ui.add_space(6.0);
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Folder:")
                            .color(Color32::from_rgb(105, 105, 112))
                            .font(FontId::proportional(12.0)),
                    );
                    ui.label(
                        RichText::new(self.language.lang_dir.display().to_string())
                            .color(Color32::from_rgb(170, 170, 175))
                            .font(FontId::monospace(14.0)),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(self.language.text("open")).clicked() {
                            self.language.open_lang_folder();
                        }
                    });
                });

                if let Some((msg, ok)) = &self.language_message {
                    ui.label(RichText::new(msg).color(if *ok {
                        Color32::from_rgb(80, 220, 120)
                    } else {
                        Color32::from_rgb(255, 120, 80)
                    }));
                }

                ui.add_space(36.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(self.language.text("close")).clicked() {
                        self.show_language_window = false;
                    }
                });
            });
        self.show_language_window = open && self.show_language_window;
    }

    fn show_network_error_dialog(&mut self, ctx: &egui::Context) {
        if !self.language.network_error {
            return;
        }

        egui::Window::new(self.language.text("language_window_title"))
            .collapsible(false)
            .resizable(false)
            .fixed_size(Vec2::new(360.0, 110.0))
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(
                        self.language.text("network_error"),
                    )
                    .color(Color32::from_rgb(255, 180, 80)),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("OK").clicked() {
                        self.language.network_error = false;
                    }
                });
            });
    }
}

fn language_display_code(stem: &str) -> String {
    let parts: Vec<&str> = stem.split('_').collect();
    if parts.len() == 2 {
        let country = parts[0];
        let lang = parts[1];
        match country {
            "EN" | "FR" | "DE" | "IT" => lang.to_string(),
            _ => format!("{}_{}", lang, country),
        }
    } else {
        stem.to_string()
    }
}

// =============================================================================
// Panneau principal — dispatch single / multi
// =============================================================================

impl HashCheckerApp {
    fn show_main(&mut self, ui: &mut egui::Ui) {
        if self.multi.is_some() {
            self.show_multi_file_view(ui);
        } else {
            self.show_single_file_view(ui);
        }
    }

    // ── Mode fichier unique ───────────────────────────────────────────────────

    fn show_single_file_view(&mut self, ui: &mut egui::Ui) {
        self.show_file_drop_zone(ui);
        ui.add_space(8.0);
        if self.target_file.is_some() {
            self.show_input_section(ui);
            ui.add_space(8.0);
            self.show_verify_button(ui);
            ui.add_space(8.0);
            self.show_result(ui);
        }
    }

    fn show_file_drop_zone(&mut self, ui: &mut egui::Ui) {
        let has_file = self.target_file.is_some();
        let label = if has_file {
            self.target_file
                .as_ref()
                .unwrap()
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string()
        } else {
            self.t("drop_file")
        };

        let border_color = if self.drag_hover {
            Color32::from_rgb(100, 180, 255)
        } else if has_file {
            Color32::from_rgb(80, 200, 120)
        } else {
            Color32::from_rgb(100, 100, 120)
        };

        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 70.0), egui::Sense::click());
        ui.painter().rect(
            rect,
            8.0,
            Color32::from_rgba_premultiplied(30, 30, 40, 200),
            Stroke::new(2.0, border_color),
            egui::StrokeKind::Outside,
        );
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            &label,
            FontId::proportional(14.0),
            if has_file {
                Color32::WHITE
            } else {
                Color32::from_rgb(160, 160, 180)
            },
        );
        if response.clicked() {
            if let Some(path) = rfd::FileDialog::new()
                .set_title(self.t("select_file_title"))
                .pick_file()
            {
                self.set_target_file(path);
            }
        }
    }

    fn show_input_section(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let modes = [
                (InputMode::Auto, self.t("mode_auto")),
                (InputMode::FileManual, self.t("mode_checksum_file")),
                (InputMode::HashManual, self.t("mode_manual_hash")),
            ];
            for (mode, label) in &modes {
                let selected = &self.input_mode == mode;
                let btn = egui::Button::new(RichText::new(label).color(Color32::WHITE)).fill(
                    if selected {
                        Color32::from_rgb(50, 100, 200)
                    } else {
                        Color32::from_rgba_premultiplied(50, 50, 70, 150)
                    },
                );
                if ui.add(btn).clicked() {
                    self.input_mode = mode.clone();
                    *self.state.lock().unwrap() = VerifyState::Idle;
                }
            }
        });

        ui.add_space(6.0);

        match &self.input_mode {
            InputMode::Auto => {
                if let Some(cs) = &self.auto_checksum_found {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(self.t("checksum_detected"))
                                .color(Color32::from_rgb(160, 160, 180)),
                        );
                        ui.label(
                            RichText::new(
                                cs.file_name()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .to_string(),
                            )
                            .color(Color32::from_rgb(80, 200, 120))
                            .strong(),
                        );
                    });
                } else {
                    ui.colored_label(
                        Color32::from_rgb(255, 180, 60),
                        self.t("no_checksum_auto"),
                    );
                }
            }
            InputMode::FileManual => {
                ui.horizontal(|ui| {
                    let cs_label = self
                        .checksum_file
                        .as_ref()
                        .map(|p| {
                            p.file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .to_string()
                        })
                        .unwrap_or_else(|| self.t("no_file_selected"));
                    ui.label(RichText::new(cs_label).color(Color32::from_rgb(200, 200, 220)));
                    if ui.button(self.t("choose")).clicked() {
                        if let Some(p) = rfd::FileDialog::new()
                            .set_title(self.t("select_checksum_title"))
                            .pick_file()
                        {
                            self.checksum_file = Some(p);
                            *self.state.lock().unwrap() = VerifyState::Idle;
                        }
                    }
                });
            }
            InputMode::HashManual => {
                let algorithm_label = self.t("algorithm");
                let expected_hash_label = self.t("expected_hash");
                let manual_hash_hint = self.t("manual_hash_hint");
                ui.horizontal(|ui| {
                    ui.label(RichText::new(algorithm_label).color(Color32::from_rgb(160, 160, 180)));
                    egui::ComboBox::from_id_salt("algo_combo")
                        .selected_text(self.selected_algo.to_string())
                        .show_ui(ui, |ui| {
                            for algo in Algorithm::all() {
                                let label = algo.to_string();
                                ui.selectable_value(&mut self.selected_algo, algo, label);
                            }
                        });
                });
                ui.add_space(4.0);
                ui.label(RichText::new(expected_hash_label).color(Color32::from_rgb(160, 160, 180)));
                ui.add(
                    egui::TextEdit::singleline(&mut self.manual_hash)
                        .desired_width(f32::INFINITY)
                        .font(FontId::monospace(12.0))
                        .hint_text(manual_hash_hint),
                );
            }
        }
    }

    fn show_verify_button(&mut self, ui: &mut egui::Ui) {
        let computing = *self.state.lock().unwrap() == VerifyState::Computing;
        let verify_label = if computing {
            self.t("computing")
        } else {
            self.t("verify_integrity")
        };
        ui.horizontal(|ui| {
            let btn = egui::Button::new(
                RichText::new(verify_label)
                .font(FontId::proportional(15.0))
                .strong(),
            )
            .fill(if computing {
                Color32::from_rgb(80, 80, 100)
            } else {
                Color32::from_rgb(50, 120, 220)
            })
            .min_size(Vec2::new(200.0, 36.0));
            if ui.add_enabled(!computing, btn).clicked() {
                self.start_verification();
            }
        });
    }

    fn show_result(&self, ui: &mut egui::Ui) {
        match self.state.lock().unwrap().clone() {
            VerifyState::Idle | VerifyState::Computing => {}
            VerifyState::Success(exp, comp) => show_result_box(
                ui,
                true,
                &self.t("verification_success"),
                &self.t("file_intact"),
                &exp,
                &comp,
                &self.t("expected"),
                &self.t("computed"),
            ),
            VerifyState::Failure(exp, comp) => show_result_box(
                ui,
                false,
                &self.t("verification_failed"),
                &self.t("file_modified"),
                &exp,
                &comp,
                &self.t("expected"),
                &self.t("computed"),
            ),
            VerifyState::Error(msg) => {
                egui::Frame::new()
                    .fill(Color32::from_rgba_premultiplied(120, 60, 0, 180))
                    .corner_radius(8.0)
                    .inner_margin(egui::Margin::same(12))
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(self.tr("error_prefix", &[("error", msg)]))
                                .color(Color32::from_rgb(255, 180, 60))
                                .strong(),
                        );
                    });
            }
        }
    }
}

// =============================================================================
// Mode multi-fichiers — toutes les phases
// =============================================================================

impl HashCheckerApp {
    /// Dispatche vers la bonne méthode selon la phase courante.
    fn show_multi_file_view(&mut self, ui: &mut egui::Ui) {
        // On lit la phase d'abord pour éviter les emprunts conflictuels
        let phase = self.multi.as_ref().map(|m| m.phase.clone());
        match phase {
            Some(MultiPhase::Review) => self.show_multi_review(ui),
            Some(MultiPhase::ManualInput) => self.show_multi_manual_input(ui),
            Some(MultiPhase::Verifying) => self.show_multi_verifying(ui),
            Some(MultiPhase::Results) => self.show_multi_results(ui),
            None => {}
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Phase 1 — Review : liste des fichiers avec cases à cocher
    // ─────────────────────────────────────────────────────────────────────────

    fn show_multi_review(&mut self, ui: &mut egui::Ui) {
        let n_total = self.multi.as_ref().map(|m| m.entries.len()).unwrap_or(0);
        let n_selected = self
            .multi
            .as_ref()
            .map(|m| m.entries.iter().filter(|e| e.selected).count())
            .unwrap_or(0);

        // En-tête
        egui::Frame::new()
            .fill(Color32::from_rgba_premultiplied(30, 30, 50, 200))
            .corner_radius(8.0)
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.label(
                    RichText::new(format!(
                        "{}",
                        self.tr(
                            "multi_review_title",
                            &[
                                ("total", n_total.to_string()),
                                ("selected", n_selected.to_string()),
                            ],
                        )
                    ))
                    .font(FontId::proportional(15.0))
                    .color(Color32::from_rgb(180, 200, 255)),
                );
                ui.add_space(4.0);
                ui.label(
                    RichText::new(self.t("multi_review_help"))
                        .color(Color32::from_rgb(140, 140, 160))
                        .font(FontId::proportional(12.0)),
                );
            });

        ui.add_space(4.0);

        // Liste scrollable des fichiers
        let available_h = ui.available_height() - 60.0; // Réserve pour les boutons
        let input_required_text = self.t("input_required");
        ScrollArea::vertical()
            .max_height(available_h.max(100.0))
            .show(ui, |ui| {
                if let Some(multi) = &mut self.multi {
                    for entry in &mut multi.entries {
                        egui::Frame::new()
                            .fill(Color32::from_rgba_premultiplied(25, 25, 40, 200))
                            .corner_radius(6.0)
                            .inner_margin(egui::Margin::symmetric(12, 8))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    // Case à cocher
                                    ui.checkbox(&mut entry.selected, "");

                                    // Nom du fichier
                                    let fname = entry
                                        .path
                                        .file_name()
                                        .unwrap_or_default()
                                        .to_string_lossy();
                                    ui.label(
                                        RichText::new(fname.as_ref())
                                            .color(if entry.selected {
                                                Color32::WHITE
                                            } else {
                                                Color32::from_rgb(100, 100, 120)
                                            })
                                            .strong(),
                                    );

                                    // Indicateur de source (checksum auto ou saisie requise)
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| match &entry.source {
                                            HashSource::AutoFound(cs) => {
                                                let cs_name = cs
                                                    .file_name()
                                                    .unwrap_or_default()
                                                    .to_string_lossy();
                                                ui.label(
                                                    RichText::new(format!("→ {}", cs_name))
                                                        .color(Color32::from_rgb(80, 200, 120))
                                                        .font(FontId::proportional(12.0)),
                                                );
                                            }
                                            HashSource::NeedsInput => {
                                                ui.label(
                                                    RichText::new(input_required_text.clone())
                                                        .color(Color32::from_rgb(255, 180, 60))
                                                        .font(FontId::proportional(12.0)),
                                                );
                                            }
                                            _ => {}
                                        },
                                    );
                                });
                            });
                        ui.add_space(2.0);
                    }
                }
            });

        ui.add_space(4.0);

        // Boutons d'action
        ui.horizontal(|ui| {
            // Tout cocher / tout décocher
            if ui.button(self.t("select_all")).clicked() {
                if let Some(multi) = &mut self.multi {
                    for e in &mut multi.entries {
                        e.selected = true;
                    }
                }
            }
            if ui.button(self.t("unselect_all")).clicked() {
                if let Some(multi) = &mut self.multi {
                    for e in &mut multi.entries {
                        e.selected = false;
                    }
                }
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Bouton Continuer
                let btn = egui::Button::new(
                    RichText::new(self.t("continue"))
                        .font(FontId::proportional(14.0))
                        .strong(),
                )
                .fill(Color32::from_rgb(50, 120, 220))
                .min_size(Vec2::new(120.0, 32.0));

                let any_selected = self
                    .multi
                    .as_ref()
                    .map(|m| m.entries.iter().any(|e| e.selected))
                    .unwrap_or(false);

                if ui.add_enabled(any_selected, btn).clicked() {
                    // Transition Review → ManualInput ou Verifying
                    let start_verify = self.multi.as_mut().unwrap().advance_from_review();
                    if start_verify {
                        self.start_multi_verification();
                    }
                }

                // Bouton Annuler (retour au mode single vide)
                if ui.button(self.t("cancel")).clicked() {
                    self.multi = None;
                    self.target_file = None;
                    *self.state.lock().unwrap() = VerifyState::Idle;
                }
            });
        });
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Phase 2 — ManualInput : saisie du hash, un fichier à la fois
    // ─────────────────────────────────────────────────────────────────────────

    fn show_multi_manual_input(&mut self, ui: &mut egui::Ui) {
        // Infos sur le fichier courant (lu avant toute mutation)
        let (current_name, queue_len, queue_pos) = {
            let multi = self.multi.as_ref().unwrap();
            let idx = multi
                .manual_queue
                .get(multi.manual_pos)
                .copied()
                .unwrap_or(0);
            let name = multi
                .entries
                .get(idx)
                .map(|e| {
                    e.path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string()
                })
                .unwrap_or_default();
            (name, multi.manual_queue.len(), multi.manual_pos)
        };

        egui::Frame::new()
            .fill(Color32::from_rgba_premultiplied(30, 30, 50, 200))
            .corner_radius(10.0)
            .inner_margin(egui::Margin::same(16))
            .show(ui, |ui| {
                // En-tête avec progression
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!(
                            "{}",
                            self.tr(
                                "manual_input_title",
                                &[
                                    ("current", (queue_pos + 1).to_string()),
                                    ("total", queue_len.to_string()),
                                ],
                            )
                        ))
                        .font(FontId::proportional(15.0))
                        .color(Color32::from_rgb(180, 200, 255))
                        .strong(),
                    );
                });
                ui.add_space(4.0);

                // Nom du fichier courant
                ui.label(
                    RichText::new(&current_name)
                        .font(FontId::monospace(13.0))
                        .color(Color32::WHITE)
                        .strong(),
                );
                ui.add_space(2.0);
                ui.label(
                    RichText::new(self.t("no_checksum_for_file"))
                        .color(Color32::from_rgb(255, 180, 60))
                        .font(FontId::proportional(12.0)),
                );

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(8.0);

                // Sélection de l'algorithme
                ui.horizontal(|ui| {
                    ui.label(RichText::new(self.t("algorithm")).color(Color32::from_rgb(160, 160, 180)));
                    if let Some(multi) = &mut self.multi {
                        egui::ComboBox::from_id_salt("multi_algo_combo")
                            .selected_text(multi.input_algo.to_string())
                            .show_ui(ui, |ui| {
                                for algo in Algorithm::all() {
                                    let label = algo.to_string();
                                    ui.selectable_value(&mut multi.input_algo, algo, label);
                                }
                            });
                    }
                });

                ui.add_space(4.0);
                ui.label(RichText::new(self.t("expected_hash")).color(Color32::from_rgb(160, 160, 180)));

                let manual_hash_hint_short = self.t("manual_hash_hint_short");
                if let Some(multi) = &mut self.multi {
                    ui.add(
                        egui::TextEdit::singleline(&mut multi.input_hash)
                            .desired_width(f32::INFINITY)
                            .font(FontId::monospace(12.0))
                            .hint_text(manual_hash_hint_short),
                    );
                }

                ui.add_space(12.0);

                // Boutons Passer / Valider
                let input_empty = self
                    .multi
                    .as_ref()
                    .map(|m| m.input_hash.trim().is_empty())
                    .unwrap_or(true);

                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Valider
                        let valider_btn = egui::Button::new(
                            RichText::new(self.t("validate"))
                                .font(FontId::proportional(14.0))
                                .strong(),
                        )
                        .fill(Color32::from_rgb(50, 120, 220))
                        .min_size(Vec2::new(100.0, 30.0));

                        if ui.add_enabled(!input_empty, valider_btn).clicked() {
                            let start = self.multi.as_mut().unwrap().validate_manual();
                            if start {
                                self.start_multi_verification();
                            }
                        }

                        // Passer
                        let passer_btn = egui::Button::new(
                            RichText::new(self.t("skip")).color(Color32::from_rgb(200, 180, 100)),
                        )
                        .fill(Color32::from_rgba_premultiplied(60, 55, 30, 180))
                        .min_size(Vec2::new(90.0, 30.0));

                        if ui.add(passer_btn).clicked() {
                            let start = self.multi.as_mut().unwrap().skip_manual();
                            if start {
                                self.start_multi_verification();
                            }
                        }
                    });
                });
            });
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Phase 3 — Verifying : calcul en cours
    // ─────────────────────────────────────────────────────────────────────────

    fn show_multi_verifying(&mut self, ui: &mut egui::Ui) {
        // Vérifie si tous les threads ont terminé
        let all_done = self.multi.as_ref().map(|m| m.all_done()).unwrap_or(false);
        if all_done {
            if let Some(multi) = &mut self.multi {
                multi.phase = MultiPhase::Results;
            }
            return; // Sera redessiné au prochain frame
        }

        egui::Frame::new()
            .fill(Color32::from_rgba_premultiplied(30, 30, 50, 200))
            .corner_radius(10.0)
            .inner_margin(egui::Margin::same(16))
            .show(ui, |ui| {
                ui.label(
                    RichText::new(self.t("computing"))
                        .font(FontId::proportional(16.0))
                        .color(Color32::from_rgb(100, 180, 255))
                        .strong(),
                );
                ui.add_space(10.0);

                // Affiche le statut de chaque fichier en temps réel
                if let Some(multi) = &self.multi {
                    ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                        for entry in &multi.entries {
                            let status = entry.status.lock().unwrap().clone();
                            let (icon, color) = match &status {
                                EntryStatus::Skipped => ("—", Color32::from_rgb(120, 120, 140)),
                                EntryStatus::Computing => ("⏳", Color32::from_rgb(100, 180, 255)),
                                EntryStatus::Success(_, _) => {
                                    ("✓", Color32::from_rgb(80, 220, 120))
                                }
                                EntryStatus::Failure(_, _) => ("✗", Color32::from_rgb(255, 80, 80)),
                                EntryStatus::Error(_) => ("!", Color32::from_rgb(255, 160, 60)),
                                EntryStatus::Pending => ("…", Color32::from_rgb(140, 140, 160)),
                            };
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(icon).color(color).strong());
                                let fname =
                                    entry.path.file_name().unwrap_or_default().to_string_lossy();
                                ui.label(
                                    RichText::new(fname.as_ref())
                                        .color(Color32::from_rgb(200, 200, 220)),
                                );
                            });
                        }
                    });
                }
            });
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Phase 4 — Results : tableau récapitulatif final
    // ─────────────────────────────────────────────────────────────────────────

    fn show_multi_results(&mut self, ui: &mut egui::Ui) {
        let (total, success, failure, error, skipped) = self
            .multi
            .as_ref()
            .map(|m| m.stats())
            .unwrap_or((0, 0, 0, 0, 0));

        // Bandeau résumé
        let all_ok = failure == 0 && error == 0 && success > 0;
        let summary_bg = if all_ok {
            Color32::from_rgba_premultiplied(0, 70, 35, 220)
        } else if failure > 0 || error > 0 {
            Color32::from_rgba_premultiplied(90, 20, 20, 220)
        } else {
            Color32::from_rgba_premultiplied(30, 30, 50, 220)
        };
        let summary_color = if all_ok {
            Color32::from_rgb(80, 220, 120)
        } else if failure > 0 || error > 0 {
            Color32::from_rgb(255, 80, 80)
        } else {
            Color32::from_rgb(160, 160, 180)
        };
        let summary_text = if all_ok {
            self.t("all_files_intact")
        } else if failure > 0 {
            self.t("multi_failed")
        } else if error > 0 {
            self.t("multi_errors")
        } else {
            self.t("verification_finished")
        };

        egui::Frame::new()
            .fill(summary_bg)
            .corner_radius(8.0)
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.label(
                    RichText::new(summary_text)
                    .font(FontId::proportional(16.0))
                    .strong()
                    .color(summary_color),
                );
                ui.add_space(4.0);
                ui.label(
                    RichText::new(self.tr(
                        "multi_stats",
                        &[
                            ("total", total.to_string()),
                            ("success", success.to_string()),
                            ("failure", failure.to_string()),
                            ("error", error.to_string()),
                            ("skipped", skipped.to_string()),
                        ],
                    ))
                    .color(Color32::from_rgb(180, 180, 200))
                    .font(FontId::proportional(12.0)),
                );
            });

        ui.add_space(4.0);

        // Tableau des résultats, scrollable
        let available_h = ui.available_height() - 50.0;
        ScrollArea::vertical()
            .max_height(available_h.max(100.0))
            .show(ui, |ui| {
                if let Some(multi) = &self.multi {
                    for entry in &multi.entries {
                        let status = entry.status.lock().unwrap().clone();

                        // Couleur de fond selon le résultat
                        let (bg, icon, status_text, status_color) = match &status {
                            EntryStatus::Success(_, _) => (
                                Color32::from_rgba_premultiplied(0, 50, 25, 180),
                                "✓",
                                self.t("status_success"),
                                Color32::from_rgb(80, 220, 120),
                            ),
                            EntryStatus::Failure(_, _) => (
                                Color32::from_rgba_premultiplied(70, 15, 15, 180),
                                "✗",
                                self.t("status_failed"),
                                Color32::from_rgb(255, 80, 80),
                            ),
                            EntryStatus::Error(msg) => (
                                Color32::from_rgba_premultiplied(80, 40, 0, 180),
                                "!",
                                msg.clone(),
                                Color32::from_rgb(255, 160, 60),
                            ),
                            EntryStatus::Skipped => (
                                Color32::from_rgba_premultiplied(20, 20, 30, 160),
                                "—",
                                self.t("status_skipped"),
                                Color32::from_rgb(100, 100, 120),
                            ),
                            _ => (
                                Color32::from_rgba_premultiplied(20, 20, 30, 160),
                                "?",
                                self.t("status_pending"),
                                Color32::from_rgb(140, 140, 160),
                            ),
                        };

                        egui::Frame::new()
                            .fill(bg)
                            .corner_radius(6.0)
                            .inner_margin(egui::Margin::symmetric(12, 8))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    // Icône de résultat
                                    ui.label(
                                        RichText::new(icon)
                                            .color(status_color)
                                            .strong()
                                            .font(FontId::proportional(16.0)),
                                    );

                                    // Nom du fichier
                                    let fname = entry
                                        .path
                                        .file_name()
                                        .unwrap_or_default()
                                        .to_string_lossy();
                                    ui.label(
                                        RichText::new(fname.as_ref())
                                            .color(Color32::WHITE)
                                            .strong(),
                                    );

                                    // Statut à droite
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.label(
                                                RichText::new(status_text)
                                                    .color(status_color)
                                                    .strong(),
                                            );
                                        },
                                    );
                                });

                                // Pour les échecs, affiche les deux hash pour comparaison
                                if let EntryStatus::Failure(expected, computed) = &status {
                                    ui.add_space(4.0);
                                    egui::Grid::new(format!("hash_cmp_{}", entry.path.display()))
                                        .num_columns(2)
                                        .spacing([6.0, 2.0])
                                        .show(ui, |ui| {
                                            ui.label(
                                                RichText::new(self.t("expected"))
                                                    .color(Color32::from_rgb(140, 140, 160))
                                                    .font(FontId::proportional(11.0)),
                                            );
                                            ui.label(
                                                RichText::new(expected)
                                                    .font(FontId::monospace(10.0))
                                                    .color(Color32::from_rgb(180, 200, 255)),
                                            );
                                            ui.end_row();
                                            ui.label(
                                                RichText::new(self.t("computed"))
                                                    .color(Color32::from_rgb(140, 140, 160))
                                                    .font(FontId::proportional(11.0)),
                                            );
                                            ui.label(
                                                RichText::new(computed)
                                                    .font(FontId::monospace(10.0))
                                                    .color(Color32::from_rgb(255, 120, 120)),
                                            );
                                            ui.end_row();
                                        });
                                }
                            });
                        ui.add_space(2.0);
                    }
                }
            });

        ui.add_space(4.0);

        // Bouton "Nouvelle vérification"
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let btn = egui::Button::new(
                    RichText::new(self.t("new_verification")).font(FontId::proportional(13.0)),
                )
                .fill(Color32::from_rgba_premultiplied(50, 50, 70, 180))
                .min_size(Vec2::new(160.0, 30.0));
                if ui.add(btn).clicked() {
                    // Réinitialise complètement → retour GUI vide
                    self.multi = None;
                    self.target_file = None;
                    self.checksum_file = None;
                    self.auto_checksum_found = None;
                    self.manual_hash.clear();
                    *self.state.lock().unwrap() = VerifyState::Idle;
                }
            });
        });
    }
}

// =============================================================================
// Panneau Paramètres (inchangé)
// =============================================================================

impl HashCheckerApp {
    fn show_settings(&mut self, ui: &mut egui::Ui) {
        // ── À propos ──
        egui::Frame::new()
            .fill(Color32::from_rgba_premultiplied(30, 30, 50, 200))
            .corner_radius(10.0)
            .inner_margin(egui::Margin::same(14))
            .show(ui, |ui| {
                ui.label(
                    RichText::new(self.t("about"))
                        .font(FontId::proportional(18.0))
                        .strong()
                        .color(Color32::from_rgb(100, 180, 255)),
                );
                ui.add_space(6.0);

                egui::Grid::new("about_grid")
                    .num_columns(2)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(self.t("version")).color(Color32::from_rgb(160, 160, 180)),
                        );
                        ui.label(RichText::new(VERSION).strong().color(Color32::WHITE));
                        ui.end_row();

                        ui.label(RichText::new(self.t("author")).color(Color32::from_rgb(160, 160, 180)));
                        ui.label(RichText::new(AUTHORS).strong().color(Color32::WHITE));
                        ui.end_row();

                        ui.label(
                            RichText::new(self.t("product_name"))
                                .color(Color32::from_rgb(160, 160, 180)),
                        );
                        ui.label(RichText::new(PRODUCTNAME).strong().color(Color32::WHITE));
                        ui.end_row();

                        ui.label(RichText::new("GitHub:").color(Color32::from_rgb(160, 160, 180)));
                        ui.label(RichText::new(GITHUB).strong().color(Color32::WHITE));
                        ui.end_row();

                        ui.label(
                            RichText::new(self.t("algorithms")).color(Color32::from_rgb(160, 160, 180)),
                        );
                        ui.label(
                            RichText::new(
                                "MD5 · SHA-1 · SHA-224 · SHA-256 · SHA-384 · SHA-512 · CRC32",
                            )
                            .color(Color32::WHITE),
                        );
                        ui.end_row();

                        ui.label(
                            RichText::new(self.t("license")).color(Color32::from_rgb(160, 160, 180)),
                        );
                        ui.label(RichText::new("MIT").color(Color32::WHITE));
                        ui.end_row();
                    });
            });

        ui.add_space(10.0);

        // ── Intégration menu contextuel ──
        egui::Frame::new()
            .fill(Color32::from_rgba_premultiplied(30, 30, 50, 200))
            .corner_radius(10.0)
            .inner_margin(egui::Margin::same(14))
            .show(ui, |ui| {
                ui.label(
                    RichText::new(self.t("context_integration"))
                        .font(FontId::proportional(18.0))
                        .strong()
                        .color(Color32::from_rgb(100, 180, 255)),
                );
                ui.label(
                    RichText::new(self.t("context_integration_help"))
                    .color(Color32::from_rgb(160, 160, 180)),
                );
                ui.add_space(10.0);

                let exe = current_exe_path();

                if let Some(installed) = self.integration_status.windows_registry {
                    self.show_integration_row(
                        ui,
                        "Windows (Explorateur)",
                        installed,
                        "windows",
                        &exe,
                    );
                }
                if let Some(installed) = self.integration_status.linux_nautilus {
                    self.show_integration_row(
                        ui,
                        "Linux — Nautilus (GNOME)",
                        installed,
                        "nautilus",
                        &exe,
                    );
                }
                if let Some(installed) = self.integration_status.linux_kde {
                    self.show_integration_row(ui, "Linux — Dolphin (KDE)", installed, "kde", &exe);
                }
                if let Some(installed) = self.integration_status.linux_thunar {
                    self.show_integration_row(
                        ui,
                        "Linux — Thunar (XFCE)",
                        installed,
                        "thunar",
                        &exe,
                    );
                }

                if let Some((msg, ok)) = &self.integration_message {
                    ui.add_space(8.0);
                    ui.label(RichText::new(msg).color(if *ok {
                        Color32::from_rgb(80, 220, 120)
                    } else {
                        Color32::from_rgb(255, 100, 100)
                    }));
                }
            });
    }

    fn show_integration_row(
        &mut self,
        ui: &mut egui::Ui,
        label: &str,
        installed: bool,
        target: &str,
        exe: &str,
    ) {
        ui.horizontal(|ui| {
            let status_color = if installed {
                Color32::from_rgb(80, 220, 120)
            } else {
                Color32::from_rgb(160, 160, 180)
            };
            let status_text = if installed {
                self.t("active_status")
            } else {
                self.t("inactive_status")
            };

            ui.label(RichText::new(format!("{:<28}", label)).color(Color32::WHITE));
            ui.label(RichText::new(status_text).color(status_color).strong());

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if installed {
                    if ui
                        .add(
                            egui::Button::new(RichText::new(self.t("disable")).color(Color32::WHITE))
                                .fill(Color32::from_rgb(160, 60, 60)),
                        )
                        .clicked()
                    {
                        let result = match target {
                            "windows" => uninstall_windows(),
                            "nautilus" => uninstall_nautilus(),
                            "kde" => uninstall_kde(),
                            "thunar" => uninstall_thunar(),
                            _ => Err("Inconnu".to_string()),
                        };
                        self.integration_message = Some(match result {
                            Ok(_) => (self.t("disabled_success"), true),
                            Err(e) => (self.tr("error_prefix", &[("error", e)]), false),
                        });
                        self.integration_status = IntegrationStatus::detect();
                    }
                } else {
                    if ui
                        .add(
                            egui::Button::new(RichText::new(self.t("enable")).color(Color32::WHITE))
                                .fill(Color32::from_rgb(40, 130, 60)),
                        )
                        .clicked()
                    {
                        let result = match target {
                            "windows" => install_windows(exe),
                            "nautilus" => install_nautilus(exe),
                            "kde" => install_kde(exe),
                            "thunar" => install_thunar(exe),
                            _ => Err("Inconnu".to_string()),
                        };
                        self.integration_message = Some(match result {
                            Ok(_) => (self.t("enabled_success"), true),
                            Err(e) => (self.tr("error_prefix", &[("error", e)]), false),
                        });
                        self.integration_status = IntegrationStatus::detect();
                    }
                }
            });
        });
        ui.add_space(4.0);
    }
}

// =============================================================================
// Fonctions libres — vérification de hash
// =============================================================================

// ── Mode fichier unique ───────────────────────────────────────────────────────

/// Calcule le hash du fichier cible et compare avec la valeur attendue.
/// Utilisé par le mode fichier unique (un thread séparé).
fn run_single_verification(
    target: PathBuf,
    expected: String,
    algo: Algorithm,
    state: Arc<Mutex<VerifyState>>,
) {
    match compute_hash(&target, &algo) {
        Ok(computed) => {
            let exp = expected.to_lowercase();
            let comp = computed.to_lowercase();
            *state.lock().unwrap() = if exp == comp {
                VerifyState::Success(exp, comp)
            } else {
                VerifyState::Failure(exp, comp)
            };
        }
        Err(e) => *state.lock().unwrap() = VerifyState::Error(e.to_string()),
    }
}

// ── Mode multi-fichiers ───────────────────────────────────────────────────────

/// Vérifie un fichier depuis un hash saisi directement (mode ManualHash).
/// Utilisé par le mode multi-fichiers (un thread par entrée).
fn run_entry_direct(
    target: PathBuf,
    expected: String,
    algo: Algorithm,
    status: Arc<Mutex<EntryStatus>>,
) {
    match compute_hash(&target, &algo) {
        Ok(computed) => {
            let exp = expected.to_lowercase();
            let comp = computed.to_lowercase();
            *status.lock().unwrap() = if exp == comp {
                EntryStatus::Success(exp, comp)
            } else {
                EntryStatus::Failure(exp, comp)
            };
        }
        Err(e) => *status.lock().unwrap() = EntryStatus::Error(e.to_string()),
    }
}

/// Vérifie un fichier depuis un fichier checksum (AutoFound ou ManualFile).
/// Parse le fichier checksum, trouve l'entrée correspondante, puis vérifie.
fn run_entry_from_file(
    target: PathBuf,
    cs_path: PathBuf,
    algo_fallback: Algorithm,
    status: Arc<Mutex<EntryStatus>>,
) {
    // Parse le fichier checksum (SHA256SUMS, *.sha256, etc.)
    let entries = match parse_checksum_file(&cs_path) {
        Ok(e) => e,
        Err(e) => {
            *status.lock().unwrap() = EntryStatus::Error(format!("Checksum illisible : {}", e));
            return;
        }
    };

    // Cherche l'entrée correspondant au fichier cible dans le checksum
    let entry = match find_entry_for_file(&entries, &target) {
        Some(e) => e.clone(),
        None => {
            let fname = target.file_name().unwrap_or_default().to_string_lossy();
            *status.lock().unwrap() =
                EntryStatus::Error(format!("'{}' absent du fichier checksum", fname));
            return;
        }
    };

    // Utilise l'algorithme indiqué dans le checksum, ou le fallback sinon
    let algo = entry.algorithm.unwrap_or(algo_fallback);
    run_entry_direct(target, entry.hash, algo, status);
}

// =============================================================================
// Affichage résultat (mode fichier unique)
// =============================================================================

fn show_result_box(
    ui: &mut egui::Ui,
    success: bool,
    title: &str,
    subtitle: &str,
    expected: &str,
    computed: &str,
    expected_label: &str,
    computed_label: &str,
) {
    let bg = if success {
        Color32::from_rgba_premultiplied(0, 80, 40, 200)
    } else {
        Color32::from_rgba_premultiplied(100, 20, 20, 200)
    };
    let accent = if success {
        Color32::from_rgb(80, 220, 120)
    } else {
        Color32::from_rgb(255, 80, 80)
    };

    egui::Frame::new()
        .fill(bg)
        .corner_radius(10.0)
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.label(
                RichText::new(title)
                    .font(FontId::proportional(18.0))
                    .strong()
                    .color(accent),
            );
            ui.label(RichText::new(subtitle).color(Color32::from_rgb(200, 200, 200)));
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);
            egui::Grid::new("hash_grid")
                .num_columns(2)
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
                    ui.label(RichText::new(expected_label).color(Color32::from_rgb(160, 160, 180)));
                    ui.label(
                        RichText::new(expected)
                            .font(FontId::monospace(11.0))
                            .color(Color32::from_rgb(200, 220, 255)),
                    );
                    ui.end_row();
                    ui.label(RichText::new(computed_label).color(Color32::from_rgb(160, 160, 180)));
                    ui.label(RichText::new(computed).font(FontId::monospace(11.0)).color(
                        if success {
                            Color32::from_rgb(80, 220, 120)
                        } else {
                            Color32::from_rgb(255, 120, 120)
                        },
                    ));
                    ui.end_row();
                });
        });
}
