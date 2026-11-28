//! The reclamation publication surface (split from the reclaim
//! monolith by change reason, P3-L9 pure move): the ledger, plan,
//! begin, journal-root, and completion publications plus the
//! campaign bound-journal creation. These change when the
//! MK-bound publication discipline changes.

use super::discovery::find_on_chain;
/// Appends the audit revision: a regular revision whose single
/// content object is the ledger, keyed deterministically by the
/// reclaimed MK, parented on the current head. Returns the candidate
/// (for the caller's Unknown reconciliation) and the outcome. A
/// duplicate key means the audit is already installed — the caller's
/// restart path.
use super::*;
pub fn publish_reclamation_ledger(
    medium: &mut dyn CanonicalMedium,
    ledger: &ReclamationLedger,
    tick: u64,
) -> Result<(Hash, AppendOutcome), MediumError> {
    if ledger.records == 0 {
        return Err(MediumError::Chain {
            rule: "reclamation-ledger/empty",
        });
    }
    // (V2-W, W2-A direction 2) The plan-existence precondition,
    // checked under the append boundary against the CAS-anchored
    // head: an MK with an installed trusted v2 plan never also
    // carries a trusted v1 audit — one retirement authority per
    // checkpoint, ever. The caller receives the typed exclusion;
    // the wire format and the legitimate v1 publication behavior
    // are otherwise unchanged (the pre-authorized narrow migration:
    // the publication rides the in-crate admitted receipt, since
    // the generic append now refuses the ledger family).
    let head = medium
        .head()?
        .ok_or(MediumError::Chain { rule: "unfounded" })?;
    if find_on_chain(
        medium,
        Some(&reclamation_plan_idempotency_key(&ledger.mk)),
        None,
    )?
    .is_some()
    {
        return Err(MediumError::Chain {
            rule: "reclamation-ledger/v2-plan-exclusion",
        });
    }
    let bytes = ledger.canonical_bytes();
    let object = StoredObject::new(bytes)?;
    let parent_epoch = medium.read_record(&head)?.epoch;
    // (F-14) The construction-boundary guard: a provider-obtained
    // epoch at u64::MAX has no representable successor — the typed
    // refusal precedes the constructor (the medium's append-side
    // checked linkage is the second layer).
    parent_epoch.checked_add(1).ok_or(MediumError::Chain {
        rule: "epoch-regression",
    })?;
    let record = RevisionRecord::regular(head, parent_epoch, tick)
        .with_idempotency_key(reclamation_ledger_idempotency_key(&ledger.mk))
        .with_content_refs(vec![object.hash()]);
    let candidate = record.id();
    let admitted = crate::medium::AdmittedAppend::generic(&record, std::slice::from_ref(&object));
    let outcome = medium.append_admitted(admitted, Some(head))?;
    Ok((candidate, outcome))
}

// --------------------------------------- the K2-C2B ratified wire formats
//
// The owner-ratified closure-expanded reclamation protocol's three
// persisted objects (P1 of the implementation ladder: the canonical
// codecs ONLY — no writer, no execution path, no crossing arm; a
// production chain cannot yet carry any of these domains, so this
// entire section is unreachable from any existing production
// behavior). The ratified semantics these codecs implement:
// per-unit provenance with the sharing rules (a duplicate unit
// RECORD is invalid; a repeated OBJECT identity ACROSS units is
// legitimate sharing; a duplicate object WITHIN one unit is
// invalid), the plan's complete-membership partition in the
// deterministic epoch order, and the two-level completion evidence
// (batch markers certify only their own storage obligations; the
// range-terminal marker certifies the whole retirement including
// the journal obligations).

/// The TEST-ONLY begin-record install (the OC-C1 reader-support
/// regressions): publishes a caller-constructed begin record through
/// the in-crate admitted append, BYPASSING the future admission's
/// verification — exactly the corrupted-in-crate-path threat the
/// OC-C3/OC-C4 authority checks must catch. COMPILE-TIME ISOLATED:
/// exists only under the composite-test-fixture feature (never in a
/// production build; the CI's feature-disabled artifact check
/// enforces the absence).
#[cfg(feature = "composite-test-fixture")]
#[doc(hidden)]
pub fn test_only_install_begin_record(
    medium: &mut dyn CanonicalMedium,
    begin: &ReclamationBegin,
    tick: u64,
) -> Result<Hash, MediumError> {
    let head = medium
        .head()?
        .ok_or(MediumError::Chain { rule: "unfounded" })?;
    let parent_epoch = medium.read_record(&head)?.epoch;
    let key = reclamation_begin_idempotency_key(&begin.mk);
    let object = StoredObject::new(begin.canonical_bytes())?;
    let record = RevisionRecord::regular(head, parent_epoch, tick)
        .with_idempotency_key(key)
        .with_content_refs(vec![object.hash()]);
    let admitted = crate::medium::AdmittedAppend::generic(&record, std::slice::from_ref(&object));
    match medium.append_admitted(admitted, Some(head))? {
        crate::medium::AppendOutcome::Accepted { .. } => Ok(record.id()),
        crate::medium::AppendOutcome::RejectedStaleParent { .. } => Err(MediumError::Chain {
            rule: "reclamation-begin/test-install-stale-parent",
        }),
        crate::medium::AppendOutcome::Unknown { .. } => Err(MediumError::Chain {
            rule: "reclamation-begin/test-install-unknown",
        }),
    }
}

