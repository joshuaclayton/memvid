//! TDD suite for paged-frame supersession (fix/paged-update-supersedes-chunks).
//!
//! Bug: when a record long enough to page (>= CHUNK_MIN_CHARS) is updated or
//! deleted, memvid supersedes/tombstones only the ROOT frame and orphans its
//! `#page-N` chunk children as Active. They stay in the lex index (searchable)
//! and vacuum cannot reclaim them (Active).
//!
//! Desired contract (latest-only default search):
//!   - After an update, the only Active frames are the current generation.
//!   - A default content search never returns pre-update content.
//!   - A delete removes the root AND every chunk child.
//!   - Prior generations are reclaimable by vacuum.
//!
//! These tests assert that contract. Several are RED until the fix lands; the
//! baselines (short/single-frame) already pass. Assertions are token- and
//! control-shard based so they do not depend on exact chunk counts.
//!
//! Run: `cargo test --test paged_supersession` (default features include `lex`).

#![cfg(feature = "lex")]

use memvid_core::types::AclEnforcementMode;
use memvid_core::{Memvid, PutOptions, SearchRequest};
use tempfile::TempDir;

// Sizes chosen to straddle the naive chunk threshold (DEFAULT_CHUNK_CHARS=1200,
// CHUNK_MIN_CHARS=2400). Exact chunk counts are asserted via control shards,
// never hard-coded.
const SHORT: usize = 500; // single frame, no chunking
const SMALL: usize = 3_000; // a few chunks
const MED: usize = 7_000; // ~5-6 chunks
const BIG: usize = 13_000; // ~10-11 chunks

/// Content of ~`chars` bytes saturated with `token`, so every chunk of the
/// paged frame contains it and a search for `token` hits the whole generation.
fn content(token: &str, chars: usize) -> Vec<u8> {
    let unit = format!("{token} ");
    let reps = (chars / unit.len()).max(1);
    unit.repeat(reps).into_bytes()
}

fn put_opts(uri: &str) -> PutOptions {
    PutOptions {
        uri: Some(uri.to_string()),
        title: Some("doc".to_string()),
        timestamp: Some(1_700_000_000),
        ..Default::default()
    }
}

/// Fresh build: create a shard, enable lex, insert one doc, commit.
fn build(path: &std::path::Path, uri: &str, bytes: &[u8]) {
    let mut mem = Memvid::create(path).unwrap();
    mem.enable_lex().unwrap();
    mem.put_bytes_with_options(bytes, put_opts(uri)).unwrap();
    mem.commit().unwrap();
}

/// Update the doc at `uri` (paged or not) with new bytes, then commit.
fn update(path: &std::path::Path, uri: &str, bytes: &[u8]) {
    let mut mem = Memvid::open(path).unwrap();
    let id = mem.frame_by_uri(uri).unwrap().id;
    mem.update_frame(id, Some(bytes.to_vec()), put_opts(uri), None)
        .unwrap();
    mem.commit().unwrap();
}

/// Delete the doc at `uri`, then commit.
fn remove(path: &std::path::Path, uri: &str) {
    let mut mem = Memvid::open(path).unwrap();
    let id = mem.frame_by_uri(uri).unwrap().id;
    mem.delete_frame(id).unwrap();
    mem.commit().unwrap();
}

fn vacuum(path: &std::path::Path) {
    let mut mem = Memvid::open(path).unwrap();
    mem.vacuum().unwrap();
}

fn active(path: &std::path::Path) -> u64 {
    Memvid::open_read_only(path)
        .unwrap()
        .stats()
        .unwrap()
        .active_frame_count
}

