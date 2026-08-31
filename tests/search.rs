//! Integration tests for Memvid search operations.
//! Tests: search (lex), timeline queries

use memvid_core::{Memvid, PutOptions, SearchRequest, TimelineQuery};
use std::num::NonZeroU64;
use tempfile::TempDir;

/// Helper to create a memory with searchable content.
fn create_searchable_memory(path: &std::path::Path) {
    let mut mem = Memvid::create(path).unwrap();
    mem.enable_lex().unwrap();

    let docs = vec![
        (
            "mv2://physics/quantum",
            "Quantum Physics",
            "Quantum mechanics describes the behavior of particles at the atomic scale",
        ),
        (
            "mv2://physics/classical",
            "Classical Mechanics",
            "Classical mechanics describes motion of macroscopic objects",
        ),
        (
            "mv2://biology/cells",
            "Cell Biology",
            "Cells are the basic building blocks of all living organisms",
        ),
        (
            "mv2://chemistry/atoms",
            "Atomic Chemistry",
            "Atoms combine to form molecules through chemical bonds",
        ),
        (
            "mv2://math/calculus",
            "Calculus",
            "Calculus studies continuous change and rates of change",
        ),
    ];

    for (uri, title, content) in docs {
        let opts = PutOptions {
            uri: Some(uri.to_string()),
            title: Some(title.to_string()),
            search_text: Some(content.to_string()),
            timestamp: Some(1700000000),
            ..Default::default()
        };
        mem.put_bytes_with_options(content.as_bytes(), opts)
            .unwrap();
    }

    mem.commit().unwrap();
}

/// Test basic lexical search.
#[test]
#[cfg(feature = "lex")]
fn search_basic_query() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("test.mv2");

    create_searchable_memory(&path);

    let mut mem = Memvid::open_read_only(&path).unwrap();
    let results = mem
        .search(SearchRequest {
            query: "quantum".to_string(),
            top_k: 10,
            snippet_chars: 200,
            uri: None,
            scope: None,
            frames: None,
            cursor: None,
            #[cfg(feature = "temporal_track")]
            temporal: None,
            as_of_frame: None,
            as_of_ts: None,
            no_sketch: false,
            acl_context: None,
            acl_enforcement_mode: memvid_core::types::AclEnforcementMode::Audit,
        })
        .unwrap();

    assert!(!results.hits.is_empty(), "Should find quantum document");
    assert!(
        results.hits[0].uri.contains("quantum"),
        "Top result should be quantum physics"
    );
}

/// Test search with multiple results.
#[test]
#[cfg(feature = "lex")]
fn search_multiple_results() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("test.mv2");

    create_searchable_memory(&path);

    let mut mem = Memvid::open_read_only(&path).unwrap();

    // Search for "mechanics" should find both quantum and classical
    let results = mem
        .search(SearchRequest {
            query: "mechanics".to_string(),
            top_k: 10,
            snippet_chars: 200,
            uri: None,
            scope: None,
            frames: None,
            cursor: None,
            #[cfg(feature = "temporal_track")]
            temporal: None,
            as_of_frame: None,
            as_of_ts: None,
            no_sketch: false,
            acl_context: None,
            acl_enforcement_mode: memvid_core::types::AclEnforcementMode::Audit,
        })
        .unwrap();

    assert_eq!(
        results.hits.len(),
        2,
        "Should find both mechanics documents"
    );
}

/// Test search with top_k limit.
#[test]
#[cfg(feature = "lex")]
fn search_respects_top_k() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("test.mv2");

    // Create memory with many documents
    {
        let mut mem = Memvid::create(&path).unwrap();
        mem.enable_lex().unwrap();

        for i in 0..20 {
            let opts = PutOptions {
                uri: Some(format!("mv2://doc{}", i)),
                title: Some(format!("Document {}", i)),
                search_text: Some(format!(
                    "This document contains searchable content number {}",
                    i
                )),
                ..Default::default()
            };
            mem.put_bytes_with_options(format!("Content {}", i).as_bytes(), opts)
                .unwrap();
        }
        mem.commit().unwrap();
    }

    let mut mem = Memvid::open_read_only(&path).unwrap();
    let results = mem
        .search(SearchRequest {
            query: "document".to_string(),
            top_k: 5,
            snippet_chars: 200,
            uri: None,
            scope: None,
            frames: None,
            cursor: None,
            #[cfg(feature = "temporal_track")]
            temporal: None,
            as_of_frame: None,
            as_of_ts: None,
            no_sketch: false,
            acl_context: None,
            acl_enforcement_mode: memvid_core::types::AclEnforcementMode::Audit,
        })
        .unwrap();

    assert_eq!(results.hits.len(), 5, "Should return exactly top_k results");
}

