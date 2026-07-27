//! Bounded backlog policy for the browser Nemotron streaming host.
//!
//! Audio capture and OPFS are browser execution concerns, but the queue law is
//! not: how much PCM may remain in RAM, when older audio must spill to disk,
//! which source decodes next, and when overload becomes a loud failure all live
//! here as deterministic, native-tested Rust policy.
//!
//! The host follows a small command protocol:
//!
//! 1. [`push_samples`](NemotronBacklog::push_samples) captured PCM.
//! 2. Persist any [`staged_spill`](NemotronBacklog::staged_spill) batches and
//!    report them ready with [`mark_spill_ready`](NemotronBacklog::mark_spill_ready).
//! 3. Poll [`next_decode`](NemotronBacklog::next_decode). Spilled audio always
//!    precedes newer in-memory audio.
//! 4. Acknowledge a successfully decoded chunk. Failed in-memory chunks can be
//!    returned to the front with [`nack_memory`](NemotronBacklog::nack_memory).
//!
//! There is no silent drop path. If OPFS cannot keep up with the bounded
//! transfer allowance, or the bounded disk backlog is exhausted, the policy
//! returns a sticky error and the browser host stops capture loudly.

use std::collections::VecDeque;
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

const SAMPLE_RATE: usize = 16_000;

/// Shipping queue limits.
///
/// The normal in-Wasm queue targets 30 seconds. It stages 10-second spill
/// segments and permits at most two segments to be crossing the JS/OPFS
/// boundary. Counting those transfer buffers, the hard resident-audio budget is
/// 60 seconds (3.84 MB of Float32 PCM). Ready spill segments live only in OPFS.
///
/// Disk overflow is also bounded: no more than 30 minutes (115.2 MB) of
/// not-yet-decoded PCM. A machine that remains farther behind than that is not
/// functioning as a live transcriber, so the honest behavior is a loud stop,
/// never unbounded growth or silent loss.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NemotronBacklogConfig {
    /// Samples handed to the decoder per host call.
    pub chunk_samples: usize,
    /// Desired in-Wasm queue after a spill is staged.
    pub ram_target_samples: usize,
    /// Samples persisted in one OPFS spill segment.
    pub spill_segment_samples: usize,
    /// Hard bound covering the Rust queue plus spill-transfer buffers.
    pub resident_budget_samples: usize,
    /// Maximum not-yet-decoded samples stored in OPFS.
    pub disk_budget_samples: usize,
    /// Maximum staged/writing spill transfers.
    pub max_spill_transfers: usize,
}

impl NemotronBacklogConfig {
    /// Production defaults for 16 kHz Float32 PCM.
    pub const SHIPPING: Self = Self {
        chunk_samples: 4_000,
        ram_target_samples: 30 * SAMPLE_RATE,
        spill_segment_samples: 10 * SAMPLE_RATE,
        resident_budget_samples: 60 * SAMPLE_RATE,
        disk_budget_samples: 30 * 60 * SAMPLE_RATE,
        max_spill_transfers: 2,
    };

    /// Shipping limits with the browser-selected decoder feed size.
    #[must_use]
    pub const fn shipping_with_chunk_samples(chunk_samples: usize) -> Self {
        Self {
            chunk_samples,
            ..Self::SHIPPING
        }
    }

    fn validate(self) -> Result<Self, BacklogError> {
        if self.chunk_samples == 0
            || self.ram_target_samples == 0
            || self.spill_segment_samples == 0
            || self.resident_budget_samples < self.ram_target_samples + self.spill_segment_samples
            || self.disk_budget_samples < self.spill_segment_samples
            || self.max_spill_transfers == 0
        {
            return Err(BacklogError::InvalidConfig);
        }
        Ok(self)
    }
}

/// A spill batch the browser host must persist before it can be decoded.
#[derive(Debug, Clone, PartialEq)]
pub struct SpillBatch {
    /// Monotonic identifier used in the OPFS filename and acknowledgements.
    pub id: u32,
    /// Oldest queued PCM samples, in exact capture order.
    pub samples: Vec<f32>,
}

/// Lightweight descriptor exposed before the samples cross the Wasm boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpillDescriptor {
    /// Spill identifier.
    pub id: u32,
    /// Number of Float32 samples in the segment.
    pub samples: usize,
}

