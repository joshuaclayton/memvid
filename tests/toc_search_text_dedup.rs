//! TOC `search_text` dedup: text that is derivable from a frame's stored
//! payload (plus TOC fields) is not persisted per-entry in the TOC; read
//! paths reconstruct it on demand. Non-derivable text (explicit
//! `search_text` differing from the payload, `no_raw` frames, skim
//! extractions) keeps persisting exactly as before.

use memvid_core::{FrameRole, Memvid, PutOptions, SearchRequest};
use tempfile::TempDir;

fn req(query: &str) -> SearchRequest {
    SearchRequest {
        query: query.to_string(),
        top_k: 10,
        snippet_chars: 200,
        uri: None,
        scope: None,
        cursor: None,
        #[cfg(feature = "temporal_track")]
        temporal: None,
        as_of_frame: None,
        as_of_ts: None,
        no_sketch: false,
        acl_context: None,
        acl_enforcement_mode: memvid_core::types::AclEnforcementMode::Audit,
    }
}

fn put_text(mem: &mut Memvid, uri: &str, title: &str, text: &str) -> u64 {
    let opts = PutOptions {
        uri: Some(uri.to_string()),
        title: Some(title.to_string()),
        timestamp: Some(1_700_000_000),
        ..Default::default()
    };
    mem.put_bytes_with_options(text.as_bytes(), opts).unwrap()
}

/// A single text frame whose search text came from the extractor must not
/// carry a copy of the text in its TOC entry after commit.
#[test]
#[cfg(feature = "lex")]
fn single_text_frame_omits_toc_search_text() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("test.mv2");

    {
        let mut mem = Memvid::create(&path).unwrap();
        mem.enable_lex().unwrap();
        put_text(
            &mut mem,
            "mv2://energy/solar",
            "Energy Overview",
            "solar panels convert sunlight into electricity",
        );
        mem.commit().unwrap();
    }

    let mut mem = Memvid::open_read_only(&path).unwrap();
    let frame = mem.frame_by_id(0).unwrap();
    assert_eq!(
        frame.search_text, None,
        "extractor-derived search_text must not persist in the TOC"
    );

    let results = mem.search(req("sunlight")).unwrap();
    assert_eq!(results.hits.len(), 1, "body term must still match");
    assert!(
        !results.hits[0].text.trim().is_empty(),
        "snippet must render non-empty"
    );
    assert!(
        results.hits[0].text.to_lowercase().contains("sunlight"),
        "snippet must contain the match"
    );
}

/// Reconstructed search text must include the augmented field dump (title,
/// uri) exactly as the write path would have persisted it, so queries that
/// match on title/uri terms keep working after reopen.
#[test]
#[cfg(feature = "lex")]
fn and_query_spanning_body_and_title_still_matches() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("test.mv2");

    {
        let mut mem = Memvid::create(&path).unwrap();
        mem.enable_lex().unwrap();
        put_text(
            &mut mem,
            "mv2://materials/plastics",
            "Catalog of Compounds",
            "polymer chains form durable structures",
        );
        // Second frame shares the body term but not the title term, so an
        // implicit-AND query must select only the first frame.
        put_text(
            &mut mem,
            "mv2://materials/other",
            "Miscellaneous Notes",
            "polymer research continues elsewhere",
        );
        mem.commit().unwrap();
    }

    let mut mem = Memvid::open_read_only(&path).unwrap();
    let results = mem.search(req("polymer catalog")).unwrap();
    assert_eq!(
        results.hits.len(),
        1,
        "implicit AND across body + title terms must match exactly the titled frame"
    );
    assert_eq!(results.hits[0].uri, "mv2://materials/plastics");
}

/// Chunked documents must not duplicate chunk text into chunk-entry or
/// parent-entry search_text; late-chunk terms must still be searchable.
#[test]
#[cfg(feature = "lex")]
fn chunked_document_omits_chunk_and_parent_search_text() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("test.mv2");

    let body: String = (0..40)
        .map(|i| {
            format!(
                "passage token{i:02} covers wetland ecology in detail with ample padding sentences. "
            )
        })
        .collect();
    assert!(
        body.chars().count() > 2400,
        "document must be large enough to chunk"
    );

    {
        let mut mem = Memvid::create(&path).unwrap();
        mem.enable_lex().unwrap();
        put_text(&mut mem, "mv2://eco/wetlands", "Wetland Survey", &body);
        mem.commit().unwrap();
    }

    let mut mem = Memvid::open_read_only(&path).unwrap();
    let frame_count = mem.stats().unwrap().frame_count;
    assert!(frame_count > 2, "expected parent + multiple chunks");

    let mut saw_parent = false;
    let mut saw_chunk = false;
    for id in 0..frame_count {
        let frame = mem.frame_by_id(id).unwrap();
        if frame.chunk_manifest.is_some() {
            saw_parent = true;
            assert_eq!(
                frame.search_text, None,
                "chunked parent must not persist search_text (frame {id})"
            );
        }
        if frame.role == FrameRole::DocumentChunk {
            saw_chunk = true;
            assert_eq!(
                frame.search_text, None,
                "chunk child must not persist search_text (frame {id})"
            );
        }
    }
    assert!(saw_parent && saw_chunk);

    let results = mem.search(req("token35")).unwrap();
    assert!(
        !results.hits.is_empty(),
        "late-chunk term must remain searchable"
    );
}

