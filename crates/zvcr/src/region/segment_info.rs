use crate::region::delta_sequence::DeltaSequence;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum SegmentStateType {
    #[default]
    Unknown = 0,
    New = 1,
    Old = 2,
}

impl SegmentStateType {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Unknown),
            1 => Some(Self::New),
            2 => Some(Self::Old),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentState {
    pub state_type: SegmentStateType,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SegmentInfo {
    pub reverse_deltas: Vec<SegmentState>,
}

impl DeltaSequence for SegmentInfo {
    type Snapshot = SegmentState;
    type SnapshotFrom = SegmentState;

    fn reverse_deltas(&self) -> &[Self::Snapshot] {
        &self.reverse_deltas
    }

    fn snapshot_timestamp(snapshot: &Self::Snapshot) -> i64 {
        snapshot.timestamp
    }

    fn snapshot_before(&self, timestamp: i64) -> Option<Self::SnapshotFrom> {
        let latest = self.latest_snapshot()?;
        let mut latest_state_type = latest.state_type;

        for state in &self.reverse_deltas {
            latest_state_type = state.state_type;
            if timestamp >= state.timestamp {
                break;
            }
        }
        Some(SegmentState {
            state_type: latest_state_type,
            timestamp,
        })
    }
}

impl SegmentInfo {
    pub fn insert_snapshot(&mut self, new_state: SegmentState) -> bool {
        if let Some(latest) = self.latest_snapshot()
            && (new_state.timestamp <= latest.timestamp
                || latest.state_type == new_state.state_type)
        {
            return false;
        }
        self.reverse_deltas.insert(0, new_state);
        true
    }
}