/// What the browser executor should do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum DecodeAction {
    /// The oldest audio is still being committed to OPFS.
    WaitForSpill,
    /// Read this exact sample range from an OPFS spill segment.
    Spool {
        /// Spill identifier.
        id: u32,
        /// Sample offset within the spill file.
        offset_samples: usize,
        /// Samples to decode in this call.
        count: usize,
    },
    /// Copy and decode the oldest in-Wasm samples.
    Memory {
        /// Samples to decode in this call.
        count: usize,
    },
    /// Nothing is currently drainable.
    Empty,
}

/// Bounded-queue telemetry. All counts are samples at 16 kHz.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BacklogSnapshot {
    /// Audio waiting to decode, including OPFS and in-memory sources.
    pub pending_samples: usize,
    /// Audio currently resident in queue/transfer buffers.
    pub resident_samples: usize,
    /// Audio waiting in ready or in-flight OPFS spill records.
    pub spooled_samples: usize,
    /// Number of spill files/records not fully consumed.
    pub spill_count: usize,
    /// Total samples successfully handed through the decoder.
    pub consumed_samples: u64,
}

/// A hard queue failure. These are deliberately terminal and sticky.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BacklogError {
    /// Configuration cannot uphold its own bounds.
    InvalidConfig,
    /// OPFS transfers did not release memory before the resident cap was hit.
    ResidentBudgetExceeded {
        /// Samples already resident.
        resident: usize,
        /// New samples the host attempted to append.
        incoming: usize,
        /// Configured hard budget.
        budget: usize,
    },
    /// The not-yet-decoded OPFS backlog reached its hard disk cap.
    DiskBudgetExceeded {
        /// Samples already waiting in spill records.
        spooled: usize,
        /// Configured hard disk budget.
        budget: usize,
    },
    /// The host violated the spill/decode acknowledgement protocol.
    Protocol(String),
    /// The browser failed to persist a staged spill.
    SpillWriteFailed {
        /// Spill identifier.
        id: u32,
        /// Browser error text.
        message: String,
    },
}

impl fmt::Display for BacklogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig => write!(f, "invalid Nemotron backlog configuration"),
            Self::ResidentBudgetExceeded {
                resident,
                incoming,
                budget,
            } => write!(
                f,
                "Nemotron audio backlog hit its RAM bound \
                 ({resident} resident + {incoming} incoming > {budget} samples); \
                 temporary OPFS spill could not keep up"
            ),
            Self::DiskBudgetExceeded { spooled, budget } => write!(
                f,
                "Nemotron fell too far behind: temporary OPFS backlog reached \
                 its bound ({spooled}/{budget} samples)"
            ),
            Self::Protocol(message) => write!(f, "Nemotron backlog protocol error: {message}"),
            Self::SpillWriteFailed { id, message } => {
                write!(f, "Nemotron temporary audio spill {id} failed: {message}")
            }
        }
    }
}

impl Error for BacklogError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SpillState {
    Staged,
    Writing,
    Ready,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SpillRecord {
    id: u32,
    samples: usize,
    consumed: usize,
    state: SpillState,
}

impl SpillRecord {
    fn remaining(self) -> usize {
        self.samples - self.consumed
    }
}

/// Pure Rust bounded backlog and spill-ordering policy.
#[derive(Debug)]
pub struct NemotronBacklog {
    config: NemotronBacklogConfig,
    memory: VecDeque<f32>,
    staged: VecDeque<SpillBatch>,
    spills: VecDeque<SpillRecord>,
    memory_inflight: Option<Vec<f32>>,
    next_spill_id: u32,
    consumed_samples: u64,
    fault: Option<BacklogError>,
}

impl NemotronBacklog {
    /// Construct a queue with validated limits.
    ///
    /// # Errors
    ///
    /// Returns [`BacklogError::InvalidConfig`] when the limits cannot uphold
    /// the declared RAM/disk bounds.
    pub fn new(config: NemotronBacklogConfig) -> Result<Self, BacklogError> {
        Ok(Self {
            config: config.validate()?,
            memory: VecDeque::new(),
            staged: VecDeque::new(),
            spills: VecDeque::new(),
            memory_inflight: None,
            next_spill_id: 1,
            consumed_samples: 0,
            fault: None,
        })
    }

