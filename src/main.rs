// Masque la fenêtre de terminal noire sur Windows quand on lance la GUI.
// Sans cet attribut, Windows ouvre un terminal en arrière-plan derrière la fenêtre.
// Note : en mode CLI (arguments), le terminal reste visible car il est nécessaire.
// Sur Linux cet attribut est ignoré.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// =============================================================================
// main.rs — Point d'entrée du programme Hash Checker
//
// Quatre scénarios de lancement :
//
//   1. Aucun argument
//      → Ouvre l'interface graphique vide (drag & drop)
//      → Démarre le serveur IPC pour recevoir des fichiers d'autres instances
//
//   2. Un ou plusieurs chemins de fichiers valides
//      → Mode multi-fichiers (clic droit explorateur sur 1 ou N fichiers)
//      → Si une instance est DÉJÀ ouverte : lui envoie les chemins via IPC
//        et quitte immédiatement (pas de nouvelle fenêtre)
//      → Sinon : devient l'instance principale, démarre l'IPC, ouvre la GUI
//
//   3. Flags CLI (--hash, --checksum, --algo, etc.)
//      → Lance le mode terminal sans interface graphique
//
// L'IPC (src/ipc.rs) garantit qu'une seule fenêtre s'ouvre même si Windows
// lance N processus pour N fichiers sélectionnés dans l'explorateur.
// =============================================================================

// Déclaration des modules du projet
mod checksum;    // Lecture et parsing des fichiers checksum (.sha256, SHA256SUMS, etc.)
mod cli;         // Interface ligne de commande (arguments, affichage résultat terminal)
mod gui;         // Interface graphique (fenêtre egui)
mod hasher;      // Calcul des empreintes hash (MD5, SHA-1, SHA-256, SHA-512, CRC32...)
mod integration; // Intégration menu contextuel OS (Windows registre, Linux Nautilus/KDE/Thunar)
mod ipc;         // Communication inter-processus (instance unique via TCP local)

use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;

fn main() {
    let raw_args: Vec<String> = std::env::args().collect();

    // Collecte tous les arguments qui sont des chemins de fichiers existants.
    // Filtre les flags CLI (qui commencent par '-') pour ne garder que les chemins.
    let file_args: Vec<PathBuf> = raw_args[1..]
        .iter()
        .filter(|a| !a.starts_with('-')) // Ignore les flags comme --hash, --algo, etc.
        .map(PathBuf::from)
        .filter(|p| p.exists() && p.is_file()) // Garde seulement les fichiers valides
        .collect();

    // ── Cas 1 : fichiers passés en argument (clic droit explorateur) ──────────
    if !file_args.is_empty() {
        // Tente d'envoyer les fichiers à une instance déjà ouverte via IPC.
        // Si une instance tourne déjà, elle reçoit les fichiers et ce processus
        // se termine immédiatement → pas de fenêtre supplémentaire.
        if ipc::send_to_existing(&file_args) {
            return;
        }
        // Pas d'instance active → on devient l'instance principale.
        // Démarre le serveur IPC pour recevoir d'éventuels fichiers supplémentaires
        // (Windows peut envoyer les fichiers en rafale avec quelques ms d'écart).
        let ipc_rx = ipc::start_server();
        launch_gui_multi(file_args, ipc_rx);
        ipc::cleanup(); // Supprime le fichier de port à la fermeture
        return;
    }

    // ── Cas 2 : flags CLI classiques (--hash, --checksum, --algo...) ──────────
    // Déclenchés seulement si des args commencent par '-' (pas de fichiers valides)
    if raw_args.len() > 1 {
        let cli = cli::Cli::parse(); // Clap parse et valide les arguments
        cli::run_cli(cli);           // Exécute la logique CLI en mode terminal
        return;
    }

    // ── Cas 3 : aucun argument → GUI vide (drag & drop) ──────────────────────
    // Démarre quand même le serveur IPC : si l'utilisateur fait un clic droit
    // pendant que cette fenêtre est ouverte, le fichier arrivera ici plutôt
    // qu'ouvrir une nouvelle fenêtre.
    let ipc_rx = ipc::start_server();
    launch_gui_empty(ipc_rx);
    ipc::cleanup();
}

// -----------------------------------------------------------------------------
// Lance la GUI en mode multi-fichiers avec une liste de fichiers initiaux.
// Utilisé quand des fichiers sont passés en argument (clic droit explorateur).
//
// `files`  : fichiers à charger immédiatement dans la liste de vérification
// `ipc_rx` : channel pour recevoir des fichiers supplémentaires via IPC
// -----------------------------------------------------------------------------
fn launch_gui_multi(files: Vec<PathBuf>, ipc_rx: std::sync::mpsc::Receiver<PathBuf>) {
    let options = build_window_options();
    eframe::run_native(
        "Hash Checker",
        options,
        Box::new(move |_cc| Ok(Box::new(gui::HashCheckerApp::with_files(files, ipc_rx)))),
    )
    .expect("Erreur lors du lancement de la GUI");
}

// -----------------------------------------------------------------------------
// Lance la GUI en mode vide (drag & drop).
// Utilisé quand aucun fichier n'est passé en argument.
//
// `ipc_rx` : channel pour recevoir des fichiers si l'utilisateur fait un
//            clic droit pendant que cette fenêtre est déjà ouverte
// -----------------------------------------------------------------------------
fn launch_gui_empty(ipc_rx: std::sync::mpsc::Receiver<PathBuf>) {
    let options = build_window_options();
    eframe::run_native(
        "Hash Checker",
        options,
        Box::new(move |cc| Ok(Box::new(gui::HashCheckerApp::new_with_ipc(cc, Some(ipc_rx))))),
    )
    .expect("Erreur lors du lancement de la GUI");
}

// -----------------------------------------------------------------------------
// Construit la configuration commune de la fenêtre principale.
// Centralisé ici pour éviter la duplication entre les deux modes de lancement.
// -----------------------------------------------------------------------------
fn build_window_options() -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Hash Checker")
            .with_inner_size([680.0, 540.0])      // Un peu plus haut pour le mode liste
            .with_min_inner_size([520.0, 400.0])
            .with_drag_and_drop(true)
            .with_icon(Arc::new(load_icon())),
        ..Default::default()
    }
}

// -----------------------------------------------------------------------------
// Charge l'icône de la fenêtre depuis assets/hash-checker-logo.png.
// Le fichier PNG est intégré dans le binaire au moment de la compilation
// via include_bytes! — aucun fichier externe requis à l'exécution.
// -----------------------------------------------------------------------------
fn load_icon() -> eframe::egui::IconData {
    let bytes = include_bytes!("../assets/hash-checker-logo.png");
    let image = image::load_from_memory(bytes).expect("Impossible de charger hash-checker-logo.png");
    let rgba = image.to_rgba8();
    let (width, height) = image::GenericImageView::dimensions(&rgba);
    eframe::egui::IconData {
        rgba: rgba.into_raw(),
        width,
        height,
    }
}
