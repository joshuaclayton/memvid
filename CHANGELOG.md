# Changelog

All notable changes to Memvid will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed
- TOC entries no longer persist `search_text` that is reconstructable from
  the frame's stored payload and TOC fields; read paths derive it on
  demand. Cuts TOC size (and open-time decode cost) by roughly the corpus
  text size on text-heavy stores. Non-derivable text (explicit
  `search_text` differing from the payload, `no_raw` frames, skim
  extractions) persists exactly as before, and files written by older
  versions read unchanged.

### Fixed
- `update_frame` no longer inherits the superseded frame's `search_text`
  when the payload is replaced and no explicit text is passed: extraction
  re-derives it from the new payload, so updated frames are searchable by
  their new text instead of the old text they replaced.

### Added
- Initial public release of Memvid core library
- Single-file `.mv2` format for portable AI memory
- Full-text search with BM25 ranking (Tantivy)
- Vector similarity search with HNSW
- PDF, DOCX, XLSX document ingestion
- CLIP visual embeddings for image search
- Whisper audio transcription
- Timeline queries for chronological browsing
- Crash-safe WAL-based writes
- Blake3 checksums for data integrity
- Ed25519 signatures for authenticity
- Optional AES-256-GCM encryption

### Security
- Embedded WAL prevents data corruption
- Atomic commits ensure consistency
- File locking prevents concurrent write conflicts

## [2.0.0] - 2026-01-05

### Added
- Complete rewrite in Rust for performance and safety
- New `.mv2` file format (single-file, no sidecars)
- Append-only frame-based architecture
- Built-in full-text and vector search
- Cross-platform support (macOS, Linux, Windows)

### Changed
- Migrated from Python to Rust
- New API design focused on simplicity
- Improved memory efficiency

### Removed
- Legacy Python implementation
- QR code video encoding (replaced with efficient binary format)

---

[Unreleased]: https://github.com/memvid/memvid/compare/v2.0.0...HEAD
[2.0.0]: https://github.com/memvid/memvid/releases/tag/v2.0.0
