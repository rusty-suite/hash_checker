// Masque la fenêtre de terminal noire sur Windows quand on lance la GUI.
// Sans cet attribut, Windows ouvre un terminal en arrière-plan derrière la fenêtre.
// Note : en mode CLI (arguments), le terminal reste visible car il est nécessaire.
// Sur Linux cet attribut est ignoré.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// =============================================================================
// main.rs — Point d'entrée du programme Hash Checker
//
// Ce fichier décide quel mode lancer selon les arguments reçus :
//   - Aucun argument        → ouvre l'interface graphique (GUI) vide
//   - Un seul argument = chemin de fichier existant
//                           → ouvre la GUI avec ce fichier déjà chargé
//                             (cas typique du clic droit depuis l'explorateur)
//   - Plusieurs arguments CLI (--hash, --checksum, --algo, etc.)
//                           → lance le mode terminal (CLI)
// =============================================================================

// Déclaration des modules du projet
mod checksum;   // Lecture et parsing des fichiers checksum (.sha256, SHA256SUMS, etc.)
mod cli;        // Interface ligne de commande (arguments, affichage résultat terminal)
mod gui;        // Interface graphique (fenêtre egui)
mod hasher;     // Calcul des empreintes hash (MD5, SHA-1, SHA-256, SHA-512, CRC32...)
mod integration; // Intégration menu contextuel OS (Windows registre, Linux Nautilus/KDE/Thunar)

use clap::Parser;

fn main() {
    // Récupère tous les arguments passés au programme
    // args[0] = nom du binaire lui-même, args[1..] = vrais arguments
    let args: Vec<String> = std::env::args().collect();

    // ── Cas 1 : un seul argument qui est un fichier existant ──────────────────
    // C'est le comportement déclenché par le clic droit dans l'explorateur.
    // L'OS appelle : hash_checker.exe "C:\chemin\vers\fichier.iso"
    if args.len() == 2 {
        let path = std::path::PathBuf::from(&args[1]);
        if path.exists() && path.is_file() {
            // Ouvre la GUI avec le fichier pré-chargé et lance la vérification
            launch_gui_with_file(path);
            return;
        }
    }

    // ── Cas 2 : arguments CLI classiques (--hash, --checksum, --algo...) ──────
    if args.len() > 1 {
        let cli = cli::Cli::parse(); // Clap parse et valide les arguments
        cli::run_cli(cli);           // Exécute la logique CLI
    } else {
        // ── Cas 3 : aucun argument → GUI vide ─────────────────────────────────
        launch_gui(None);
    }
}

// -----------------------------------------------------------------------------
// Lance la fenêtre graphique.
// Si `preloaded` contient un chemin, le fichier est immédiatement chargé
// dans l'interface sans que l'utilisateur ait à le sélectionner manuellement.
// -----------------------------------------------------------------------------
fn launch_gui(preloaded: Option<std::path::PathBuf>) {
    // Configuration de la fenêtre native
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Hash Checker")
            .with_inner_size([640.0, 500.0])      // Taille initiale en pixels
            .with_min_inner_size([480.0, 380.0])  // Taille minimale redimensionnable
            .with_drag_and_drop(true),             // Active le glisser-déposer de fichiers
        ..Default::default()
    };

    // Démarre la boucle principale egui/eframe
    eframe::run_native(
        "Hash Checker",
        options,
        Box::new(move |cc| {
            Ok(Box::new(match preloaded {
                // Fichier fourni → crée l'application avec ce fichier chargé
                Some(p) => gui::HashCheckerApp::with_file(p),
                // Pas de fichier → interface vide, l'utilisateur sélectionne
                None => gui::HashCheckerApp::new(cc),
            }))
        }),
    )
    .expect("Erreur lors du lancement de la GUI");
}

// Raccourci pratique pour appeler launch_gui avec un fichier
fn launch_gui_with_file(path: std::path::PathBuf) {
    launch_gui(Some(path));
}
