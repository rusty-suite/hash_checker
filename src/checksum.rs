// =============================================================================
// checksum.rs — Recherche et parsing des fichiers checksum
//
// Ce module gère tout ce qui concerne les fichiers de vérification d'intégrité :
//   1. Recherche automatique d'un fichier checksum dans le même répertoire
//      que le fichier cible (plusieurs stratégies par ordre de priorité)
//   2. Détection de l'algorithme depuis le nom/extension du fichier checksum
//   3. Lecture et parsing du fichier checksum (formats GNU coreutils et variantes)
//   4. Recherche de la ligne correspondant au fichier cible dans la liste
//
// Formats de fichiers checksum supportés :
//   - "<hash>  <fichier>"  (format sha256sum / md5sum standard Linux)
//   - "<hash> *<fichier>"  (mode binaire, le * est ignoré)
//   - "<fichier>:<hash>"   (format alternatif)
//   - "<fichier>=<hash>"   (format alternatif)
// =============================================================================

use std::fs;
use std::path::{Path, PathBuf};

use crate::hasher::Algorithm;

// -----------------------------------------------------------------------------
// Structure représentant une ligne parsée dans un fichier checksum.
// Un fichier comme SHA256SUMS peut contenir des dizaines d'entrées,
// une par ligne, chacune associant un hash à un nom de fichier.
// -----------------------------------------------------------------------------
#[derive(Debug, Clone)]
pub struct ChecksumEntry {
    pub hash: String,              // Valeur du hash en hexadécimal minuscule
    pub filename: String,          // Nom du fichier tel qu'écrit dans le checksum
    pub algorithm: Option<Algorithm>, // Algorithme déduit (peut être None si inconnu)
}

// Extensions de fichiers checksum reconnues, en minuscules uniquement.
// Dans find_checksum_file, on teste chaque extension en minuscule ET en majuscule
// pour couvrir Linux (sensible à la casse) sans dupliquer la liste ici.
const KNOWN_EXTENSIONS: &[(&str, Algorithm)] = &[
    ("md5",    Algorithm::Md5),
    ("sha1",   Algorithm::Sha1),
    ("sha256", Algorithm::Sha256),
    ("sha384", Algorithm::Sha384),
    ("sha512", Algorithm::Sha512),
];

// Noms de fichiers checksum génériques souvent distribués avec les téléchargements.
// Exemples : distributions Linux (SHA256SUMS), projets open-source (checksums.txt).
const KNOWN_FILENAMES: &[&str] = &[
    "checksums.txt",
    "CHECKSUMS",
    "CHECKSUMS.txt",
    "SHA256SUMS",
    "SHA512SUMS",
    "SHA1SUMS",
    "MD5SUMS",
    "sha256sums",
    "sha512sums",
    "sha1sums",
    "md5sums",
    "checksum.txt",
    "checksum.sha256",
    "checksum.md5",
];