fn file_size(path: &std::path::Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// Hits for `query` (lower-cased by the engine) across a generous top_k.
fn hits(path: &std::path::Path, query: &str) -> usize {
    let mut mem = Memvid::open_read_only(path).unwrap();
    mem.search(SearchRequest {
        query: query.to_string(),
        top_k: 1_000,
        snippet_chars: 64,
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
        acl_enforcement_mode: AclEnforcementMode::Audit,
    })
    .unwrap()
    .hits
    .len()
}

/// Active frame count of a fresh, standalone build of `bytes` — the control
/// value a correct update must converge to (old generation fully superseded).
fn fresh_active(bytes: &[u8]) -> u64 {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("control.mv2");
    build(&path, "mv2://control", bytes);
    active(&path)
}

fn fresh_size(bytes: &[u8]) -> u64 {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("control.mv2");
    build(&path, "mv2://control", bytes);
    vacuum(&path);
    file_size(&path)
}

fn shard() -> (TempDir, std::path::PathBuf) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("t.mv2");
    (dir, path)
}

const URI: &str = "mv2://t/doc";

// ── Shapes ──────────────────────────────────────────────────────────────

#[test]
fn shape_short_is_single_frame() {
    let (_d, p) = shard();
    build(&p, URI, &content("ALPHA", SHORT));
    assert_eq!(active(&p), 1, "sub-threshold content must not chunk");
}

#[test]
fn shape_long_is_multi_chunk() {
    let (_d, p) = shard();
    build(&p, URI, &content("ALPHA", MED));
    assert!(
        active(&p) >= 3,
        "MED content must page into multiple chunks (got {})",
        active(&p)
    );
}

// ── Update transitions (latest-only) ────────────────────────────────────

#[test]
fn update_short_to_short() {
    let (_d, p) = shard();
    build(&p, URI, &content("ALPHA", SHORT));
    update(&p, URI, &content("BETA", SHORT));
    assert_eq!(hits(&p, "alpha"), 0, "stale content must be gone");
    assert!(hits(&p, "beta") > 0, "new content must be searchable");
    assert_eq!(active(&p), 1);
}

#[test]
fn update_short_to_multi() {
    let (_d, p) = shard();
    build(&p, URI, &content("ALPHA", SHORT));
    update(&p, URI, &content("BETA", MED));
    assert_eq!(hits(&p, "alpha"), 0, "stale single frame must be gone");
    assert!(hits(&p, "beta") > 0);
    assert_eq!(
        active(&p),
        fresh_active(&content("BETA", MED)),
        "active must equal a fresh build of the new content"
    );
}

#[test]
fn update_multi_grow() {
    // 5 -> 10 pages (their scenario)
    let (_d, p) = shard();
    build(&p, URI, &content("ALPHA", MED));
    update(&p, URI, &content("BETA", BIG));
    assert_eq!(
        hits(&p, "alpha"),
        0,
        "old chunk children must be superseded"
    );
    assert!(hits(&p, "beta") > 0);
    assert_eq!(active(&p), fresh_active(&content("BETA", BIG)));
}

#[test]
fn update_multi_shrink() {
    // 5 -> 2 pages (their scenario). The pages with no new counterpart must vanish.
    let (_d, p) = shard();
    build(&p, URI, &content("ALPHA", MED));
    update(&p, URI, &content("BETA", SMALL));
    assert_eq!(hits(&p, "alpha"), 0, "dropped pages must not linger");
    assert!(hits(&p, "beta") > 0);
    assert_eq!(active(&p), fresh_active(&content("BETA", SMALL)));
}

#[test]
fn update_multi_to_single() {
    // 5 -> 1 (collapse below the chunk threshold).
    let (_d, p) = shard();
    build(&p, URI, &content("ALPHA", MED));
    update(&p, URI, &content("BETA", SHORT));
    assert_eq!(hits(&p, "alpha"), 0, "all old chunks must be superseded");
    assert!(hits(&p, "beta") > 0);
    assert_eq!(active(&p), 1, "collapsed doc is a single frame");
}

#[test]
fn update_multi_same_count() {
    // 5 -> 5, new content: worst case for #page-N uri collision.
    let (_d, p) = shard();
    build(&p, URI, &content("ALPHA", MED));
    update(&p, URI, &content("BETA", MED));
    assert_eq!(
        hits(&p, "alpha"),
        0,
        "same-count edit must still evict old text"
    );
    assert!(hits(&p, "beta") > 0);
    assert_eq!(active(&p), fresh_active(&content("BETA", MED)));
}