/// Caller-supplied search text that differs from the payload is not
/// derivable and must keep persisting (and stay authoritative for search).
#[test]
#[cfg(feature = "lex")]
fn explicit_search_text_differing_from_payload_is_persisted() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("test.mv2");

    {
        let mut mem = Memvid::create(&path).unwrap();
        mem.enable_lex().unwrap();
        let opts = PutOptions {
            uri: Some("mv2://notes/1".to_string()),
            title: Some("Field Notes".to_string()),
            search_text: Some("gamma delta findings".to_string()),
            timestamp: Some(1_700_000_000),
            ..Default::default()
        };
        mem.put_bytes_with_options(b"alpha beta observations", opts)
            .unwrap();
        mem.commit().unwrap();
    }

    let mut mem = Memvid::open_read_only(&path).unwrap();
    let frame = mem.frame_by_id(0).unwrap();
    let persisted = frame
        .search_text
        .as_deref()
        .expect("explicit non-derivable search_text must persist");
    assert!(persisted.contains("gamma delta findings"));

    assert_eq!(
        mem.search(req("gamma")).unwrap().hits.len(),
        1,
        "explicit search text must match"
    );
    assert!(
        mem.search(req("alpha")).unwrap().hits.is_empty(),
        "payload text was never the search text; it must not match"
    );
}

/// `no_raw` frames store no payload, so their search text is the only copy
/// and must keep persisting.
#[test]
#[cfg(feature = "lex")]
fn no_raw_frame_persists_search_text() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("test.mv2");

    {
        let mut mem = Memvid::create(&path).unwrap();
        mem.enable_lex().unwrap();
        let opts = PutOptions {
            uri: Some("mv2://cards/1".to_string()),
            title: Some("Reference Card".to_string()),
            no_raw: true,
            timestamp: Some(1_700_000_000),
            ..Default::default()
        };
        mem.put_bytes_with_options(b"compact reference card text", opts)
            .unwrap();
        mem.commit().unwrap();
    }

    let mut mem = Memvid::open_read_only(&path).unwrap();
    let frame = mem.frame_by_id(0).unwrap();
    assert_eq!(frame.payload_length, 0, "no_raw stores no payload");
    assert!(
        frame.search_text.is_some(),
        "no_raw search_text is the only copy and must persist"
    );
    assert_eq!(mem.search(req("compact")).unwrap().hits.len(), 1);
}

/// Companion fix: replacing a frame's payload without passing search_text
/// must re-derive search text from the NEW payload — the update must be
/// searchable by its new text and no longer discoverable by the old text.
#[test]
#[cfg(feature = "lex")]
fn update_with_new_payload_is_searchable_by_new_text() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("test.mv2");

    let mut mem = Memvid::create(&path).unwrap();
    mem.enable_lex().unwrap();
    // auto_tag off: inherited auto-tags would legitimately keep the old
    // text's terms matchable through the tag dump, which is not what this
    // test is about.
    let opts = PutOptions {
        uri: Some("mv2://animals/1".to_string()),
        title: Some("Sighting Log".to_string()),
        timestamp: Some(1_700_000_000),
        auto_tag: false,
        ..Default::default()
    };
    mem.put_bytes_with_options(b"observed one aardvark near the ridge", opts)
        .unwrap();
    mem.commit().unwrap();

    let opts = PutOptions {
        uri: Some("mv2://animals/1".to_string()),
        title: Some("Sighting Log".to_string()),
        timestamp: Some(1_700_000_100),
        auto_tag: false,
        ..Default::default()
    };
    mem.update_frame(
        0,
        Some(b"observed one zebra near the ridge".to_vec()),
        opts,
        None,
    )
    .unwrap();
    mem.commit().unwrap();
    drop(mem);

    let mut mem = Memvid::open_read_only(&path).unwrap();
    let zebra = mem.search(req("zebra")).unwrap();
    assert_eq!(
        zebra.hits.len(),
        1,
        "updated frame must be searchable by its new payload text"
    );
    assert!(
        mem.search(req("aardvark")).unwrap().hits.is_empty(),
        "superseded text must no longer match"
    );

    let updated = mem.frame_by_id(zebra.hits[0].frame_id).unwrap();
    assert_eq!(
        updated.search_text, None,
        "re-derived search_text is derivable and must not persist"
    );
}

/// The fallback (non-lex) search path reconstructs search text per frame;
/// title terms must still match on a reopened store.
#[test]
fn fallback_search_without_lex_matches_title_terms() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("test.mv2");

    {
        let mut mem = Memvid::create(&path).unwrap();
        put_text(
            &mut mem,
            "mv2://recipes/7",
            "Sourdough Method",
            "mix flour water salt and starter",
        );
        mem.commit().unwrap();
    }

    let mut mem = Memvid::open_read_only(&path).unwrap();
    let results = mem.search(req("sourdough")).unwrap();
    assert_eq!(
        results.hits.len(),
        1,
        "title term must match through reconstruction on the fallback path"
    );
}

/// Vacuum and verify must hold on a store written in the new format.
#[test]
#[cfg(feature = "lex")]
fn vacuum_and_verify_hold_on_new_format() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("test.mv2");

    {
        let mut mem = Memvid::create(&path).unwrap();
        mem.enable_lex().unwrap();
        put_text(
            &mut mem,
            "mv2://logs/1",
            "Morning",
            "fog lifted over the harbor",
        );
        put_text(
            &mut mem,
            "mv2://logs/2",
            "Evening",
            "tide returned before dusk",
        );
        mem.commit().unwrap();
        mem.vacuum().unwrap();
    }

    Memvid::verify(&path, true).unwrap();

    let mut mem = Memvid::open_read_only(&path).unwrap();
    assert_eq!(mem.search(req("harbor")).unwrap().hits.len(), 1);
}
