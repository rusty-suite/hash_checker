// =============================================================================
// gui.rs — Interface graphique (egui / eframe)
//
// Ce module dessine toute l'interface utilisateur avec la bibliothèque egui
// (mode immédiat : l'interface est redessinée entièrement à chaque frame).
//
// Structure de l'interface :
//   ┌─────────────────────────────────────────┐
//   │  Hash Checker              [Paramètres] │  ← Barre de titre
//   ├─────────────────────────────────────────┤
//   │  [Zone de dépôt / sélection de fichier] │  ← Glisser-déposer ou clic
//   │  [Automatique] [Fichier] [Hash manuel]  │  ← Onglets de mode
//   │  ... contenu selon le mode ...          │
//   │  [Vérifier l'intégrité]                 │  ← Bouton d'action
//   │  ┌ Résultat ──────────────────────────┐ │
//   │  │ VERIFICATION REUSSIE / ECHOUEE     │ │  ← Résultat coloré
//   │  └────────────────────────────────────┘ │
//   └─────────────────────────────────────────┘
//
// Le panneau "Paramètres" remplace le panneau principal quand on clique
// sur le bouton "Paramètres" en haut à droite.
//
// Le calcul du hash tourne dans un thread séparé (std::thread::spawn)
// pour ne pas bloquer l'interface. Le résultat est partagé via Arc<Mutex<>>.
// =============================================================================

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;

use eframe::egui::{self, Color32, FontId, RichText, Stroke, Vec2};

use crate::checksum::{find_checksum_file, find_entry_for_file, parse_checksum_file};
use crate::hasher::{compute_hash, Algorithm};
use crate::integration::{
    current_exe_path, install_kde, install_nautilus, install_thunar, install_windows,
    uninstall_kde, uninstall_nautilus, uninstall_thunar, uninstall_windows, IntegrationStatus,
};

// Informations affichées dans le panneau "À propos"
const VERSION: &str = env!("CARGO_PKG_VERSION"); // Lue automatiquement depuis Cargo.toml
const AUTHORS: &str = "Rusty-Suite.com";
const PRODUCTNAME: &str = "Hash Checker";
const GITHUB: &str = "https://github.com/rusty-suite/hash_checker";

// -----------------------------------------------------------------------------
// État de la vérification — partagé entre le thread de calcul et l'UI
// -----------------------------------------------------------------------------
#[derive(Debug, Clone, PartialEq)]
enum VerifyState {
    Idle,                       // Aucune vérification en cours ou demandée
    Computing,                  // Calcul en cours dans un thread séparé
    Success(String, String),    // (hash_attendu, hash_calculé) — identiques
    Failure(String, String),    // (hash_attendu, hash_calculé) — différents
    Error(String),              // Message d'erreur (fichier illisible, etc.)
}

// -----------------------------------------------------------------------------
// Mode de saisie du hash de référence
// -----------------------------------------------------------------------------
#[derive(Debug, Clone, PartialEq)]
enum InputMode {
    Auto,       // Recherche automatique d'un fichier checksum dans le répertoire
    FileManual, // L'utilisateur sélectionne manuellement le fichier checksum
    HashManual, // L'utilisateur saisit directement la valeur du hash
}

// -----------------------------------------------------------------------------
// Panneau affiché dans la fenêtre principale
// -----------------------------------------------------------------------------
#[derive(Debug, Clone, PartialEq)]
enum Panel {
    Main,     // Interface principale de vérification
    Settings, // Panneau paramètres (à propos + intégration OS)
}

// -----------------------------------------------------------------------------
// Structure principale de l'application — contient tout l'état de l'UI
// egui recrée l'interface depuis cet état à chaque frame (~60 fps)
// -----------------------------------------------------------------------------
pub struct HashCheckerApp {
    // Fichier cible à vérifier
    target_file: Option<PathBuf>,

    // Mode de saisie actuellement sélectionné (onglets)
    input_mode: InputMode,

    // Fichier checksum sélectionné manuellement par l'utilisateur
    checksum_file: Option<PathBuf>,
    // Fichier checksum trouvé automatiquement dans le répertoire du fichier cible
    auto_checksum_found: Option<PathBuf>,

    // Hash saisi manuellement et algorithme choisi dans le menu déroulant
    manual_hash: String,
    selected_algo: Algorithm,

    // État partagé avec le thread de calcul (Arc = multi-propriétaire, Mutex = verrou)
    state: Arc<Mutex<VerifyState>>,

    // True quand l'utilisateur survole la fenêtre avec un fichier (drag & drop)
    drag_hover: bool,

    // Panneau actuellement affiché (principal ou paramètres)
    active_panel: Panel,

