// =============================================================================
// cli.rs — Interface ligne de commande (terminal)
//
// Ce module gère l'utilisation du programme en mode terminal.
// Il définit les arguments acceptés (via la crate `clap`) et orchestre
// la logique de vérification selon le mode demandé :
//
//   Mode 1 — Calcul seul (--compute) :
//     Calcule et affiche le hash du fichier sans le comparer à rien.
//
//   Mode 2 — Hash manuel (--hash) :
//     Compare le hash calculé à la valeur fournie directement.
//     L'algorithme peut être forcé avec --algo ou deviné depuis la longueur du hash.
//
//   Mode 3 — Fichier checksum (par défaut) :
//     Cherche automatiquement un fichier checksum dans le même répertoire,
//     ou utilise celui fourni via --checksum.
//     Extrait la ligne correspondant au fichier cible et compare.
//
// Codes de retour :
//   0 → vérification réussie (hash identique)
//   1 → erreur (fichier introuvable, checksum illisible, etc.)
//   2 → vérification échouée (hash différent = fichier corrompu/modifié)
// =============================================================================

use std::path::{Path, PathBuf};
use std::process;

use clap::Parser;

use crate::checksum::{find_checksum_file, find_entry_for_file, parse_checksum_file};
use crate::hasher::{compute_hash, Algorithm};

// -----------------------------------------------------------------------------
// Définition des arguments de la ligne de commande.
// La dérivation `Parser` de clap génère automatiquement le parseur d'arguments
// et l'aide (--help) depuis les commentaires et attributs ci-dessous.
// -----------------------------------------------------------------------------
#[derive(Parser, Debug)]
#[command(
    name = "hash_checker",
    version,
    about = "Vérifie l'intégrité d'un fichier par comparaison de hash"
)]
pub struct Cli {
    /// Fichier à vérifier
    pub file: Option<PathBuf>,

    /// Fichier checksum à utiliser (optionnel — auto-détecté sinon)
    #[arg(short, long, value_name = "CHECKSUM_FILE")]
    pub checksum: Option<PathBuf>,

    /// Valeur du hash à comparer manuellement (sans fichier checksum)
    #[arg(long, value_name = "HASH")]
    pub hash: Option<String>,

    /// Algorithme à utiliser : md5, sha1, sha256, sha512, etc.
    /// Si omis, déduit automatiquement depuis la longueur du hash.
    #[arg(short, long, value_name = "ALGO")]
    pub algo: Option<String>,

    /// Affiche uniquement le hash calculé du fichier, sans comparaison
    #[arg(long)]
    pub compute: bool,
}

