// =============================================================================
// hasher.rs — Calcul des empreintes hash (checksums)
//
// Ce module fournit :
//   - L'énumération `Algorithm` listant tous les algorithmes supportés
//   - La fonction `compute_hash` qui lit un fichier par blocs et calcule
//     son empreinte selon l'algorithme choisi
//
// Algorithmes supportés : MD5, SHA-1, SHA-224, SHA-256, SHA-384, SHA-512, CRC32
// =============================================================================

use std::fmt;
use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::Path;

use sha1::Digest as Sha1Digest; // Trait commun aux fonctions de hash SHA-1

// -----------------------------------------------------------------------------
// Énumération des algorithmes de hash disponibles
// -----------------------------------------------------------------------------
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Algorithm {
    Md5,    // 128 bits → hash de 32 caractères hex (obsolète mais encore très utilisé)
    Sha1,   // 160 bits → hash de 40 caractères hex (déprécié pour la sécurité)
    Sha224, // 224 bits → hash de 56 caractères hex
    Sha256, // 256 bits → hash de 64 caractères hex (standard actuel recommandé)
    Sha384, // 384 bits → hash de 96 caractères hex
    Sha512, // 512 bits → hash de 128 caractères hex (très robuste)
    Crc32,  // 32 bits  → hash de 8 caractères hex (vérification d'erreur, pas de sécurité)
}

impl Algorithm {
    /// Convertit une chaîne de texte en algorithme correspondant.
    /// Accepte les variantes avec ou sans tiret : "sha256" et "sha-256" sont équivalents.
    /// Retourne `None` si le nom n'est pas reconnu.
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "md5"               => Some(Self::Md5),
            "sha1" | "sha-1"   => Some(Self::Sha1),
            "sha224" | "sha-224" => Some(Self::Sha224),
            "sha256" | "sha-256" => Some(Self::Sha256),
            "sha384" | "sha-384" => Some(Self::Sha384),
            "sha512" | "sha-512" => Some(Self::Sha512),
            "crc32"             => Some(Self::Crc32),
            _                   => None, // Algorithme inconnu
        }
    }

    /// Devine l'algorithme utilisé à partir de la longueur du hash en hexadécimal.
    /// Chaque algorithme produit une sortie de longueur fixe et unique.
    /// Utile quand l'utilisateur colle un hash sans préciser l'algorithme.
    pub fn from_hash_len(hash: &str) -> Option<Self> {
        match hash.len() {
            8   => Some(Self::Crc32),  //  32 bits ÷ 4 bits/char =   8 chars
            32  => Some(Self::Md5),    // 128 bits ÷ 4 bits/char =  32 chars
            40  => Some(Self::Sha1),   // 160 bits ÷ 4 bits/char =  40 chars
            56  => Some(Self::Sha224), // 224 bits ÷ 4 bits/char =  56 chars
            64  => Some(Self::Sha256), // 256 bits ÷ 4 bits/char =  64 chars
            96  => Some(Self::Sha384), // 384 bits ÷ 4 bits/char =  96 chars
            128 => Some(Self::Sha512), // 512 bits ÷ 4 bits/char = 128 chars
            _   => None,               // Longueur non reconnue
        }
    }

    /// Retourne la liste complète des algorithmes pour les menus déroulants de la GUI.
    pub fn all() -> Vec<Self> {
        vec![
            Self::Md5,
            Self::Sha1,
            Self::Sha224,
            Self::Sha256,
            Self::Sha384,
            Self::Sha512,
            Self::Crc32,
        ]
    }
}

/// Affichage lisible de l'algorithme (utilisé dans l'interface et le terminal)
impl fmt::Display for Algorithm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Md5    => write!(f, "MD5"),
            Self::Sha1   => write!(f, "SHA-1"),
            Self::Sha224 => write!(f, "SHA-224"),
            Self::Sha256 => write!(f, "SHA-256"),
            Self::Sha384 => write!(f, "SHA-384"),
            Self::Sha512 => write!(f, "SHA-512"),
            Self::Crc32  => write!(f, "CRC32"),
        }
    }
}

