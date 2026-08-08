use crate::definitions::*;
use crate::region::delta_sequence::DeltaSequence;
use crate::region::packed_data::{PackedData, PackedSnapshot};
use crate::region::unpacked_view::UnpackedData;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PackedDeltaData<const UNPACKED_SIZE: usize> {
    pub reverse_deltas: Vec<PackedSnapshot<UNPACKED_SIZE>>,
}

impl<const UNPACKED_SIZE: usize> DeltaSequence for PackedDeltaData<UNPACKED_SIZE> {
    type Snapshot = PackedSnapshot<UNPACKED_SIZE>;
    type SnapshotFrom = UnpackedData<UNPACKED_SIZE>;

    fn reverse_deltas(&self) -> &[Self::Snapshot] {
        &self.reverse_deltas
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

        for delta in self.reverse_deltas.iter().skip(1) {
            let unpacked = delta.data.unpack();
            for j in 0..UNPACKED_SIZE {
                let state = unpacked[j];
                if state != STATE_UNCHANGED {
                    latest_unpacked[j] = state;
                }
            }
            if timestamp >= delta.timestamp {
                break;
            }
        }
        Some(latest_unpacked)
    }
}

impl<const UNPACKED_SIZE: usize> PackedDeltaData<UNPACKED_SIZE> {
    pub fn insert_snapshot(
        &mut self,
        new_snapshot: PackedSnapshot<UNPACKED_SIZE>,
    ) -> Result<usize, DeltaInsertionStatus> {
        if let Some(latest) = self.latest_snapshot() {
            if new_snapshot.timestamp <= latest.timestamp {
                return Err(DeltaInsertionStatus::SnapshotOlderThanLatest);
            }

            let previous_unpacked = latest.data.unpack();
            let new_unpacked = new_snapshot.data.unpack();
            let mut delta_snapshot_builder = [0u16; UNPACKED_SIZE];
            let mut changes = 0;

            for i in 0..UNPACKED_SIZE {
                let previous = previous_unpacked[i];
                let changed = new_unpacked[i] != previous;
                delta_snapshot_builder[i] = if changed { previous } else { STATE_UNCHANGED };
                if changed {
                    changes += 1;
                }
            }

            if changes == 0 {
                return Err(DeltaInsertionStatus::NoChangesMade);
            }

            let delta_snapshot = PackedSnapshot {
                data: PackedData::pack(&delta_snapshot_builder),
                timestamp: latest.timestamp,
            };

            self.reverse_deltas.remove(0);
            self.reverse_deltas.insert(0, delta_snapshot);
            self.reverse_deltas.insert(0, new_snapshot);

            Ok(changes)
        } else {
            self.reverse_deltas.push(new_snapshot);
            Ok(UNPACKED_SIZE)
        }
    }
}