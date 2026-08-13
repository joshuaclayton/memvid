use std::io::{Read, Seek, SeekFrom, Write};

use blake3::Hasher;

use crate::{
    constants::TIME_INDEX_MAGIC,
    error::{MemvidError, Result},
};

/// Raw entry used to build the time index track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeIndexEntry {
    pub timestamp: i64,
    pub frame_id: u64,
}

impl TimeIndexEntry {
    #[must_use]
    pub fn new(timestamp: i64, frame_id: u64) -> Self {
        Self {
            timestamp,
            frame_id,
        }
    }
}

/// Appends entries to the time index track, returning `(offset, length, checksum)`.
/// Entries are sorted by `(timestamp, frame_id)` prior to writing.
pub fn append_track<W: Write + Seek>(
    writer: &mut W,
    entries: &mut [TimeIndexEntry],
) -> Result<(u64, u64, [u8; 32])> {
    entries.sort_by_key(|entry| (entry.timestamp, entry.frame_id));

    let offset = writer.stream_position()?;
    let mut hasher = Hasher::new();

    writer.write_all(&TIME_INDEX_MAGIC)?;
    hasher.update(&TIME_INDEX_MAGIC);

    let count = entries.len() as u64;
    let count_bytes = count.to_le_bytes();
    writer.write_all(&count_bytes)?;
    hasher.update(&count_bytes);

    for entry in entries.iter() {
        let ts_bytes = entry.timestamp.to_le_bytes();
        let id_bytes = entry.frame_id.to_le_bytes();
        writer.write_all(&ts_bytes)?;
        writer.write_all(&id_bytes)?;
        hasher.update(&ts_bytes);
        hasher.update(&id_bytes);
    }

    let end = writer.stream_position()?;
    let length = end - offset;
    Ok((offset, length, *hasher.finalize().as_bytes()))
}

/// Reads the time index entries located at `(offset, length)` and validates ordering.
pub fn read_track<R: Read + Seek>(
    reader: &mut R,
    offset: u64,
    length: u64,
) -> Result<Vec<TimeIndexEntry>> {
    reader.seek(SeekFrom::Start(offset))?;

    let mut magic = [0u8; MAGIC_LEN];
    reader.read_exact(&mut magic)?;
    if magic != TIME_INDEX_MAGIC {
        return Err(MemvidError::InvalidTimeIndex {
            reason: "magic mismatch".into(),
        });
    }

    let mut count_buf = [0u8; COUNT_LEN];
    reader.read_exact(&mut count_buf)?;
    let count = u64::from_le_bytes(count_buf);

    if length < HEADER_LEN {
        return Err(MemvidError::InvalidTimeIndex {
            reason: "length shorter than header".into(),
        });
    }
    let payload_bytes = length - HEADER_LEN;
    let expected_payload =
        count
            .checked_mul(ENTRY_LEN as u64)
            .ok_or(MemvidError::InvalidTimeIndex {
                reason: "entry count overflow".into(),
            })?;
    if payload_bytes != expected_payload {
        return Err(MemvidError::InvalidTimeIndex {
            reason: "length does not match declared count".into(),
        });
    }

    // Safe: count validated by checked_mul and payload_bytes comparison above
    #[allow(clippy::cast_possible_truncation)]
    let mut entries = Vec::with_capacity(count as usize);
    let mut prev: Option<TimeIndexEntry> = None;
    for _ in 0..count {
        let mut ts_buf = [0u8; 8];
        reader.read_exact(&mut ts_buf)?;
        let timestamp = i64::from_le_bytes(ts_buf);

        let mut id_buf = [0u8; 8];
        reader.read_exact(&mut id_buf)?;
        let frame_id = u64::from_le_bytes(id_buf);

        let entry = TimeIndexEntry {
            timestamp,
            frame_id,
        };
        if let Some(prev_entry) = prev {
            if entry.timestamp < prev_entry.timestamp
                || (entry.timestamp == prev_entry.timestamp && entry.frame_id < prev_entry.frame_id)
            {
                return Err(MemvidError::InvalidTimeIndex {
                    reason: "entries not sorted".into(),
                });
            }
        }
        prev = Some(entry);
        entries.push(entry);
    }

    Ok(entries)
}

/// On-disk layout, derived once so writer and both readers can't drift:
/// header = magic + `u64` count; entry = LE `i64` ts + `u64` frame id.
const MAGIC_LEN: usize = TIME_INDEX_MAGIC.len();
const COUNT_LEN: usize = std::mem::size_of::<u64>();
const HEADER_LEN: u64 = (MAGIC_LEN + COUNT_LEN) as u64;
const ENTRY_LEN: usize = std::mem::size_of::<i64>() + std::mem::size_of::<u64>();