/// (V2-W F4) The trusted journal-root binding admission: publishes
/// the canonical binding for a freshly bootstrapped campaign. The
/// founding record is AUTHENTICATED first (the canonical chain
/// membership — it IS the parentless root — never a caller-asserted
/// identity); the deterministic key makes the record write-once per
/// campaign (the chain's duplicate-idempotency-key rule); an
/// already-bound campaign REFUSES any differing root ID or location
/// (never a rebind); an identical installed binding verifies as the
/// exact duplicate; the publication rides the exact-parent CAS with
/// fresh re-validation on the stale retry and the Unknown outcome
/// reconciled against the verified identity, key, and exact
/// canonical content. (F4-R6) THE TRUSTED-BOOTSTRAP RESTRICTION: a
/// FIRST binding is publishable only in the fresh-bootstrap shape —
/// the chain's head is the founding itself (the bootstrap's own
/// intermediate state: a truly fresh campaign, or the crash between
/// the founding append and the binding publication, converging on
/// re-entry). An existing campaign with history — a legacy lineage,
/// or any chain that progressed past the founding — refuses typed:
/// an ordinary external caller cannot directly establish an
/// arbitrary first binding over an established campaign's fate; the
/// identity and location derive from the verified bootstrap
/// authority (the crossing's `open_bound_bootstrap`). A call on an
/// already-bound campaign is never a first binding (the occupied
/// key routes it into the duplicate/conflict discipline below, and
/// no second binding with the same key can ever append).
pub fn publish_journal_root_binding(
    medium: &mut dyn CanonicalMedium,
    founding: &Hash,
    root_id: &Hash,
    location: &str,
    tick: u64,
) -> Result<(Hash, JournalRootBindingOutcome), MediumError> {
    if *founding == Hash::ZERO || *root_id == Hash::ZERO || location.is_empty() {
        return Err(MediumError::Chain {
            rule: "journal-root/binding-invalid",
        });
    }
    // (F4-R13) THE CANONICAL LOCATION DESCRIPTOR: the supplied
    // location must BE the exact normalized absolute form required
    // by the accepted contract — a relative, malformed, or
    // noncanonical descriptor (anything that is not a fixed point
    // of the approved normalization rule) refuses BEFORE any
    // publication work, so the permanent key is never installed
    // over a descriptor the verifications could not round-trip. The
    // root ID's derivation check downstream then verifies the ID
    // against THIS canonical representation.
    let canonical_location = JournalRootBinding::normalize_location(std::path::Path::new(location));
    if location != canonical_location.as_str() {
        return Err(MediumError::Chain {
            rule: "journal-root/location-not-canonical",
        });
    }

    // The founding authentication: the canonical chain membership —
    // the parentless root record itself (a caller-supplied founding
    // hash that is not the chain's root refuses). The membership
    // proof is the ROOT-IDENTITY comparison (the gap-aware descent
    // to the chain's parentless record): the production founding —
    // the crossing's fresh append — carries NO content objects, so
    // an object-based lookup cannot distinguish the found
    // contentless founding from an absent one.
    let founding_record = medium.read_record(founding)?;
    if founding_record.parent.is_some() {
        return Err(MediumError::Chain {
            rule: "journal-root/founding-not-root",
        });
    }
    if founding_identity_medium(medium)? != *founding {
        return Err(MediumError::Chain {
            rule: "journal-root/founding-not-installed",
        });
    }
    let binding = JournalRootBinding {
        founding: *founding,
        root_id: *root_id,
        location: location.to_owned(),
    };
    let object = StoredObject::new(binding.canonical_bytes())?;
    let key = journal_root_binding_idempotency_key(founding);
    for attempt in 0..2 {
        let head = medium
            .head()?
            .ok_or(MediumError::Chain { rule: "unfounded" })?;
        // The already-bound check (fresh at THIS head): an identical
        // installed binding verifies as the exact duplicate; ANY
        // differing root ID or location refuses — never a rebind.
        if let Some((installed_id, installed_bytes)) = find_on_chain(medium, Some(&key), None)? {
            if installed_bytes == object.bytes() {
                return Ok((
                    installed_id,
                    JournalRootBindingOutcome::DuplicateVerified { id: installed_id },
                ));
            }
            return Err(MediumError::Chain {
                rule: "journal-root/binding-conflict",
            });
        }
        // (F4-R6) THE FIRST-BINDING FRESH-SHAPE GATE: execution
        // reaches this point only when NO binding occupies the key —
        // this append would be the campaign's FIRST binding, and it
        // is admissible only in the fresh-bootstrap shape: the
        // chain's head is the founding itself (a truly fresh
        // campaign, or the crash-between founding-and-binding state
        // converging on re-entry — the two are indistinguishable and
        // both are the bootstrap's own intermediate state). A chain
        // that progressed past the founding — a legacy lineage with
        // real history — refuses: the direct first-binding
        // publication over an established campaign is exactly what
        // the trusted bootstrap exists to prevent.
        if head != *founding {
            return Err(MediumError::Chain {
                rule: "journal-root/not-fresh-bootstrap",
            });
        }
        // (F4-R8) THE FIRST-BINDING BOOTSTRAP-EVIDENCE VERIFICATION:
        // an arbitrary caller holding a founding hash, a root ID,
        // and a path can never establish the first binding — the
        // append demands (a) the root ID's INDEPENDENT derivation
        // from the founding and the normalized location (a
        // caller-supplied root ID refuses), and (b) the DURABLE
        // JOURNAL-BOOTSTRAP EVIDENCE: the bound journal file
        // existing at the claimed location and carrying the EXACT
        // expected identities (a complete, matching bound header —
        // the trusted bootstrap's journal-first ordering always
        // leaves this residue; a missing, torn, legacy, or foreign
        // journal at the location refuses). Bound campaigns never
        // reach this point (the occupied key returned into the
        // duplicate/conflict discipline above), so the
        // duplicate/conflict refusals keep their delivered shape.
        if *root_id != journal_root_id(founding, location) {
            return Err(MediumError::Chain {
                rule: "journal-root/root-id-derivation",
            });
        }
        let bootstrap_journal = std::path::Path::new(location);
        // Read only the bound header prefix; the full journal is not needed.
        let bound_header_len = crate::journal::bound_header_bytes(founding, root_id).len();
        let bootstrap_bytes = crate::journal::read_prefix(bootstrap_journal, bound_header_len)
            .map_err(|_error| MediumError::Chain {
                rule: "journal-root/bootstrap-evidence-missing",
            })?;
        match crate::journal::read_bound_header(&bootstrap_bytes, founding, root_id) {
            Ok(crate::journal::BoundHeaderState::Matching { .. }) => {}
            _ => {
                return Err(MediumError::Chain {
                    rule: "journal-root/bootstrap-evidence-invalid",
                });
            }
        }
        let parent_epoch = medium.read_record(&head)?.epoch;
        // (F-14) The construction-boundary guard: a provider-obtained
        // epoch at u64::MAX has no representable successor — the typed
        // refusal precedes the constructor (the medium's append-side
        // checked linkage is the second layer).
        parent_epoch.checked_add(1).ok_or(MediumError::Chain {
            rule: "epoch-regression",
        })?;
        let record = RevisionRecord::regular(head, parent_epoch, tick)
            .with_idempotency_key(key.clone())
            .with_content_refs(vec![object.hash()]);
        let candidate = record.id();
        let admitted =
            crate::medium::AdmittedAppend::generic(&record, std::slice::from_ref(&object));
        match medium.append_admitted(admitted, Some(head))? {
            AppendOutcome::Accepted { id, .. } => {
                return Ok((id, JournalRootBindingOutcome::Accepted { id }));
            }
            AppendOutcome::RejectedStaleParent { .. } => {
                if attempt == 1 {
                    return Err(MediumError::Chain {
                        rule: "journal-root/stale-unresolved",
                    });
                }
                continue;
            }
            AppendOutcome::Unknown { .. } => match medium.reconcile_unknown(&candidate)? {
                crate::medium::Reconciliation::Installed { id } => {
                    let installed = medium.read_record(&id)?;
                    if installed.idempotency_key.as_deref() != Some(key.as_slice()) {
                        return Err(MediumError::Chain {
                            rule: "journal-root/unknown-key-mismatch",
                        });
                    }
                    let installed_object = medium.read_object(&installed.content_refs[0])?;
                    if installed_object != object.bytes() {
                        return Err(MediumError::Chain {
                            rule: "journal-root/unknown-content-mismatch",
                        });
                    }
                    return Ok((id, JournalRootBindingOutcome::ReconciledInstalled { id }));
                }
                crate::medium::Reconciliation::NotInstalled { .. } => {
                    if attempt == 1 {
                        return Err(MediumError::Chain {
                            rule: "journal-root/unknown-unresolved",
                        });
                    }
                    continue;
                }
                crate::medium::Reconciliation::Unverifiable { .. } => {
                    return Err(MediumError::Chain {
                        rule: "journal-root/unknown-unverifiable",
                    });
                }
            },
        }
    }
    unreachable!("the retry loop returns on every path")
}

