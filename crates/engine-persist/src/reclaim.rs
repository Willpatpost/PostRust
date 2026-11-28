//! The reclamation ledger (PP-B3, D-PP-4's verify-before-reclaim
//! retention protocol — the audit half): the versioned
//! `engine-persist/reclamation-ledger/1` object referenced by the
//! reclamation's audit revision on the canonical chain. The audit is
//! appended BEFORE any destructive action (Phase A — the eligibility
//! verified against the still-present bytes, the digest recomputed,
//! the retention window checked) and read back verified; the deletes
//! (Phase B) are idempotent and restartable — a re-run re-derives
//! the eligibility, recognizes the installed audit by its
//! deterministic idempotency key bound to the reclaimed checkpoint,
//! and deletes only what remains.
//!
//! **The binding, never the duplication:** the ledger binds to the
//! reclaimed checkpoint's MK — the PERMANENT metadata object (never
//! GC'd) already carrying the exact identities, keys, epochs, skips,
//! and digests. The audit records the event's bindings (the MK and
//! SK identities, the pre-verified range digest and its seed, the
//! counts, the journal files removed alongside) — the exact
//! identity list lives in the permanent metadata, never
//! reserialized here.

//!
//! **Module tree (P3-L9, the A13/A01-persist file-moves half):**
//! this root keeps the module contract — the original import block
//! (which every submodule sees through `use super::*`) and the full
//! re-export surface, so `crate::reclaim::*` and
//! `engine_persist::reclaim::*` paths resolve unchanged. The four
//! submodules split by change reason: `ledger` (the record codecs
//! and wire contracts), `discovery` (the reader-generic evidence
//! discovery and verification), `plan` (the publications), and
//! `execution` (the destructive partition and completion).

use std::collections::HashSet;

use engine_core::{Decoder, Encoder, SaveError};

use crate::hash::Hash;
use crate::medium::{AppendOutcome, CanonicalMedium, MediumError, StoredObject, peek_domain};
use crate::record::RevisionRecord;

mod discovery;
mod execution;
mod ledger;
mod plan;

pub use discovery::{
    BeginReads, MediumReads, collect_bound_reclaimable_journal_files,
    collect_reclaimable_journal_files, find_installed_completion,
    find_installed_journal_root_binding, find_installed_plan, find_installed_v2_audit,
    founding_identity, founding_identity_medium, journal_root_id, verify_bound_journal_authority,
    verify_canonical_v1_audit, verify_completion_state, verify_first_execution_evidence,
    verify_installed_begin, verify_installed_begin_v2, verify_installed_v1_audit,
    verify_reclamation_complete, verify_v2_execution_readiness, verify_v2_execution_readiness_via,
};
pub(crate) use discovery::{
    collect_unit_objects, discover_keyed_set_via, find_installed_plan_via, find_on_chain_via,
    read_record_via, verify_v2_begin_authority_evidence_via,
};
#[cfg(test)]
pub(crate) use discovery::{collect_unit_objects_via, discover_keyed_set};
pub use execution::{
    complete_reclamation, partition_reclaim, survivor_objects, verify_founding_seed,
    verify_newest_first_order,
};
pub use ledger::{
    BeginAuthority, BeginPublicationOutcome, BoundCreationError, BoundJournalAuthority,
    CompletionKind, JOURNAL_ROOT_BINDING_DOMAIN, JOURNAL_ROOT_BINDING_KEY_DOMAIN,
    JOURNAL_ROOT_BINDING_VERSION, JournalRootBinding, JournalRootBindingOutcome,
    LedgerV2PublicationOutcome, MAX_RECLAMATION_DEFERRED_IDENTITIES, MAX_RECLAMATION_JOURNAL_FILES,
    MAX_RECLAMATION_JOURNAL_NAME, MAX_RECLAMATION_LEDGER_BYTES, MAX_RECLAMATION_UNIT_OBJECTS,
    MAX_RECLAMATION_UNIT_OBJECTS_V2, MAX_RECLAMATION_UNITS, MIN_SUPPORTED_EFFECTIVE_OBJECT_BYTES,
    PlanBatch, PlanPublicationOutcome, RECLAMATION_BEGIN_DOMAIN, RECLAMATION_BEGIN_FAMILY,
    RECLAMATION_BEGIN_KEY_DOMAIN, RECLAMATION_BEGIN_VERSION, RECLAMATION_COMPLETE_DOMAIN,
    RECLAMATION_COMPLETE_KEY_DOMAIN, RECLAMATION_COMPLETE_VERSION, RECLAMATION_LEDGER_DOMAIN,
    RECLAMATION_LEDGER_FAMILY, RECLAMATION_LEDGER_KEY_DOMAIN, RECLAMATION_LEDGER_V2_DOMAIN,
    RECLAMATION_LEDGER_V2_KEY_DOMAIN, RECLAMATION_LEDGER_V2_VERSION, RECLAMATION_LEDGER_VERSION,
    RECLAMATION_PLAN_DOMAIN, RECLAMATION_PLAN_DOMAIN_PREFIX, RECLAMATION_PLAN_KEY_DOMAIN,
    RECLAMATION_PLAN_VERSION, ReclaimDirective, ReclaimOutcome, ReclaimPlan, ReclaimUnit,
    ReclamationBegin, ReclamationComplete, ReclamationLedger, ReclamationLedgerV2,
    V2BeginAuthorityEvidence, V2ExecutionReadiness, V2InstalledBegin,
    enforce_completion_marker_gate, enforce_journal_root_binding_gate,
    enforce_reclamation_begin_gate, enforce_reclamation_ledger_family_gate,
    enforce_reclamation_plan_gate, is_reclamation_begin_family, is_reclamation_ledger_family,
    journal_root_binding_idempotency_key, reclamation_begin_idempotency_key,
    reclamation_complete_idempotency_key, reclamation_ledger_idempotency_key,
    reclamation_ledger_v2_idempotency_key, reclamation_plan_idempotency_key,
};
pub use plan::{
    create_campaign_bound_journal, publish_journal_root_binding, publish_reclamation_complete,
    publish_reclamation_ledger, publish_reclamation_ledger_v2, publish_reclamation_plan,
    publish_reclamation_plan_authorized,
};
#[cfg(feature = "composite-test-fixture")]
pub use plan::{test_only_install_begin_record, test_only_install_completion_marker};

#[cfg(test)]
mod tests;
