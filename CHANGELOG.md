# Changelog

All notable changes to FileNova are documented in this file.

## [0.2.1] - 2026-07-23

### Added

- Complete Settings workflow with persistent language, appearance, output, and privacy preferences.
- Functional Light, Dark, and System themes.
- Complete local Processing History for Image, PDF, and Video tools, with a voluntary 500-entry limit.
- Open File, Open Folder, Remove Entry, Clear History, Refresh, and Clear Filters actions.
- Immediate history updates, safe handling of unavailable results, localized metadata, and Arabic right-to-left support.

### Changed

- Improved processing-history serialization, deduplication, error isolation, and status validation.
- Expanded frontend and Rust coverage for file processing, Unicode paths, cancellation, temporary-output cleanup, and original-file preservation.
- Removed obsolete provisional interface text and stale internal implementation notes.

### Fixed

- Prevented history storage failures from interrupting successful file processing.
- Prevented incomplete, temporary, failed, or cancelled outputs from being recorded as completed results.

## [0.2.0] - 2026-07-16

### Added

- Complete image workspace with compression, conversion, and resizing tools.
- Complete PDF workspace for merging, splitting, organizing, image-to-PDF, and PDF-to-image workflows.
- Complete video workspace for compression, conversion, audio extraction, trimming, GIF creation, and social-media exports.
- English, French, and Arabic interface support, including Arabic right-to-left layouts.

### Changed

- Consolidated all completed video tools under a single Video Tools navigation hub.
- Prepared consistent FileNova 0.2.0 package metadata for Windows x64.

### Privacy

- Processing remains local, with no file uploads, account, or subscription required.