#[test]
fn update_chain_collapses_to_last() {
    // 5 -> 10 -> 2 -> 6 : only the final generation survives.
    let (_d, p) = shard();
    build(&p, URI, &content("ALPHA", MED));
    update(&p, URI, &content("BETA", BIG));
    update(&p, URI, &content("GAMMA", SMALL));
    update(&p, URI, &content("DELTA", MED));
    for stale in ["alpha", "beta", "gamma"] {
        assert_eq!(hits(&p, stale), 0, "generation `{stale}` must be gone");
    }
    assert!(hits(&p, "delta") > 0);
    assert_eq!(active(&p), fresh_active(&content("DELTA", MED)));
}

// ── Delete (cascade to chunk children) ──────────────────────────────────

#[test]
fn delete_single() {
    let (_d, p) = shard();
    build(&p, URI, &content("ALPHA", SHORT));
    remove(&p, URI);
    assert_eq!(hits(&p, "alpha"), 0);
    assert_eq!(active(&p), 0, "nothing active after delete");
}

#[test]
fn delete_multi_removes_all_pages() {
    let (_d, p) = shard();
    build(&p, URI, &content("ALPHA", MED));
    remove(&p, URI);
    assert_eq!(hits(&p, "alpha"), 0, "no chunk child may survive a delete");
    assert_eq!(active(&p), 0, "root and all pages removed");
}

#[test]
fn delete_after_grow() {
    let (_d, p) = shard();
    build(&p, URI, &content("ALPHA", MED));
    update(&p, URI, &content("BETA", BIG));
    remove(&p, URI);
    assert_eq!(hits(&p, "alpha"), 0, "old generation gone");
    assert_eq!(hits(&p, "beta"), 0, "current generation gone");
    assert_eq!(active(&p), 0);
}

#[test]
fn delete_after_shrink() {
    let (_d, p) = shard();
    build(&p, URI, &content("ALPHA", MED));
    update(&p, URI, &content("BETA", SMALL));
    remove(&p, URI);
    assert_eq!(hits(&p, "alpha"), 0);
    assert_eq!(hits(&p, "beta"), 0);
    assert_eq!(active(&p), 0);
}

// ── Vacuum reclaim ──────────────────────────────────────────────────────

#[test]
fn vacuum_reclaims_after_shrinking_update() {
    // 10 -> 2 then vacuum: the file must shrink to about a fresh 2-chunk build,
    // and no stale content remains.
    let (_d, p) = shard();
    build(&p, URI, &content("ALPHA", BIG));
    update(&p, URI, &content("BETA", SMALL));
    vacuum(&p);
    assert_eq!(hits(&p, "alpha"), 0, "vacuum must not leave stale chunks");
    assert!(hits(&p, "beta") > 0);
    assert_eq!(active(&p), fresh_active(&content("BETA", SMALL)));
    let control = fresh_size(&content("BETA", SMALL));
    assert!(
        file_size(&p) <= control * 2,
        "vacuumed file ({}) should be near a fresh 2-chunk build ({}), not carry \
         the old 10-chunk generation",
        file_size(&p),
        control
    );
}

// ── Added blind (before investigating), to guard the fix ─────────────────

/// Insert a new doc into an existing shard (used for the restore path).
fn insert(path: &std::path::Path, uri: &str, bytes: &[u8]) {
    let mut mem = Memvid::open(path).unwrap();
    mem.put_bytes_with_options(bytes, put_opts(uri)).unwrap();
    mem.commit().unwrap();
}

fn root_text(path: &std::path::Path, uri: &str) -> String {
    let mut mem = Memvid::open_read_only(path).unwrap();
    let f = mem.frame_by_uri(uri).unwrap();
    mem.frame_text_by_id(f.id).unwrap()
}