// -----------------------------------------------------------------------------
// Cherche automatiquement un fichier checksum dans le répertoire du fichier cible.
//
// Ordre de priorité (du plus spécifique au plus générique) :
//   1. <nom_complet_du_fichier>.<algo>  →  ubuntu-24.04.iso.sha256
//   2. <nom_sans_extension>.<algo>      →  ubuntu-24.04.sha256
//   3. Noms génériques connus           →  SHA256SUMS, checksums.txt, etc.
//
// S'arrête à la première correspondance trouvée.
// Retourne None si aucun fichier checksum n'est détecté.
// -----------------------------------------------------------------------------
pub fn find_checksum_file(target: &Path) -> Option<PathBuf> {
    let dir = target.parent()?;
    let stem = target.file_name()?.to_string_lossy(); // Nom complet : "ubuntu.iso"

    // Priorité 1 : cherche "ubuntu.iso.sha256" et "ubuntu.iso.SHA256", etc.
    // On teste minuscule ET majuscule au moment de la recherche,
    // sans dupliquer la liste KNOWN_EXTENSIONS (qui ne contient que des minuscules).
    for (ext, _) in KNOWN_EXTENSIONS {
        for variant in [ext.to_string(), ext.to_uppercase()] {
            let candidate = dir.join(format!("{}.{}", stem, variant));
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }

    // Priorité 2 : cherche "ubuntu.sha256" / "ubuntu.SHA256" (sans l'extension .iso)
    if let Some(file_stem) = target.file_stem() {
        let file_stem = file_stem.to_string_lossy();
        for (ext, _) in KNOWN_EXTENSIONS {
            for variant in [ext.to_string(), ext.to_uppercase()] {
                let candidate = dir.join(format!("{}.{}", file_stem, variant));
                if candidate.exists() {
                    return Some(candidate);
                }
            }
        }
    }

    // Priorité 3 : noms de fichiers génériques (SHA256SUMS, sha256sums, etc.)
    // Ces noms sont des fichiers distincts sur Linux → on les garde tous explicitement.
    for name in KNOWN_FILENAMES {
        let candidate = dir.join(name);
        if candidate.exists() {
            return Some(candidate);
        }
    }

    None // Aucun fichier checksum trouvé
}

// -----------------------------------------------------------------------------
// Détecte l'algorithme de hash depuis l'extension ou le nom du fichier checksum.
//
// Exemples :
//   "fichier.sha256"  → SHA-256
//   "SHA256SUMS"      → SHA-256 (détecté depuis le nom)
//   "checksums.txt"   → None (impossible à deviner depuis le nom seul)
// -----------------------------------------------------------------------------
pub fn detect_algorithm_from_file(path: &Path) -> Option<Algorithm> {
    let ext = path.extension()?.to_string_lossy().to_lowercase();
    match ext.as_str() {
        "md5"    => Some(Algorithm::Md5),
        "sha1"   => Some(Algorithm::Sha1),
        "sha224" => Some(Algorithm::Sha224),
        "sha256" => Some(Algorithm::Sha256),
        "sha384" => Some(Algorithm::Sha384),
        "sha512" => Some(Algorithm::Sha512),
        _ => {
            // L'extension n'est pas révélatrice, on tente depuis le nom complet
            // ex: "SHA256SUMS" → contient "SHA256" → SHA-256
            let name = path.file_name()?.to_string_lossy().to_uppercase();
            if name.contains("SHA512") { return Some(Algorithm::Sha512); }
            if name.contains("SHA384") { return Some(Algorithm::Sha384); }
            if name.contains("SHA256") { return Some(Algorithm::Sha256); }
            if name.contains("SHA224") { return Some(Algorithm::Sha224); }
            if name.contains("SHA1")   { return Some(Algorithm::Sha1); }
            if name.contains("MD5")    { return Some(Algorithm::Md5); }
            None // Impossible à déterminer depuis le nom
        }
    }
}

// -----------------------------------------------------------------------------
// Lit un fichier checksum et retourne la liste de toutes les entrées valides.
//
// Ignore les lignes vides et les commentaires (commençant par #).
// Utilise `detect_algorithm_from_file` pour donner un indice à chaque ligne.
// Retourne une erreur si le fichier est illisible ou ne contient rien de valide.
// -----------------------------------------------------------------------------
pub fn parse_checksum_file(path: &Path) -> Result<Vec<ChecksumEntry>, String> {
    let content = fs::read_to_string(path)
        .map_err(|e| format!("Impossible de lire le fichier : {}", e))?;

    // On donne un "hint" d'algorithme à toutes les lignes du fichier
    // basé sur son nom/extension (ex: SHA256SUMS → on sait que c'est SHA-256)
    let algo_hint = detect_algorithm_from_file(path);
    let mut entries = Vec::new();

    for line in content.lines() {
        let line = line.trim();
        // Ignore les lignes vides et les commentaires (#)
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Tente de parser la ligne selon les formats connus
        if let Some(entry) = parse_line(line, algo_hint.clone()) {
            entries.push(entry);
        }
    }

    if entries.is_empty() {
        Err("Aucune entrée valide trouvée dans le fichier checksum.".to_string())
    } else {
        Ok(entries)
    }
}

// -----------------------------------------------------------------------------
// Tente de parser une ligne d'un fichier checksum.
//
// Formats reconnus (par ordre de tentative) :
//   Format 1 : "a1b2c3...  ubuntu.iso"  ou  "a1b2c3... *ubuntu.iso"
//              (standard GNU coreutils — sha256sum, md5sum, etc.)
//   Format 2 : "ubuntu.iso:a1b2c3..."  ou  "ubuntu.iso=a1b2c3..."
//              (format alternatif utilisé par certains outils Windows)
//
// Retourne None si la ligne ne correspond à aucun format connu.
// -----------------------------------------------------------------------------
fn parse_line(line: &str, algo_hint: Option<Algorithm>) -> Option<ChecksumEntry> {
    // ── Format 1 : "<hash>  <fichier>" ────────────────────────────────────────
    let parts: Vec<&str> = line.splitn(2, ' ').collect();
    if parts.len() == 2 {
        let hash = parts[0].trim();
        // Le * devant le nom indique le mode binaire — on l'ignore
        let filename = parts[1].trim().trim_start_matches('*');
        if is_valid_hex(hash) {
            let algorithm = algo_hint
                .clone()
                .or_else(|| Algorithm::from_hash_len(hash)); // Deviner depuis la longueur si pas d'indice
            return Some(ChecksumEntry {
                hash: hash.to_lowercase(), // Normalise en minuscules pour la comparaison
                filename: filename.to_string(),
                algorithm,
            });
        }
    }

    // ── Format 2 : "<fichier>:<hash>" ou "<fichier>=<hash>" ──────────────────
    for sep in [':', '='] {
        if let Some((filename, hash)) = line.split_once(sep) {
            let hash = hash.trim();
            let filename = filename.trim();
            if is_valid_hex(hash) {
                let algorithm = algo_hint
                    .clone()
                    .or_else(|| Algorithm::from_hash_len(hash));
                return Some(ChecksumEntry {
                    hash: hash.to_lowercase(),
                    filename: filename.to_string(),
                    algorithm,
                });
            }
        }
    }

    None // Ligne non reconnue, elle sera ignorée
}

// -----------------------------------------------------------------------------
// Cherche dans la liste d'entrées celle qui correspond au fichier cible.
//
// Stratégie :
//   1. Correspondance exacte sur le nom de fichier (insensible à la casse)
//   2. Si le fichier checksum ne contient qu'une seule entrée, on l'utilise
//      directement (cas d'un fichier checksum dédié à un seul fichier)
//
// Retourne None si aucune correspondance n'est trouvée et qu'il y a
// plusieurs entrées (ambiguïté impossible à résoudre automatiquement).
// -----------------------------------------------------------------------------
pub fn find_entry_for_file<'a>(
    entries: &'a [ChecksumEntry],
    target: &Path,
) -> Option<&'a ChecksumEntry> {
    let target_name = target.file_name()?.to_string_lossy().to_lowercase();

    // Correspondance exacte sur le nom de fichier (insensible à la casse)
    entries.iter().find(|e| {
        let entry_name = Path::new(&e.filename)
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        entry_name == target_name
    })
    // Fallback : fichier checksum ne contenant qu'une seule ligne → on l'utilise
    .or_else(|| if entries.len() == 1 { entries.first() } else { None })
}

// -----------------------------------------------------------------------------
// Valide qu'une chaîne ressemble à un hash hexadécimal.
// Critères : non vide, au moins 8 caractères, uniquement des chiffres hex (0-9, a-f, A-F).
// -----------------------------------------------------------------------------
fn is_valid_hex(s: &str) -> bool {
    !s.is_empty()
        && s.len() >= 8
        && s.chars().all(|c| c.is_ascii_hexdigit())
}
