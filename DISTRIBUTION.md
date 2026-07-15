FileNova distributions

This workspace provides two distribution options for FileNova (version 0.1.0):

1) FileNova Standard
- Lightweight installer (current generated NSIS and MSI in `src-tauri/target/release/bundle/`).
- Uses the current WebView2 behavior: if WebView2 is missing, the installer attempts to download the WebView2 bootstrapper from Microsoft at install time.
- Intended for users with an Internet connection.

2) FileNova Offline
- Offline-ready installer that embeds the WebView2 Evergreen offline installer into the NSIS installer so the product can be installed without Internet access.
- Includes PDFium and all resources (same as the standard installer).
- The offline installer is generated from `src-tauri/target/release/nsis/x64/offline/installer-offline-full.nsi` using an NSIS compiler and requires placing the Evergreen offline WebView2 runtime file `MicrosoftEdgeWebView2RuntimeInstaller.exe` in the same folder prior to build.

Notes
- The project keeps both distributions separate; existing `msi` and `setup.exe` files are not replaced.
- Installers are not code-signed by default in this repo.
- To build the offline installer, run the helper `build-offline.ps1` in `src-tauri/target/release/nsis/x64/offline/` which will attempt to download NSIS and the WebView2 runtime and then you can run `makensis` to compile `installer-offline-full.nsi`.
- See the repo `src-tauri/target/release/nsis/x64/offline/README-build.txt` for exact commands (or run the commands shown in the project root README).