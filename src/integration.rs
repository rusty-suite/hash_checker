// =============================================================================
// integration.rs — Intégration menu contextuel du système d'exploitation
//
// Ce module permet d'ajouter ou retirer Hash Checker du menu clic droit
// de l'explorateur de fichiers, sur Windows et Linux.
//
// Chaque plateforme a sa propre méthode d'intégration :
//
//   Windows    → Registre Windows (HKCU\Software\Classes\*\shell\...)
//                L'OS appelle le programme avec le chemin du fichier en argument.
//                MultiSelectModel=Player : une instance par fichier sélectionné.
//
//   Linux GNOME (Nautilus) → Script bash dans ~/.local/share/nautilus/scripts/
//                Nautilus exécute le script avec les fichiers sélectionnés en $@.
//
//   Linux KDE (Dolphin) → Fichier .desktop dans ~/.local/share/kio/servicemenus/
//                KDE lit ce fichier pour ajouter l'action dans Dolphin.
//
//   Linux XFCE (Thunar) → Entrée XML dans ~/.config/Thunar/uca.xml
//                Thunar lit ce fichier pour afficher des actions personnalisées.
//
// Les fonctions sont compilées conditionnellement avec #[cfg(windows)] et
// #[cfg(unix)] pour éviter d'inclure du code inutile selon la plateforme cible.
// =============================================================================

// -----------------------------------------------------------------------------
// État de l'intégration pour chaque gestionnaire de fichiers supporté.
// `Option<bool>` : None = non applicable sur ce système, Some(true/false) = état.
// -----------------------------------------------------------------------------
#[derive(Debug, Clone)]
pub struct IntegrationStatus {
    pub windows_registry: Option<bool>, // None si on n'est pas sur Windows
    pub linux_nautilus: Option<bool>,   // None si on n'est pas sur Linux
    pub linux_kde: Option<bool>,
    pub linux_thunar: Option<bool>,
}

impl IntegrationStatus {
    /// Détecte l'état actuel de chaque intégration sur le système en cours.
    /// Appelé au démarrage de la GUI et après chaque activation/désactivation.
    pub fn detect() -> Self {
        Self {
            windows_registry: detect_windows(),
            linux_nautilus: detect_nautilus(),
            linux_kde: detect_kde(),
            linux_thunar: detect_thunar(),
        }
    }

    /// Retourne true si au moins une intégration est active (utile pour affichage).
    pub fn any_installed(&self) -> bool {
        self.windows_registry == Some(true)
            || self.linux_nautilus == Some(true)
            || self.linux_kde == Some(true)
            || self.linux_thunar == Some(true)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Section Windows — Registre système
//
// L'intégration Windows fonctionne via le registre :
//   HKCU\Software\Classes\*\shell\Hash Checker\
//     (Default)         = texte affiché dans le menu clic droit
//     Icon              = icône du programme (chemin,index)
//     MultiSelectModel  = "Player" → une instance par fichier sélectionné
//     command\(Default) = commande à exécuter avec %1 = chemin du fichier
//
// On utilise HKCU (utilisateur courant) et non HKLM (machine)
// pour ne pas nécessiter les droits administrateur.
// ─────────────────────────────────────────────────────────────────────────────

/// Vérifie si l'entrée registre existe déjà (Windows uniquement)
#[cfg(windows)]
fn detect_windows() -> Option<bool> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    Some(
        hkcu.open_subkey(r"Software\Classes\*\shell\Hash Checker")
            .is_ok(), // true si la clé existe
    )
}

/// Sur Linux/macOS : Windows n'est pas disponible → retourne None
#[cfg(not(windows))]
fn detect_windows() -> Option<bool> {
    None
}

/// Crée les clés de registre nécessaires pour le menu clic droit Windows
#[cfg(windows)]
pub fn install_windows(exe_path: &str) -> Result<(), String> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);

    // Crée la clé principale (crée aussi les sous-clés manquantes automatiquement)
    let (shell_key, _) = hkcu
        .create_subkey(r"Software\Classes\*\shell\Hash Checker")
        .map_err(|e| e.to_string())?;

    // Texte affiché dans le menu contextuel de l'explorateur
    shell_key
        .set_value("", &"Vérifier l'intégrité (Hash Checker)")
        .map_err(|e| e.to_string())?;
    // Icône : chemin vers l'exe suivi de ,0 pour prendre la première icône embarquée
    shell_key
        .set_value("Icon", &format!("{},0", exe_path))
        .map_err(|e| e.to_string())?;
    // Player = une instance du programme lancée par fichier sélectionné
    shell_key
        .set_value("MultiSelectModel", &"Player")
        .map_err(|e| e.to_string())?;

    // Sous-clé "command" contenant la commande réelle à exécuter
    // %1 sera remplacé par le chemin complet du fichier cliqué
    let (cmd_key, _) = shell_key
        .create_subkey("command")
        .map_err(|e| e.to_string())?;
    cmd_key
        .set_value("", &format!("\"{}\" \"%1\"", exe_path))
        .map_err(|e| e.to_string())?;

    Ok(())
}