// ------------------------------------------ the V2-W trusted publications
//
// The provider-neutral, NON-DESTRUCTIVE publication of the v2
// reclamation authority evidence: the canonical ReclaimPlan (the
// all-audits-first protocol's authority record) and every
// plan-defined ReclamationLedgerV2 batch audit. This checkpoint
// publishes durable evidence ONLY — no batch deletion, no v2
// completion marker, no range-terminal certification, no journal
// deletion, no crossing routing (the later batched-execution
// checkpoint independently enforces the all-audits-first
// prerequisite). The W2 bidirectional v1/v2 exclusion (one MK can
// never carry both a trusted v1 audit and a trusted v2 plan) is
// enforced at every reachable installation path: the generic-append
// family gates (no ledger or plan object enters unrestricted), the
// v1 publication's plan-existence precondition, and the plan
// admission's v1-audit exclusion — each anchored to the exact
// expected-parent CAS with fresh re-validation on every retry, and
// the Unknown outcome reconciled against the verified record
// identity, key, and exact canonical content (never a false
// acknowledgment).

/// (V2-W) The trusted plan publication: authenticates the MK-bound
/// checkpoint and its permanent metadata, derives the COMPLETE
/// membership partition from the checkpoint index in the ratified
/// epoch order (each batch's actual canonical v2 ledger encoding
/// respecting the admitted byte limit — a single unit that cannot
/// fit refuses BEFORE an unusable plan installs), binds the
/// journal obligations collected through the provenance-preserving
/// boundary (W2-B), enforces the W2 v1-audit exclusion, and
/// publishes through the deterministic MK-bound key with exact-
/// parent CAS, fresh exclusion re-validation on every retry, and
/// the Unknown outcome reconciled against the verified identity,
/// key, and exact canonical content. NON-DESTRUCTIVE: no deletion,
/// no begin, no completion marker, no journal retirement.
pub fn publish_reclamation_plan(
    medium: &mut dyn CanonicalMedium,
    mk: &Hash,
    ledger_byte_limit: u64,
    journal_root: &std::path::Path,
    tick: u64,
) -> Result<(Hash, PlanPublicationOutcome), MediumError> {
    // (V2-W F4, F4-R7) THE STANDALONE ROUTE — the offline/standalone
    // publication policy: the full campaign-bound verification, then
    // the root's exclusive sidecar lock EXPLICITLY HELD (the
    // publication permit issued from it — the serialization against
    // a concurrent rotation; possessing a bound root's path is never
    // itself authorization), then the authorized core. A LIVE
    // crossing holding the root's own lock (its active journal)
    // can hold this call until the acquisition deadline. The crossing
    // publishes using its existing lock.
    let authority = verify_bound_journal_authority(medium, journal_root)?;
    let guard =
        crate::journal::hold_publication_lock(journal_root).map_err(|error| match error {
            crate::journal::JournalError::Busy { .. } => MediumError::Contention {
                detail: error.to_string(),
            },
            crate::journal::JournalError::Io { op, path, source } => {
                MediumError::Io { op, path, source }
            }
            _ => MediumError::Chain {
                rule: "journal-root/lock",
            },
        })?;
    publish_reclamation_plan_authorized(
        medium,
        mk,
        ledger_byte_limit,
        &authority,
        &guard.permit(),
        journal_root,
        tick,
    )
}

