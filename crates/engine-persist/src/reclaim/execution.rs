//! The destructive execution and partition (split from the reclaim
//! monolith by change reason, P3-L9 pure move): the newest-first
//! partition, the founding-seed and ordering authorizations, the
//! survivor closure, and the Phase A/B completion. These change
//! when the verify-before-delete execution semantics change.

use super::*;

/// The survivor-aware Mandatory/Deferred partition (the trusted
/// directive's decision core): given the verified audit's units and
/// the SURVIVOR SET — the exclusion walk's reachable identities,
/// everything that must remain reachable after the authorized
/// retirement — classify every audited identity. Identity-set
/// semantics throughout: a shared object appearing in multiple
/// units classifies exactly once (deleted at most once), and an
/// object referenced by ANY surviving root is DEFERRED regardless
/// of how many retiring units name it. The retiring records
/// themselves are mandatory (the audit's exact membership
/// authorizes their removal; the exclusion walk never reaches
/// them). The result is reproducible from verified canonical
/// evidence after an interruption (the same units and the same
/// walk derive the same partition).
pub fn partition_reclaim(
    units: &[ReclaimUnit],
    survivors: &std::collections::BTreeSet<Hash>,
) -> Result<ReclaimOutcome, MediumError> {
    // The defensive invariant (the provider-execution review): a
    // retiring-record identity that ALSO appears in the survivor set
    // is an internal inconsistency — the authenticated exclusion
    // never reaches the audited records, so an overlap means the
    // inputs disagree. Refuse before deletion; never silently
    // override reachability.
    for unit in units {
        if survivors.contains(&unit.record) {
            return Err(MediumError::Chain {
                rule: "reclaim-partition/record-survivor-overlap",
            });
        }
    }
    let mut mandatory: Vec<Hash> = Vec::new();
    let mut deferred: Vec<Hash> = Vec::new();
    let mut seen: std::collections::BTreeSet<Hash> = std::collections::BTreeSet::new();
    for unit in units {
        if seen.insert(unit.record) {
            mandatory.push(unit.record);
        }
        for object in &unit.objects {
            if survivors.contains(object) {
                if seen.insert(*object) {
                    deferred.push(*object);
                }
            } else if seen.insert(*object) {
                mandatory.push(*object);
            }
        }
    }
    Ok(ReclaimOutcome {
        removed: 0,
        mandatory,
        deferred,
    })
}

/// The E2 retirement-order verification: the persisted unit
/// sequence must be strictly DESCENDING by indexed epoch (the
/// newest-first retirement order — the crossing's Phase A
/// collection order). The provider never trusts the caller's
/// construction: an oldest-first or unsorted ledger refuses before
/// any mutation (the interrupted-prefix recovery property depends
/// on the order).
pub fn verify_newest_first_order(
    units: &[ReclaimUnit],
    metadata: &crate::checkpoint::CheckpointMetadata,
) -> Result<(), MediumError> {
    let mut previous: Option<u64> = None;
    for unit in units {
        let Some(entry) = metadata.index.iter().find(|e| e.hash == unit.record) else {
            return Err(MediumError::Chain {
                rule: "reclaim/unit-not-indexed",
            });
        };
        if let Some(previous_epoch) = previous
            && entry.epoch >= previous_epoch
        {
            return Err(MediumError::Chain {
                rule: "reclaim/unit-order-not-newest-first",
            });
        }
        previous = Some(entry.epoch);
    }
    Ok(())
}

/// The E1 founding-derived seed verification: the checkpoint's
/// stored seed must equal `compute_digest_seed(founding)` — the
/// campaign's permanent founding identity (never reclaimed; the
/// check is computable on BOTH the first execution and the resume).
/// A fabricated seed value refuses.
pub fn verify_founding_seed(
    metadata: &crate::checkpoint::CheckpointMetadata,
    founding: &Hash,
) -> Result<(), MediumError> {
    let expected = crate::checkpoint::compute_digest_seed(founding);
    if metadata.seed != expected {
        return Err(MediumError::Chain {
            rule: "reclaim/seed-not-founding-derived",
        });
    }
    Ok(())
}

/// The founding revision's identity: the gap-aware descent from the
/// authoritative HEAD to the parentless record (the completion
/// verification's seed derivation — the founding is permanent, so
/// this is always computable).
pub(super) fn founding_of(medium: &mut dyn CanonicalMedium) -> Result<Hash, MediumError> {
    const MAX_DESCENT: u64 = 1 << 20;
    let mut current = medium
        .head()?
        .ok_or(MediumError::Chain { rule: "unfounded" })?;
    let mut skip_table: Option<crate::checkpoint::CheckpointMetadata> = None;
    for _ in 0..MAX_DESCENT {
        let record = medium.read_record(&current)?;
        if let Some(object) = record.content_refs.first().copied() {
            let bytes = medium.read_object(&object)?;
            if StoredObject::new(bytes.clone())?.domain()
                == crate::checkpoint::CHECKPOINT_METADATA_DOMAIN
            {
                skip_table = Some(
                    crate::checkpoint::CheckpointMetadata::decode(&bytes)
                        .map_err(MediumError::Format)?,
                );
            }
        }
        let Some(parent) = record.parent else {
            return Ok(current);
        };
        match medium.read_record(&parent) {
            Ok(_) => current = parent,
            Err(MediumError::MissingObject { .. }) => {
                let Some(table) = skip_table.as_ref() else {
                    return Err(MediumError::MissingObject { hash: parent });
                };
                let target = if table.sk == parent {
                    table
                        .self_skip()
                        .map(|entry| entry.nearest_retained_ancestor)
                } else {
                    table
                        .skip_for(&current)
                        .map(|entry| entry.nearest_retained_ancestor)
                };
                let Some(target) = target else {
                    return Err(MediumError::MissingObject { hash: parent });
                };
                current = target;
            }
            Err(other) => return Err(other),
        }
    }
    Err(MediumError::Chain {
        rule: "reclamation-complete/descent-bound",
    })
}

