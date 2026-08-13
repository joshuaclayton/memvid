//! Correctness of `Memvid::frame_by_uri` and invalidation of its lookup cache
//! across frame-table mutations.
//!
//! `frame_by_uri` resolves a uri to a single frame: it prefers the highest-index
//! active frame carrying that uri, and otherwise falls back to the highest-index
//! frame carrying the uri regardless of status. A read-path index caches this
//! resolution, so any mutation that adds a frame or changes a frame's status
//! must invalidate the cache. These tests exercise add and delete against a
//! live `Memvid` handle (no reopen between steps, so a stale cache would be
//! observable) and confirm the resolution rules hold.

use memvid_core::{FrameStatus, Memvid, MemvidError, PutOptions};
use tempfile::TempDir;

fn put(mem: &mut Memvid, uri: &str, body: &str) {
    let opts = PutOptions {
        uri: Some(uri.to_string()),
        ..Default::default()
    };
    // The returned id is a put ordinal, not the resolved frame id (a put may
    // create a parent plus chunk frames); resolve real frame ids via lookup.
    mem.put_bytes_with_options(body.as_bytes(), opts).unwrap();
}

#[test]
fn frame_by_uri_resolves_and_invalidates_across_mutations() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("index.mv2");

    let mut mem = Memvid::create(&path).unwrap();
    put(&mut mem, "mv2://doc/0", "first payload for zero");
    put(&mut mem, "mv2://doc/1", "first payload for one");
    put(&mut mem, "mv2://doc/2", "first payload for two");
    mem.commit().unwrap();

    // Known uri resolves to its active frame.
    let one = mem.frame_by_uri("mv2://doc/1").unwrap();
    assert_eq!(one.uri.as_deref(), Some("mv2://doc/1"));
    assert_eq!(one.status, FrameStatus::Active);

    // Unknown uri returns the typed not-found error.
    assert!(matches!(
        mem.frame_by_uri("mv2://doc/absent"),
        Err(MemvidError::FrameNotFoundByUri { .. })
    ));

    // A brand-new uri added while the cache is warm must become resolvable.
    // This has no superseded predecessor, so only the add path can invalidate
    // the cache; a stale cache would report the uri as absent.
    let _ = mem.frame_by_uri("mv2://doc/2").unwrap();
    put(&mut mem, "mv2://doc/new", "payload for a brand-new uri");
    mem.commit().unwrap();
    let fresh = mem.frame_by_uri("mv2://doc/new").unwrap();
    assert_eq!(fresh.uri.as_deref(), Some("mv2://doc/new"));
    assert_eq!(fresh.status, FrameStatus::Active);

    // Build the cache with a lookup, then add a second, higher-index frame for
    // an existing uri. The newer active frame must win; a stale cache would
    // keep returning the original.
    let before_add = mem.frame_by_uri("mv2://doc/1").unwrap().id;
    put(&mut mem, "mv2://doc/1", "second, distinct payload for one");
    mem.commit().unwrap();
    let after_add = mem.frame_by_uri("mv2://doc/1").unwrap();
    assert!(
        after_add.id > before_add,
        "a newer (higher-index) active frame must win after an add (was {before_add}, got {})",
        after_add.id
    );
    assert_eq!(after_add.status, FrameStatus::Active);
    assert_eq!(after_add.uri.as_deref(), Some("mv2://doc/1"));

    // Delete the sole active frame for a uri. The frame stays in the table as
    // non-active, so the lookup falls back to it (rule 2). This exercises both
    // the fallback path and delete-invalidation. Source the frame id from the
    // lookup itself (a genuine frame id), which also warms the cache.
    let zero = mem.frame_by_uri("mv2://doc/0").unwrap();
    assert_eq!(zero.status, FrameStatus::Active);
    mem.delete_frame(zero.id).unwrap();
    mem.commit().unwrap();
    let zero_after_delete = mem.frame_by_uri("mv2://doc/0").unwrap();
    assert_eq!(
        zero_after_delete.id, zero.id,
        "fallback resolves the same frame once it is the only one carrying the uri"
    );
    assert_ne!(
        zero_after_delete.status,
        FrameStatus::Active,
        "the fallback frame is the now non-active one"
    );
}
