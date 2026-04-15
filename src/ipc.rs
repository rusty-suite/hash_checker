// =============================================================================
// ipc.rs — Communication inter-processus (IPC) pour instance unique
//
// Problème résolu :
//   Windows lance un processus séparé par fichier quand l'utilisateur
//   sélectionne plusieurs fichiers dans l'explorateur et choisit
//   "Vérifier l'intégrité". Sans IPC, on obtiendrait N fenêtres.
//
// Solution :
//   On utilise un serveur TCP sur localhost avec un port aléatoire.
//   Le port est écrit dans un fichier temporaire à l'ouverture.
//   Les instances suivantes lisent ce port, y envoient leur chemin,
//   et quittent immédiatement. La première instance accumule tous
//   les chemins reçus et les affiche dans une interface unifiée.
//
// Protocole :
//   Client → Serveur : un chemin de fichier par ligne, encodé en UTF-8.
//   La connexion est fermée après envoi (pas de réponse attendue).
//
// Sécurité :
//   Le serveur n'accepte que des connexions depuis 127.0.0.1.
//   Le nom du fichier de port inclut le nom d'utilisateur pour éviter
//   les conflits entre sessions sur un serveur multi-utilisateurs.
// =============================================================================

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

// -----------------------------------------------------------------------------
// Chemin du fichier temporaire contenant le port du serveur actif.
// Inclut le nom d'utilisateur pour éviter les conflits entre sessions.
// -----------------------------------------------------------------------------
fn port_file() -> PathBuf {
    // Préférence : USERNAME (Windows) puis USER (Unix)
    let user = std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "default".to_string());
    std::env::temp_dir().join(format!("hash_checker_{}.port", user))
}

// Lit le port enregistré par une instance déjà ouverte.
// Retourne None si le fichier n'existe pas ou si son contenu n'est pas valide.
fn read_existing_port() -> Option<u16> {
    std::fs::read_to_string(port_file())
        .ok()?
        .trim()
        .parse()
        .ok()
}

// -----------------------------------------------------------------------------
// Tente d'envoyer une liste de chemins à une instance déjà ouverte.
//
// Retourne true si la connexion a réussi et que les chemins ont été envoyés.
// Retourne false si aucune instance n'est active ou si la connexion échoue
// (l'instance précédente a pu se fermer sans nettoyer le fichier de port).
//
// En cas de retour false, l'appelant doit démarrer sa propre instance.
// -----------------------------------------------------------------------------
pub fn send_to_existing(paths: &[PathBuf]) -> bool {
    let Some(port) = read_existing_port() else {
        return false; // Pas de fichier de port → pas d'instance active
    };

    let addr: std::net::SocketAddr = match format!("127.0.0.1:{}", port).parse() {
        Ok(a) => a,
        Err(_) => return false,
    };

    // Timeout court : si personne n'écoute on le sait rapidement
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(300)) else {
        // Connexion refusée = l'instance précédente s'est fermée sans cleanup
        return false;
    };

    for path in paths {
        // Format : une ligne par chemin, terminée par '\n'
        let line = format!("{}\n", path.display());
        if stream.write_all(line.as_bytes()).is_err() {
            return false; // Connexion interrompue en cours d'envoi
        }
    }

    true
}

// -----------------------------------------------------------------------------
// Démarre le serveur IPC en arrière-plan.
//
// Lance un thread d'écoute sur 127.0.0.1 avec un port aléatoire (port=0,
// l'OS choisit un port libre). Le port est écrit dans le fichier temporaire
// pour que les instances suivantes puissent s'y connecter.
//
// Chaque chemin reçu est transmis via le Receiver retourné. L'appelant
// (la GUI) doit appeler try_recv() à chaque frame pour récupérer les
// nouveaux fichiers arrivés via IPC.
//
// Si le serveur ne peut pas démarrer (port occupé, erreur système), la
// fonction retourne quand même un Receiver valide mais vide — l'application
// fonctionne normalement sans IPC.
// -----------------------------------------------------------------------------
pub fn start_server() -> Receiver<PathBuf> {
    let (tx, rx) = mpsc::channel::<PathBuf>();

    // Port 0 : le système d'exploitation assigne un port libre automatiquement
    let listener = match TcpListener::bind("127.0.0.1:0") {
        Ok(l) => l,
        Err(_) => {
            // Impossible de créer le serveur : on retourne un receiver vide
            // et l'application continue sans IPC
            return rx;
        }
    };

    // Récupère le port choisi par l'OS et l'écrit dans le fichier temporaire
    let port = listener.local_addr().unwrap().port();
    let _ = std::fs::write(port_file(), port.to_string());

    // Thread d'écoute principal : accepte les connexions entrantes
    thread::spawn(move || {
        for stream in listener.incoming() {
            if let Ok(stream) = stream {
                let tx_clone = tx.clone();
                // Un thread par connexion pour ne pas bloquer les suivantes
                thread::spawn(move || handle_client(stream, tx_clone));
            }
        }
    });

    rx
}

// -----------------------------------------------------------------------------
// Traite une connexion cliente : lit les chemins ligne par ligne et les
// envoie dans le channel de l'interface graphique.
//
// Chaque ligne non vide est interprétée comme un chemin de fichier.
// Les chemins inexistants sont silencieusement ignorés (le fichier a pu
// être supprimé entre le moment du clic droit et la connexion IPC).
// -----------------------------------------------------------------------------
fn handle_client(stream: TcpStream, tx: Sender<PathBuf>) {
    let reader = BufReader::new(stream);
    for line in reader.lines().flatten() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let path = PathBuf::from(trimmed);
        if path.exists() && path.is_file() {
            let _ = tx.send(path); // Ignore l'erreur si le receiver est fermé
        }
    }
}

// -----------------------------------------------------------------------------
// Nettoie le fichier de port à la fermeture de l'application.
//
// À appeler depuis main() après que eframe::run_native() a retourné.
// Sans ce nettoyage, une future instance trouverait un fichier de port
// périmé et tenterait une connexion qui échouerait (comportement géré,
// mais mieux vaut nettoyer proprement).
// -----------------------------------------------------------------------------
pub fn cleanup() {
    let _ = std::fs::remove_file(port_file());
}