/// Test search with scope filter.
#[test]
#[cfg(feature = "lex")]
fn search_with_scope() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("test.mv2");

    create_searchable_memory(&path);

    let mut mem = Memvid::open_read_only(&path).unwrap();

    // Search only in physics scope
    let results = mem
        .search(SearchRequest {
            query: "mechanics".to_string(),
            top_k: 10,
            snippet_chars: 200,
            uri: None,
            scope: Some("mv2://physics/".to_string()),
            frames: None,
            cursor: None,
            #[cfg(feature = "temporal_track")]
            temporal: None,
            as_of_frame: None,
            as_of_ts: None,
            no_sketch: false,
            acl_context: None,
            acl_enforcement_mode: memvid_core::types::AclEnforcementMode::Audit,
        })
        .unwrap();

    // All results should be from physics scope
    for hit in &results.hits {
        assert!(
            hit.uri.starts_with("mv2://physics/"),
            "Results should be from physics scope"
        );
    }
}

/// Test search returns snippets.
#[test]
#[cfg(feature = "lex")]
fn search_returns_snippets() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("test.mv2");

    create_searchable_memory(&path);

    let mut mem = Memvid::open_read_only(&path).unwrap();
    let results = mem
        .search(SearchRequest {
            query: "quantum".to_string(),
            top_k: 10,
            snippet_chars: 200,
            uri: None,
            scope: None,
            frames: None,
            cursor: None,
            #[cfg(feature = "temporal_track")]
            temporal: None,
            as_of_frame: None,
            as_of_ts: None,
            no_sketch: false,
            acl_context: None,
            acl_enforcement_mode: memvid_core::types::AclEnforcementMode::Audit,
        })
        .unwrap();

    assert!(!results.hits.is_empty());
    let hit = &results.hits[0];

    // Snippet should contain matched content
    assert!(!hit.text.is_empty(), "Hit should include text snippet");
}

/// Test search with no results.
#[test]
#[cfg(feature = "lex")]
fn search_no_results() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("test.mv2");

    create_searchable_memory(&path);

    let mut mem = Memvid::open_read_only(&path).unwrap();
    let results = mem
        .search(SearchRequest {
            query: "xyznonexistentterm".to_string(),
            top_k: 10,
            snippet_chars: 200,
            uri: None,
            scope: None,
            frames: None,
            cursor: None,
            #[cfg(feature = "temporal_track")]
            temporal: None,
            as_of_frame: None,
            as_of_ts: None,
            no_sketch: false,
            acl_context: None,
            acl_enforcement_mode: memvid_core::types::AclEnforcementMode::Audit,
        })
        .unwrap();

    assert_eq!(results.hits.len(), 0, "Should return no results");
}

/// Test search on empty memory.
#[test]
#[cfg(feature = "lex")]
fn search_empty_memory() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("test.mv2");

    {
        let mut mem = Memvid::create(&path).unwrap();
        mem.enable_lex().unwrap();
        mem.commit().unwrap();
    }

    let mut mem = Memvid::open_read_only(&path).unwrap();
    let results = mem
        .search(SearchRequest {
            query: "anything".to_string(),
            top_k: 10,
            snippet_chars: 200,
            uri: None,
            scope: None,
            frames: None,
            cursor: None,
            #[cfg(feature = "temporal_track")]
            temporal: None,
            as_of_frame: None,
            as_of_ts: None,
            no_sketch: false,
            acl_context: None,
            acl_enforcement_mode: memvid_core::types::AclEnforcementMode::Audit,
        })
        .unwrap();

    assert_eq!(
        results.hits.len(),
        0,
        "Empty memory should return no results"
    );
}

