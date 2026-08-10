// Safe expect: guaranteed non-empty iterators after length check.
#![allow(clippy::expect_used)]
use std::collections::HashMap;

use crate::memvid::lifecycle::Memvid;
use crate::memvid::mutation::{augment_search_text, extract_via_registry};
use crate::types::Frame;
use crate::types::{
    EmbeddingIdentity, EmbeddingIdentityCount, EmbeddingIdentitySummary, FrameRole, FrameStatus,
};
use crate::{DEFAULT_SEARCH_TEXT_LIMIT, MemvidError, Result, normalize_text};

impl Memvid {
    /// Returns the vector index dimension stored in the MV2 file, if available.
    /// This is useful for auto-detecting which embedding model was used to create the file.
    #[must_use]
    pub fn vec_index_dimension(&self) -> Option<u32> {
        self.toc
            .indexes
            .vec
            .as_ref()
            .map(|manifest| manifest.dimension)
            .filter(|dim| *dim > 0)
    }

    /// Returns the best-effort vector index dimension for this memory.
    ///
    /// This is segment-aware and supports both:
    /// - Monolithic vector index manifests (`toc.indexes.vec.dimension`)
    /// - Segment-based vector storage (`toc.segment_catalog.vec_segments[*].dimension`)
    ///
    /// Returns `Ok(None)` when no vector dimension information exists.
    /// Returns an error if multiple, conflicting dimensions are detected.
    pub fn effective_vec_index_dimension(&self) -> Result<Option<u32>> {
        let manifest_dim = self
            .toc
            .indexes
            .vec
            .as_ref()
            .map(|manifest| manifest.dimension)
            .filter(|dim| *dim > 0);

        let mut segment_dim: Option<u32> = None;
        for descriptor in &self.toc.segment_catalog.vec_segments {
            if descriptor.dimension == 0 {
                continue;
            }
            match segment_dim {
                None => segment_dim = Some(descriptor.dimension),
                Some(existing) if existing == descriptor.dimension => {}
                Some(existing) => {
                    return Err(MemvidError::InvalidToc {
                        reason: format!(
                            "mixed vector dimensions detected in segment catalog: {} and {}",
                            existing, descriptor.dimension
                        )
                        .into(),
                    });
                }
            }
        }

        match (manifest_dim, segment_dim) {
            (Some(manifest), Some(segment)) if manifest != segment => {
                Err(MemvidError::InvalidToc {
                    reason: format!(
                        "vector dimension mismatch between manifest ({manifest}) and segment catalog ({segment})"
                    )
                    .into(),
                })
            }
            (Some(manifest), _) => Ok(Some(manifest)),
            (None, Some(segment)) => Ok(Some(segment)),
            (None, None) => Ok(None),
        }
    }

    /// Summarize the embedding identity used by this memory, if present.
    ///
    /// The identity is inferred from per-frame `extra_metadata` keys:
    /// - `memvid.embedding.provider`
    /// - `memvid.embedding.model`
    /// - `memvid.embedding.dimension` (optional)
    /// - `memvid.embedding.normalized` (optional)
    ///
    /// To preserve `.mv2` binary compatibility, this metadata is stored per-frame rather than in
    /// the TOC schema. This helper scans up to `max_frames` active frames and returns:
    /// - `Unknown` if no identity metadata is present
    /// - `Single` if exactly one identity is observed
    /// - `Mixed` if multiple identities are observed (counts included, descending)
    #[must_use]
    pub fn embedding_identity_summary(&self, max_frames: usize) -> EmbeddingIdentitySummary {
        let mut counts: HashMap<EmbeddingIdentity, u64> = HashMap::new();
        let mut scanned = 0usize;

        for frame in &self.toc.frames {
            if frame.status != FrameStatus::Active {
                continue;
            }
            if scanned >= max_frames {
                break;
            }
            scanned += 1;

            if let Some(identity) = EmbeddingIdentity::from_extra_metadata(&frame.extra_metadata) {
                *counts.entry(identity).or_insert(0) += 1;
            }
        }

        if counts.is_empty() {
            return EmbeddingIdentitySummary::Unknown;
        }
        if counts.len() == 1 {
            return EmbeddingIdentitySummary::Single(
                counts
                    .into_iter()
                    .next()
                    .map(|(identity, _)| identity)
                    .expect("counts.len()==1 implies one entry"),
            );
        }

        let mut identities: Vec<EmbeddingIdentityCount> = counts
            .into_iter()
            .map(|(identity, count)| EmbeddingIdentityCount { identity, count })
            .collect();
        identities.sort_by(|a, b| b.count.cmp(&a.count));
        EmbeddingIdentitySummary::Mixed(identities)
    }

