use crate::definitions::*;
use crate::region::delta_sequence::DeltaSequence;
use crate::region::packed_data::{PackedData, PackedSnapshot};
use crate::region::unpacked_view::UnpackedData;
use std::ops::Range;
use std::sync::Arc;

#[derive(Clone)]
pub struct PackedDeltaData<const UNPACKED_SIZE: usize> {
    storage: Arc<Vec<PackedSnapshot<UNPACKED_SIZE>>>,
    range: Range<usize>,
}

impl<const UNPACKED_SIZE: usize> Default for PackedDeltaData<UNPACKED_SIZE> {
    fn default() -> Self {
        Self {
            storage: Arc::new(Vec::new()),
            range: 0..0,
        }
    }
}

impl<const UNPACKED_SIZE: usize> PartialEq for PackedDeltaData<UNPACKED_SIZE> {
    fn eq(&self, other: &Self) -> bool {
        self.snapshots() == other.snapshots()
    }
}

impl<const UNPACKED_SIZE: usize> Eq for PackedDeltaData<UNPACKED_SIZE> {}

impl<const UNPACKED_SIZE: usize> std::fmt::Debug for PackedDeltaData<UNPACKED_SIZE> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.snapshots().iter()).finish()
    }
}

impl<const UNPACKED_SIZE: usize> PackedDeltaData<UNPACKED_SIZE> {
    pub(crate) fn from_shared(
        storage: Arc<Vec<PackedSnapshot<UNPACKED_SIZE>>>,
        range: Range<usize>,
    ) -> Self {
        Self { storage, range }
    }

    pub fn snapshots(&self) -> &[PackedSnapshot<UNPACKED_SIZE>] {
        &self.storage[self.range.clone()]
    }

    pub fn new(snapshots: Vec<PackedSnapshot<UNPACKED_SIZE>>) -> Self {
        let len = snapshots.len();
        Self {
            storage: Arc::new(snapshots),
            range: 0..len,
        }
    }

    fn ensure_owned_storage(&mut self) {
        let needs_normalize = self.range.start != 0 || self.range.end != self.storage.len();
        if needs_normalize || Arc::strong_count(&self.storage) > 1 {
            let owned = self.storage[self.range.clone()].to_vec();
            self.storage = Arc::new(owned);
            self.range = 0..self.storage.len();
        }
    }

    pub fn insert_snapshot(
        &mut self,
        new_snapshot: PackedSnapshot<UNPACKED_SIZE>,
    ) -> Result<usize, DeltaInsertionStatus> {
        self.ensure_owned_storage();
        let result = {
            let storage = Arc::get_mut(&mut self.storage).expect("uniquely owned after ensure");
            if storage.is_empty() {
                storage.push(new_snapshot);
                Ok(UNPACKED_SIZE)
            } else {
                if new_snapshot.timestamp <= storage[0].timestamp {
                    Err(DeltaInsertionStatus::SnapshotOlderThanLatest)
                } else {
                    let previous_unpacked = storage[0].data.unpack();
                    let new_unpacked = new_snapshot.data.unpack();
                    let mut delta_snapshot_builder = [0u16; UNPACKED_SIZE];
                    let mut changes = 0;

                    for i in 0..UNPACKED_SIZE {
                        let previous = previous_unpacked[i];
                        let changed = new_unpacked[i] != previous;
                        delta_snapshot_builder[i] =
                            if changed { previous } else { STATE_UNCHANGED };
                        if changed {
                            changes += 1;
                        }
                    }

                    if changes == 0 {
                        Err(DeltaInsertionStatus::NoChangesMade)
                    } else {
                        let latest_timestamp = storage[0].timestamp;
                        storage.remove(0);
                        storage.insert(
                            0,
                            PackedSnapshot {
                                data: PackedData::pack(&delta_snapshot_builder),
                                timestamp: latest_timestamp,
                            },
                        );
                        storage.insert(0, new_snapshot);
                        Ok(changes)
                    }
                }
            }
        };
        if result.is_ok() {
            self.range = 0..self.storage.len();
        }
        result
    }
}

impl<const UNPACKED_SIZE: usize> DeltaSequence for PackedDeltaData<UNPACKED_SIZE> {
    type Snapshot = PackedSnapshot<UNPACKED_SIZE>;
    type SnapshotFrom = UnpackedData<UNPACKED_SIZE>;

    fn reverse_deltas(&self) -> &[Self::Snapshot] {
        self.snapshots()
    }

    fn snapshot_timestamp(snapshot: &Self::Snapshot) -> i64 {
        snapshot.timestamp
    }

    fn snapshot_before(&self, timestamp: i64) -> Option<Self::SnapshotFrom> {
        let latest_packed = self.latest_snapshot()?;
        let mut latest_unpacked = latest_packed.data.unpack();

        if timestamp >= latest_packed.timestamp {
            return Some(latest_unpacked);
        }

        for delta in self.snapshots().iter().skip(1) {
            delta.data.unpack_delta_into(&mut latest_unpacked);
            if timestamp >= delta.timestamp {
                break;
            }
        }
        Some(latest_unpacked)
    }
}