#[cfg(not(windows))]
pub fn install_windows(_exe_path: &str) -> Result<(), String> {
    Err("Non disponible sur ce système.".to_string())
}

/// Supprime récursivement toutes les clés de registre créées par install_windows
#[cfg(windows)]
pub fn uninstall_windows() -> Result<(), String> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    hkcu.delete_subkey_all(r"Software\Classes\*\shell\Hash Checker")
        .map_err(|e| e.to_string())
}

#[cfg(not(windows))]
pub fn uninstall_windows() -> Result<(), String> {
    Err("Non disponible sur ce système.".to_string())
}

// ─────────────────────────────────────────────────────────────────────────────
// Section Linux — Nautilus (gestionnaire de fichiers GNOME/Ubuntu)
//
// Nautilus affiche dans son menu clic droit tous les scripts exécutables
// présents dans ~/.local/share/nautilus/scripts/.
// Le nom du script devient le libellé du menu.
//
// Notre script bash reçoit les fichiers sélectionnés dans $@ et lance
// une instance de hash_checker pour chacun (&  = en arrière-plan).
// ─────────────────────────────────────────────────────────────────────────────

/// Chemin du script Nautilus à créer/supprimer
#[cfg(unix)]
fn nautilus_script_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    std::path::PathBuf::from(home)
        .join(".local/share/nautilus/scripts/Vérifier le hash")
}

/// Vérifie si le script Nautilus existe déjà
#[cfg(unix)]
fn detect_nautilus() -> Option<bool> {
    Some(nautilus_script_path().exists())
}

#[cfg(not(unix))]
fn detect_nautilus() -> Option<bool> {
    None
}

/// Crée le script bash et le rend exécutable (chmod 755)
#[cfg(unix)]
pub fn install_nautilus(exe_path: &str) -> Result<(), String> {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let script_path = nautilus_script_path();
    // Crée le répertoire scripts/ s'il n'existe pas
    if let Some(parent) = script_path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    // Contenu du script bash : lance hash_checker pour chaque fichier sélectionné
    // $@ contient tous les fichiers passés par Nautilus au script
    let script = format!(
        "#!/bin/bash\nfor f in \"$@\"; do\n  \"{}\" \"$f\" &\ndone\n",
        exe_path
    );
    fs::write(&script_path, script).map_err(|e| e.to_string())?;

    // Rend le script exécutable (rwxr-xr-x = 0o755)
    // Sans ça, Nautilus ne l'afficherait pas dans le menu
    let mut perms = fs::metadata(&script_path)
        .map_err(|e| e.to_string())?
        .permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script_path, perms).map_err(|e| e.to_string())?;

    Ok(())
}

#[cfg(not(unix))]
pub fn install_nautilus(_exe_path: &str) -> Result<(), String> {
    Err("Non disponible sur ce système.".to_string())
}