/// (F4-R11/R11-A) The single narrowly controlled bound-journal
/// creation operation: the CURRENT canonical lifecycle verification
/// and the creation/repair happen INDIVISIBLY in one call — no
/// detachable, reusable, or stale authority value exists anywhere in
/// the API (the authority IS the call's own verified control flow).
/// The lifecycle is verified against the supplied campaign's OWN
/// medium AT THE MOMENT of creation: an empty chain (the fresh
/// bootstrap's pre-founding medium) or a chain whose root IS the
/// supplied founding with the binding lookup resolving to the
/// AUTHENTICATED absence (a failed founding discovery or binding
/// read/verification PROPAGATES — never confused with absence; an
/// installed binding REFUSES — the bound lifecycle's journal is
/// never recreated or repaired). The authorized root location and
/// root ID are derived from THIS call's founding and journal root —
/// bound to the operation, never caller-supplied. The missing-file
/// creation applies only at a root showing no prior journal life;
/// the torn pre-binding creation resolves; the existing complete
/// residue is adopted only when empty (F4-R12). The binding-
/// publication race cannot invalidate the creation proof: the
/// journal's durable initialization precedes the caller's binding
/// publication, and a concurrent publication converges through the
/// admission's own CAS/duplicate/conflict discipline.
pub fn create_campaign_bound_journal(
    medium: &mut dyn CanonicalMedium,
    journal_root: &std::path::Path,
    founding: &Hash,
) -> Result<crate::journal::T1Journal, BoundCreationError> {
    match medium.head()? {
        None => {}
        Some(_) => {
            let chain_root = founding_identity_medium(medium)?;
            if chain_root != *founding {
                return Err(MediumError::Chain {
                    rule: "journal-root/creation-authority-founding",
                }
                .into());
            }
            if find_installed_journal_root_binding(medium, founding)?.is_some() {
                return Err(MediumError::Chain {
                    rule: "journal-root/creation-authority-bound",
                }
                .into());
            }
        }
    }
    let location = JournalRootBinding::normalize_location(journal_root);
    let root_id = journal_root_id(founding, &location);
    crate::journal::T1Journal::open_bound_creating_at(journal_root, founding, &root_id)
        .map_err(BoundCreationError::Journal)
}

