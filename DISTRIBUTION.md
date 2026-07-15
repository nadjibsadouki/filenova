FileNova distributions

This workspace provides two distribution options for FileNova (version 0.1.0):

1) FileNova Standard
- Lightweight installer (current generated NSIS and MSI in `src-tauri/target/release/bundle/`).
- Uses the current WebView2 behavior: if WebView2 is missing, the installer attempts to download the WebView2 bootstrapper from Microsoft at install time.
- Intended for users with an Internet connection.

2) FileNova Offline
- Offline-ready installer that embeds the WebView2 Evergreen offline installer into the NSIS installer so the product can be installed without Internet access.
- Includes PDFium and all resources (same as the standard installer).
- Future offline installers must be generated through Tauri with `npm run tauri:build:offline`. The overlay `src-tauri/tauri.offline.conf.json` selects Tauri's real `offlineInstaller` WebView2 mode while inheriting the same application, PDFium, FFmpeg/FFprobe sidecars, shared DLLs, licenses, and resources as the standard bundle.

Notes
- The project keeps both distributions separate; existing `msi` and `setup.exe` files are not replaced.
- Installers are not code-signed by default in this repo.
- The preserved v0.1.0 offline artifact was created by an older manual NSIS flow. Its embedded 1,688,792-byte Microsoft Edge Update Setup file is a network bootstrapper, not the complete Evergreen Standalone Installer, so that artifact cannot be certified as fully offline.
- Do not reuse the ignored `src-tauri/target/release/nsis/x64/offline/installer-offline-full.nsi` flow for future releases. It does not inherit Tauri resource and sidecar changes automatically.