    /// Append captured PCM and stage old audio for OPFS when the in-Wasm target
    /// is crossed.
    ///
    /// # Errors
    ///
    /// Returns a sticky hard-bound error. No samples are silently discarded.
    pub fn push_samples(&mut self, samples: &[f32]) -> Result<BacklogSnapshot, BacklogError> {
        self.ensure_healthy()?;
        let resident = self.resident_samples();
        if resident.saturating_add(samples.len()) > self.config.resident_budget_samples {
            return self.fail(BacklogError::ResidentBudgetExceeded {
                resident,
                incoming: samples.len(),
                budget: self.config.resident_budget_samples,
            });
        }
        self.memory.extend(samples.iter().copied());
        self.stage_spills()?;
        Ok(self.snapshot())
    }

    /// Descriptor for the oldest staged batch, if the host should write one.
    #[must_use]
    pub fn staged_spill(&self) -> Option<SpillDescriptor> {
        self.staged.front().map(|batch| SpillDescriptor {
            id: batch.id,
            samples: batch.samples.len(),
        })
    }

    /// Move a staged batch across the host boundary and mark it writing.
    ///
    /// # Errors
    ///
    /// Returns a protocol error for an out-of-order or unknown id.
    pub fn take_staged_spill(&mut self, id: u32) -> Result<SpillBatch, BacklogError> {
        self.ensure_healthy()?;
        let Some(front) = self.staged.front() else {
            return Err(BacklogError::Protocol(format!(
                "spill {id} requested but no spill is staged"
            )));
        };
        if front.id != id {
            return Err(BacklogError::Protocol(format!(
                "spill {id} requested before staged spill {}",
                front.id
            )));
        }
        let Some(record) = self.spills.iter_mut().find(|record| record.id == id) else {
            return Err(BacklogError::Protocol(format!(
                "staged spill {id} has no ordering record"
            )));
        };
        if record.state != SpillState::Staged {
            return Err(BacklogError::Protocol(format!(
                "spill {id} is not in staged state"
            )));
        }
        record.state = SpillState::Writing;
        self.staged.pop_front().ok_or_else(|| {
            BacklogError::Protocol(format!("staged spill {id} disappeared before transfer"))
        })
    }

    /// Mark a browser spill commit complete and make it decodeable.
    ///
    /// # Errors
    ///
    /// Returns a protocol error for an unknown/out-of-state id.
    pub fn mark_spill_ready(&mut self, id: u32) -> Result<BacklogSnapshot, BacklogError> {
        self.ensure_healthy()?;
        let Some(record) = self.spills.iter_mut().find(|record| record.id == id) else {
            return Err(BacklogError::Protocol(format!(
                "ready acknowledgement for unknown spill {id}"
            )));
        };
        if record.state != SpillState::Writing {
            return Err(BacklogError::Protocol(format!(
                "spill {id} became ready from an invalid state"
            )));
        }
        record.state = SpillState::Ready;
        self.stage_spills()?;
        Ok(self.snapshot())
    }

    /// Turn a browser write failure into a sticky, loud queue fault.
    pub fn mark_spill_failed(&mut self, id: u32, message: impl Into<String>) -> BacklogError {
        let error = BacklogError::SpillWriteFailed {
            id,
            message: message.into(),
        };
        self.fault = Some(error.clone());
        error
    }

    /// Decide which ordered source the host should decode next.
    ///
    /// `final_pass` permits the last partial in-memory chunk. Normal streaming
    /// waits for a whole configured decoder chunk.
    ///
    /// # Errors
    ///
    /// Returns a sticky queue error or a protocol error when a memory chunk is
    /// already awaiting acknowledgement.
    pub fn next_decode(&self, final_pass: bool) -> Result<DecodeAction, BacklogError> {
        self.ensure_healthy()?;
        if self.memory_inflight.is_some() {
            return Err(BacklogError::Protocol(
                "next decode requested before the in-memory chunk was acknowledged".to_owned(),
            ));
        }
        if let Some(spill) = self.spills.front().copied() {
            return Ok(match spill.state {
                SpillState::Staged | SpillState::Writing => DecodeAction::WaitForSpill,
                SpillState::Ready => DecodeAction::Spool {
                    id: spill.id,
                    offset_samples: spill.consumed,
                    count: self.config.chunk_samples.min(spill.remaining()),
                },
            });
        }
        if self.memory.len() >= self.config.chunk_samples || (final_pass && !self.memory.is_empty())
        {
            return Ok(DecodeAction::Memory {
                count: self.config.chunk_samples.min(self.memory.len()),
            });
        }
        Ok(DecodeAction::Empty)
    }