/// (F4-R2/R3/R7) THE AUTHORIZED CORE — the crossing-scoped
/// publication route: the identity-verified obligation collection
/// and the canonical plan publication UNDER THE CALLER'S PROVEN
/// serialization boundary. The caller presents the NONFORGEABLE,
/// lifetime-bound [`crate::journal::PublicationPermit`] — proof the
/// journal root's exclusive sidecar lock is held for the call (via a
/// live journal handle that IS the root — the unrotated crossing's
/// no-reacquisition path — or via the explicitly held publication
/// lock — the rotated crossing and the standalone route). The
/// permit's locked root must agree with this publication's journal
/// root: a permit for any other lock never authorizes it. All the
/// F1-F5, W2, and W3A gates, the duplicate-content verification,
/// and the installed-plan immutability are inside.
pub fn publish_reclamation_plan_authorized(
    medium: &mut dyn CanonicalMedium,
    mk: &Hash,
    ledger_byte_limit: u64,
    authority: &BoundJournalAuthority,
    permit: &crate::journal::PublicationPermit<'_>,
    journal_root: &std::path::Path,
    tick: u64,
) -> Result<(Hash, PlanPublicationOutcome), MediumError> {
    // (F4-R7) THE SERIALIZATION-POSSESSION PROOF: the permit's held
    // lock must be THIS publication's root's lock — the normalized
    // path agreement. Without a permit the core is unreachable
    // (the type is nonconstructible outside this crate); with a
    // permit for a different root the publication refuses before
    // any journal evidence is read. This is the boundary the
    // production rotation honors (the same root lock across its
    // flip), so a correctly permitted publication cannot race the
    // rotation interval.
    if JournalRootBinding::normalize_location(permit.locked_root())
        != JournalRootBinding::normalize_location(journal_root)
    {
        return Err(MediumError::Chain {
            rule: "reclamation-plan/permit-root-mismatch",
        });
    }
    let journal_files = collect_bound_reclaimable_journal_files(
        journal_root,
        &authority.founding,
        &authority.root_id,
    )?;
    // (a) The admitted byte limit: the caller-supplied operational
    // packing bound, validated against the unchanged canonical
    // ceiling (512 MiB) and the honest nonzero minimum.
    if ledger_byte_limit == 0 || ledger_byte_limit > MAX_RECLAMATION_LEDGER_BYTES {
        return Err(MediumError::Chain {
            rule: "reclamation-plan/ledger-byte-limit-invalid",
        });
    }
    // (b) The W3A provider capacity: the effective per-object limit
    // must meet the supported floor (the typed
    // unsupported-configuration refusal — reads/writes otherwise
    // unaffected).
    let effective_limit = medium.effective_object_byte_limit()?;
    if effective_limit < MIN_SUPPORTED_EFFECTIVE_OBJECT_BYTES {
        return Err(MediumError::Chain {
            rule: "reclamation-plan/provider-limit-unsupported",
        });
    }
    // (c) The checkpoint authentication: the MK's canonical chain
    // membership (the descent match), its permanent metadata, and
    // the SK binding.
    let Some((_mk_id, meta_bytes)) = find_on_chain(medium, None, Some(mk))? else {
        return Err(MediumError::Chain {
            rule: "reclamation-plan/mk-not-installed",
        });
    };
    let metadata =
        crate::checkpoint::CheckpointMetadata::decode(&meta_bytes).map_err(MediumError::Format)?;
    let mk_record = medium.read_record(mk)?;
    if Some(metadata.sk) != mk_record.parent {
        return Err(MediumError::Chain {
            rule: "reclamation-plan/metadata-binding",
        });
    }
    if metadata.index.is_empty() {
        return Err(MediumError::Chain {
            rule: "reclamation-plan/empty-index",
        });
    }
    // (V2W-F2) THE COMPLETE FIRST-EXECUTION EVIDENCE — the SAME
    // accepted trusted-begin contract, not a second or weaker
    // definition: the authoritative founding identity (the seed's
    // authority), the v1-shaped full-membership ledger over the
    // index, and the OC-C2 verifier (`verify_first_execution_evidence`)
    // proving — before any plan installs — that every indexed range
    // record exists and is hash-valid with its manifest exactly
    // equal to its actual content references, every content object
    // present and hash-verified (the composite closure
    // envelope-verified), the units exactly match the index, the
    // newest-first order holds, the founding-derived seed agrees,
    // and the INDEPENDENT range-digest recomputation over the
    // original canonical record bytes equals the checkpoint's
    // stored commitment. A fabricated, incomplete, or contradictory
    // checkpoint never becomes trusted publication authority.
    let founding = founding_identity_medium(medium)?;
    let mut evidence_units: Vec<ReclaimUnit> = Vec::new();
    let mut evidence_keys = 0u64;
    let mut indexed_members: Vec<&crate::checkpoint::ReclaimedIndexEntry> =
        metadata.index.iter().collect();
    indexed_members.sort_unstable_by_key(|entry| std::cmp::Reverse(entry.epoch));
    for entry in &indexed_members {
        let record = medium.read_record(&entry.hash)?;
        if record.idempotency_key.is_some() {
            evidence_keys += 1;
        }
        evidence_units.push(ReclaimUnit {
            record: entry.hash,
            objects: record.content_refs.clone(),
        });
    }
    let evidence_ledger = ReclamationLedger {
        mk: *mk,
        sk: metadata.sk,
        range_digest: metadata.range_digest,
        seed: metadata.seed,
        records: evidence_units.len() as u64,
        reclaimed_keys: evidence_keys,
        journal_files: Vec::new(),
        units: evidence_units,
    };
    {
        let medium_cell = std::cell::RefCell::new(&mut *medium);
        let read_record = |id: &Hash| -> Result<Vec<u8>, MediumError> {
            medium_cell
                .borrow_mut()
                .read_record(id)
                .map(|r| r.canonical_bytes())
        };
        let read_object = |hash: &Hash| -> Result<Vec<u8>, MediumError> {
            medium_cell.borrow_mut().read_object(hash)
        };
        verify_first_execution_evidence(
            &evidence_ledger,
            &metadata,
            &founding,
            read_record,
            read_object,
        )?;
    }
    // (d) The COMPLETE membership partition in the ratified epoch
    // order (newest-first — batch 0 carries the newest slice),
    // computed from the actual canonical v2 ledger encodings: each
    // unit's record is read and hash-verified while present, its
    // closure-expanded manifest collected, and the accumulating
    // batch's encoded size checked against the admitted limit. A
    // single unit whose minimal ledger cannot fit refuses BEFORE
    // the plan installs.
    let mut indexed: Vec<&crate::checkpoint::ReclaimedIndexEntry> = metadata.index.iter().collect();
    indexed.sort_unstable_by_key(|entry| std::cmp::Reverse(entry.epoch));
    let mut batches: Vec<PlanBatch> = Vec::new();
    let mut current: PlanBatch = PlanBatch {
        index: 0,
        records: Vec::new(),
    };
    let mut current_units: Vec<ReclaimUnit> = Vec::new();
    for entry in &indexed {
        let objects = collect_unit_objects(medium, &entry.hash)?;
        let probe_units: Vec<ReclaimUnit> = current_units
            .iter()
            .cloned()
            .chain(std::iter::once(ReclaimUnit {
                record: entry.hash,
                objects: objects.clone(),
            }))
            .collect();
        let probe = ReclamationLedgerV2 {
            mk: *mk,
            sk: metadata.sk,
            batch: current.index,
            range_digest: metadata.range_digest,
            seed: metadata.seed,
            records: probe_units.len() as u64,
            reclaimed_keys: 0,
            units: probe_units,
        };
        if probe.canonical_bytes().len() as u64 > ledger_byte_limit {
            // (V2W-F1) The singleton check on BOTH sides of the
            // split: a unit whose own minimal ledger cannot fit the
            // admitted limit never enters ANY batch — before the
            // first batch (the empty-current case) AND after a split
            // (the newly opened batch), measured with the same
            // canonical-encoding byte accounting the audit writer
            // enforces. No plan installs with an unfulfillable batch.
            let singleton = ReclamationLedgerV2 {
                mk: *mk,
                sk: metadata.sk,
                batch: current.index,
                range_digest: metadata.range_digest,
                seed: metadata.seed,
                records: 1,
                reclaimed_keys: 0,
                units: vec![ReclaimUnit {
                    record: entry.hash,
                    objects: objects.clone(),
                }],
            };
            if singleton.canonical_bytes().len() as u64 > ledger_byte_limit {
                return Err(MediumError::Chain {
                    rule: "reclamation-plan/unit-exceeds-limit",
                });
            }
            // The singleton fits; close the current batch and open
            // the next (the consecutive-index discipline).
            let next_index = current.index + 1;
            batches.push(std::mem::replace(
                &mut current,
                PlanBatch {
                    index: next_index,
                    records: Vec::new(),
                },
            ));
            current_units.clear();
        }
        current_units.push(ReclaimUnit {
            record: entry.hash,
            objects,
        });
        current.records.push(entry.hash);
    }
    if !current.records.is_empty() {
        batches.push(current);
    }
    // (e) The journal obligations: collected ABOVE under the root's
    // exclusive lock, with every identity verified against the
    // installed canonical binding (the active and every stale
    // generation) — the complete authoritative obligation set,
    // immutable in the plan.
    // (f) The plan, constructed from the verified values only.
    let plan = ReclaimPlan {
        mk: *mk,
        sk: metadata.sk,
        range_digest: metadata.range_digest,
        seed: metadata.seed,
        ledger_byte_limit,
        batches,
        journal_files,
    };
    let object = StoredObject::new(plan.canonical_bytes())?;
    // (V2W-F5A) The plan object's OWN encoded size must fit the
    // provider's effective per-object capacity (the framed stored
    // bytes — the same byte accounting the audit writer enforces);
    // a plan whose partitioned audits fit their limits but whose
    // own object cannot fit the provider never installs.
    if object.bytes().len() as u64 > effective_limit {
        return Err(MediumError::Chain {
            rule: "reclamation-plan/provider-limit-exceeded",
        });
    }
    let key = reclamation_plan_idempotency_key(mk);
    let v1_key = reclamation_ledger_idempotency_key(mk);
    // (g) The publication: the duplicate resolution first (the
    // deterministic-key discovery with the EXACT content check — a
    // conflicting content refuses, never an equivalent duplicate),
    // then the W2 v1-audit exclusion anchored to the CAS head, the
    // exact-parent append, the fresh re-validation on the stale-
    // parent retry, and the Unknown reconciliation with the
    // verified identity, key, and content.
    for attempt in 0..2 {
        let head = medium
            .head()?
            .ok_or(MediumError::Chain { rule: "unfounded" })?;
        // The W2 v1-audit exclusion (fresh at THIS head — the CAS
        // anchors to the same one; a competing publication moves the
        // head and forces the fresh re-check on the retry).
        if find_on_chain(medium, Some(&v1_key), None)?.is_some() {
            return Err(MediumError::Chain {
                rule: "reclamation-plan/v1-audit-exclusion",
            });
        }
        // The duplicate resolution (an installed plan under the
        // deterministic key): the exact canonical content matches →
        // the idempotent DuplicateVerified; anything else refuses.
        if let Some((installed_id, installed_bytes)) = find_on_chain(medium, Some(&key), None)? {
            if installed_bytes == object.bytes() {
                return Ok((
                    installed_id,
                    PlanPublicationOutcome::DuplicateVerified { id: installed_id },
                ));
            }
            return Err(MediumError::Chain {
                rule: "reclamation-plan/duplicate-content-mismatch",
            });
        }
        let parent_epoch = medium.read_record(&head)?.epoch;
        // (F-14) The construction-boundary guard: a provider-obtained
        // epoch at u64::MAX has no representable successor — the typed
        // refusal precedes the constructor (the medium's append-side
        // checked linkage is the second layer).
        parent_epoch.checked_add(1).ok_or(MediumError::Chain {
            rule: "epoch-regression",
        })?;
        let record = RevisionRecord::regular(head, parent_epoch, tick)
            .with_idempotency_key(key.clone())
            .with_content_refs(vec![object.hash()]);
        let candidate = record.id();
        let admitted =
            crate::medium::AdmittedAppend::generic(&record, std::slice::from_ref(&object));
        match medium.append_admitted(admitted, Some(head))? {
            AppendOutcome::Accepted { id, .. } => {
                return Ok((id, PlanPublicationOutcome::Accepted { id }));
            }
            AppendOutcome::RejectedStaleParent { .. } => {
                if attempt == 1 {
                    return Err(MediumError::Chain {
                        rule: "reclamation-plan/stale-unresolved",
                    });
                }
                continue;
            }
            AppendOutcome::Unknown { .. } => match medium.reconcile_unknown(&candidate)? {
                crate::medium::Reconciliation::Installed { id } => {
                    let installed = medium.read_record(&id)?;
                    if installed.idempotency_key.as_deref() != Some(key.as_slice()) {
                        return Err(MediumError::Chain {
                            rule: "reclamation-plan/unknown-key-mismatch",
                        });
                    }
                    let installed_object = medium.read_object(&installed.content_refs[0])?;
                    if installed_object != object.bytes() {
                        return Err(MediumError::Chain {
                            rule: "reclamation-plan/unknown-content-mismatch",
                        });
                    }
                    return Ok((id, PlanPublicationOutcome::ReconciledInstalled { id }));
                }
                crate::medium::Reconciliation::NotInstalled { .. } => {
                    if attempt == 1 {
                        return Err(MediumError::Chain {
                            rule: "reclamation-plan/unknown-unresolved",
                        });
                    }
                    continue;
                }
                crate::medium::Reconciliation::Unverifiable { .. } => {
                    return Err(MediumError::Chain {
                        rule: "reclamation-plan/unknown-unverifiable",
                    });
                }
            },
        }
    }
    unreachable!("the retry loop returns on every path")
}

