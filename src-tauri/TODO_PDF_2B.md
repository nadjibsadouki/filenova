# TODO - Groupe 2B : connecter les outils PDF à Processing History

## Étape 1 — Analyse (fait)
- Vérifier que `PdfToolsPage.tsx` ne fait pas encore d’appels à l’historique pour Merge/Split/Organize/Images→PDF/PDF→Images.

## Étape 2 — Implémentation (à faire)
- Modifier `src/pages/PdfToolsPage.tsx` pour appeler `appendProcessingHistoryEntry/appendProcessingHistoryEntries` pour :
  - Merge PDFs (1 entrée)
  - Split PDF (1 entrée par fichier généré, batch)
  - Organize Pages (1 entrée)
  - Images to PDF (1 entrée)
  - PDF to Images (1 entrée par image créée, batch)
- Respecter `enableProcessingHistory` : ne créer aucune entrée si désactivé.
- Éviter les doublons : `id` unique mais stable pour un même résultat.
- Écrire : id, datetime, category=PDF, tool exact, noms sources, createdFileName, createdFilePath (chemin complet), status, sizes avant/après si disponible, durée, message d’erreur si échec.
- Ne pas enregistrer les fichiers temporaires.

## Étape 3 — Tests (à faire)
- Ajouter/completer tests ciblés pour :
  - Merge PDFs réussi
  - Split PDF avec plusieurs résultats
  - Organize Pages réussi
  - Images to PDF réussi
  - PDF to Images avec plusieurs images
  - PDF invalide
  - annulation
  - historique désactivé
  - absence de doublons
  - noms Unicode/espaces
  - tailles avant/après
  - durée
  - apparition immédiate dans l’historique

## Étape 4 — Validation
- Depuis `src-tauri` :
  - cargo test pdf_merge::tests -- --test-threads=1
  - cargo test pdf_split::tests -- --test-threads=1
  - cargo test pdf_organize::tests -- --test-threads=1
  - cargo test images_pdf::tests -- --test-threads=1
  - cargo test pdf_to_images::tests -- --test-threads=1
- Depuis la racine :
  - npm run build
  - git diff --check
  - npm run tauri dev

## Étape 5 — Bilan final
- Fichiers modifiés
- Outils PDF connectés
- Nombre tests réussis/échoués
- Résultat npm run build
- Résultat git diff --check
- Opérations testées manuellement
- Limitations restantes