    /// Copy the commanded memory chunk to the host while retaining a bounded
    /// acknowledgement copy inside the policy.
    ///
    /// # Errors
    ///
    /// Returns a protocol error if the count/source does not match
    /// [`next_decode`](Self::next_decode).
    pub fn take_memory(&mut self, count: usize) -> Result<Vec<f32>, BacklogError> {
        self.ensure_healthy()?;
        if self.memory_inflight.is_some() || !self.spills.is_empty() {
            return Err(BacklogError::Protocol(
                "in-memory audio requested while another/older source is active".to_owned(),
            ));
        }
        let allowed = count > 0 && count <= self.config.chunk_samples && count <= self.memory.len();
        if !allowed {
            return Err(BacklogError::Protocol(format!(
                "invalid in-memory decode count {count}"
            )));
        }
        let chunk: Vec<f32> = self.memory.drain(..count).collect();
        self.memory_inflight = Some(chunk.clone());
        Ok(chunk)
    }

    /// Commit a successful in-memory decode.
    ///
    /// # Errors
    ///
    /// Returns a protocol error for a mismatched acknowledgement.
    pub fn ack_memory(&mut self, count: usize) -> Result<BacklogSnapshot, BacklogError> {
        self.ensure_healthy()?;
        let Some(chunk) = self.memory_inflight.take() else {
            return Err(BacklogError::Protocol(
                "memory acknowledgement without an in-flight chunk".to_owned(),
            ));
        };
        if chunk.len() != count {
            self.memory_inflight = Some(chunk);
            return Err(BacklogError::Protocol(format!(
                "memory acknowledgement count {count} does not match in-flight length"
            )));
        }
        self.consumed_samples = self.consumed_samples.saturating_add(count as u64);
        self.stage_spills()?;
        Ok(self.snapshot())
    }

    /// Return a failed in-memory decode to the exact front of the queue.
    ///
    /// # Errors
    ///
    /// Returns a protocol error when no memory chunk is in flight.
    pub fn nack_memory(&mut self) -> Result<BacklogSnapshot, BacklogError> {
        self.ensure_healthy()?;
        let Some(chunk) = self.memory_inflight.take() else {
            return Err(BacklogError::Protocol(
                "memory rejection without an in-flight chunk".to_owned(),
            ));
        };
        for sample in chunk.into_iter().rev() {
            self.memory.push_front(sample);
        }
        Ok(self.snapshot())
    }

    /// Commit a successful OPFS-backed decode chunk.
    ///
    /// Returns `true` when the entire spill is consumed and its host file may
    /// be deleted.
    ///
    /// # Errors
    ///
    /// Returns a protocol error for out-of-order ids/counts.
    pub fn ack_spill(&mut self, id: u32, count: usize) -> Result<bool, BacklogError> {
        self.ensure_healthy()?;
        let Some(front) = self.spills.front_mut() else {
            return Err(BacklogError::Protocol(format!(
                "spill {id} acknowledged with no spill queued"
            )));
        };
        if front.id != id || front.state != SpillState::Ready {
            return Err(BacklogError::Protocol(format!(
                "spill {id} acknowledged out of order or before ready"
            )));
        }
        let expected = self.config.chunk_samples.min(front.remaining());
        if count != expected {
            return Err(BacklogError::Protocol(format!(
                "spill {id} acknowledgement count {count} != expected {expected}"
            )));
        }
        front.consumed += count;
        self.consumed_samples = self.consumed_samples.saturating_add(count as u64);
        let completed = front.consumed == front.samples;
        if completed {
            self.spills.pop_front();
        }
        Ok(completed)
    }

    /// Current bounded telemetry.
    #[must_use]
    pub fn snapshot(&self) -> BacklogSnapshot {
        BacklogSnapshot {
            pending_samples: self.pending_samples(),
            resident_samples: self.resident_samples(),
            spooled_samples: self.spilled_samples(),
            spill_count: self.spills.len(),
            consumed_samples: self.consumed_samples,
        }
    }