/// (V2-W) The trusted v2 batch-audit publication: discovers and
/// verifies the installed plan (the authoritative membership — the
/// caller names the MK and the batch index, never supplies content),
/// reads and hash-verifies every retiring record while the
/// first-execution evidence remains intact (the closure-expanded
/// manifests), independently recomputes the original range digest
/// against the canonical evidence (a fabricated commitment refuses),
/// checks the encoded audit against the plan's admitted limit and
/// the provider's effective capacity, and publishes through the
/// deterministic (MK, batch) key with the same duplicate, CAS,
/// stale-retry, and Unknown-reconciliation discipline as the plan.
/// NON-DESTRUCTIVE.
pub fn publish_reclamation_ledger_v2(
    medium: &mut dyn CanonicalMedium,
    mk: &Hash,
    batch: u64,
    tick: u64,
) -> Result<(Hash, LedgerV2PublicationOutcome), MediumError> {
    // (a) The installed plan: the authoritative membership.
    let Some((_plan_id, plan)) = find_installed_plan(medium, mk)? else {
        return Err(MediumError::Chain {
            rule: "reclamation-ledger-v2/plan-not-installed",
        });
    };
    // (V2W-F5B) The installed plan's IDENTITY BINDING: the record
    // was discovered by THIS MK's deterministic key, but the
    // decoded content must independently agree — a record with the
    // expected key but mismatched or malformed content (the
    // in-crate adversarial shape) never authorizes batch
    // publication.
    if plan.mk != *mk {
        return Err(MediumError::Chain {
            rule: "reclamation-ledger-v2/plan-key-mismatch",
        });
    }
    // (b) The requested batch exists in THIS exact plan.
    let Some(plan_batch) = plan.batches.iter().find(|b| b.index == batch) else {
        return Err(MediumError::Chain {
            rule: "reclamation-ledger-v2/batch-not-in-plan",
        });
    };
    // (c) The checkpoint metadata re-verification (the plan's
    // bindings against the permanent record).
    let Some((_mk_id, meta_bytes)) = find_on_chain(medium, None, Some(mk))? else {
        return Err(MediumError::Chain {
            rule: "reclamation-ledger-v2/mk-not-installed",
        });
    };
    let metadata =
        crate::checkpoint::CheckpointMetadata::decode(&meta_bytes).map_err(MediumError::Format)?;
    if plan.sk != metadata.sk
        || plan.range_digest != metadata.range_digest
        || plan.seed != metadata.seed
    {
        return Err(MediumError::Chain {
            rule: "reclamation-ledger-v2/plan-binding-mismatch",
        });
    }
    // (d) The batch members equal the plan's designated slice, each
    // retiring record read and hash-verified while present, the
    // closure-expanded manifest collected (the sharing rules: the
    // intra-unit identities deduplicated; cross-unit repetition is
    // legitimate).
    let mut units: Vec<ReclaimUnit> = Vec::new();
    let mut reclaimed_keys = 0u64;
    for record_id in &plan_batch.records {
        let record = medium.read_record(record_id)?;
        if record.idempotency_key.is_some() {
            reclaimed_keys += 1;
        }
        let objects = collect_unit_objects(medium, record_id)?;
        units.push(ReclaimUnit {
            record: *record_id,
            objects,
        });
    }
    // (e) The original range digest independently recomputed against
    // the canonical evidence (every digest member read and
    // hash-verified while present — the SK is an index member but
    // never a digest member, the established discipline).
    let mut digest_members: Vec<(u64, Hash)> = metadata
        .index
        .iter()
        .filter(|entry| entry.hash != metadata.sk)
        .map(|entry| (entry.epoch, entry.hash))
        .collect();
    digest_members.sort_unstable_by_key(|(epoch, _)| *epoch);
    let mut range_bytes = Vec::with_capacity(digest_members.len());
    for (_, record_id) in &digest_members {
        range_bytes.push(medium.read_record(record_id)?.canonical_bytes());
    }
    let recomputed = crate::checkpoint::chain_range_digest(&metadata.seed, &range_bytes);
    if recomputed != plan.range_digest {
        return Err(MediumError::Chain {
            rule: "reclamation-ledger-v2/range-digest-mismatch",
        });
    }
    // (f) The audit construction and the capacity checks: the
    // encoded audit must fit the plan's admitted byte limit AND the
    // provider's effective per-object capacity (including the
    // object framing — the stored bytes, not just the payload).
    let ledger = ReclamationLedgerV2 {
        mk: *mk,
        sk: plan.sk,
        batch,
        range_digest: plan.range_digest,
        seed: plan.seed,
        records: units.len() as u64,
        reclaimed_keys,
        units,
    };
    let object = StoredObject::new(ledger.canonical_bytes())?;
    if object.bytes().len() as u64 > plan.ledger_byte_limit {
        return Err(MediumError::Chain {
            rule: "reclamation-ledger-v2/ledger-exceeds-limit",
        });
    }
    let effective_limit = medium.effective_object_byte_limit()?;
    if effective_limit < MIN_SUPPORTED_EFFECTIVE_OBJECT_BYTES {
        return Err(MediumError::Chain {
            rule: "reclamation-plan/provider-limit-unsupported",
        });
    }
    if object.bytes().len() as u64 > effective_limit {
        return Err(MediumError::Chain {
            rule: "reclamation-ledger-v2/provider-limit-exceeded",
        });
    }
    // (g) The publication (the plan writer's discipline verbatim:
    // the duplicate resolution with the exact content check, the
    // CAS, the fresh retry, the Unknown reconciliation).
    let key = reclamation_ledger_v2_idempotency_key(mk, batch);
    for attempt in 0..2 {
        let head = medium
            .head()?
            .ok_or(MediumError::Chain { rule: "unfounded" })?;
        if let Some((installed_id, installed_bytes)) = find_on_chain(medium, Some(&key), None)? {
            if installed_bytes == object.bytes() {
                return Ok((
                    installed_id,
                    LedgerV2PublicationOutcome::DuplicateVerified { id: installed_id },
                ));
            }
            return Err(MediumError::Chain {
                rule: "reclamation-ledger-v2/duplicate-content-mismatch",
            });
        }
        let parent_epoch = medium.read_record(&head)?.epoch;
        // (F-14) The construction-boundary guard: a provider-obtained
        // epoch at u64::MAX has no representable successor — the typed
        // refusal precedes the constructor (the medium's append-side
        // checked linkage is the second layer).
        parent_epoch.checked_add(1).ok_or(MediumError::Chain {
            rule: "epoch-regression",
        })?;
        let record = RevisionRecord::regular(head, parent_epoch, tick)
            .with_idempotency_key(key.clone())
            .with_content_refs(vec![object.hash()]);
        let candidate = record.id();
        let admitted =
            crate::medium::AdmittedAppend::generic(&record, std::slice::from_ref(&object));
        match medium.append_admitted(admitted, Some(head))? {
            AppendOutcome::Accepted { id, .. } => {
                return Ok((id, LedgerV2PublicationOutcome::Accepted { id }));
            }
            AppendOutcome::RejectedStaleParent { .. } => {
                if attempt == 1 {
                    return Err(MediumError::Chain {
                        rule: "reclamation-ledger-v2/stale-unresolved",
                    });
                }
                continue;
            }
            AppendOutcome::Unknown { .. } => match medium.reconcile_unknown(&candidate)? {
                crate::medium::Reconciliation::Installed { id } => {
                    let installed = medium.read_record(&id)?;
                    if installed.idempotency_key.as_deref() != Some(key.as_slice()) {
                        return Err(MediumError::Chain {
                            rule: "reclamation-ledger-v2/unknown-key-mismatch",
                        });
                    }
                    let installed_object = medium.read_object(&installed.content_refs[0])?;
                    if installed_object != object.bytes() {
                        return Err(MediumError::Chain {
                            rule: "reclamation-ledger-v2/unknown-content-mismatch",
                        });
                    }
                    return Ok((id, LedgerV2PublicationOutcome::ReconciledInstalled { id }));
                }
                crate::medium::Reconciliation::NotInstalled { .. } => {
                    if attempt == 1 {
                        return Err(MediumError::Chain {
                            rule: "reclamation-ledger-v2/unknown-unresolved",
                        });
                    }
                    continue;
                }
                crate::medium::Reconciliation::Unverifiable { .. } => {
                    return Err(MediumError::Chain {
                        rule: "reclamation-ledger-v2/unknown-unverifiable",
                    });
                }
            },
        }
    }
    unreachable!("the retry loop returns on every path")
}