/// Supprime le script Nautilus
#[cfg(unix)]
pub fn uninstall_nautilus() -> Result<(), String> {
    let path = nautilus_script_path();
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| e.to_string())
    } else {
        Ok(()) // Déjà absent, pas d'erreur
    }
}

#[cfg(not(unix))]
pub fn uninstall_nautilus() -> Result<(), String> {
    Err("Non disponible sur ce système.".to_string())
}

// ─────────────────────────────────────────────────────────────────────────────
// Section Linux — KDE Dolphin (gestionnaire de fichiers KDE Plasma)
//
// KDE utilise des fichiers .desktop placés dans ~/.local/share/kio/servicemenus/
// pour ajouter des actions dans le menu clic droit de Dolphin.
//
// Le format est un fichier .desktop standard avec une section [Desktop Action].
// %F = liste de tous les fichiers sélectionnés (passés à l'exécutable).
// ─────────────────────────────────────────────────────────────────────────────

/// Chemin du fichier .desktop KDE à créer/supprimer
#[cfg(unix)]
fn kde_service_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    std::path::PathBuf::from(home)
        .join(".local/share/kio/servicemenus/hash_checker.desktop")
}

/// Vérifie si le fichier de service KDE existe déjà
#[cfg(unix)]
fn detect_kde() -> Option<bool> {
    Some(kde_service_path().exists())
}

#[cfg(not(unix))]
fn detect_kde() -> Option<bool> {
    None
}

/// Crée le fichier .desktop pour l'intégration Dolphin/KDE
#[cfg(unix)]
pub fn install_kde(exe_path: &str) -> Result<(), String> {
    use std::fs;

    let path = kde_service_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    // Format .desktop KDE pour un service menu
    // MimeType : les types de fichiers sur lesquels l'action est proposée
    // %F : sera remplacé par la liste des fichiers sélectionnés
    let content = format!(
        "[Desktop Entry]\nType=Service\nServiceTypes=KonqPopupMenu/Plugin\nMimeType=application/octet-stream;application/x-iso9660-image;\nActions=hash_check\nX-KDE-Priority=TopLevel\n\n[Desktop Action hash_check]\nName=Vérifier l'intégrité (Hash Checker)\nIcon=security-high\nExec=\"{}\" %F\n",
        exe_path
    );

    fs::write(&path, content).map_err(|e| e.to_string())
}

#[cfg(not(unix))]
pub fn install_kde(_exe_path: &str) -> Result<(), String> {
    Err("Non disponible sur ce système.".to_string())
}

/// Supprime le fichier .desktop KDE
#[cfg(unix)]
pub fn uninstall_kde() -> Result<(), String> {
    let path = kde_service_path();
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| e.to_string())
    } else {
        Ok(())
    }
}

#[cfg(not(unix))]
pub fn uninstall_kde() -> Result<(), String> {
    Err("Non disponible sur ce système.".to_string())
}

// ─────────────────────────────────────────────────────────────────────────────
// Section Linux — Thunar (gestionnaire de fichiers XFCE)
//
// Thunar utilise un fichier XML ~/.config/Thunar/uca.xml (User Customizable Actions)
// qui liste toutes les actions personnalisées du menu clic droit.
//
// On injecte un bloc <action>...</action> dans ce fichier XML.
// Si le fichier n'existe pas, on le crée avec une structure minimale valide.
// %f = chemin du fichier sélectionné (variable Thunar).
// ─────────────────────────────────────────────────────────────────────────────

/// Chemin du fichier de configuration des actions Thunar
#[cfg(unix)]
fn thunar_uca_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    std::path::PathBuf::from(home).join(".config/Thunar/uca.xml")
}