    /// Clear per-session audio while retaining the monotonic spill id, so late
    /// acknowledgements from an old browser write can never alias a new spill.
    pub fn reset(&mut self) {
        self.memory.clear();
        self.staged.clear();
        self.spills.clear();
        self.memory_inflight = None;
        self.consumed_samples = 0;
        self.fault = None;
    }

    fn ensure_healthy(&self) -> Result<(), BacklogError> {
        match &self.fault {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }

    fn fail<T>(&mut self, error: BacklogError) -> Result<T, BacklogError> {
        self.fault = Some(error.clone());
        Err(error)
    }

    fn stage_spills(&mut self) -> Result<(), BacklogError> {
        let high_watermark = self.config.ram_target_samples + self.config.spill_segment_samples;
        while self.memory.len() > high_watermark
            && self.active_spill_transfers() < self.config.max_spill_transfers
        {
            let spooled = self.spilled_samples();
            if spooled.saturating_add(self.config.spill_segment_samples)
                > self.config.disk_budget_samples
            {
                return self.fail(BacklogError::DiskBudgetExceeded {
                    spooled,
                    budget: self.config.disk_budget_samples,
                });
            }
            let id = self.next_spill_id;
            self.next_spill_id = self.next_spill_id.wrapping_add(1).max(1);
            let samples: Vec<f32> = self
                .memory
                .drain(..self.config.spill_segment_samples)
                .collect();
            self.spills.push_back(SpillRecord {
                id,
                samples: samples.len(),
                consumed: 0,
                state: SpillState::Staged,
            });
            self.staged.push_back(SpillBatch { id, samples });
        }
        Ok(())
    }

    fn active_spill_transfers(&self) -> usize {
        self.spills
            .iter()
            .filter(|spill| matches!(spill.state, SpillState::Staged | SpillState::Writing))
            .count()
    }

    fn pending_samples(&self) -> usize {
        self.memory.len().saturating_add(self.spilled_samples())
    }

    fn spilled_samples(&self) -> usize {
        self.spills.iter().map(|spill| spill.remaining()).sum()
    }

    fn resident_samples(&self) -> usize {
        let staged: usize = self.staged.iter().map(|spill| spill.samples.len()).sum();
        let writing: usize = self
            .spills
            .iter()
            .filter(|spill| spill.state == SpillState::Writing)
            .map(|spill| spill.remaining())
            .sum();
        let inflight = self.memory_inflight.as_ref().map_or(0, Vec::len);
        self.memory
            .len()
            .saturating_add(staged)
            .saturating_add(writing)
            .saturating_add(inflight)
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests use unwrap/expect as the assertion mechanism; the workspace \
              lint config permits this in test code (PRD 'Rust engineering bar')"
)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    const TEST: NemotronBacklogConfig = NemotronBacklogConfig {
        chunk_samples: 4,
        ram_target_samples: 8,
        spill_segment_samples: 4,
        resident_budget_samples: 20,
        disk_budget_samples: 1_000,
        max_spill_transfers: 2,
    };

    fn persist_staged(
        queue: &mut NemotronBacklog,
        disk: &mut BTreeMap<u32, Vec<f32>>,
    ) -> Result<(), BacklogError> {
        while let Some(desc) = queue.staged_spill() {
            let batch = queue.take_staged_spill(desc.id)?;
            assert_eq!(batch.samples.len(), desc.samples);
            disk.insert(batch.id, batch.samples);
            queue.mark_spill_ready(batch.id)?;
        }
        Ok(())
    }

    #[test]
    fn spill_and_memory_decode_preserve_every_sample_in_order() {
        let mut queue = NemotronBacklog::new(TEST).unwrap();
        let mut disk = BTreeMap::new();
        let expected: Vec<f32> = (0_u16..100).map(f32::from).collect();

        for pair in expected.chunks(2) {
            queue.push_samples(pair).unwrap();
            persist_staged(&mut queue, &mut disk).unwrap();
            assert!(queue.snapshot().resident_samples <= TEST.resident_budget_samples);
        }

        let mut decoded = Vec::new();
        loop {
            match queue.next_decode(true).unwrap() {
                DecodeAction::Spool {
                    id,
                    offset_samples,
                    count,
                } => {
                    let samples = &disk[&id][offset_samples..offset_samples + count];
                    decoded.extend_from_slice(samples);
                    if queue.ack_spill(id, count).unwrap() {
                        disk.remove(&id);
                    }
                }
                DecodeAction::Memory { count } => {
                    let samples = queue.take_memory(count).unwrap();
                    decoded.extend_from_slice(&samples);
                    queue.ack_memory(count).unwrap();
                }
                DecodeAction::WaitForSpill => panic!("test host commits every staged spill"),
                DecodeAction::Empty => break,
            }
        }

        assert_eq!(decoded, expected);
        assert!(disk.is_empty());
        assert_eq!(queue.snapshot().pending_samples, 0);
        assert_eq!(queue.snapshot().consumed_samples, expected.len() as u64);
    }

    #[test]
    fn ten_minute_slow_session_keeps_resident_audio_bounded() {
        let mut queue = NemotronBacklog::new(NemotronBacklogConfig::SHIPPING).unwrap();
        let mut disk = BTreeMap::new();
        let capture = vec![0.25f32; 4_000];
        let ticks = 10 * 60 * 4;

        for _ in 0..ticks {
            queue.push_samples(&capture).unwrap();
            persist_staged(&mut queue, &mut disk).unwrap();
            assert!(
                queue.snapshot().resident_samples
                    <= NemotronBacklogConfig::SHIPPING.resident_budget_samples
            );
        }

        let snapshot = queue.snapshot();
        assert_eq!(snapshot.pending_samples, ticks * capture.len());
        assert!(snapshot.spooled_samples > 0);
        assert!(
            snapshot.resident_samples <= NemotronBacklogConfig::SHIPPING.resident_budget_samples
        );
    }

    #[test]
    fn stalled_spill_writer_fails_loudly_at_ram_bound() {
        let mut queue = NemotronBacklog::new(TEST).unwrap();
        let input = [0.0f32; 2];
        let mut saw_error = None;

        for _ in 0..20 {
            match queue.push_samples(&input) {
                Ok(_) => {
                    while let Some(desc) = queue.staged_spill() {
                        // Take the batch, but never report the OPFS write ready.
                        queue.take_staged_spill(desc.id).unwrap();
                    }
                }
                Err(error) => {
                    saw_error = Some(error);
                    break;
                }
            }
        }

        assert!(matches!(
            saw_error,
            Some(BacklogError::ResidentBudgetExceeded { .. })
        ));
        assert!(matches!(
            queue.push_samples(&input),
            Err(BacklogError::ResidentBudgetExceeded { .. })
        ));
    }

    #[test]
    fn disk_backlog_limit_is_terminal_instead_of_unbounded() {
        let config = NemotronBacklogConfig {
            disk_budget_samples: 8,
            ..TEST
        };
        let mut queue = NemotronBacklog::new(config).unwrap();
        let mut disk = BTreeMap::new();
        let input = [0.0f32; 2];
        let mut saw_error = None;

        for _ in 0..30 {
            match queue.push_samples(&input) {
                Ok(_) => persist_staged(&mut queue, &mut disk).unwrap(),
                Err(error) => {
                    saw_error = Some(error);
                    break;
                }
            }
        }

        assert!(matches!(
            saw_error,
            Some(BacklogError::DiskBudgetExceeded { .. })
        ));
    }

    #[test]
    fn failed_decode_can_be_retried_without_reordering() {
        let mut queue = NemotronBacklog::new(TEST).unwrap();
        queue.push_samples(&[1.0, 2.0, 3.0, 4.0]).unwrap();
        assert_eq!(
            queue.next_decode(false).unwrap(),
            DecodeAction::Memory { count: 4 }
        );
        assert_eq!(queue.take_memory(4).unwrap(), vec![1.0, 2.0, 3.0, 4.0]);
        queue.nack_memory().unwrap();
        assert_eq!(queue.take_memory(4).unwrap(), vec![1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn reset_clears_audio_but_does_not_reuse_spill_ids() {
        let mut queue = NemotronBacklog::new(TEST).unwrap();
        let mut disk = BTreeMap::new();
        queue.push_samples(&[0.0; 14]).unwrap();
        let first = queue.staged_spill().unwrap().id;
        persist_staged(&mut queue, &mut disk).unwrap();
        queue.reset();
        queue.push_samples(&[0.0; 14]).unwrap();
        let second = queue.staged_spill().unwrap().id;
        assert!(second > first);
        assert_eq!(queue.snapshot().consumed_samples, 0);
    }
}