    pub(crate) fn render_binary_summary(len: usize) -> String {
        if len == 0 {
            "<binary payload: 0 bytes>".into()
        } else {
            format!("<binary payload: {len} bytes>")
        }
    }

    pub(crate) fn augment_text_for_frame(
        &mut self,
        base: Option<String>,
        frame: &Frame,
    ) -> Option<String> {
        augment_search_text(
            base,
            frame.uri.as_deref(),
            frame.title.as_deref(),
            frame.track.as_deref(),
            &frame.tags,
            &frame.labels,
            &frame.extra_metadata,
            &frame.content_dates,
            frame.metadata.as_ref(),
        )
    }

    /// The frame's search text: the persisted TOC copy when present,
    /// otherwise reconstructed from the payload and TOC fields with the
    /// same rules the write path used to produce it.
    pub(crate) fn frame_search_text(&mut self, frame: &Frame) -> Result<String> {
        if let Some(text) = &frame.search_text {
            return Ok(text.clone());
        }
        self.derive_frame_search_text(frame)
    }

    /// Reconstruct the search text a frame's TOC entry would have carried.
    /// Mirrors the write path per frame shape:
    /// - chunk children: normalized chunk payload text
    /// - chunked parents: normalized first-chunk text
    /// - other frames: augmented normalized extraction of the payload
    pub(crate) fn derive_frame_search_text(&mut self, frame: &Frame) -> Result<String> {
        if frame.role == FrameRole::DocumentChunk {
            let bytes = self.frame_canonical_bytes(frame)?;
            let text = match String::from_utf8(bytes) {
                Ok(text) => text,
                Err(err) => Self::render_binary_summary(err.into_bytes().len()),
            };
            return Ok(normalize_text(&text, DEFAULT_SEARCH_TEXT_LIMIT)
                .map(|n| n.text)
                .unwrap_or_default());
        }

        if frame.chunk_manifest.is_some() {
            let payloads = self.document_chunk_payloads(frame)?;
            let Some((_, bytes)) = payloads.into_iter().next() else {
                return Ok(String::new());
            };
            let text = String::from_utf8_lossy(&bytes);
            return Ok(normalize_text(&text, DEFAULT_SEARCH_TEXT_LIMIT)
                .map(|n| n.text)
                .unwrap_or_default());
        }

        // The base must match the write-side derivability comparison exactly
        // (extraction only, no fallback), or reconstructed text would diverge
        // from what the write path decided was droppable.
        let base = if frame.payload_length == 0 {
            None
        } else {
            let bytes = self.frame_canonical_bytes(frame)?;
            let mime_hint = frame.metadata.as_ref().and_then(|m| m.mime.as_deref());
            let uri_hint = frame.uri.as_deref();
            extract_via_registry(&bytes, mime_hint, uri_hint)
                .ok()
                .and_then(|doc| doc.text)
                .and_then(|text| normalize_text(&text, DEFAULT_SEARCH_TEXT_LIMIT).map(|n| n.text))
        };
        Ok(self.augment_text_for_frame(base, frame).unwrap_or_default())
    }
}