/// Détecte si notre entrée est déjà présente dans uca.xml
#[cfg(unix)]
fn detect_thunar() -> Option<bool> {
    let path = thunar_uca_path();
    if !path.exists() {
        return Some(false); // Pas de fichier = pas d'intégration
    }
    // Cherche notre marqueur unique dans le contenu XML
    let content = std::fs::read_to_string(&path).unwrap_or_default();
    Some(content.contains("Hash Checker"))
}

#[cfg(not(unix))]
fn detect_thunar() -> Option<bool> {
    None
}

/// Ajoute notre entrée dans uca.xml (ou crée le fichier si absent)
#[cfg(unix)]
pub fn install_thunar(exe_path: &str) -> Result<(), String> {
    use std::fs;

    let path = thunar_uca_path();

    // Bloc XML de notre action personnalisée
    // unique-id : identifiant stable pour éviter les doublons
    // patterns   : * = applicable à tous les types de fichiers
    let entry = format!(
        "<action>\n  <icon>security-high</icon>\n  <name>Vérifier l'intégrité (Hash Checker)</name>\n  <unique-id>hash-checker-001</unique-id>\n  <command>\"{}\" \"%f\"</command>\n  <description>Vérifier le hash du fichier</description>\n  <patterns>*</patterns>\n  <directories/>\n  <audio-files/>\n  <image-files/>\n  <other-files/>\n  <text-files/>\n  <video-files/>\n</action>",
        exe_path
    );

    if path.exists() {
        let content = fs::read_to_string(&path).map_err(|e| e.to_string())?;
        if content.contains("Hash Checker") {
            return Ok(()); // Déjà présent, rien à faire
        }
        // Insère notre bloc juste avant la balise fermante </actions>
        let updated = content.replace("</actions>", &format!("{}\n</actions>", entry));
        fs::write(&path, updated).map_err(|e| e.to_string())
    } else {
        // Le fichier uca.xml n'existe pas encore → on le crée de zéro
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let xml = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<actions>\n{}\n</actions>\n",
            entry
        );
        fs::write(&path, xml).map_err(|e| e.to_string())
    }
}

#[cfg(not(unix))]
pub fn install_thunar(_exe_path: &str) -> Result<(), String> {
    Err("Non disponible sur ce système.".to_string())
}

/// Retire notre bloc <action> du fichier uca.xml de Thunar
#[cfg(unix)]
pub fn uninstall_thunar() -> Result<(), String> {
    let path = thunar_uca_path();
    if !path.exists() {
        return Ok(()); // Fichier absent = déjà propre
    }
    let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    // Filtre ligne par ligne pour retirer le bloc contenant "Hash Checker"
    let result = remove_xml_action(&content, "Hash Checker");
    std::fs::write(&path, result).map_err(|e| e.to_string())
}

#[cfg(not(unix))]
pub fn uninstall_thunar() -> Result<(), String> {
    Err("Non disponible sur ce système.".to_string())
}

// -----------------------------------------------------------------------------
// Supprime un bloc <action>...</action> d'un XML en se basant sur un marqueur.
// Parcourt le fichier ligne par ligne : dès qu'on trouve <action> suivi du
// marqueur dans le reste du texte, on passe en mode "skip" jusqu'à </action>.
// -----------------------------------------------------------------------------
fn remove_xml_action(xml: &str, marker: &str) -> String {
    let mut result = String::new();
    let mut skip = false;
    for line in xml.lines() {
        if line.contains("<action>") && xml[xml.find(line).unwrap_or(0)..].contains(marker) {
            skip = true; // Début du bloc à supprimer
        }
        if !skip {
            result.push_str(line);
            result.push('\n');
        }
        if skip && line.contains("</action>") {
            skip = false; // Fin du bloc à supprimer, on reprend la copie
        }
    }
    result
}

// -----------------------------------------------------------------------------
// Retourne le chemin absolu de l'exécutable hash_checker en cours d'exécution.
// Utilisé pour écrire le chemin correct dans le registre / les scripts.
// -----------------------------------------------------------------------------
pub fn current_exe_path() -> String {
    std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "hash_checker".to_string())
}
