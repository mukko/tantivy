use super::segment_register::SegmentRegister;
use std::sync::RwLock;
use core::SegmentMeta;
use core::META_FILEPATH;
use core::SegmentId;
use indexer::{SegmentEntry, SegmentState};
use std::path::PathBuf;
use std::collections::hash_set::HashSet;
use std::sync::{RwLockReadGuard, RwLockWriteGuard};
use std::fmt::{self, Debug, Formatter};

#[derive(Default)]
struct SegmentRegisters {
    uncommitted: SegmentRegister,
    committed: SegmentRegister,
    writing: HashSet<SegmentId>,    
}



/// The segment manager stores the list of segments
/// as well as their state.
///
/// It guarantees the atomicity of the 
/// changes (merges especially)
#[derive(Default)]
pub struct SegmentManager {
    registers: RwLock<SegmentRegisters>,
}

impl Debug for SegmentManager {
    fn fmt(&self, f: &mut Formatter) -> Result<(), fmt::Error> {
        let lock = self.read();
        write!(f, "{{ uncommitted: {:?}, committed: {:?} }}", lock.uncommitted, lock.committed)
    }
}


/// Returns the `SegmentMeta`s for (committed segment, uncommitted segments).
/// The result is consistent with other transactions.
///
/// For instance, a segment will not appear in both committed and uncommitted 
/// segments
pub fn get_segments(segment_manager: &SegmentManager,) -> (Vec<SegmentMeta>, Vec<SegmentMeta>) {
    let registers_lock = segment_manager.read();
    (registers_lock.committed.get_segments(),
     registers_lock.uncommitted.get_segments())
}

impl SegmentManager {
    
    pub fn from_segments(segment_metas: Vec<SegmentMeta>) -> SegmentManager {
        SegmentManager {
            registers: RwLock::new(SegmentRegisters {
                uncommitted: SegmentRegister::default(),
                committed: SegmentRegister::new(segment_metas),
                writing: HashSet::new(),
            }),
        }
    }

    pub fn segment_entries(&self,) -> Vec<SegmentEntry> {
        let mut segment_entries = self.read()
            .uncommitted
            .segment_entries();
        segment_entries.extend(
            self.read()
            .committed
            .segment_entries()
        );
        segment_entries
    }

    pub fn list_files(&self) -> HashSet<PathBuf> {
        let registers_lock = self.read();
        let mut files = HashSet::new();
        files.insert(META_FILEPATH.clone());
        
        let segment_metas =
            registers_lock.committed
                .get_segments()
                .into_iter()
                .chain(registers_lock.uncommitted
                    .get_segments()
                    .into_iter())
                .chain(registers_lock.writing
                    .iter()
                    .cloned()
                    .map(SegmentMeta::new));
        
        for segment_meta in segment_metas {
            files.extend(segment_meta.list_files());
        }
        files
    }

    pub fn segment_state(&self, segment_id: &SegmentId) -> Option<SegmentState> {
        self.segment_entry(segment_id)
            .map(|segment_entry| segment_entry.state())
    }

    pub fn segment_entry(&self, segment_id: &SegmentId) -> Option<SegmentEntry> {
        let registers = self.read();
        registers
            .committed
            .segment_entry(segment_id)
            .or_else(|| registers.uncommitted.segment_entry(segment_id))        
    }

    // Lock poisoning should never happen :
    // The lock is acquired and released within this class,
    // and the operations cannot panic. 
    fn read(&self,) -> RwLockReadGuard<SegmentRegisters> { 
        self.registers.read().expect("Failed to acquire read lock on SegmentManager.")
    }

    fn write(&self,) -> RwLockWriteGuard<SegmentRegisters> {
        self.registers.write().expect("Failed to acquire write lock on SegmentManager.")
    }

    /// Removes all of the uncommitted segments
    /// and returns them.
    pub fn rollback(&self,) -> Vec<SegmentId> {
        let mut registers_lock = self.write();
        let segment_ids = registers_lock.uncommitted.segment_ids();
        registers_lock.uncommitted.clear();
        segment_ids
    }

    pub fn commit(&self, segment_metas: Vec<SegmentMeta>) {
         let committed_segment_entries = segment_metas
                .into_iter()
                .map(|segment_meta| {
                    let segment_id = segment_meta.id();
                    let mut segment_entry = SegmentEntry::new(segment_meta);
                    if let Some(state) = self.segment_state(&segment_id) {
                        segment_entry.set_state(state);
                    }
                    segment_entry
                })
                .collect::<Vec<_>>();
        let mut registers_lock = self.write();
        registers_lock.committed.clear();
        registers_lock.uncommitted.clear();
        for segment_entry in committed_segment_entries {
            registers_lock.committed.add_segment_entry(segment_entry);
        }
    }
    
    pub fn start_merge(&self, segment_ids: &[SegmentId]) {
        let mut registers_lock = self.write();
        if registers_lock.uncommitted.contains_all(segment_ids) {
            for segment_id in segment_ids {
                registers_lock.uncommitted.start_merge(segment_id);
            }
        }
        else if registers_lock.committed.contains_all(segment_ids) {
            for segment_id in segment_ids {
                registers_lock.committed.start_merge(segment_id);
            }
        }
        else {
            error!("Merge operation sent for segments that are not all uncommited or commited.");
        }
    }

    pub fn write_segment(&self, segment_id: SegmentId) {
        let mut registers_lock = self.write();
        registers_lock.writing.insert(segment_id);
    }

    pub fn add_segment(&self, segment_entry: SegmentEntry) {
        let mut registers_lock = self.write();
        registers_lock.writing.remove(&segment_entry.segment_id());
        registers_lock.uncommitted.add_segment_entry(segment_entry);
    }
    
    pub fn end_merge(&self, merged_segment_metas: &[SegmentMeta], merged_segment_entry: SegmentEntry) {
        let mut registers_lock = self.write();
        let merged_segment_ids: Vec<SegmentId> = merged_segment_metas.iter().map(|meta| meta.id()).collect();
        if registers_lock.uncommitted.contains_all(&merged_segment_ids) {
            for segment_id in &merged_segment_ids {
                registers_lock.uncommitted.remove_segment(segment_id);
            }
            registers_lock.uncommitted.add_segment_entry(merged_segment_entry);
        }
        else if registers_lock.committed.contains_all(&merged_segment_ids) {
            for segment_id in &merged_segment_ids {
                registers_lock.committed.remove_segment(segment_id);
            }
            registers_lock.committed.add_segment_entry(merged_segment_entry);
        } else {
            warn!("couldn't find segment in SegmentManager");
        }
    }

    pub fn committed_segment_metas(&self,) -> Vec<SegmentMeta> {
        let registers_lock = self.read();
        registers_lock.committed.segment_metas()
    }
}