// 1. Sibling docs must not be touched when one is updated.
#[test]
fn update_leaves_sibling_doc_intact() {
    let (_d, p) = shard();
    let mut mem = Memvid::create(&p).unwrap();
    mem.enable_lex().unwrap();
    mem.put_bytes_with_options(&content("ALPHA", MED), put_opts("mv2://t/a-doc"))
        .unwrap();
    mem.put_bytes_with_options(&content("BETA", MED), put_opts("mv2://t/b-doc"))
        .unwrap();
    mem.commit().unwrap();
    drop(mem);

    let beta_before = hits(&p, "beta");
    update(&p, "mv2://t/a-doc", &content("GAMMA", MED));
    assert_eq!(hits(&p, "alpha"), 0, "updated doc's old content gone");
    assert!(hits(&p, "gamma") > 0, "updated doc's new content present");
    assert_eq!(
        hits(&p, "beta"),
        beta_before,
        "sibling doc must be completely untouched by the update"
    );
}

// 2. uri-prefix boundary: `doc` update must not evict `doc2`'s chunk children,
//    and `a` must not evict `ab`. A naive starts_with(root_uri) without the
//    `#page-` separator would wrongly cascade across these.
#[test]
fn update_respects_uri_prefix_boundary() {
    let (_d, p) = shard();
    let mut mem = Memvid::create(&p).unwrap();
    mem.enable_lex().unwrap();
    for (uri, tok) in [
        ("mv2://t/doc", "ALPHA"),
        ("mv2://t/doc2", "BETA"),
        ("mv2://t/a", "GAMMA"),
        ("mv2://t/ab", "DELTA"),
    ] {
        mem.put_bytes_with_options(&content(tok, MED), put_opts(uri))
            .unwrap();
    }
    mem.commit().unwrap();
    drop(mem);

    let beta = hits(&p, "beta");
    let delta = hits(&p, "delta");
    update(&p, "mv2://t/doc", &content("OMEGA", MED));
    update(&p, "mv2://t/a", &content("SIGMA", MED));
    assert_eq!(hits(&p, "beta"), beta, "`doc2` must survive a `doc` update");
    assert_eq!(hits(&p, "delta"), delta, "`ab` must survive an `a` update");
    assert!(hits(&p, "omega") > 0);
    assert!(hits(&p, "sigma") > 0);
}

// 3. Split facet: after a single-frame update, is the old root superseded
//    (count) even if the lex index is stale (search)? Isolates the two bugs.
#[test]
fn update_single_frame_supersedes_root_by_count() {
    let (_d, p) = shard();
    build(&p, URI, &content("ALPHA", SHORT));
    update(&p, URI, &content("BETA", SHORT));
    assert_eq!(
        active(&p),
        1,
        "old single frame must be superseded (exactly one active frame)"
    );
}

// 4. Delete then re-insert at the same uri (the sink's restore path).
#[test]
fn delete_then_reinsert_same_uri() {
    let (_d, p) = shard();
    build(&p, URI, &content("ALPHA", MED));
    remove(&p, URI);
    insert(&p, URI, &content("BETA", MED));
    assert_eq!(hits(&p, "alpha"), 0, "deleted generation must not return");
    assert!(hits(&p, "beta") > 0, "restored content searchable");
    assert_eq!(active(&p), fresh_active(&content("BETA", MED)));
}

// 5. Text integrity (I4): the reassembled root is exactly the latest content.
#[test]
fn root_text_is_latest_only() {
    let (_d, p) = shard();
    build(&p, URI, &content("ALPHA", MED));
    update(&p, URI, &content("BETA", MED));
    let text = root_text(&p, URI);
    assert!(text.contains("BETA"), "root must carry new content");
    assert!(
        !text.contains("ALPHA"),
        "root reassembly must not include stale chunk text"
    );
}

// 6. Vacuum after delete reclaims everything.
#[test]
fn vacuum_after_delete_reclaims() {
    let (_d, p) = shard();
    build(&p, URI, &content("ALPHA", BIG));
    let before = file_size(&p);
    remove(&p, URI);
    vacuum(&p);
    assert_eq!(hits(&p, "alpha"), 0);
    assert_eq!(active(&p), 0, "nothing active after delete + vacuum");
    assert!(
        file_size(&p) < before,
        "file must shrink after deleting all content ({} !< {})",
        file_size(&p),
        before
    );
}

