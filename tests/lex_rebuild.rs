//! What the lex index answers after updates and deletes force a full rebuild.

#![cfg(feature = "lex")]

use memvid_core::{Memvid, PutOptions, SearchRequest};
use std::path::Path;
use tempfile::TempDir;

const WORDS: [&str; 5] = ["alpha", "bravo", "charlie", "delta", "echo"];

fn uris_matching(mem: &mut Memvid, query: &str) -> Vec<String> {
    let response = mem
        .search(SearchRequest {
            query: query.to_string(),
            top_k: 10,
            snippet_chars: 200,
            uri: None,
            scope: None,
            cursor: None,
            frames: None,
            #[cfg(feature = "temporal_track")]
            temporal: None,
            as_of_frame: None,
            as_of_ts: None,
            no_sketch: false,
            acl_context: None,
            acl_enforcement_mode: memvid_core::types::AclEnforcementMode::Audit,
        })
        .unwrap();
    let mut uris: Vec<String> = response.hits.into_iter().map(|hit| hit.uri).collect();
    uris.sort();
    uris
}

/// Search matches uris and tags too, so neither carries a frame's words.
fn options(word: &str) -> PutOptions {
    PutOptions {
        uri: Some(uri(word)),
        auto_tag: false,
        extract_dates: false,
        ..Default::default()
    }
}

fn uri(word: &str) -> String {
    let position = WORDS.iter().position(|w| *w == word).unwrap();
    format!("mv2://doc/{position}")
}

fn edited_shard(path: &Path) -> Memvid {
    let mut mem = Memvid::create(path).unwrap();
    mem.enable_lex().unwrap();
    for word in WORDS {
        mem.put_bytes_with_options(format!("{word} report").as_bytes(), options(word))
            .unwrap();
    }
    mem.commit().unwrap();
    let bravo = mem.frame_by_uri(&uri("bravo")).unwrap().id;
    let charlie = mem.frame_by_uri(&uri("charlie")).unwrap().id;
    let update = PutOptions {
        search_text: Some("zulu report".to_string()),
        ..options("bravo")
    };
    mem.update_frame(bravo, Some(b"zulu report".to_vec()), update, None)
        .unwrap();
    mem.delete_frame(charlie).unwrap();
    mem.commit().unwrap();
    mem
}

fn assert_edits_indexed(mem: &mut Memvid, when: &str) {
    assert_eq!(
        uris_matching(mem, "zulu"),
        [uri("bravo")],
        "{when}: updated text"
    );
    assert!(
        uris_matching(mem, "bravo").is_empty(),
        "{when}: replaced text"
    );
    assert!(
        uris_matching(mem, "charlie").is_empty(),
        "{when}: deleted frame"
    );
    for word in ["alpha", "delta", "echo"] {
        assert_eq!(
            uris_matching(mem, word),
            [uri(word)],
            "{when}: untouched frame {word}"
        );
    }
    assert_eq!(
        uris_matching(mem, "report"),
        ["alpha", "bravo", "delta", "echo"].map(uri),
        "{when}: every live frame"
    );
}

#[test]
fn a_commit_after_an_update_and_a_delete_indexes_only_live_text() {
    let dir = TempDir::new().unwrap();
    let mut mem = edited_shard(&dir.path().join("edits.mv2"));

    assert_edits_indexed(&mut mem, "after commit");
}

#[test]
fn a_vacuum_after_an_update_and_a_delete_indexes_only_live_text() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("edits.mv2");
    let mut mem = edited_shard(&path);
    mem.vacuum().unwrap();
    assert_edits_indexed(&mut mem, "after vacuum");
    drop(mem);

    let mut reopened = Memvid::open(&path).unwrap();
    assert_edits_indexed(&mut reopened, "after reopen");
}
