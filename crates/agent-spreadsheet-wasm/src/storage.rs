//! Volatile owner-local journal. Physical persistence is never implied by this adapter.
use agent_spreadsheet::core::resident_storage::*;
use anyhow::{Result, bail};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

#[derive(Clone)]
pub(crate) struct MemoryJournal {
    session_id: String,
    base_sha256: String,
    records: Rc<RefCell<Vec<PreparedResidentCommit>>>,
    bytes: Rc<Cell<usize>>,
}
impl MemoryJournal {
    pub fn new(session_id: String, base_sha256: String) -> Self {
        Self {
            session_id,
            base_sha256,
            records: Rc::new(RefCell::new(Vec::new())),
            bytes: Rc::new(Cell::new(2)),
        }
    }
    fn bound(&self, session_id: &str) -> Result<()> {
        if session_id != self.session_id {
            bail!("journal resource binding mismatch");
        }
        Ok(())
    }
}
#[async_trait::async_trait(?Send)]
impl ResidentCommitStorage for MemoryJournal {
    async fn load(&self, session_id: &str) -> Result<Vec<PreparedResidentCommit>> {
        self.bound(session_id)?;
        Ok(self.records.borrow().clone())
    }
    async fn commit(&self, prepared: &PreparedResidentCommit) -> Result<DurableCommitOutcome> {
        self.bound(&prepared.session_id)?;
        if let ReconcileOutcome::Committed(outcome) = self
            .reconcile(
                &prepared.session_id,
                &prepared.request_id,
                &prepared.request_fingerprint,
            )
            .await?
        {
            return Ok(outcome);
        }
        let next_bytes = self
            .bytes
            .get()
            .saturating_add(serde_json::to_vec(prepared)?.len() + 1);
        if next_bytes > 32 * 1024 * 1024 {
            bail!("volatile journal exceeds its 32 MiB limit; record was not appended");
        }
        let mut records = self.records.borrow().clone();
        records.push(prepared.clone());
        PortableHistoryState::replay_bound(&records, &self.session_id, &self.base_sha256)?;
        let outcome = DurableCommitOutcome {
            commit_id: prepared.commit_id.clone(),
            sequence: records.len() as u64,
        };
        *self.records.borrow_mut() = records;
        self.bytes.set(next_bytes);
        Ok(outcome)
    }
    async fn reconcile(
        &self,
        session_id: &str,
        request_id: &str,
        fingerprint: &str,
    ) -> Result<ReconcileOutcome> {
        self.bound(session_id)?;
        for (index, record) in self.records.borrow().iter().enumerate() {
            if record.request_id == request_id {
                if record.request_fingerprint != fingerprint {
                    bail!("request identity reuse with different input");
                }
                return Ok(ReconcileOutcome::Committed(DurableCommitOutcome {
                    commit_id: record.commit_id.clone(),
                    sequence: index as u64 + 1,
                }));
            }
        }
        Ok(ReconcileOutcome::NotFound)
    }
    fn outcome_retention(&self) -> OutcomeRetention {
        OutcomeRetention::UnlimitedWhileJournalExists
    }
}
