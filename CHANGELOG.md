# Changelog

Toutes les modifications notables de ce projet sont documentées ici.

Le format suit [Keep a Changelog](https://keepachangelog.com/fr/1.1.0/)
et le projet respecte le [Versionnage sémantique](https://semver.org/lang/fr/) :
`MAJEUR.MINEUR.CORRECTIF`, tags Git `vX.Y.Z` identiques à la version de `Cargo.toml`.

## [Non publié]

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

[Non publié]: https://github.com/rusty-suite/hash_checker/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/rusty-suite/hash_checker/compare/v0.2.1...v0.3.0
[0.2.1]: https://github.com/rusty-suite/hash_checker/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/rusty-suite/hash_checker/compare/Win_%26_Linux...v0.2.0
[0.1.0]: https://github.com/rusty-suite/hash_checker/releases/tag/Win_%26_Linux