    // État détecté de l'intégration OS (clic droit explorateur)
    integration_status: IntegrationStatus,
    // Dernier message suite à une action d'activation/désactivation : (texte, succès?)
    integration_message: Option<(String, bool)>,
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
            integration_status: IntegrationStatus::detect(),
            integration_message: None,
        }
    }
}

impl HashCheckerApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        Self::default()
    }

    pub fn with_file(path: PathBuf) -> Self {
        let mut app = Self::default();
        app.set_target_file(path);
        app
    }

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

    fn start_verification(&mut self) {
        let Some(target) = self.target_file.clone() else { return; };
        *self.state.lock().unwrap() = VerifyState::Computing;
        let state = Arc::clone(&self.state);

        match &self.input_mode {
            InputMode::HashManual => {
                let expected = self.manual_hash.trim().to_lowercase();
                if expected.is_empty() {
                    *state.lock().unwrap() =
                        VerifyState::Error("Veuillez entrer une valeur de hash.".to_string());
                    return;
                }
                let algo = self.selected_algo.clone();
                thread::spawn(move || run_verification(target, expected, algo, state));
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
                        Err(e) => { *state.lock().unwrap() = VerifyState::Error(e); return; }
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
                    run_verification(target, entry.hash, algo, state);
                });
            }
        }
    }
}

fn run_verification(
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

impl eframe::App for HashCheckerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Drag & drop
        ctx.input(|i| {
            self.drag_hover = !i.raw.hovered_files.is_empty();
            if !i.raw.dropped_files.is_empty() {
                if let Some(path) = i.raw.dropped_files[0].path.clone() {
                    self.set_target_file(path);
                    self.active_panel = Panel::Main;
                }
            }
        });

        let mut style = (*ctx.style()).clone();
        style.spacing.item_spacing = Vec2::new(8.0, 8.0);
        style.spacing.button_padding = Vec2::new(14.0, 7.0);
        ctx.set_style(style);

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(10.0);

            // ── Barre de titre ──
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
                        RichText::new(if self.active_panel == Panel::Settings { "X Fermer" } else { "Parametres" })
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
                });
            });

            ui.separator();
            ui.add_space(6.0);

            match self.active_panel {
                Panel::Main => self.show_main(ui),
                Panel::Settings => self.show_settings(ui),
            }

            if self.drag_hover {
                ui.painter().rect_filled(
                    ui.clip_rect(),
                    8.0,
                    Color32::from_rgba_premultiplied(100, 180, 255, 30),
                );
            }
        });

        let state = self.state.lock().unwrap().clone();
        if state == VerifyState::Computing {
            ctx.request_repaint();
        }
    }
}

// ─────────────────────────────────────────────
// Panneau principal
// ─────────────────────────────────────────────
impl HashCheckerApp {
    fn show_main(&mut self, ui: &mut egui::Ui) {
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
            self.target_file.as_ref().unwrap()
                .file_name().unwrap_or_default()
                .to_string_lossy().to_string()
        } else {
            "Glissez un fichier ici ou cliquez pour selectionner".to_string()
        };

        let border_color = if self.drag_hover {
            Color32::from_rgb(100, 180, 255)
        } else if has_file {
            Color32::from_rgb(80, 200, 120)
        } else {
            Color32::from_rgb(100, 100, 120)
        };