/// The trait-level survivor object set: the gap-aware cold walk
/// from the authoritative HEAD to the founding, EXCLUDING the
/// retiring records — the partition input for the completion
/// verification. The closure is PARITY-EXACT with the providers'
/// authenticated exclusion walks (P2-R3): every walked record's
/// identity, every content object it references, AND the VERIFIED
/// transitive composite closure — a surviving /2 snapshot root's
/// chunks are reachable through its manifest (each dependency
/// hash-verified and envelope-verified against its manifest entry
/// before it joins the set; a missing, corrupt, or malformed chunk
/// fails the walk closed).
pub fn survivor_objects(
    medium: &mut dyn CanonicalMedium,
    exclusion: &std::collections::BTreeSet<Hash>,
) -> Result<std::collections::BTreeSet<Hash>, MediumError> {
    const MAX_DESCENT: u64 = 1 << 20;
    let mut current = medium
        .head()?
        .ok_or(MediumError::Chain { rule: "unfounded" })?;
    let mut reachable = std::collections::BTreeSet::new();
    let mut skip_table: Option<crate::checkpoint::CheckpointMetadata> = None;
    for _ in 0..MAX_DESCENT {
        let record = medium.read_record(&current)?;
        if !exclusion.contains(&current) {
            reachable.insert(current);
            for reference in &record.content_refs {
                let bytes = medium.read_object(reference)?;
                reachable.insert(*reference);
                // The composite-closure expansion: a record whose
                // first reference is a /2 snapshot manifest carries
                // its chunks transitively — the same verified
                // semantics as the providers' walks.
                if reference == record.content_refs.first().unwrap()
                    && StoredObject::new(bytes.clone())?.domain()
                        == crate::composite_manifest::SESSION_SNAPSHOT_V2_DOMAIN
                {
                    let manifest =
                        crate::composite_manifest::decode(&bytes).map_err(MediumError::Format)?;
                    for entry in &manifest.entries {
                        let chunk_bytes = medium.read_object(&entry.identity)?;
                        crate::composite_append::verify_chunk_dependency(
                            &chunk_bytes,
                            &entry.identity,
                            &manifest,
                        )
                        .map_err(|_error| MediumError::Chain {
                            rule: "composite-closure-verification",
                        })?;
                        reachable.insert(entry.identity);
                    }
                }
            }
            if let Some(object) = record.content_refs.first().copied() {
                let bytes = medium.read_object(&object)?;
                if StoredObject::new(bytes.clone())?.domain()
                    == crate::checkpoint::CHECKPOINT_METADATA_DOMAIN
                {
                    skip_table = Some(
                        crate::checkpoint::CheckpointMetadata::decode(&bytes)
                            .map_err(MediumError::Format)?,
                    );
                }
            }
        }
        let Some(parent) = record.parent else {
            return Ok(reachable);
        };
        match medium.read_record(&parent) {
            Ok(_) => current = parent,
            Err(MediumError::MissingObject { .. }) => {
                let Some(table) = skip_table.as_ref() else {
                    return Err(MediumError::MissingObject { hash: parent });
                };
                let target = if table.sk == parent {
                    table
                        .self_skip()
                        .map(|entry| entry.nearest_retained_ancestor)
                } else {
                    table
                        .skip_for(&current)
                        .map(|entry| entry.nearest_retained_ancestor)
                };
                let Some(target) = target else {
                    return Err(MediumError::MissingObject { hash: parent });
                };
                current = target;
            }
            Err(other) => return Err(other),
        }
    }
    Err(MediumError::Chain {
        rule: "reclamation-complete/descent-bound",
    })
}