/// Publishes the completion marker revision: a regular revision
/// whose single content object is the marker, keyed
/// deterministically by the marker's KIND (the batch form by the MK
/// and batch index — P2-F3: the range-terminal form by the MK and
/// the `range` tag; the two namespaces never collide), and CAS-bound
/// to the EXACT expected parent (P2-F2: the HEAD under which the
/// completion obligations were verified — an advancement between
/// validation and publication surfaces `RejectedStaleParent`, never
/// a silently rebased marker; the caller rederives the completion
/// conditions against the new HEAD before retrying). Returns the
/// candidate (for the caller's Unknown reconciliation) and the
/// outcome — the same shape as the audit's own publication. A
/// duplicate-key result is never alone proof of completion: the
/// caller must discover and verify the existing marker's contents
/// and underlying obligations.
pub fn publish_reclamation_complete(
    medium: &mut dyn CanonicalMedium,
    marker: &ReclamationComplete,
    expected_parent: Hash,
    tick: u64,
) -> Result<(Hash, AppendOutcome), MediumError> {
    // THE ADMISSION (P2-F4): a completion marker may become trusted
    // retirement evidence ONLY through this boundary — the generic
    // append refuses the marker domain typed, and this admission
    // verifies the marker's semantic authorization BEFORE the
    // receipt: the source audit must be an installed, keyed,
    // MK-bound ledger revision. A syntactically valid marker with a
    // nonexistent, orphaned, mismatched, or unkeyed source refuses
    // typed with nothing mutated.
    if marker.kind == CompletionKind::Range {
        // The range-terminal protocol belongs to the separately
        // authorized later package — it cannot be used to claim
        // completion under the v1 protocol.
        return Err(MediumError::Chain {
            rule: "reclamation-complete/range-unauthorized",
        });
    }
    // P2-F4C: the v1-only execution boundary — the admitted source
    // is a v1 audit, the batch index is zero (the v1 single-batch
    // interpretation), and the v2 form is refused until the
    // separately authorized v2 protocol exists.
    if marker.batch != 0 {
        return Err(MediumError::Chain {
            rule: "reclamation-complete/batch-nonzero",
        });
    }
    // P2-F4B: the CANONICAL AUDIT AUTHORITY — not merely an
    // audit-shaped record: the source must be an installed v1 audit
    // ON THE AUTHORITATIVE CHAIN whose MK is a real checkpoint,
    // whose bindings agree with the permanent metadata, and whose
    // units exactly match the checkpoint's retirement index —
    // verified in the RECOVERY-SAFE form (the completed
    // retirement's records are absent by construction).
    let (ledger, _metadata) = verify_installed_v1_audit(medium, &marker.source, &marker.mk)?;
    // (OC-C3 CP-2) THE BEGIN-AUTHORIZED COMPLETION PUBLICATION:
    // every new terminal marker requires the previously
    // established, canonical, exactly bound, provider-durable
    // ReclamationBegin authority — verified through the provider's
    // CP-1 capability (never a caller-supplied value; canonical
    // membership alone is never proof of the adequate grade — the
    // local reaffirmation and the SQLite visibility proof run
    // inside the provider under its own coordination). This gate is
    // inside the independently callable publication boundary: the
    // direct call, the complete_reclamation wrapper, and every
    // crossing path inherit it; no route can publish a beginless
    // marker.
    medium.verify_begin_authority(&marker.source, &marker.mk)?;
    // P2-F4: the verified-completion RECEIPT — the marker's
    // certified obligations are checked against the ACTUAL
    // post-execution state, never the caller's claim: every
    // retiring record absent, the partition recomputed from the
    // surviving chain, every mandatory identity absent, every
    // deferred identity present and hash-verified.
    let (mandatory_count, deferred) = verify_completion_state(medium, &ledger)?;
    if marker.mandatory_count != mandatory_count {
        return Err(MediumError::Chain {
            rule: "reclamation-complete/mandatory-count-mismatch",
        });
    }
    if marker
        .deferred
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<Hash>>()
        != deferred
    {
        return Err(MediumError::Chain {
            rule: "reclamation-complete/deferred-mismatch",
        });
    }
    // The provider-internal residue (the quarantine forms): the
    // mandatory-empty postcondition across every authenticated
    // storage form — the advisory probe's observation, bound into
    // the admission.
    let directive = ReclaimDirective {
        audit_record: marker.source,
        units: ledger.units.clone(),
    };
    if medium.reclaim_pending(&directive)? {
        return Err(MediumError::Chain {
            rule: "reclamation-complete/mandatory-residue",
        });
    }
    // The CAS-bound publication: the receipt is constructed
    // IN-CRATE (the generic gate does not apply — this admission IS
    // the authority), the append's expected parent binding to the
    // exact verified HEAD.
    let bytes = marker.canonical_bytes();
    let object = StoredObject::new(bytes)?;
    let parent_epoch = medium.read_record(&expected_parent)?.epoch;
    // (F-14) The construction-boundary guard: a provider-obtained
    // epoch at u64::MAX has no representable successor — the typed
    // refusal precedes the constructor (the medium's append-side
    // checked linkage is the second layer).
    parent_epoch.checked_add(1).ok_or(MediumError::Chain {
        rule: "epoch-regression",
    })?;
    let key = reclamation_complete_idempotency_key(&marker.mk, Some(marker.batch));
    let record = RevisionRecord::regular(expected_parent, parent_epoch, tick)
        .with_idempotency_key(key)
        .with_content_refs(vec![object.hash()]);
    let candidate = record.id();
    let admitted = crate::medium::AdmittedAppend::generic(&record, &[object]);
    let outcome = medium.append_admitted(admitted, Some(expected_parent))?;
    Ok((candidate, outcome))
}

