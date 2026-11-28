# Changelog

Toutes les modifications notables de ce projet sont documentées ici.

Le format suit [Keep a Changelog](https://keepachangelog.com/fr/1.1.0/)
et le projet respecte le [Versionnage sémantique](https://semver.org/lang/fr/) :
`MAJEUR.MINEUR.CORRECTIF`, tags Git `vX.Y.Z` identiques à la version de `Cargo.toml`.

## [Non publié]

## [0.4.1] - 2026-11-28

### Corrigé
- Fenêtre trop haute sur un écran 1080p à 150 % : elle s'ouvre centrée, 560 pt de haut,
  au-dessus de la barre des tâches.

## [0.4.0] - 2026-11-28

### Ajouté
- Nouvelle interface conforme à la charte Rusty Suite (thème sombre, polices Inter et
  JetBrains Mono, logo animé, navigation clavier Alt+1 à Alt+5, F6, F1).
- Mise à jour automatique avec contrôle SHA-256 et retour à la version précédente si la
  nouvelle ne démarre pas.
- Journal de l'application avec rotation, ouvrable depuis les paramètres.
- Programme portable : langues, réglages et journal dans `hash_checker-data` à côté de
  l'exécutable.

### Modifié
- Source du hash choisie par un slider 3 positions ; résultats avec valeurs copiables.
- Menu contextuel activé ou désactivé par des sliders dans les paramètres.
- Langues gérées par la bibliothèque commune de la suite (rusty_kit) : packs mis à jour
  depuis GitHub seulement s'ils sont aussi récents que le programme.

### Corrigé
- `hash_checker <fichier> --hash …` ouvrait l'interface au lieu du mode terminal.

### Supprimé
- Ancienne fenêtre de langue, téléchargement via curl et communication inter-processus
  maison (remplacés par rusty_kit).

## [0.3.0] - 2026-11-28

### Modifié
- Licence : GNU GPL v3 ou ultérieure (`GPL-3.0-or-later`) à la place de PolyForm Noncommercial 1.0.0. Le programme devient un logiciel libre, utilisable par tous.

## [0.2.1] - 2026-11-27

### Corrigé
- La fenêtre « À propos » affichait la licence MIT : elle affiche désormais PolyForm Noncommercial 1.0.0,
  lue depuis `Cargo.toml` (qui la déclare maintenant).

## [0.2.0] - 2026-11-26

### Ajouté
- Ligne de commande traduite : aide (`--help`) et messages dans la langue active.
- Tests garantissant que toutes les langues ont les mêmes clés et que chaque clé utilisée existe.
- Binaire Linux x86_64 publié avec la release.

### Modifié
- Messages d'erreur de vérification traduits à l'affichage (ils suivent un changement de langue).
- Le libellé du menu contextuel (Windows, KDE, Thunar, Nautilus) suit la langue active.
- Un fichier de langue local incomplet est complété par sa version intégrée, puis par l'anglais.

### Corrigé
- La fenêtre de choix de langue affichait « Language / Langue » et des libellés anglais fixes.
- Plusieurs messages d'erreur restaient en français quelle que soit la langue.
- La désinstallation Nautilus retrouve le script quelle que soit la langue d'installation.

### Supprimé
- Code inutilisé (ancien constructeur, variante de source non utilisée).

## [0.1.0] - 2026-04-23

Première pré-release publique (tag historique `Win_&_Linux`).

[Non publié]: https://github.com/rusty-suite/hash_checker/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/rusty-suite/hash_checker/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/rusty-suite/hash_checker/compare/v0.2.1...v0.3.0
[0.2.1]: https://github.com/rusty-suite/hash_checker/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/rusty-suite/hash_checker/compare/Win_%26_Linux...v0.2.0
[0.1.0]: https://github.com/rusty-suite/hash_checker/releases/tag/Win_%26_Linux
