# FileNova 0.2.0

## Informations générales

- Produit : FileNova
- Version : 0.2.0
- Plateforme : Windows
- Architecture : x64
- Langues : anglais, français et arabe
- Fonctionnement : entièrement local sur l'appareil
- Compte requis : aucun
- Abonnement requis : aucun
- Téléversement des fichiers : aucun

FileNova traite les fichiers localement. Les documents, images et vidéos sélectionnés ne sont pas envoyés vers un service distant.

## Fonctionnalités

### Images

- Compress Images
- Convert Images
- Resize Images

### PDF

- Merge PDFs
- Split PDF
- Organize Pages
- Images to PDF
- PDF to Images

### Vidéo

- Compress Video
- Convert Video
- Extract Audio
- Trim Video
- Video to GIF
- Social Media Tools

## Installateurs Windows x64

### Installateur Standard

- Fichier : `FileNova_0.2.0_x64-setup.exe`
- Chemin complet : `C:\Users\nadjibsadouki\Desktop\Filenova\src-tauri\target\release\bundle\nsis\FileNova_0.2.0_x64-setup.exe`
- Taille exacte : 46 381 575 octets
- SHA-256 : `27B18963D6787069DE4554DC080413FA96C322D04F0C5B444DE6ABE1AC9E9DD8`

L'installateur Standard est plus léger. Il utilise le mode WebView2 Standard de Tauri et peut télécharger WebView2 si le runtime requis n'est pas déjà disponible sur Windows.

### Installateur Offline

- Fichier : `FileNova_0.2.0_x64_offline-setup.exe`
- Chemin complet : `C:\Users\nadjibsadouki\Desktop\Filenova\src-tauri\target\release\bundle\nsis\FileNova_0.2.0_x64_offline-setup.exe`
- Taille exacte : 252 663 809 octets
- SHA-256 : `FB1B7EC11BF5BEAA70E35D87D83337DE60B7F47D4031CFC9B2A4DEC2835D7E7B`

L'installateur Offline est plus volumineux, car il intègre `MicrosoftEdgeWebView2RuntimeInstallerX64.exe`, l'installateur autonome WebView2 Evergreen x64. Il permet d'installer WebView2 sans téléchargement Internet.

## Ressources de production intégrées

Les deux éditions contiennent :

- PDFium et `pdfium.dll` ;
- FFmpeg et `ffmpeg.exe` ;
- FFprobe et `ffprobe.exe` ;
- les sept DLL d'exécution FFmpeg :
  - `avcodec-61.dll` ;
  - `avdevice-61.dll` ;
  - `avfilter-10.dll` ;
  - `avformat-61.dll` ;
  - `avutil-59.dll` ;
  - `swresample-5.dll` ;
  - `swscale-8.dll` ;
- les traductions anglaise, française et arabe ;
- les icônes et ressources Tauri nécessaires à l'application.

L'édition Offline contient en plus WebView2 Evergreen Offline Installer x64.

## Signature et avertissements

Les installateurs FileNova 0.2.0 ne disposent actuellement d'aucune signature numérique. Windows SmartScreen peut donc afficher un avertissement lors de leur téléchargement ou de leur première exécution.

Le build frontend Vite signale un chunk JavaScript supérieur à 500 kB après minification. Cet avertissement est une limitation non bloquante : la compilation, l'installation et l'exécution de FileNova réussissent.

## Recommandation de validation Offline

Avant une diffusion commerciale large, il est recommandé de tester `FileNova_0.2.0_x64_offline-setup.exe` sur une machine Windows x64 propre, déconnectée d'Internet et sans runtime WebView2 préinstallé. Ce test doit confirmer l'installation autonome de WebView2 et le lancement de FileNova sans accès réseau.