/// Test timeline query returns ordered results.
#[test]
fn timeline_returns_ordered() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("test.mv2");

    {
        let mut mem = Memvid::create(&path).unwrap();

        // Add frames with different timestamps
        let timestamps = [1700000000i64, 1700003000, 1700001000, 1700002000];

        for (i, ts) in timestamps.iter().enumerate() {
            let opts = PutOptions {
                uri: Some(format!("mv2://doc{}", i)),
                title: Some(format!("Document {}", i)),
                timestamp: Some(*ts),
                ..Default::default()
            };
            mem.put_bytes_with_options(format!("Content {}", i).as_bytes(), opts)
                .unwrap();
        }
        mem.commit().unwrap();
    }

    let mut mem = Memvid::open_read_only(&path).unwrap();
    let query = TimelineQuery::builder()
        .limit(NonZeroU64::new(10).unwrap())
        .build();
    let entries = mem.timeline(query).unwrap();

    // Verify timeline is ordered by timestamp (either ascending or descending)
    if entries.len() > 1 {
        let is_descending = entries[0].timestamp >= entries[1].timestamp;
        for i in 1..entries.len() {
            if is_descending {
                assert!(
                    entries[i - 1].timestamp >= entries[i].timestamp,
                    "Timeline should be consistently ordered (descending)"
                );
            } else {
                assert!(
                    entries[i - 1].timestamp <= entries[i].timestamp,
                    "Timeline should be consistently ordered (ascending)"
                );
            }
        }
    }
}

/// Test timeline with since filter.
#[test]
fn timeline_with_since_filter() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("test.mv2");

    {
        let mut mem = Memvid::create(&path).unwrap();

        let timestamps = [1700000000i64, 1700001000, 1700002000, 1700003000];

        for (i, ts) in timestamps.iter().enumerate() {
            let opts = PutOptions {
                uri: Some(format!("mv2://doc{}", i)),
                timestamp: Some(*ts),
                ..Default::default()
            };
            mem.put_bytes_with_options(format!("Content {}", i).as_bytes(), opts)
                .unwrap();
        }
        mem.commit().unwrap();
    }

    let mut mem = Memvid::open_read_only(&path).unwrap();

    // Get entries since 1700001500
    let query = TimelineQuery::builder()
        .limit(NonZeroU64::new(10).unwrap())
        .since(1700001500)
        .build();
    let entries = mem.timeline(query).unwrap();

    for entry in &entries {
        assert!(
            entry.timestamp >= 1700001500,
            "All entries should be >= since timestamp"
        );
    }
}

/// Test timeline with until filter.
#[test]
fn timeline_with_until_filter() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("test.mv2");

    {
        let mut mem = Memvid::create(&path).unwrap();

        let timestamps = [1700000000i64, 1700001000, 1700002000, 1700003000];

        for (i, ts) in timestamps.iter().enumerate() {
            let opts = PutOptions {
                uri: Some(format!("mv2://doc{}", i)),
                timestamp: Some(*ts),
                ..Default::default()
            };
            mem.put_bytes_with_options(format!("Content {}", i).as_bytes(), opts)
                .unwrap();
        }
        mem.commit().unwrap();
    }

    let mut mem = Memvid::open_read_only(&path).unwrap();

    // Get entries until 1700001500
    let query = TimelineQuery::builder()
        .limit(NonZeroU64::new(10).unwrap())
        .until(1700001500)
        .build();
    let entries = mem.timeline(query).unwrap();

    for entry in &entries {
        assert!(
            entry.timestamp <= 1700001500,
            "All entries should be <= until timestamp"
        );
    }
}

/// Test timeline respects limit.
#[test]
fn timeline_respects_limit() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("test.mv2");

    {
        let mut mem = Memvid::create(&path).unwrap();

        for i in 0..20 {
            let opts = PutOptions {
                uri: Some(format!("mv2://doc{}", i)),
                timestamp: Some(1700000000 + i as i64 * 1000),
                ..Default::default()
            };
            mem.put_bytes_with_options(format!("Content {}", i).as_bytes(), opts)
                .unwrap();
        }
        mem.commit().unwrap();
    }

    let mut mem = Memvid::open_read_only(&path).unwrap();
    let query = TimelineQuery::builder()
        .limit(NonZeroU64::new(5).unwrap())
        .build();
    let entries = mem.timeline(query).unwrap();

    assert_eq!(
        entries.len(),
        5,
        "Timeline should return exactly limit entries"
    );
}

// ── Frame-set narrowing ────────────────────────────────────────────────