        let (rect, response) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), 70.0),
            egui::Sense::click(),
        );
        ui.painter().rect(
            rect, 8.0,
            Color32::from_rgba_premultiplied(30, 30, 40, 200),
            Stroke::new(2.0, border_color),
            egui::StrokeKind::Outside,
        );
        ui.painter().text(
            rect.center(), egui::Align2::CENTER_CENTER, &label,
            FontId::proportional(14.0),
            if has_file { Color32::WHITE } else { Color32::from_rgb(160, 160, 180) },
        );
        if response.clicked() {
            if let Some(path) = rfd::FileDialog::new()
                .set_title("Sélectionner le fichier à vérifier")
                .pick_file()
            {
                self.set_target_file(path);
            }
        }
    }

    fn show_input_section(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let modes = [
                (InputMode::Auto, "Automatique"),
                (InputMode::FileManual, "Fichier checksum"),
                (InputMode::HashManual, "Hash manuel"),
            ];
            for (mode, label) in &modes {
                let selected = &self.input_mode == mode;
                let btn = egui::Button::new(
                    RichText::new(*label).color(if selected { Color32::WHITE } else { Color32::from_rgb(160, 160, 180) }),
                )
                .fill(if selected { Color32::from_rgb(50, 100, 200) } else { Color32::from_rgba_premultiplied(50, 50, 70, 150) });
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
                        ui.label(RichText::new("Checksum détecté :").color(Color32::from_rgb(160, 160, 180)));
                        ui.label(RichText::new(cs.file_name().unwrap_or_default().to_string_lossy().to_string())
                            .color(Color32::from_rgb(80, 200, 120)).strong());
                    });
                } else {
                    ui.colored_label(Color32::from_rgb(255, 180, 60), "Aucun fichier checksum trouvé automatiquement.");
                }
            }
            InputMode::FileManual => {
                ui.horizontal(|ui| {
                    let cs_label = self.checksum_file.as_ref()
                        .map(|p| p.file_name().unwrap_or_default().to_string_lossy().to_string())
                        .unwrap_or_else(|| "Aucun fichier sélectionné".to_string());
                    ui.label(RichText::new(cs_label).color(Color32::from_rgb(200, 200, 220)));
                    if ui.button("Choisir...").clicked() {
                        if let Some(p) = rfd::FileDialog::new()
                            .set_title("Sélectionner le fichier checksum")
                            .pick_file()
                        {
                            self.checksum_file = Some(p);
                            *self.state.lock().unwrap() = VerifyState::Idle;
                        }
                    }
                });
            }
            InputMode::HashManual => {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Algorithme :").color(Color32::from_rgb(160, 160, 180)));
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
                ui.label(RichText::new("Hash attendu :").color(Color32::from_rgb(160, 160, 180)));
                ui.add(egui::TextEdit::singleline(&mut self.manual_hash)
                    .desired_width(f32::INFINITY)
                    .font(FontId::monospace(12.0))
                    .hint_text("Collez ici la valeur du hash..."));
            }
        }
    }

    fn show_verify_button(&mut self, ui: &mut egui::Ui) {
        let computing = *self.state.lock().unwrap() == VerifyState::Computing;
        ui.horizontal(|ui| {
            let btn = egui::Button::new(
                RichText::new(if computing { "Calcul en cours..." } else { "Verifier l'integrite" })
                    .font(FontId::proportional(15.0)).strong(),
            )
            .fill(if computing { Color32::from_rgb(80, 80, 100) } else { Color32::from_rgb(50, 120, 220) })
            .min_size(Vec2::new(200.0, 36.0));
            if ui.add_enabled(!computing, btn).clicked() {
                self.start_verification();
            }
        });
    }

    fn show_result(&self, ui: &mut egui::Ui) {
        match self.state.lock().unwrap().clone() {
            VerifyState::Idle | VerifyState::Computing => {}
            VerifyState::Success(exp, comp) => show_result_box(ui, true, "VERIFICATION REUSSIE", "Le fichier est intact et non modifie.", &exp, &comp),
            VerifyState::Failure(exp, comp) => show_result_box(ui, false, "VERIFICATION ECHOUEE", "Le fichier est corrompu ou a ete modifie !", &exp, &comp),
            VerifyState::Error(msg) => {
                egui::Frame::new()
                    .fill(Color32::from_rgba_premultiplied(120, 60, 0, 180))
                    .corner_radius(8.0)
                    .inner_margin(egui::Margin::same(12))
                    .show(ui, |ui| {
                        ui.label(RichText::new(format!("Erreur : {}", msg))
                            .color(Color32::from_rgb(255, 180, 60)).strong());
                    });
            }
        }
    }
}