/// The trusted completion-receipt path: verifies the recovery-safe
/// audit authority and the ACTUAL postconditions, constructs the
/// marker FROM the verified state, and installs it through the
/// CAS-bound admission with the full reconciliation (the stale
/// parent revalidated against the fresh HEAD; the Unknown outcome
/// reconciled; the duplicate deterministic key resolved by
/// discovering and EXACTLY verifying the installed marker — never
/// inferring success from the key alone).
///
/// (P2-R4) The completion-publication coordination: the marker's
/// LIVE certified claims — every retiring record absent, every
/// mandatory identity absent — are STABLE under every legal
/// concurrent storage operation (the only mutations are deletions
/// by authenticated reclamation; a deletion never falsifies an
/// absence claim), so the multi-read verification sequence plus the
/// CAS append cannot publish a marker whose live claims are false
/// at installation, even when another maintenance operation runs
/// without HEAD advancement. The deferred list is the batch's
/// AUTHORIZATION record (what this audit classified as shared and
/// authorized to preserve): its admission-time presence check
/// either passes on a consistent observation or fails closed
/// (retryable — the retry recomputes against the then-current
/// state); a concurrent later authorized retirement that consumes a
/// deferred object does not falsify the authorization, and the
/// consumer's later-evidence relaxation recognizes exactly that
/// case.
pub fn complete_reclamation(
    medium: &mut dyn CanonicalMedium,
    source: &Hash,
    mk: &Hash,
) -> Result<(Hash, ReclamationComplete), MediumError> {
    let (ledger, _metadata) = verify_installed_v1_audit(medium, source, mk)?;
    let (mandatory_count, deferred) = verify_completion_state(medium, &ledger)?;
    // The provider-internal residue (the quarantine forms) — the
    // mandatory-empty postcondition across every authenticated
    // storage form.
    let directive = ReclaimDirective {
        audit_record: *source,
        units: ledger.units.clone(),
    };
    if medium.reclaim_pending(&directive)? {
        return Err(MediumError::Chain {
            rule: "reclamation-complete/mandatory-residue",
        });
    }
    let marker = ReclamationComplete {
        kind: CompletionKind::Batch,
        mk: *mk,
        batch: 0,
        source: *source,
        mandatory_count,
        deferred: deferred.into_iter().collect(),
        journal_files: Vec::new(),
    };
    for attempt in 0..2 {
        let head = medium
            .head()?
            .ok_or(MediumError::Chain { rule: "unfounded" })?;
        let tick = medium.read_record(&head)?.tick.saturating_add(1);
        let outcome = publish_reclamation_complete(medium, &marker, head, tick);
        match outcome {
            Ok((id, crate::medium::AppendOutcome::Accepted { .. })) => return Ok((id, marker)),
            Ok((_, crate::medium::AppendOutcome::RejectedStaleParent { .. })) => {
                if attempt == 1 {
                    return Err(MediumError::Chain {
                        rule: "reclamation-complete/stale-unresolved",
                    });
                }
                // The fresh revalidation: the next iteration re-reads
                // the authoritative HEAD and the admission re-verifies
                // the entire postcondition set against it.
                continue;
            }
            Ok((candidate, crate::medium::AppendOutcome::Unknown { .. })) => {
                match medium.reconcile_unknown(&candidate)? {
                    crate::medium::Reconciliation::Installed { id } => return Ok((id, marker)),
                    crate::medium::Reconciliation::NotInstalled { .. } => {
                        if attempt == 1 {
                            return Err(MediumError::Chain {
                                rule: "reclamation-complete/unknown-unresolved",
                            });
                        }
                        continue;
                    }
                    crate::medium::Reconciliation::Unverifiable { .. } => {
                        return Err(MediumError::Chain {
                            rule: "reclamation-complete/unknown-unverifiable",
                        });
                    }
                }
            }
            Err(MediumError::Chain {
                rule: "duplicate-idempotency-key",
            }) => {
                // The deterministic key is taken: discover and
                // verify the installed marker's EXACT content — the
                // certified obligations must match the verified
                // state, never the key's mere existence.
                let Some((id, installed)) = find_installed_completion(medium, mk)? else {
                    return Err(MediumError::Chain {
                        rule: "reclamation-complete/duplicate-unresolved",
                    });
                };
                if installed != marker {
                    return Err(MediumError::Chain {
                        rule: "reclamation-complete/duplicate-content-mismatch",
                    });
                }
                return Ok((id, installed));
            }
            Err(other) => return Err(other),
        }
    }
    unreachable!("the retry loop returns on every path")
}

/// (OC-C4) The completion-reader begin authority: a completed marker
/// never becomes authoritative without an authenticated, correctly
/// bound, provider-durable ReclamationBegin. The verification is
/// provider-issued (never a caller-supplied value — the provider
/// internally re-derives the ledger and discovers the begin under
/// its own coordination), and the eligible durability grade is
/// required on the returned authority. This strengthens the
/// verification side deliberately left at the marker-only strength
/// during OC-C3 (the R2-F6 dissolution superseded by the owner's
/// OC-C4 policy): no grandfathering — a legacy beginless marker
/// refuses trusted certification while remaining discoverable for
/// non-authoritative reporting through the discovery arm (which
/// carries no completion authority).
pub(super) fn verify_completion_begin_authority(
    medium: &mut dyn CanonicalMedium,
    source: &Hash,
    mk: &Hash,
) -> Result<(), MediumError> {
    let authority = medium.verify_begin_authority(source, mk)?;
    if authority.tier != crate::medium::SyncTier::FullPosixSequence
        && authority.tier != crate::medium::SyncTier::TransactionalWalFull
    {
        return Err(MediumError::Chain {
            rule: "reclamation-complete/begin-tier-insufficient",
        });
    }
    Ok(())
}
