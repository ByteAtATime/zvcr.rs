use crate::time_utils::find_nearest_timestamp;

pub trait DeltaSequence {
    type Snapshot;
    type SnapshotFrom;

    fn reverse_deltas(&self) -> &[Self::Snapshot];
    fn snapshot_timestamp(snapshot: &Self::Snapshot) -> i64;
    fn snapshot_before(&self, timestamp: i64) -> Option<Self::SnapshotFrom>;

    fn latest_snapshot(&self) -> Option<&Self::Snapshot> {
        self.reverse_deltas().first()
    }

    fn delta(&self, delta_index: usize) -> Option<&Self::Snapshot> {
        self.reverse_deltas().get(delta_index)
    }

    fn snapshot_from(&self, timestamp: i64) -> Option<Self::SnapshotFrom> {
        let nearest =
            find_nearest_timestamp(self.reverse_deltas(), Self::snapshot_timestamp, timestamp);
        self.snapshot_before(nearest)
    }
}