// ─────────────────────────────────────────────
// Panneau Paramètres
// ─────────────────────────────────────────────
impl HashCheckerApp {
    fn show_settings(&mut self, ui: &mut egui::Ui) {
        // ── À propos ──
        egui::Frame::new()
            .fill(Color32::from_rgba_premultiplied(30, 30, 50, 200))
            .corner_radius(10.0)
            .inner_margin(egui::Margin::same(14))
            .show(ui, |ui| {
                ui.label(RichText::new("A propos").font(FontId::proportional(18.0)).strong().color(Color32::from_rgb(100, 180, 255)));
                ui.add_space(6.0);

                egui::Grid::new("about_grid").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                    ui.label(RichText::new("Version :").color(Color32::from_rgb(160, 160, 180)));
                    ui.label(RichText::new(VERSION).strong().color(Color32::WHITE));
                    ui.end_row();

                    ui.label(RichText::new("Auteur :").color(Color32::from_rgb(160, 160, 180)));
                    ui.label(RichText::new(AUTHORS).strong().color(Color32::WHITE));
                    ui.end_row();

                    ui.label(RichText::new("Nom du produit :").color(Color32::from_rgb(160, 160, 180)));
                    ui.label(RichText::new(PRODUCTNAME).strong().color(Color32::WHITE));
                    ui.end_row();

                    ui.label(RichText::new("Nom du produit :").color(Color32::from_rgb(160, 160, 180)));
                    ui.label(RichText::new(GITHUB).strong().color(Color32::WHITE));
                    ui.end_row();

                    ui.label(RichText::new("Algorithmes :").color(Color32::from_rgb(160, 160, 180)));
                    ui.label(RichText::new("MD5 · SHA-1 · SHA-224 · SHA-256 · SHA-384 · SHA-512 · CRC32").color(Color32::WHITE));
                    ui.end_row();

                    ui.label(RichText::new("Licence :").color(Color32::from_rgb(160, 160, 180)));
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
                ui.label(RichText::new("Integration menu contextuel").font(FontId::proportional(18.0)).strong().color(Color32::from_rgb(100, 180, 255)));
                ui.label(RichText::new("Permet de verifier un fichier via un clic droit dans l'explorateur.").color(Color32::from_rgb(160, 160, 180)));
                ui.add_space(10.0);

                let exe = current_exe_path();

                // Windows
                if let Some(installed) = self.integration_status.windows_registry {
                    self.show_integration_row(ui, "Windows (Explorateur)", installed, "windows", &exe);
                }

                // Linux Nautilus
                if let Some(installed) = self.integration_status.linux_nautilus {
                    self.show_integration_row(ui, "Linux — Nautilus (GNOME)", installed, "nautilus", &exe);
                }

                // Linux KDE
                if let Some(installed) = self.integration_status.linux_kde {
                    self.show_integration_row(ui, "Linux — Dolphin (KDE)", installed, "kde", &exe);
                }

                // Linux Thunar
                if let Some(installed) = self.integration_status.linux_thunar {
                    self.show_integration_row(ui, "Linux — Thunar (XFCE)", installed, "thunar", &exe);
                }

                // Message de retour
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

    fn show_integration_row(&mut self, ui: &mut egui::Ui, label: &str, installed: bool, target: &str, exe: &str) {
        ui.horizontal(|ui| {
            let status_color = if installed { Color32::from_rgb(80, 220, 120) } else { Color32::from_rgb(160, 160, 180) };
            let status_text = if installed { "Actif" } else { "Inactif" };

            ui.label(RichText::new(format!("{:<28}", label)).color(Color32::WHITE));
            ui.label(RichText::new(status_text).color(status_color).strong());

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if installed {
                    if ui.add(egui::Button::new(RichText::new("Désactiver").color(Color32::WHITE))
                        .fill(Color32::from_rgb(160, 60, 60))).clicked()
                    {
                        let result = match target {
                            "windows" => uninstall_windows(),
                            "nautilus" => uninstall_nautilus(),
                            "kde" => uninstall_kde(),
                            "thunar" => uninstall_thunar(),
                            _ => Err("Inconnu".to_string()),
                        };
                        self.integration_message = Some(match result {
                            Ok(_) => ("Desactive avec succes.".to_string(), true),
                            Err(e) => (format!("Erreur : {}", e), false),
                        });
                        self.integration_status = IntegrationStatus::detect();
                    }
                } else {
                    if ui.add(egui::Button::new(RichText::new("Activer").color(Color32::WHITE))
                        .fill(Color32::from_rgb(40, 130, 60))).clicked()
                    {
                        let result = match target {
                            "windows" => install_windows(exe),
                            "nautilus" => install_nautilus(exe),
                            "kde" => install_kde(exe),
                            "thunar" => install_thunar(exe),
                            _ => Err("Inconnu".to_string()),
                        };
                        self.integration_message = Some(match result {
                            Ok(_) => ("Active avec succes.".to_string(), true),
                            Err(e) => (format!("Erreur : {}", e), false),
                        });
                        self.integration_status = IntegrationStatus::detect();
                    }
                }
            });
        });
        ui.add_space(4.0);
    }
}

// ─────────────────────────────────────────────
// Affichage résultat
// ─────────────────────────────────────────────
fn show_result_box(ui: &mut egui::Ui, success: bool, title: &str, subtitle: &str, expected: &str, computed: &str) {
    let bg = if success {
        Color32::from_rgba_premultiplied(0, 80, 40, 200)
    } else {
        Color32::from_rgba_premultiplied(100, 20, 20, 200)
    };
    let accent = if success { Color32::from_rgb(80, 220, 120) } else { Color32::from_rgb(255, 80, 80) };

    egui::Frame::new()
        .fill(bg)
        .corner_radius(10.0)
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.label(RichText::new(title).font(FontId::proportional(18.0)).strong().color(accent));
            ui.label(RichText::new(subtitle).color(Color32::from_rgb(200, 200, 200)));
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);
            egui::Grid::new("hash_grid").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
                ui.label(RichText::new("Attendu :").color(Color32::from_rgb(160, 160, 180)));
                ui.label(RichText::new(expected).font(FontId::monospace(11.0)).color(Color32::from_rgb(200, 220, 255)));
                ui.end_row();
                ui.label(RichText::new("Calcule :").color(Color32::from_rgb(160, 160, 180)));
                ui.label(RichText::new(computed).font(FontId::monospace(11.0)).color(
                    if success { Color32::from_rgb(80, 220, 120) } else { Color32::from_rgb(255, 120, 120) }
                ));
                ui.end_row();
            });
        });
}