/// The TEST-ONLY adversarial completion-marker install (the R4-L2/
/// R4-L3 regressions): publishes a caller-constructed marker through
/// the in-crate admitted append, BYPASSING the admission's state
/// verification — exactly the corrupted-in-crate-path threat the
/// consumer's independent verification must catch. COMPILE-TIME
/// ISOLATED: exists only under the composite-test-fixture feature
/// (never in a production build).
#[cfg(feature = "composite-test-fixture")]
#[doc(hidden)]
pub fn test_only_install_completion_marker(
    medium: &mut dyn CanonicalMedium,
    marker: &ReclamationComplete,
    tick: u64,
) -> Result<Hash, MediumError> {
    let head = medium
        .head()?
        .ok_or(MediumError::Chain { rule: "unfounded" })?;
    let parent_epoch = medium.read_record(&head)?.epoch;
    let key = reclamation_complete_idempotency_key(&marker.mk, Some(marker.batch));
    let object = StoredObject::new(marker.canonical_bytes())?;
    let record = RevisionRecord::regular(head, parent_epoch, tick)
        .with_idempotency_key(key)
        .with_content_refs(vec![object.hash()]);
    let admitted = crate::medium::AdmittedAppend::generic(&record, std::slice::from_ref(&object));
    medium.append_admitted(admitted, Some(head))?;
    Ok(record.id())
}