/// A request for `query`, optionally narrowed to an explicit frame set.
#[cfg(feature = "lex")]
fn frame_scoped_request(query: &str, frames: Option<Vec<memvid_core::FrameId>>) -> SearchRequest {
    SearchRequest {
        query: query.to_string(),
        top_k: 10,
        snippet_chars: 200,
        uri: None,
        scope: None,
        frames,
        cursor: None,
        #[cfg(feature = "temporal_track")]
        temporal: None,
        as_of_frame: None,
        as_of_ts: None,
        no_sketch: false,
        acl_context: None,
        acl_enforcement_mode: memvid_core::AclEnforcementMode::Audit,
    }
}

/// A caller-supplied frame set narrows what the query is evaluated
/// against, without touching how matches are ranked.
#[test]
#[cfg(feature = "lex")]
fn search_narrows_to_a_supplied_frame_set() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("test.mv2");
    create_searchable_memory(&path);
    let mut mem = Memvid::open_read_only(&path).unwrap();

    // "mechanics" is in both physics documents.
    let everywhere = mem.search(frame_scoped_request("mechanics", None)).unwrap();
    let found: Vec<&str> = everywhere.hits.iter().map(|h| h.uri.as_str()).collect();
    assert_eq!(found.len(), 2, "expected both physics docs, got {found:?}");

    // Narrowed to one of them, the other cannot come back however well
    // it matches.
    let classical = mem.frame_by_uri("mv2://physics/classical").unwrap().id;
    let narrowed = mem
        .search(frame_scoped_request("mechanics", Some(vec![classical])))
        .unwrap();
    let found: Vec<&str> = narrowed.hits.iter().map(|h| h.uri.as_str()).collect();
    assert_eq!(found, ["mv2://physics/classical"]);
}

/// A frame in the set that does not match the query is still not a hit —
/// the set narrows, it does not force.
#[test]
#[cfg(feature = "lex")]
fn a_supplied_frame_set_does_not_force_non_matches() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("test.mv2");
    create_searchable_memory(&path);
    let mut mem = Memvid::open_read_only(&path).unwrap();

    let cells = mem.frame_by_uri("mv2://biology/cells").unwrap().id;
    let results = mem
        .search(frame_scoped_request("mechanics", Some(vec![cells])))
        .unwrap();
    assert!(
        results.hits.is_empty(),
        "biology does not match `mechanics`, so narrowing to it finds nothing"
    );
}

/// An empty set means empty results, never "unset". A caller whose own
/// filtering found nothing must not silently get the whole corpus.
#[test]
#[cfg(feature = "lex")]
fn an_empty_frame_set_finds_nothing() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("test.mv2");
    create_searchable_memory(&path);
    let mut mem = Memvid::open_read_only(&path).unwrap();

    let results = mem
        .search(frame_scoped_request("mechanics", Some(Vec::new())))
        .unwrap();
    assert!(results.hits.is_empty());
}

/// The set intersects with the other candidate filters rather than
/// replacing them: a frame inside the set but outside the time bound is
/// still excluded.
#[test]
#[cfg(all(feature = "lex", feature = "temporal_track"))]
fn a_frame_set_intersects_the_time_bound() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("test.mv2");
    create_searchable_memory(&path);
    let mut mem = Memvid::open_read_only(&path).unwrap();

    let classical = mem.frame_by_uri("mv2://physics/classical").unwrap().id;
    // A window wide enough to hold BOTH physics documents, so anything
    // dropped here is dropped by the frame set and not by the clock.
    let window = memvid_core::TemporalFilter {
        start_utc: Some(1699999999),
        end_utc: Some(1700000001),
        phrase: None,
        tz: None,
    };

    let mut unrestricted = frame_scoped_request("mechanics", None);
    unrestricted.temporal = Some(window.clone());
    let both = mem.search(unrestricted).unwrap();
    assert_eq!(both.hits.len(), 2, "the window admits both physics docs");

    let mut restricted = frame_scoped_request("mechanics", Some(vec![classical]));
    restricted.temporal = Some(window);
    let results = mem.search(restricted).unwrap();
    let found: Vec<&str> = results.hits.iter().map(|h| h.uri.as_str()).collect();
    assert_eq!(
        found,
        ["mv2://physics/classical"],
        "both filters apply: the window keeps two, the frame set keeps one of them"
    );
}
