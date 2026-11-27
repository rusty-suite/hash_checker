// Masque la fenêtre de terminal noire sur Windows quand on lance la GUI.
// Note : en mode CLI (arguments), le terminal reste visible car il est nécessaire.
// Sur Linux cet attribut est ignoré.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// =============================================================================
// main.rs — Point d'entrée de Hash Checker
//
// Scénarios de lancement :
//   1. Aucun argument : interface vide (dépôt de fichier).
//   2. Un ou plusieurs fichiers (clic droit dans l'explorateur) : si une fenêtre est
//      déjà ouverte, les fichiers lui sont transmis et ce processus se termine
//      (instance unique, `rusty_core::instance`) ; sinon la fenêtre s'ouvre avec eux.
//   3. Options de ligne de commande (--hash, --checksum, --algo…) : mode terminal.
//
// Avant l'interface : dossier de données portable, journal, réglages et contrôle de
// la dernière mise à jour (retour arrière automatique si elle a planté, charte 06).
// =============================================================================

mod checksum; // Lecture des fichiers checksum (.sha256, SHA256SUMS…)
mod cli; // Ligne de commande
mod gui; // Interface graphique (charte Rusty Suite, via rusty_kit)
mod hasher; // Calcul des empreintes (MD5, SHA-1, SHA-2, CRC32)
mod integration; // Menu contextuel (registre Windows, Nautilus, Dolphin, Thunar)
mod language; // Packs de langue de l'application et messages traduits

use std::path::PathBuf;
use std::sync::Arc;

use rusty_core::{instance, journal, lang, paths, settings, update};

fn main() {
    let data_dir = paths::data_dir(gui::APP_ID);
    let log_path = journal::init(gui::APP_ID, gui::VERSION, &data_dir);
    lang::install_local(language::PACKS, &data_dir, gui::VERSION);
    let settings: gui::Settings = settings::load(&data_dir);

    let rolled_back = match update::startup(&data_dir, gui::VERSION) {
        update::Startup::Relaunched => return, // l'ancienne version a été relancée
        update::Startup::RolledBack(v) => Some(v),
        update::Startup::Normal => None,
    };

    let raw_args: Vec<String> = std::env::args().skip(1).collect();
    let file_args: Vec<PathBuf> = raw_args
        .iter()
        .filter(|a| !a.starts_with('-'))
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .collect();

    // Mode terminal dès qu'une option est présente (`hash_checker f.iso --hash …`), ou si
    // les arguments ne sont pas des fichiers existants. Le clic droit ne passe que des
    // chemins : il ouvre toujours l'interface.
    let has_option = raw_args.iter().any(|a| a.starts_with('-'));
    if has_option || (file_args.is_empty() && !raw_args.is_empty()) {
        let lang = lang::Lang::load(language::PACKS, &data_dir, &settings.common.language);
        let texts = language::Texts(&lang);
        let cli = cli::parse_localized(&texts);
        cli::run_cli(cli, &texts);
        return;
    }

    // Instance unique : Windows lance un processus par fichier sélectionné.
    let forwarded: Vec<String> = file_args.iter().map(|p| p.display().to_string()).collect();
    let args_rx = match instance::start(gui::APP_ID, &forwarded) {
        instance::Instance::Secondary => return,
        instance::Instance::Primary(rx) => rx,
    };

    // La nouvelle version est déclarée saine après un temps de fonctionnement minimal.
    update::mark_healthy_later(data_dir.clone());

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Hash Checker")
            .with_inner_size([780.0, 680.0])
            .with_min_inner_size([560.0, 460.0])
            .with_drag_and_drop(true)
            .with_icon(Arc::new(load_icon())),
        ..Default::default()
    };
    if let Err(e) = eframe::run_native(
        "Hash Checker",
        options,
        Box::new(move |cc| Ok(Box::new(gui::HashCheckerApp::new(cc, file_args, args_rx, data_dir, log_path, settings, rolled_back)))),
    ) {
        log::error!("Impossible de démarrer l'interface : {e}");
    }
}

/// Icône de la fenêtre, intégrée au binaire.
fn load_icon() -> eframe::egui::IconData {
    let bytes = include_bytes!("../assets/hash-checker-logo.png");
    let image = image::load_from_memory(bytes).expect("invalid embedded icon hash-checker-logo.png");
    let rgba = image.to_rgba8();
    let (width, height) = image::GenericImageView::dimensions(&rgba);
    eframe::egui::IconData { rgba: rgba.into_raw(), width, height }
}
