# FileNova

FileNova 0.2.0 is a private desktop workspace for everyday image, PDF, and video processing on Windows x64.

All processing runs locally on the user's computer. Files are never uploaded, and FileNova requires no account and no subscription. The interface is available in English, French, and Arabic, including right-to-left layout support for Arabic.

## Features

### Image Tools

- Compress Images
- Convert Images
- Resize Images

### PDF Tools

- Merge PDFs
- Split PDF
- Organize Pages
- Images to PDF
- PDF to Images

### Video Tools

- Compress Video
- Convert Video
- Extract Audio
- Trim Video
- Video to GIF
- Social Media Tools

## Platform

- Windows x64
- Local and offline file processing
- No file uploads
- No account required
- No subscription required
- English, French, and Arabic interface

## Development

FileNova is built with Tauri, Rust, React, TypeScript, and Vite. FFmpeg, FFprobe, and PDFium are bundled for local media and PDF processing. See `THIRD_PARTY_LICENSES.txt` for the corresponding notices.

```shell
npm install
npm run tauri dev
```

Run the validation suites with:

```shell
cd src-tauri
cargo test
cd ..
npm run build
```