// 7. Degenerate shrink: multi -> empty content.
#[test]
fn update_multi_to_empty() {
    let (_d, p) = shard();
    build(&p, URI, &content("ALPHA", MED));
    update(&p, URI, b"");
    assert_eq!(hits(&p, "alpha"), 0, "no stale chunks after emptying");
    assert!(
        active(&p) <= 1,
        "empty content leaves at most one frame, no chunk children (got {})",
        active(&p)
    );
}

// 8. Repeated identical update must not accumulate generations.
#[test]
fn repeated_identical_update_no_accumulation() {
    let (_d, p) = shard();
    build(&p, URI, &content("ALPHA", MED));
    update(&p, URI, &content("ALPHA", MED));
    update(&p, URI, &content("ALPHA", MED));
    assert!(hits(&p, "alpha") > 0, "current content still present");
    assert_eq!(
        active(&p),
        fresh_active(&content("ALPHA", MED)),
        "re-updating identical content must not pile up superseded generations"
    );
}

// 9. Soak: many docs, mixed ops -> only survivors' latest content remains.
#[test]
fn multi_doc_soak_only_survivors() {
    let (_d, p) = shard();
    let mut mem = Memvid::create(&p).unwrap();
    mem.enable_lex().unwrap();
    for (uri, tok) in [
        ("mv2://t/x", "ALPHA"),
        ("mv2://t/y", "BETA"),
        ("mv2://t/z", "GAMMA"),
    ] {
        mem.put_bytes_with_options(&content(tok, MED), put_opts(uri))
            .unwrap();
    }
    mem.commit().unwrap();
    drop(mem);

    update(&p, "mv2://t/x", &content("DELTA", BIG));
    update(&p, "mv2://t/y", &content("EPSILON", SMALL));
    remove(&p, "mv2://t/z");

    for stale in ["alpha", "beta", "gamma"] {
        assert_eq!(hits(&p, stale), 0, "stale/deleted `{stale}` must be gone");
    }
    assert!(hits(&p, "delta") > 0);
    assert!(hits(&p, "epsilon") > 0);
    assert_eq!(
        active(&p),
        fresh_active(&content("DELTA", BIG)) + fresh_active(&content("EPSILON", SMALL)),
        "exactly the two surviving generations remain active"
    );
}

// 10. Time-travel parity: paging must not change whether as_of still sees prior
//     content, relative to a single-frame edit. (Contract B — parity.)
fn as_of_alpha_after_edit(bytes_a: &[u8], bytes_b: &[u8]) -> usize {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("t.mv2");
    build(&path, URI, bytes_a);
    let as_of = Memvid::open_read_only(&path)
        .unwrap()
        .stats()
        .unwrap()
        .frame_count
        - 1; // highest frame id at the pre-edit state
    update(&path, URI, bytes_b);
    let mut mem = Memvid::open_read_only(&path).unwrap();
    mem.search(SearchRequest {
        query: "alpha".to_string(),
        top_k: 1_000,
        snippet_chars: 64,
        uri: None,
        scope: None,
        frames: None,
        cursor: None,
        #[cfg(feature = "temporal_track")]
        temporal: None,
        as_of_frame: Some(as_of),
        as_of_ts: None,
        no_sketch: false,
        acl_context: None,
        acl_enforcement_mode: AclEnforcementMode::Audit,
    })
    .unwrap()
    .hits
    .len()
}

#[test]
fn as_of_time_travel_parity_paged_vs_single() {
    let single = as_of_alpha_after_edit(&content("ALPHA", SHORT), &content("BETA", SHORT));
    let paged = as_of_alpha_after_edit(&content("ALPHA", MED), &content("BETA", MED));
    assert_eq!(
        paged > 0,
        single > 0,
        "as_of history recall must not differ between paged and single-frame \
         edits (single={single}, paged={paged})"
    );
}
