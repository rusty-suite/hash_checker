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

use clap::{CommandFactory, FromArgMatches, Parser};

use crate::checksum::{find_checksum_file, find_entry_for_file, parse_checksum_file};
use crate::hasher::{compute_hash, Algorithm};
use crate::language::LanguageManager;

// -----------------------------------------------------------------------------
// Définition des arguments de la ligne de commande.
// La dérivation `Parser` de clap génère automatiquement le parseur d'arguments
// et l'aide (--help) depuis les commentaires et attributs ci-dessous.
// -----------------------------------------------------------------------------
#[derive(Parser, Debug)]
#[command(
    name = "hash_checker",
    version,
    about = "Verifies the integrity of a file by comparing hashes"
)]
pub struct Cli {
    // Help texts below are English fallbacks; `parse_localized` replaces them
    // with the user language at runtime.
    /// File to verify
    pub file: Option<PathBuf>,

    /// Checksum file to use (optional, detected automatically otherwise)
    #[arg(short, long, value_name = "CHECKSUM_FILE")]
    pub checksum: Option<PathBuf>,

    /// Hash value to compare manually (without checksum file)
    #[arg(long, value_name = "HASH")]
    pub hash: Option<String>,

    /// Algorithm: md5, sha1, sha256, sha512... Deduced from the hash length if omitted.
    #[arg(short, long, value_name = "ALGO")]
    pub algo: Option<String>,

    /// Only print the computed hash, without comparison
    #[arg(long)]
    pub compute: bool,
}

// -----------------------------------------------------------------------------
// Point d'entrée du mode CLI.
// Reçoit les arguments déjà parsés par clap et exécute la logique appropriée.
// -----------------------------------------------------------------------------
/// Parses the command line with `--help` texts in the user language.
pub fn parse_localized(lang: &LanguageManager) -> Cli {
    let cmd = Cli::command()
        .about(lang.text("cli_about"))
        .mut_arg("file", |a| a.help(lang.text("cli_arg_file")))
        .mut_arg("checksum", |a| a.help(lang.text("cli_arg_checksum")))
        .mut_arg("hash", |a| a.help(lang.text("cli_arg_hash")))
        .mut_arg("algo", |a| a.help(lang.text("cli_arg_algo")))
        .mut_arg("compute", |a| a.help(lang.text("cli_arg_compute")));
    let matches = cmd.get_matches();
    Cli::from_arg_matches(&matches).unwrap_or_else(|e| e.exit())
}

pub fn run_cli(cli: Cli, lang: &LanguageManager) {
    // Vérifie qu'un fichier cible a bien été fourni
    let file = match &cli.file {
        Some(f) => f.clone(),
        None => {
            eprintln!("{}", lang.text("cli_no_file"));
            eprintln!("{}", lang.text("cli_usage"));
            process::exit(1);
        }
    };

    // Vérifie que le fichier existe sur le disque
    if !file.exists() {
        eprintln!("{}", lang.text_replace("cli_file_not_found", &[("path", file.display().to_string())]));
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

        println!("{}", lang.text_replace("cli_computing_of", &[("algo", algo.to_string()), ("path", file.display().to_string())]));
        match compute_hash(&file, &algo) {
            Ok(h)  => println!("{}", h),
            Err(e) => {
                eprintln!("{}", lang.text_replace("cli_compute_error", &[("error", e.to_string())]));
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

        println!("{}", lang.text_replace("cli_algo_detected", &[("algo", algo.to_string())]));
        verify_against_hash(&file, expected, &algo, lang);
        return;
    }

    // ── Mode 3 : fichier checksum (comportement par défaut) ──────────────────
    // Cherche un fichier checksum soit fourni via --checksum,
    // soit détecté automatiquement dans le répertoire du fichier cible.
    let checksum_path = match &cli.checksum {
        Some(p) => p.clone(), // Chemin fourni explicitement par l'utilisateur
        None => match find_checksum_file(&file) {
            Some(p) => {
                println!("{}", lang.text_replace("cli_checksum_found", &[("path", p.display().to_string())]));
                p
            }
            None => {
                // Aucun fichier checksum trouvé → on ne peut pas continuer
                eprintln!("{}", lang.text_replace("cli_no_checksum", &[("path", file.display().to_string())]));
                eprintln!("{}", lang.text("cli_checksum_hint"));
                process::exit(1);
            }
        },
    };

    // Parse le fichier checksum pour en extraire toutes les entrées
    let entries = match parse_checksum_file(&checksum_path) {
        Ok(e)  => e,
        Err(e) => {
            eprintln!("{}", lang.msg(&e));
            process::exit(1);
        }
    };

    // Cherche la ligne qui correspond au fichier cible dans le checksum
    let entry = match find_entry_for_file(&entries, &file) {
        Some(e) => e,
        None => {
            eprintln!("{}", lang.text_replace("cli_not_in_checksum", &[
                ("file", file.file_name().unwrap_or_default().to_string_lossy().to_string()),
                ("count", entries.len().to_string()),
            ]));
            process::exit(1);
        }
    };

    // Détermine l'algorithme : depuis l'entrée checksum, sinon --algo, sinon SHA-256
    let algo = entry
        .algorithm
        .clone()
        .or_else(|| cli.algo.as_deref().and_then(Algorithm::from_str))
        .unwrap_or(Algorithm::Sha256);

    println!("{}", lang.text_replace("cli_algorithm", &[("algo", algo.to_string())]));
    verify_against_hash(&file, &entry.hash, &algo, lang);
}

// -----------------------------------------------------------------------------
// Calcule le hash du fichier et le compare à la valeur attendue.
// Affiche le résultat et quitte avec le code approprié.
//
// Code 0 → identique (fichier intact)
// Code 2 → différent (fichier corrompu ou modifié)
// -----------------------------------------------------------------------------
fn verify_against_hash(file: &Path, expected: &str, algo: &Algorithm, lang: &LanguageManager) {
    println!("{}", lang.text("cli_computing"));
    match compute_hash(file, algo) {
        Ok(computed) => {
            // Normalise les deux hash en minuscules avant comparaison
            // pour éviter les faux négatifs dus à la casse (A1b2 ≠ a1B2)
            let expected_lower = expected.to_lowercase();
            let computed_lower = computed.to_lowercase();

            println!("{}", lang.text_replace("cli_expected", &[("hash", expected_lower.clone())]));
            println!("{}", lang.text_replace("cli_computed", &[("hash", computed_lower.clone())]));

            if computed_lower == expected_lower {
                println!("\n{}", lang.text("cli_ok"));
                process::exit(0); // Succès
            } else {
                println!("\n{}", lang.text("cli_failed"));
                process::exit(2); // Hash différent
            }
        }
        Err(e) => {
            eprintln!("{}", lang.text_replace("cli_hash_error", &[("error", e.to_string())]));
            process::exit(1); // Erreur technique
        }
    }
}