// Taille du tampon de lecture en mémoire : 1 Mo par passage.
// Permet de traiter des fichiers très volumineux (ISOs, archives)
// sans les charger entièrement en RAM.
const BUFFER_SIZE: usize = 1024 * 1024; // 1 Mo

// -----------------------------------------------------------------------------
// Calcule le hash d'un fichier selon l'algorithme spécifié.
//
// Lecture par blocs de 1 Mo → mise à jour progressive du hasher →
// finalisation → résultat en hexadécimal minuscule.
//
// Retourne une erreur d'I/O si le fichier est inaccessible.
// -----------------------------------------------------------------------------
pub fn compute_hash(path: &Path, algo: &Algorithm) -> io::Result<String> {
    let file = File::open(path)?;
    // BufReader améliore les performances en groupant les lectures système
    let mut reader = BufReader::with_capacity(BUFFER_SIZE, file);
    let mut buf = vec![0u8; BUFFER_SIZE]; // Tampon réutilisable en mémoire

    match algo {
        // ── MD5 ──────────────────────────────────────────────────────────────
        // La crate `md5` v0.7 utilise une API Context au lieu du trait Digest
        Algorithm::Md5 => {
            let mut ctx = md5::Context::new();
            loop {
                let n = reader.read(&mut buf)?;
                if n == 0 { break; }       // Fin du fichier
                ctx.consume(&buf[..n]);    // Ajoute le bloc au calcul
            }
            // .compute() finalise et retourne les 16 octets bruts (.0)
            Ok(hex::encode(ctx.compute().0))
        }

        // ── SHA-1 ─────────────────────────────────────────────────────────────
        Algorithm::Sha1 => {
            let mut hasher = sha1::Sha1::new();
            loop {
                let n = reader.read(&mut buf)?;
                if n == 0 { break; }
                sha1::Digest::update(&mut hasher, &buf[..n]);
            }
            Ok(hex::encode(sha1::Digest::finalize(hasher)))
        }

        // ── SHA-224 ───────────────────────────────────────────────────────────
        Algorithm::Sha224 => {
            let mut hasher = sha2::Sha224::new();
            loop {
                let n = reader.read(&mut buf)?;
                if n == 0 { break; }
                sha2::Digest::update(&mut hasher, &buf[..n]);
            }
            Ok(hex::encode(sha2::Digest::finalize(hasher)))
        }

        // ── SHA-256 ───────────────────────────────────────────────────────────
        Algorithm::Sha256 => {
            let mut hasher = sha2::Sha256::new();
            loop {
                let n = reader.read(&mut buf)?;
                if n == 0 { break; }
                sha2::Digest::update(&mut hasher, &buf[..n]);
            }
            Ok(hex::encode(sha2::Digest::finalize(hasher)))
        }

        // ── SHA-384 ───────────────────────────────────────────────────────────
        Algorithm::Sha384 => {
            let mut hasher = sha2::Sha384::new();
            loop {
                let n = reader.read(&mut buf)?;
                if n == 0 { break; }
                sha2::Digest::update(&mut hasher, &buf[..n]);
            }
            Ok(hex::encode(sha2::Digest::finalize(hasher)))
        }

        // ── SHA-512 ───────────────────────────────────────────────────────────
        Algorithm::Sha512 => {
            let mut hasher = sha2::Sha512::new();
            loop {
                let n = reader.read(&mut buf)?;
                if n == 0 { break; }
                sha2::Digest::update(&mut hasher, &buf[..n]);
            }
            Ok(hex::encode(sha2::Digest::finalize(hasher)))
        }

        // ── CRC32 ─────────────────────────────────────────────────────────────
        // Résultat formaté sur 8 caractères hex avec zéros de remplissage (:08x)
        Algorithm::Crc32 => {
            let mut hasher = crc32fast::Hasher::new();
            loop {
                let n = reader.read(&mut buf)?;
                if n == 0 { break; }
                hasher.update(&buf[..n]);
            }
            Ok(format!("{:08x}", hasher.finalize()))
        }
    }
}
