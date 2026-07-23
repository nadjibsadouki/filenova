# FileNova 0.2.1

## Release summary

FileNova 0.2.1 completes the application settings, appearance themes, and local Processing History across all Image, PDF, and Video tools.

All file processing remains local. FileNova requires no account or subscription and does not upload selected files.

## Included tools

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

### Video

- Compress Video
- Convert Video
- Extract Audio
- Trim Video
- Video to GIF
- Social Media Tools

## Settings and appearance

- English, French, and Arabic interface
- Arabic right-to-left layout
- Light, Dark, and System themes
- Persistent settings after restart
- Optional Processing History

## Processing History

- Image, PDF, and Video processing entries
- Completed, Failed, and Cancelled statuses
- Source and result names, complete result paths, sizes, durations, and errors
- Immediate updates without restarting FileNova
- Open File and Open Folder actions with safe availability checks
- Remove Entry and Clear History without deleting processed files
- Refresh and Clear Filters actions
- Serialized writes, identifier deduplication, and a 500-entry limit
- History failures isolated from file-processing operations

## Validation

- Frontend tests cover Image, PDF, Video, and Processing History integration.
- Rust tests cover the complete native processing library.
- TypeScript and Vite production builds complete without bypassing diagnostics.
- Original files remain preserved and temporary outputs are excluded from Processing History.

Installers are intentionally not generated as part of this release-preparation step.
