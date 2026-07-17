# TODO - Fix isolation/isolation test failure for video_gif

- [ ] Reproduire l’échec en capturant l’assertion exacte (idéalement via sortie copiable côté terminal).
- [x] Implémenter correction: rendu plus robuste via garantie de stabilité IO après rename.
- [x] Ajouter une attente/fence d’IO après `fs::rename` (stabilité de taille de fichier) **sans** relâcher les assertions.

- [ ] Vérifier nettoyage: s’assurer que tous les fichiers temporaires (palette, tmp gif) sont supprimés, même en cas d’erreur.
- [ ] Exécuter 3 fois: `cargo test video_gif::tests -- --test-threads=1`
- [ ] Exécuter suite complète: `cargo test --lib -- --test-threads=1`
- [ ] Depuis la racine: `npm run build` et `git diff --check`