/// Read the timestamp of entry `index` without loading the rest of the
/// track (one seek + 8-byte read). `base` is the first entry's byte offset.
fn timestamp_at<R: Read + Seek>(reader: &mut R, base: u64, index: u64) -> Result<i64> {
    reader.seek(SeekFrom::Start(base + index * ENTRY_LEN as u64))?;
    let mut buf = [0u8; 8];
    reader.read_exact(&mut buf)?;
    Ok(i64::from_le_bytes(buf))
}

/// Reads only the entries whose timestamp falls in `[start, end]` (inclusive;
/// `None` leaves that side unbounded), binary-searching the on-disk sorted
/// track. O(log N) seeks + O(matched) read — it never loads the whole track,
/// which is the point: a narrow window over a mult-million-entry index costs
/// a handful of seeks, not a full scan. Equivalent result to
/// [`read_track`] followed by a `range.contains` filter.
///
/// # Errors
/// Returns [`MemvidError::InvalidTimeIndex`] on a malformed header/length.
pub fn read_range<R: Read + Seek>(
    reader: &mut R,
    offset: u64,
    length: u64,
    start: Option<i64>,
    end: Option<i64>,
) -> Result<Vec<TimeIndexEntry>> {
    if let (Some(s), Some(e)) = (start, end) {
        if s > e {
            return Ok(Vec::new());
        }
    }

    reader.seek(SeekFrom::Start(offset))?;
    let mut magic = [0u8; MAGIC_LEN];
    reader.read_exact(&mut magic)?;
    if magic != TIME_INDEX_MAGIC {
        return Err(MemvidError::InvalidTimeIndex {
            reason: "magic mismatch".into(),
        });
    }
    let mut count_buf = [0u8; COUNT_LEN];
    reader.read_exact(&mut count_buf)?;
    let count = u64::from_le_bytes(count_buf);

    if length < HEADER_LEN {
        return Err(MemvidError::InvalidTimeIndex {
            reason: "length shorter than header".into(),
        });
    }
    let expected_payload =
        count
            .checked_mul(ENTRY_LEN as u64)
            .ok_or(MemvidError::InvalidTimeIndex {
                reason: "entry count overflow".into(),
            })?;
    if length - HEADER_LEN != expected_payload {
        return Err(MemvidError::InvalidTimeIndex {
            reason: "length does not match declared count".into(),
        });
    }
    if count == 0 {
        return Ok(Vec::new());
    }

    let base = offset + HEADER_LEN;

    // lower = first index with timestamp >= start (0 when unbounded).
    let mut lower = 0u64;
    if let Some(s) = start {
        let (mut lo, mut hi) = (0u64, count);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if timestamp_at(reader, base, mid)? < s {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        lower = lo;
    }

    // upper = first index with timestamp > end (count when unbounded).
    let mut upper = count;
    if let Some(e) = end {
        let (mut lo, mut hi) = (lower, count);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if timestamp_at(reader, base, mid)? <= e {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        upper = lo;
    }

    if upper <= lower {
        return Ok(Vec::new());
    }

    // Read exactly the matched slice, one entry at a time (same decode as
    // `read_track`, avoiding a fallible slice→array conversion). The reads
    // are sequential from `lower`, so the OS read-ahead keeps them cheap.
    reader.seek(SeekFrom::Start(base + lower * ENTRY_LEN as u64))?;
    #[allow(clippy::cast_possible_truncation)]
    let span = (upper - lower) as usize;
    let mut entries = Vec::with_capacity(span);
    for _ in 0..span {
        let mut ts_buf = [0u8; 8];
        reader.read_exact(&mut ts_buf)?;
        let mut id_buf = [0u8; 8];
        reader.read_exact(&mut id_buf)?;
        entries.push(TimeIndexEntry {
            timestamp: i64::from_le_bytes(ts_buf),
            frame_id: u64::from_le_bytes(id_buf),
        });
    }
    Ok(entries)
}

/// Calculates the checksum for the provided entries in canonical order.
#[must_use]
pub fn calculate_checksum(entries: &[TimeIndexEntry]) -> [u8; 32] {
    let mut sorted = entries.to_vec();
    sorted.sort_by_key(|entry| (entry.timestamp, entry.frame_id));

    let mut hasher = Hasher::new();
    hasher.update(&TIME_INDEX_MAGIC);
    hasher.update(&(sorted.len() as u64).to_le_bytes());
    for entry in &sorted {
        hasher.update(&entry.timestamp.to_le_bytes());
        hasher.update(&entry.frame_id.to_le_bytes());
    }
    *hasher.finalize().as_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Seek, SeekFrom, Write};
    use tempfile::tempfile;

    #[test]
    fn append_and_read_roundtrip() {
        let mut file = tempfile().expect("temp file");
        let mut entries = vec![
            TimeIndexEntry::new(30, 2),
            TimeIndexEntry::new(10, 0),
            TimeIndexEntry::new(20, 1),
        ];

        let (offset, length, checksum) =
            append_track(&mut file, &mut entries).expect("append track");
        assert_eq!(entries[0].timestamp, 10); // sorted in place
        let read_entries = read_track(&mut file, offset, length).expect("read track");
        assert_eq!(read_entries.len(), 3);
        assert!(
            read_entries
                .windows(2)
                .all(|w| w[0].timestamp <= w[1].timestamp)
        );

        let expected_checksum = calculate_checksum(&read_entries);
        assert_eq!(checksum, expected_checksum);
    }

    #[test]
    fn read_rejects_unsorted_entries() {
        let mut file = tempfile().expect("temp file");
        // Craft an invalid track where entries descend.
        file.write_all(&TIME_INDEX_MAGIC).unwrap();
        file.write_all(&(2u64).to_le_bytes()).unwrap();
        file.write_all(&50i64.to_le_bytes()).unwrap();
        file.write_all(&5u64.to_le_bytes()).unwrap();
        file.write_all(&40i64.to_le_bytes()).unwrap();
        file.write_all(&4u64.to_le_bytes()).unwrap();

        let length = file.seek(SeekFrom::End(0)).unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();
        let err = read_track(&mut file, 0, length).expect_err("unsorted entries must fail");
        matches!(err, MemvidError::InvalidTimeIndex { .. });
    }

    fn oracle_range(entries: &[TimeIndexEntry], start: Option<i64>, end: Option<i64>) -> Vec<u64> {
        let mut ids: Vec<u64> = entries
            .iter()
            .filter(|e| {
                start.is_none_or(|s| e.timestamp >= s) && end.is_none_or(|x| e.timestamp <= x)
            })
            .map(|e| e.frame_id)
            .collect();
        ids.sort_unstable();
        ids
    }

    #[test]
    fn read_range_matches_linear_filter_across_windows() {
        let mut file = tempfile().expect("temp file");
        // Duplicate timestamps + gaps exercise the (ts, frame_id) ordering
        // and the boundary binary searches.
        let mut entries = vec![
            TimeIndexEntry::new(10, 0),
            TimeIndexEntry::new(10, 1),
            TimeIndexEntry::new(20, 2),
            TimeIndexEntry::new(30, 3),
            TimeIndexEntry::new(30, 4),
            TimeIndexEntry::new(50, 5),
            TimeIndexEntry::new(90, 6),
        ];
        let (offset, length, _) = append_track(&mut file, &mut entries).expect("append");
        let sorted = read_track(&mut file, offset, length).expect("read");

        // Every window shape: inclusive bounds, on/off entry timestamps,
        // one-sided, fully outside, and inverted.
        let bounds = [
            (Some(10), Some(90)),
            (Some(10), Some(10)),
            (Some(30), Some(30)),
            (Some(25), Some(55)),
            (Some(0), Some(5)),
            (Some(91), Some(1000)),
            (Some(30), None),
            (None, Some(20)),
            (None, None),
            (Some(60), Some(20)), // inverted → empty
        ];
        for (start, end) in bounds {
            let mut got: Vec<u64> = read_range(&mut file, offset, length, start, end)
                .expect("read_range")
                .into_iter()
                .map(|e| e.frame_id)
                .collect();
            got.sort_unstable();
            assert_eq!(
                got,
                oracle_range(&sorted, start, end),
                "range {start:?}..={end:?} mismatch"
            );
        }
    }

    #[test]
    fn read_range_on_empty_track_is_empty() {
        let mut file = tempfile().expect("temp file");
        let mut entries: Vec<TimeIndexEntry> = vec![];
        let (offset, length, _) = append_track(&mut file, &mut entries).expect("append");
        let got = read_range(&mut file, offset, length, Some(0), Some(100)).expect("read_range");
        assert!(got.is_empty());
    }

    #[test]
    fn calculate_checksum_is_deterministic() {
        let entries = vec![
            TimeIndexEntry::new(5, 10),
            TimeIndexEntry::new(1, 2),
            TimeIndexEntry::new(5, 9),
        ];
        let checksum_a = calculate_checksum(&entries);
        let checksum_b = calculate_checksum(&entries);
        assert_eq!(checksum_a, checksum_b);
    }
}