// -----------------------------------------------------------------------------
// Point d'entrée du mode CLI.
// Reçoit les arguments déjà parsés par clap et exécute la logique appropriée.
// -----------------------------------------------------------------------------
pub fn run_cli(cli: Cli) {
    // Vérifie qu'un fichier cible a bien été fourni
    let file = match &cli.file {
        Some(f) => f.clone(),
        None => {
            eprintln!("Erreur : vous devez spécifier un fichier.");
            eprintln!("Usage : hash_checker <fichier> [options]");
            process::exit(1);
        }
    };

    // Vérifie que le fichier existe sur le disque
    if !file.exists() {
        eprintln!("Erreur : fichier introuvable : {}", file.display());
        process::exit(1);
    }

    // ── Mode 1 : calcul seul (--compute) ─────────────────────────────────────
    // Calcule et affiche le hash sans comparaison.
    // Utile pour générer un hash à copier-coller ou à sauvegarder.
    if cli.compute {
        let algo = cli
            .algo
            .as_deref()
            .and_then(Algorithm::from_str)
            .unwrap_or(Algorithm::Sha256); // SHA-256 par défaut si non spécifié

        println!("Calcul {} de : {}", algo, file.display());
        match compute_hash(&file, &algo) {
            Ok(h)  => println!("{}", h),
            Err(e) => {
                eprintln!("Erreur lors du calcul : {}", e);
                process::exit(1);
            }
        }
        return;
    }

    // ── Mode 2 : hash fourni manuellement (--hash) ───────────────────────────
    // L'utilisateur donne directement la valeur de hash attendue.
    // L'algorithme est déduit de la longueur si --algo n'est pas fourni.
    if let Some(expected) = &cli.hash {
        let algo = cli
            .algo
            .as_deref()
            .and_then(Algorithm::from_str)          // --algo explicite en priorité
            .or_else(|| Algorithm::from_hash_len(expected)) // sinon depuis la longueur
            .unwrap_or(Algorithm::Sha256);           // fallback SHA-256

        println!("Algorithme détecté : {}", algo);
        verify_against_hash(&file, expected, &algo);
        return;
    }

    // ── Mode 3 : fichier checksum (comportement par défaut) ──────────────────
    // Cherche un fichier checksum soit fourni via --checksum,
    // soit détecté automatiquement dans le répertoire du fichier cible.
    let checksum_path = match &cli.checksum {
        Some(p) => p.clone(), // Chemin fourni explicitement par l'utilisateur
        None => match find_checksum_file(&file) {
            Some(p) => {
                println!("Fichier checksum trouvé : {}", p.display());
                p
            }
            None => {
                // Aucun fichier checksum trouvé → on ne peut pas continuer
                eprintln!("Aucun fichier checksum trouvé pour : {}", file.display());
                eprintln!("Utilisez --checksum <fichier> ou --hash <valeur> [--algo <algo>]");
                process::exit(1);
            }
        },
    };

    // Parse le fichier checksum pour en extraire toutes les entrées
    let entries = match parse_checksum_file(&checksum_path) {
        Ok(e)  => e,
        Err(e) => {
            eprintln!("Erreur de lecture du fichier checksum : {}", e);
            process::exit(1);
        }
    };

    // Cherche la ligne qui correspond au fichier cible dans le checksum
    let entry = match find_entry_for_file(&entries, &file) {
        Some(e) => e,
        None => {
            eprintln!(
                "Fichier '{}' non trouvé dans le checksum ({} entrée(s) disponibles).",
                file.file_name().unwrap_or_default().to_string_lossy(),
                entries.len()
            );
            process::exit(1);
        }
    };

    // Détermine l'algorithme : depuis l'entrée checksum, sinon --algo, sinon SHA-256
    let algo = entry
        .algorithm
        .clone()
        .or_else(|| cli.algo.as_deref().and_then(Algorithm::from_str))
        .unwrap_or(Algorithm::Sha256);

    println!("Algorithme : {}", algo);
    verify_against_hash(&file, &entry.hash, &algo);
}

// -----------------------------------------------------------------------------
// Calcule le hash du fichier et le compare à la valeur attendue.
// Affiche le résultat et quitte avec le code approprié.
//
// Code 0 → identique (fichier intact)
// Code 2 → différent (fichier corrompu ou modifié)
// -----------------------------------------------------------------------------
fn verify_against_hash(file: &Path, expected: &str, algo: &Algorithm) {
    println!("Calcul en cours...");
    match compute_hash(file, algo) {
        Ok(computed) => {
            // Normalise les deux hash en minuscules avant comparaison
            // pour éviter les faux négatifs dus à la casse (A1b2 ≠ a1B2)
            let expected_lower = expected.to_lowercase();
            let computed_lower = computed.to_lowercase();

            println!("Hash attendu  : {}", expected_lower);
            println!("Hash calculé  : {}", computed_lower);

            if computed_lower == expected_lower {
                println!("\n[OK] VERIFICATION REUSSIE - Le fichier est intact.");
                process::exit(0); // Succès
            } else {
                println!("\n[ECHEC] VERIFICATION ECHOUEE - Le fichier est corrompu ou modifié !");
                process::exit(2); // Hash différent
            }
        }
        Err(e) => {
            eprintln!("Erreur lors du calcul du hash : {}", e);
            process::exit(1); // Erreur technique
        }
    }
}
