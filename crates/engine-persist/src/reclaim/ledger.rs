//! The reclamation record codecs and wire contracts (split from the
//! reclaim monolith by change reason, P3-L9 pure move): the ledger,
//! begin, plan, completion, and journal-root record types with their
//! domains, versions, idempotency keys, and admission gates. These
//! change when the durable reclamation formats change.

use super::*;

/// The reclamation-ledger object's versioned domain (the audit
/// revision's single content reference).
pub const RECLAMATION_LEDGER_DOMAIN: &str = "engine-persist/reclamation-ledger/1";
pub const RECLAMATION_LEDGER_VERSION: u32 = 1;

/// The audit revision's idempotency-key domain (deterministically
/// derived from the reclaimed MK — a retried audit over the same
/// checkpoint carries the same key, so the chain-wide uniqueness
/// makes the retry idempotent and the restart recognizes the
/// installed audit).
pub const RECLAMATION_LEDGER_KEY_DOMAIN: &str = "engine-persist/reclamation-ledger-key/1";

/// Documented maxima (bounded decode).
pub const MAX_RECLAMATION_JOURNAL_FILES: u64 = 1 << 16;
pub const MAX_RECLAMATION_JOURNAL_NAME: u64 = 1024;
pub const MAX_RECLAMATION_UNITS: u64 = 1 << 16;
pub const MAX_RECLAMATION_UNIT_OBJECTS: u64 = 1 << 16;

/// One audited deletion unit (the review corrective F4): the record
/// identity and its referenced content objects — the COMPLETE
/// manifest collected while the range bytes were still present
/// (Phase A's digest verification reads every range record). The
/// manifest makes an interrupted Phase B resumable: the re-run
/// deletes from the AUDIT, never a re-read of a possibly-absent
/// record, so a process death between a record deletion and its
/// object deletions converges on the retry instead of orphaning
/// content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReclaimUnit {
    /// The reclaimed revision's identity (its record object).
    pub record: Hash,
    /// The revision's referenced content objects (from its
    /// content_refs, read before any deletion).
    pub objects: Vec<Hash>,
}

/// The reclamation audit's bindings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReclamationLedger {
    /// The checkpoint whose range is reclaimed (MK — the permanent
    /// metadata carrying the exact identities, keys, epochs, skips,
    /// and digests; this audit binds to it, never duplicates it).
    pub mk: Hash,
    /// The retired snapshot revision (SK — MK's parent; SK is
    /// reclaimed together with its range, the whole-revision unit).
    pub sk: Hash,
    /// The pre-verified range digest — RECOMPUTED against the
    /// still-present range bytes BEFORE this audit was appended (the
    /// eligibility proof; after the deletes it is a
    /// previously-verified commitment).
    pub range_digest: [u8; 32],
    /// The digest chain's seed (carried from the checkpoint metadata
    /// for the self-contained binding).
    pub seed: [u8; 32],
    /// The reclaimed record count (the checkpoint's index length —
    /// the range publications plus SK's own retirement entry).
    pub records: u64,
    /// The idempotency keys among the reclaimed records (the
    /// membership universe preserves them — observational).
    pub reclaimed_keys: u64,
    /// The journal file NAMES removed alongside the range (only
    /// after the rotation's verified active journal and survival set
    /// stand; empty when no rotation preceded).
    pub journal_files: Vec<String>,
    /// The COMPLETE deletion manifest (the review corrective F4):
    /// one unit per indexed record — the record identity and its
    /// referenced content objects — collected during Phase A's
    /// digest verification while the bytes were present. Phase B
    /// deletes from this manifest, so an interrupted deletion
    /// converges on the retry.
    pub units: Vec<ReclaimUnit>,
}

impl ReclamationLedger {
    /// Canonical bytes (the audit revision's single content object).
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut enc = Encoder::new();
        enc.write_str(RECLAMATION_LEDGER_DOMAIN);
        enc.write_u32(RECLAMATION_LEDGER_VERSION);
        enc.write_bytes(self.mk.as_bytes());
        enc.write_bytes(self.sk.as_bytes());
        enc.write_bytes(&self.range_digest);
        enc.write_bytes(&self.seed);
        enc.write_u64(self.records);
        enc.write_u64(self.reclaimed_keys);
        enc.write_count(self.units.len());
        for unit in &self.units {
            enc.write_bytes(unit.record.as_bytes());
            enc.write_count(unit.objects.len());
            for object in &unit.objects {
                enc.write_bytes(object.as_bytes());
            }
        }
        enc.write_count(self.journal_files.len());
        for name in &self.journal_files {
            enc.write_str(name);
        }
        enc.into_bytes()
    }

    /// Strict decode: the domain and version gates, truncation and
    /// trailing bytes, bounded counts and names, real identities,
    /// and the keys-within-records coherence.
    pub fn decode(bytes: &[u8]) -> Result<Self, SaveError> {
        const SECTION: &str = "reclamation-ledger";
        let mut dec = Decoder::new(bytes);
        dec.read_domain(RECLAMATION_LEDGER_DOMAIN)?;
        let version = dec.read_u32()?;
        if version != RECLAMATION_LEDGER_VERSION {
            return Err(SaveError::Version {
                observed: version,
                supported: RECLAMATION_LEDGER_VERSION,
            });
        }
        let mk = read_hash(&mut dec, SECTION)?;
        let sk = read_hash(&mut dec, SECTION)?;
        if mk == Hash::ZERO || sk == Hash::ZERO {
            return Err(SaveError::Integrity("reclamation-ledger/zero-identity"));
        }
        let range_digest = read_digest(&mut dec, SECTION)?;
        let seed = read_digest(&mut dec, SECTION)?;
        let records = dec.read_u64()?;
        let reclaimed_keys = dec.read_u64()?;
        if reclaimed_keys > records {
            return Err(SaveError::Integrity(
                "reclamation-ledger/keys-exceed-records",
            ));
        }
        let unit_count = dec.read_count(MAX_RECLAMATION_UNITS, SECTION, "unit-count")?;
        let mut units = Vec::with_capacity(unit_count as usize);
        for _ in 0..unit_count {
            let record = read_hash(&mut dec, SECTION)?;
            if record == Hash::ZERO {
                return Err(SaveError::Integrity("reclamation-ledger/zero-identity"));
            }
            let object_count =
                dec.read_count(MAX_RECLAMATION_UNIT_OBJECTS, SECTION, "unit-object-count")?;
            let mut objects = Vec::with_capacity(object_count as usize);
            for _ in 0..object_count {
                objects.push(read_hash(&mut dec, SECTION)?);
            }
            units.push(ReclaimUnit { record, objects });
        }
        if records != units.len() as u64 {
            return Err(SaveError::Integrity("reclamation-ledger/unit-count"));
        }
        let count = dec.read_count(MAX_RECLAMATION_JOURNAL_FILES, SECTION, "journal-count")?;
        let mut journal_files = Vec::with_capacity(count as usize);
        for _ in 0..count {
            journal_files.push(
                dec.read_str(MAX_RECLAMATION_JOURNAL_NAME, SECTION)?
                    .to_owned(),
            );
        }
        dec.finish()?;
        Ok(ReclamationLedger {
            mk,
            sk,
            range_digest,
            seed,
            records,
            reclaimed_keys,
            journal_files,
            units,
        })
    }
}

fn read_hash(dec: &mut Decoder, section: &'static str) -> Result<Hash, SaveError> {
    let bytes = dec.read_bytes(Hash::WIDTH, section)?;
    Hash::from_bytes(bytes).ok_or(SaveError::Integrity("reclamation-ledger/hash-width"))
}

fn read_digest(dec: &mut Decoder, section: &'static str) -> Result<[u8; 32], SaveError> {
    let bytes = dec.read_bytes(32, section)?;
    // The exact-width requirement (the OC-C1-E corrective):
    // read_bytes treats its argument as a MAXIMUM, so a shorter
    // declared length reaches the copy and would panic — a malformed
    // digest is a typed failure, never a panic.
    if bytes.len() != 32 {
        return Err(SaveError::Integrity("reclamation/digest-width"));
    }
    let mut digest = [0u8; 32];
    digest.copy_from_slice(bytes);
    Ok(digest)
}

/// The audit revision's deterministic idempotency key (derived from
/// the reclaimed MK — one audit per checkpoint, ever).
pub fn reclamation_ledger_idempotency_key(mk: &Hash) -> Vec<u8> {
    format!("{RECLAMATION_LEDGER_KEY_DOMAIN}:{}", mk.hex()).into_bytes()
}

/// The v2 reclamation ledger's versioned domain: the closure-
/// expanded, batch-scoped audit (one ledger per plan batch).
pub const RECLAMATION_LEDGER_V2_DOMAIN: &str = "engine-persist/reclamation-ledger/2";
pub const RECLAMATION_LEDGER_V2_VERSION: u32 = 2;

/// The v2 audit revision's idempotency-key domain (deterministic in
/// the MK and the plan-defined batch index — one v2 audit per batch,
/// ever; the restart recognizes the installed audit by the key).
pub const RECLAMATION_LEDGER_V2_KEY_DOMAIN: &str = "engine-persist/reclamation-ledger-key/2";

/// The canonical batch plan's versioned domain: the complete
/// retirement membership partition for one MK, published before any
/// destructive work (the all-audits-first protocol's authority
/// record).
pub const RECLAMATION_PLAN_DOMAIN: &str = "engine-persist/reclamation-plan/1";
pub const RECLAMATION_PLAN_VERSION: u32 = 1;

/// The plan-family prefix — EVERY version in this family is refused
/// by the generic-append publication gate (the W2-A exclusion's
/// admission-boundary discipline).
pub const RECLAMATION_PLAN_DOMAIN_PREFIX: &str = "engine-persist/reclamation-plan/";

/// The plan revision's idempotency-key domain (deterministic in the
/// MK — one plan per checkpoint, ever).
pub const RECLAMATION_PLAN_KEY_DOMAIN: &str = "engine-persist/reclamation-plan-key/1";

/// The two-level completion marker's versioned domain: the batch
/// marker (this batch's storage obligations) and the range-terminal
/// marker (the complete retirement, journals included).
pub const RECLAMATION_COMPLETE_DOMAIN: &str = "engine-persist/reclamation-complete/1";
pub const RECLAMATION_COMPLETE_VERSION: u32 = 1;

/// The completion markers' idempotency-key domain (deterministic in
/// the MK and the batch index, or the MK and the `range` tag for
/// the terminal marker).
pub const RECLAMATION_COMPLETE_KEY_DOMAIN: &str = "engine-persist/reclamation-complete-key/1";

/// The v2 unit's object-count ceiling — COMPOSED from the enforced
/// schema bounds rather than inherited from v1: one record's direct
/// references (`MAX_HASH_LIST_LEN`) plus its verified composite
/// closure (`MAX_MANIFEST_CHUNKS`). The closure-expanded
/// representation's honest worst case.
pub const MAX_RECLAMATION_UNIT_OBJECTS_V2: u64 =
    crate::record::MAX_HASH_LIST_LEN + crate::composite_manifest::MAX_MANIFEST_CHUNKS;

/// The ratified maximum ledger byte ceiling (the admission's L) —
/// the codec-visible form of the owner-ratified bound.
pub const MAX_RECLAMATION_LEDGER_BYTES: u64 = 512 * 1024 * 1024;

/// The maximum deferred-identity cardinality a batch marker may
/// certify — derived from the LEDGER byte ceiling, not the
/// single-unit object bound (a batch spans many units, so its
/// combined distinct deferred set may legitimately exceed
/// `MAX_RECLAMATION_UNIT_OBJECTS_V2`): every deferred identity
/// appears in its batch's ledger as a unit object costing at least
/// `Hash::WIDTH` bytes plus per-unit count framing, so a ledger
/// within the byte ceiling cannot declare more distinct objects
/// than this. The bound is therefore always sufficient — a batch
/// can never become unfinishable because its valid completion
/// evidence exceeded the decoder limit.
pub const MAX_RECLAMATION_DEFERRED_IDENTITIES: u64 =
    MAX_RECLAMATION_LEDGER_BYTES / (Hash::WIDTH + 8);

/// The reclamation-ledger family prefix — every versioned audit
/// domain begins with it. The family gate's classification input:
/// a domain in the family with no matching exact arm refuses typed
/// as recognized-but-unsupported, never silently walked past.
pub const RECLAMATION_LEDGER_FAMILY: &str = "engine-persist/reclamation-ledger/";

/// Whether a domain belongs to the recognized reclamation-ledger
/// family (any version).
pub fn is_reclamation_ledger_family(domain: &str) -> bool {
    domain.starts_with(RECLAMATION_LEDGER_FAMILY)
}

/// The completion-marker domain gate for the GENERIC append (the
/// P2-F4 correction, extended to the whole family by the C1-F1
/// corrective): a completion-marker object may enter the chain
/// ONLY through the admission-controlled
/// [`publish_reclamation_complete`] boundary — a syntactically
/// valid marker appended through the unrestricted generic path
/// refuses typed, so no caller can install trusted terminal
/// evidence merely by encoding correct bytes. The gate covers
/// EVERY versioned completion-marker family domain — the accepted
/// V1 `/1`, the C1-delivered V2 `/2` Batch marker, and every
/// unrecognized future family variant — through the
/// [`crate::batch_completion::is_reclamation_complete_family`]
/// classifier (the family prefix
/// `engine-persist/reclamation-complete/` matches no other
/// accepted domain: the key namespaces continue with `-key`, the
/// witness family with `-ancestry-witness`), while unrelated
/// content domains remain accepted and the V1 refusal rule and
/// its typed error are preserved exactly.
pub fn enforce_completion_marker_gate(objects: &[StoredObject]) -> Result<(), MediumError> {
    for object in objects {
        if crate::batch_completion::is_reclamation_complete_family(object.domain()) {
            return Err(MediumError::Chain {
                rule: "reclamation-complete/generic-append-refused",
            });
        }
    }
    Ok(())
}

// ------------------------------------- the OC-C1 ratified begin-record codec
//
// The owner-ratified durable execution-authorization evidence
// (engine-persist/reclamation-begin/1, the 206-byte canonical form).
// OC-C1 delivers the codec, the publication gate, and the reader
// support ONLY — the trusted admission, the execution-boundary
// enforcement, and the completion integration belong to the later
// separately authorized checkpoints.

/// The ratified begin-record domain (34 UTF-8 bytes).
pub const RECLAMATION_BEGIN_DOMAIN: &str = "engine-persist/reclamation-begin/1";

/// The ratified begin-record version.
pub const RECLAMATION_BEGIN_VERSION: u32 = 1;

/// The begin-family prefix: EVERY version in this family is refused
/// by the generic publication path (an unsupported future variant
/// can never ride it).
pub const RECLAMATION_BEGIN_FAMILY: &str = "engine-persist/reclamation-begin/";

/// The begin-record's deterministic key domain (one begin per MK,
/// ever — mirroring the audit and completion-marker disciplines).
pub const RECLAMATION_BEGIN_KEY_DOMAIN: &str = "engine-persist/reclamation-begin-key/1";

/// Whether a domain belongs to the reclamation-begin family (any
/// version).
pub fn is_reclamation_begin_family(domain: &str) -> bool {
    domain.starts_with(RECLAMATION_BEGIN_FAMILY)
}

/// The begin revision's deterministic idempotency key (derived from
/// the reclaimed MK).
pub fn reclamation_begin_idempotency_key(mk: &Hash) -> Vec<u8> {
    format!("{RECLAMATION_BEGIN_KEY_DOMAIN}:{}", mk.hex()).into_bytes()
}

/// The ratified durable execution-authorization evidence: the exact
/// MK, the installed source-audit identity (transitively the whole
/// hash-verified ledger), the verified range digest, and the
/// founding-derived seed — the bindings a future trusted admission
/// constructs from verified values and the recovery reader
/// re-establishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReclamationBegin {
    /// The retired checkpoint's key-record identity.
    pub mk: Hash,
    /// The installed v1 audit's revision identity.
    pub source: Hash,
    /// The recomputed-and-matched range digest.
    pub range_digest: [u8; 32],
    /// The founding-derived seed.
    pub seed: [u8; 32],
}

impl ReclamationBegin {
    /// The canonical 206-byte encoding (the ratified layout: the
    /// u64-length-prefixed 34-byte domain, the u32-LE version, and
    /// four u64-length-prefixed 32-byte fields).
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut enc = Encoder::new();
        enc.write_str(RECLAMATION_BEGIN_DOMAIN);
        enc.write_u32(RECLAMATION_BEGIN_VERSION);
        enc.write_bytes(self.mk.as_bytes());
        enc.write_bytes(self.source.as_bytes());
        enc.write_bytes(&self.range_digest);
        enc.write_bytes(&self.seed);
        enc.into_bytes()
    }

    /// The strict decode: the exact domain and version, the exact
    /// hash and digest widths, the zero-identity refusal, and the
    /// trailing-data refusal — every malformed input is a typed
    /// failure, never a panic.
    pub fn decode(bytes: &[u8]) -> Result<Self, SaveError> {
        const SECTION: &str = "reclamation-begin";
        let mut dec = Decoder::new(bytes);
        dec.read_domain(RECLAMATION_BEGIN_DOMAIN)?;
        let version = dec.read_u32()?;
        if version != RECLAMATION_BEGIN_VERSION {
            return Err(SaveError::Version {
                observed: version,
                supported: RECLAMATION_BEGIN_VERSION,
            });
        }
        let read_begin_hash = |dec: &mut Decoder| -> Result<Hash, SaveError> {
            let bytes = dec.read_bytes(Hash::WIDTH, SECTION)?;
            Hash::from_bytes(bytes).ok_or(SaveError::Integrity("reclamation-begin/hash-width"))
        };
        let mk = read_begin_hash(&mut dec)?;
        let source = read_begin_hash(&mut dec)?;
        if mk == Hash::ZERO || source == Hash::ZERO {
            return Err(SaveError::Integrity("reclamation-begin/zero-identity"));
        }
        // The begin-specific exact-width digest read (the ratified
        // malformed-input contract: reclamation-begin/digest-width —
        // distinct from the shared ledger helper's classification).
        let read_begin_digest = |dec: &mut Decoder| -> Result<[u8; 32], SaveError> {
            let bytes = dec.read_bytes(32, SECTION)?;
            if bytes.len() != 32 {
                return Err(SaveError::Integrity("reclamation-begin/digest-width"));
            }
            let mut digest = [0u8; 32];
            digest.copy_from_slice(bytes);
            Ok(digest)
        };
        let range_digest = read_begin_digest(&mut dec)?;
        let seed = read_begin_digest(&mut dec)?;
        dec.finish()?;
        Ok(Self {
            mk,
            source,
            range_digest,
            seed,
        })
    }
}

/// The begin-family publication gate: NO version of
/// engine-persist/reclamation-begin/ may enter the chain through the
/// unrestricted generic append — the begin is trusted
/// execution-authorization evidence, installable only through the
/// future OC-C2 in-crate admission after the full first-execution
/// verification. A valid codec alone never grants publication
/// authority.
pub fn enforce_reclamation_begin_gate(objects: &[StoredObject]) -> Result<(), MediumError> {
    for object in objects {
        if is_reclamation_begin_family(object.domain()) {
            return Err(MediumError::Chain {
                rule: "reclamation-begin/generic-append-refused",
            });
        }
    }
    Ok(())
}

/// The plan-family publication gate (V2-W, the W2-A exclusion): no
/// reclamation-plan object (any version) may enter the chain through
/// the unrestricted generic append — the plan is the all-audits-first
/// protocol's authority record, installable only through the
/// in-crate plan admission after the checkpoint authentication and
/// the W2 v1-exclusion check. A valid codec alone never grants
/// publication authority.
pub fn enforce_reclamation_plan_gate(objects: &[StoredObject]) -> Result<(), MediumError> {
    for object in objects {
        if object.domain().starts_with(RECLAMATION_PLAN_DOMAIN_PREFIX) {
            return Err(MediumError::Chain {
                rule: "reclamation-plan/generic-append-refused",
            });
        }
    }
    Ok(())
}

/// The reclamation-ledger-family publication gate (V2-W, the W2-A
/// exclusion): no v1 or v2 reclamation audit (any version in the
/// family) may enter the chain through the unrestricted generic
/// append — the audits are trusted retirement evidence, installable
/// only through the in-crate publications (the v1
/// [`publish_reclamation_ledger`] and the v2
/// [`publish_reclamation_ledger_v2`]) that enforce the W2
/// bidirectional exclusion. This is the pre-authorized narrow
/// migration's enforcement side: the legitimate v1 publication path
/// moves to the in-crate admitted receipt, its wire format and
/// behavior unchanged.
pub fn enforce_reclamation_ledger_family_gate(objects: &[StoredObject]) -> Result<(), MediumError> {
    for object in objects {
        if is_reclamation_ledger_family(object.domain()) {
            return Err(MediumError::Chain {
                rule: "reclamation-ledger/generic-append-refused",
            });
        }
    }
    Ok(())
}

// ------------------------------------- the OC-C2 trusted begin admission
//
// The provider-coordinated publication of the permanent
// execution-authorization evidence. The admission runs the FULL
// first-execution verification — the exact canonical audit identity,
// the MK binding, the complete original retirement evidence (every
// record present, hash-verified, manifest-equal, SK included), and
// the INDEPENDENT range-digest recomputation over the actual
// original bytes — BEFORE constructing the begin from the verified
// values. A caller may name the audit and the MK; the begin payload
// is always derived from verified canonical state. OC-C2 does NOT
// connect this to the destructive execution path (OC-C3).

/// The typed begin-publication outcome: the truthful classification
/// an installation, never a guess. `Accepted` carries the provider's
/// durably-acknowledged installation AT THE ACTUAL TIER (C2-R8: the
/// truthful tier propagated from the accepted append — never
/// synthesized); `ReconciledInstalled` carries a
/// canonical-membership-only proof (the durability acknowledgment
/// remains UNCONFIRMED — the destructive-authorization grade
/// requires the OC-C3 reaffirmation — NO implied tier); the
/// refusals are typed errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BeginPublicationOutcome {
    /// Installed and durably acknowledged at the provider's ACTUAL
    /// tier (the in-memory contract only — the persisted begin
    /// format is unaffected).
    Accepted {
        id: Hash,
        tier: crate::medium::SyncTier,
    },
    /// An uncertain publication reconciled to canonical
    /// installation — membership proven, the durability
    /// acknowledgment UNCONFIRMED.
    ReconciledInstalled { id: Hash },
    /// An uncertain publication reconciled to definite absence — a
    /// clean retry is appropriate.
    ReconciledNotInstalled,
    /// A same-key begin was already installed and its EXACT content
    /// verified (the idempotent restart — never inferred from key
    /// occupancy alone).
    DuplicateVerified { id: Hash },
}

// ------------------------------------------------- the OC-C3 CP-1 facilities

/// (OC-C3 CP-1) The verified installed-begin authority: the
/// canonical revision identity and the truthfully established
/// durability grade. Constructible only inside the provider
/// implementations of
/// [`CanonicalMedium::verify_begin_authority`](crate::medium::
/// CanonicalMedium::verify_begin_authority) — a caller names
/// identities and can never supply begin bytes, fabricate a
/// receipt, or upgrade the grade. The fields are `pub(crate)`: an
/// external crate can hold, transport, and inspect a
/// provider-issued value (the read-only accessors) but can never
/// construct or modify one — the type is not a caller-accepted
/// authorization token. The destructive-authorization consumer
/// (the separately authorized CP-2 execution gate) treats this as
/// evidence, never as a deletion instruction.
///
/// ```compile_fail
/// // An external crate cannot fabricate an authority by
/// // constructing the type (the fields are crate-private):
/// let forged = engine_persist::reclaim::BeginAuthority {
///     id: engine_persist::Hash::ZERO,
///     tier: engine_persist::SyncTier::FullPosixSequence,
/// };
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BeginAuthority {
    /// The verified canonical begin revision.
    pub(crate) id: Hash,
    /// The truthfully established durability grade at verification
    /// time (the POSIX local provider: `FullPosixSequence` ONLY
    /// after the completed reaffirmation sequence; SQLite:
    /// `TransactionalWalFull` from the read-transaction visibility
    /// proof under the verified WAL/FULL configuration).
    pub(crate) tier: crate::medium::SyncTier,
}

impl BeginAuthority {
    /// The verified canonical begin revision identity.
    pub fn id(&self) -> Hash {
        self.id
    }

    /// The truthfully established durability grade.
    pub fn tier(&self) -> crate::medium::SyncTier {
        self.tier
    }
}

/// (V2X-CP2) The verified installed V2 begin with its canonical
/// audit identities (the plan's deterministic batch order).
#[derive(Debug)]
pub struct V2InstalledBegin {
    /// The begin revision's canonical identity.
    pub id: Hash,
    /// The strictly decoded 206-byte begin payload.
    pub begin: ReclamationBegin,
    /// The audits' revision identities, in the plan's batch order.
    pub audit_records: Vec<Hash>,
}

/// (V2X-CP2) The retained-evidence V2 begin authority facts — the
/// recovery-safe verification's output (the durable grade is the
/// PROVIDER's to establish, never this struct's).
#[derive(Debug)]
pub struct V2BeginAuthorityEvidence {
    /// The begin revision's canonical identity.
    pub begin_id: Hash,
    /// The installed plan revision's canonical identity (the begin's
    /// bound source).
    pub plan_record: Hash,
    /// The authenticated plan.
    pub plan: ReclaimPlan,
    /// The audits' revision identities, in the plan's batch order.
    pub audit_records: Vec<Hash>,
}

/// The v2 reclamation ledger: the ratified closure-expanded,
/// batch-scoped audit. The units carry each retiring record's
/// direct references PLUS its verified composite closure (collected
/// and envelope-verified at Phase A, while the bytes were present —
/// the durable provenance the manifest-driven resume and the
/// post-deletion authentication rely on). The journal files are
/// RANGE-level obligations and live in the plan, not here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReclamationLedgerV2 {
    /// The checkpoint whose range is reclaimed (the permanent
    /// metadata binding).
    pub mk: Hash,
    /// The retired snapshot revision (MK's parent).
    pub sk: Hash,
    /// The plan-defined batch index this ledger covers.
    pub batch: u64,
    /// The pre-verified range digest (recomputed at Phase A against
    /// the still-present bytes).
    pub range_digest: [u8; 32],
    /// The digest chain's seed (carried from the checkpoint
    /// metadata).
    pub seed: [u8; 32],
    /// The reclaimed record count (this batch's units).
    pub records: u64,
    /// The idempotency keys among this batch's records
    /// (observational).
    pub reclaimed_keys: u64,
    /// The complete deletion manifest for this batch: one unit per
    /// retiring record, its objects closure-expanded.
    pub units: Vec<ReclaimUnit>,
}

impl ReclamationLedgerV2 {
    /// Canonical bytes (the batch audit revision's single content
    /// object) — deterministic in the given order.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut enc = Encoder::new();
        enc.write_str(RECLAMATION_LEDGER_V2_DOMAIN);
        enc.write_u32(RECLAMATION_LEDGER_V2_VERSION);
        enc.write_bytes(self.mk.as_bytes());
        enc.write_bytes(self.sk.as_bytes());
        enc.write_u64(self.batch);
        enc.write_bytes(&self.range_digest);
        enc.write_bytes(&self.seed);
        enc.write_u64(self.records);
        enc.write_u64(self.reclaimed_keys);
        enc.write_count(self.units.len());
        for unit in &self.units {
            enc.write_bytes(unit.record.as_bytes());
            enc.write_count(unit.objects.len());
            for object in &unit.objects {
                enc.write_bytes(object.as_bytes());
            }
        }
        enc.into_bytes()
    }

    /// Strict decode: the domain and version gates, the bounded
    /// counts, the zero-identity refusals, the count coherence, and
    /// THE SHARING RULES — a duplicate unit record (the index
    /// members are unique) or a duplicate object WITHIN one unit (a
    /// record's references and closure are sets) refuses typed;
    /// repeated object identities ACROSS units are legitimate
    /// sharing and pass.
    pub fn decode(bytes: &[u8]) -> Result<Self, SaveError> {
        const SECTION: &str = "reclamation-ledger-v2";
        let mut dec = Decoder::new(bytes);
        dec.read_domain(RECLAMATION_LEDGER_V2_DOMAIN)?;
        let version = dec.read_u32()?;
        if version != RECLAMATION_LEDGER_V2_VERSION {
            return Err(SaveError::Version {
                observed: version,
                supported: RECLAMATION_LEDGER_V2_VERSION,
            });
        }
        let mk = read_hash(&mut dec, SECTION)?;
        let sk = read_hash(&mut dec, SECTION)?;
        if mk == Hash::ZERO || sk == Hash::ZERO {
            return Err(SaveError::Integrity("reclamation-ledger-v2/zero-identity"));
        }
        let batch = dec.read_u64()?;
        let range_digest = read_digest(&mut dec, SECTION)?;
        let seed = read_digest(&mut dec, SECTION)?;
        let records = dec.read_u64()?;
        let reclaimed_keys = dec.read_u64()?;
        if reclaimed_keys > records {
            return Err(SaveError::Integrity(
                "reclamation-ledger-v2/keys-exceed-records",
            ));
        }
        let unit_count = dec.read_count(MAX_RECLAMATION_UNITS, SECTION, "unit-count")?;
        let mut units = Vec::with_capacity(unit_count as usize);
        let mut seen_records = std::collections::BTreeSet::new();
        for _ in 0..unit_count {
            let record = read_hash(&mut dec, SECTION)?;
            if record == Hash::ZERO {
                return Err(SaveError::Integrity("reclamation-ledger-v2/zero-identity"));
            }
            if !seen_records.insert(record) {
                return Err(SaveError::Integrity(
                    "reclamation-ledger-v2/duplicate-unit-record",
                ));
            }
            let object_count = dec.read_count(
                MAX_RECLAMATION_UNIT_OBJECTS_V2,
                SECTION,
                "unit-object-count",
            )?;
            let mut objects = Vec::with_capacity(object_count as usize);
            let mut seen_objects = std::collections::BTreeSet::new();
            for _ in 0..object_count {
                let object = read_hash(&mut dec, SECTION)?;
                if object == Hash::ZERO {
                    return Err(SaveError::Integrity("reclamation-ledger-v2/zero-identity"));
                }
                if !seen_objects.insert(object) {
                    return Err(SaveError::Integrity(
                        "reclamation-ledger-v2/duplicate-unit-object",
                    ));
                }
                objects.push(object);
            }
            units.push(ReclaimUnit { record, objects });
        }
        if records != units.len() as u64 {
            return Err(SaveError::Integrity("reclamation-ledger-v2/unit-count"));
        }
        dec.finish()?;
        Ok(ReclamationLedgerV2 {
            mk,
            sk,
            batch,
            range_digest,
            seed,
            records,
            reclaimed_keys,
            units,
        })
    }
}

/// One batch of the canonical plan: the plan-defined index and the
/// retiring-record membership (a contiguous slice of the
/// deterministic epoch order).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanBatch {
    /// The plan-defined batch index (0-based, strictly increasing —
    /// the deterministic identity component).
    pub index: u64,
    /// This batch's retiring-record identities, in the plan's epoch
    /// order.
    pub records: Vec<Hash>,
}

/// The canonical batch plan: the complete, admission-validated
/// retirement membership partition for one MK — the durable
/// authority record that defines the expected batch set, the
/// execution order, and the range-level journal obligations.
/// Published before any destructive work; the restart reads the
/// plan, never re-derives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReclaimPlan {
    /// The checkpoint whose range is retired.
    pub mk: Hash,
    /// The retired snapshot revision (MK's parent).
    pub sk: Hash,
    /// The pre-verified range digest and its seed (the checkpoint
    /// bindings).
    pub range_digest: [u8; 32],
    pub seed: [u8; 32],
    /// The ledger byte bound the partitioning used (recorded for
    /// cross-checking the batches' ledgers).
    pub ledger_byte_limit: u64,
    /// The batches, in the deterministic order (index 0 first — the
    /// newest epoch slice).
    pub batches: Vec<PlanBatch>,
    /// The range-level journal retirement obligations (the stale
    /// generations removable after every batch completes — the
    /// rotation's survival-evidence-derived names).
    pub journal_files: Vec<String>,
}

impl ReclaimPlan {
    /// The total retiring-record count (the partition's union).
    pub fn total_units(&self) -> u64 {
        self.batches.iter().map(|b| b.records.len() as u64).sum()
    }

    /// Canonical bytes (the plan revision's single content object).
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut enc = Encoder::new();
        enc.write_str(RECLAMATION_PLAN_DOMAIN);
        enc.write_u32(RECLAMATION_PLAN_VERSION);
        enc.write_bytes(self.mk.as_bytes());
        enc.write_bytes(self.sk.as_bytes());
        enc.write_bytes(&self.range_digest);
        enc.write_bytes(&self.seed);
        enc.write_u64(self.ledger_byte_limit);
        enc.write_count(self.batches.len());
        for batch in &self.batches {
            enc.write_u64(batch.index);
            enc.write_count(batch.records.len());
            for record in &batch.records {
                enc.write_bytes(record.as_bytes());
            }
        }
        enc.write_count(self.journal_files.len());
        for name in &self.journal_files {
            enc.write_str(name);
        }
        enc.into_bytes()
    }

    /// Strict decode: the gates, the bounded counts, the
    /// partition rules — the batch indices strictly increasing from
    /// zero (the deterministic order), no empty batch, no duplicate
    /// record across ANY batch (the complete-membership partition:
    /// no overlap), and the journal-name bounds.
    pub fn decode(bytes: &[u8]) -> Result<Self, SaveError> {
        const SECTION: &str = "reclamation-plan";
        let mut dec = Decoder::new(bytes);
        dec.read_domain(RECLAMATION_PLAN_DOMAIN)?;
        let version = dec.read_u32()?;
        if version != RECLAMATION_PLAN_VERSION {
            return Err(SaveError::Version {
                observed: version,
                supported: RECLAMATION_PLAN_VERSION,
            });
        }
        let mk = read_hash(&mut dec, SECTION)?;
        let sk = read_hash(&mut dec, SECTION)?;
        if mk == Hash::ZERO || sk == Hash::ZERO {
            return Err(SaveError::Integrity("reclamation-plan/zero-identity"));
        }
        let range_digest = read_digest(&mut dec, SECTION)?;
        let seed = read_digest(&mut dec, SECTION)?;
        let ledger_byte_limit = dec.read_u64()?;
        if ledger_byte_limit == 0 {
            return Err(SaveError::Integrity("reclamation-plan/zero-byte-limit"));
        }
        let batch_count = dec.read_count(MAX_RECLAMATION_UNITS, SECTION, "batch-count")?;
        if batch_count == 0 {
            // A real retirement has at least one record — a
            // zero-batch plan is not a legitimate ratified state.
            return Err(SaveError::Integrity("reclamation-plan/empty-plan"));
        }
        let mut batches = Vec::with_capacity(batch_count as usize);
        let mut seen_records = std::collections::BTreeSet::new();
        let mut aggregate_records: u64 = 0;
        for expected_index in 0..batch_count {
            let index = dec.read_u64()?;
            if index != expected_index {
                return Err(SaveError::Integrity("reclamation-plan/batch-index-order"));
            }
            let record_count =
                dec.read_count(MAX_RECLAMATION_UNITS, SECTION, "batch-record-count")?;
            if record_count == 0 {
                return Err(SaveError::Integrity("reclamation-plan/empty-batch"));
            }
            // The aggregate membership ceiling (checked
            // arithmetic): the plan partitions ONE MK retirement
            // range, whose membership is bounded by the checkpoint
            // index — the same bound the v1 ledger's unit count
            // uses.
            aggregate_records =
                aggregate_records
                    .checked_add(record_count)
                    .ok_or(SaveError::Integrity(
                        "reclamation-plan/aggregate-membership",
                    ))?;
            if aggregate_records > MAX_RECLAMATION_UNITS {
                return Err(SaveError::Integrity(
                    "reclamation-plan/aggregate-membership",
                ));
            }
            let mut records = Vec::with_capacity(record_count as usize);
            for _ in 0..record_count {
                let record = read_hash(&mut dec, SECTION)?;
                if record == Hash::ZERO {
                    return Err(SaveError::Integrity("reclamation-plan/zero-identity"));
                }
                if !seen_records.insert(record) {
                    return Err(SaveError::Integrity("reclamation-plan/duplicate-record"));
                }
                records.push(record);
            }
            batches.push(PlanBatch { index, records });
        }
        let journal_count =
            dec.read_count(MAX_RECLAMATION_JOURNAL_FILES, SECTION, "journal-count")?;
        let mut journal_files = Vec::with_capacity(journal_count as usize);
        for _ in 0..journal_count {
            journal_files.push(
                dec.read_str(MAX_RECLAMATION_JOURNAL_NAME, SECTION)?
                    .to_owned(),
            );
        }
        dec.finish()?;
        Ok(ReclaimPlan {
            mk,
            sk,
            range_digest,
            seed,
            ledger_byte_limit,
            batches,
            journal_files,
        })
    }
}

/// Which completion level a marker certifies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionKind {
    /// This batch's own provider-storage obligations (its mandatory
    /// object set, across normal and quarantine forms) — never the
    /// journals, never another batch.
    Batch,
    /// The complete MK retirement: every planned batch's marker
    /// installed and verified, the journal retirement and ownership
    /// updates applied.
    Range,
}

/// The two-level completion marker: the persisted terminal evidence.
/// A batch marker binds its audited ledger's revision identity and
/// records the deferred (shared, still-live) identities it
/// authorized to preserve; the range marker binds the plan and the
/// retired journal names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReclamationComplete {
    /// Which level this marker certifies.
    pub kind: CompletionKind,
    /// The retired checkpoint.
    pub mk: Hash,
    /// The plan-defined batch index (batch markers only).
    pub batch: u64,
    /// The audited source's revision identity: the batch ledger's
    /// record for a batch marker; the plan's record for the range
    /// marker.
    pub source: Hash,
    /// The batch's mandatory (deleted) identity count (batch
    /// markers only — cross-checkable against the ledger and the
    /// partition).
    pub mandatory_count: u64,
    /// The deferred identities this batch authorized to preserve
    /// (shared with surviving roots — the visible debt the later
    /// audit claims).
    pub deferred: Vec<Hash>,
    /// The retired journal-file names (range markers only — the
    /// completed obligations the marker certifies).
    pub journal_files: Vec<String>,
}

impl ReclamationComplete {
    /// Canonical bytes (the marker revision's single content
    /// object) — kind-gated: a batch marker encodes its batch,
    /// source, mandatory count, and deferred set; a range marker
    /// encodes its source and the retired journal names.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut enc = Encoder::new();
        enc.write_str(RECLAMATION_COMPLETE_DOMAIN);
        enc.write_u32(RECLAMATION_COMPLETE_VERSION);
        enc.write_u8(match self.kind {
            CompletionKind::Batch => 0,
            CompletionKind::Range => 1,
        });
        enc.write_bytes(self.mk.as_bytes());
        match self.kind {
            CompletionKind::Batch => {
                enc.write_u64(self.batch);
                enc.write_bytes(self.source.as_bytes());
                enc.write_u64(self.mandatory_count);
                enc.write_count(self.deferred.len());
                for identity in &self.deferred {
                    enc.write_bytes(identity.as_bytes());
                }
            }
            CompletionKind::Range => {
                enc.write_bytes(self.source.as_bytes());
                enc.write_count(self.journal_files.len());
                for name in &self.journal_files {
                    enc.write_str(name);
                }
            }
        }
        enc.into_bytes()
    }

    /// Strict decode: the gates, the kind discriminant, the
    /// kind-coherence (a batch marker carries no journal names; a
    /// range marker carries no batch, deferred set, or mandatory
    /// count), the zero-identity refusals, and the deferred set's
    /// duplicate rejection.
    pub fn decode(bytes: &[u8]) -> Result<Self, SaveError> {
        const SECTION: &str = "reclamation-complete";
        let mut dec = Decoder::new(bytes);
        dec.read_domain(RECLAMATION_COMPLETE_DOMAIN)?;
        let version = dec.read_u32()?;
        if version != RECLAMATION_COMPLETE_VERSION {
            return Err(SaveError::Version {
                observed: version,
                supported: RECLAMATION_COMPLETE_VERSION,
            });
        }
        let kind = match dec.read_u8()? {
            0 => CompletionKind::Batch,
            1 => CompletionKind::Range,
            other => {
                return Err(SaveError::Enum {
                    section: SECTION,
                    discriminant: u64::from(other),
                });
            }
        };
        let mk = read_hash(&mut dec, SECTION)?;
        if mk == Hash::ZERO {
            return Err(SaveError::Integrity("reclamation-complete/zero-identity"));
        }
        match kind {
            CompletionKind::Batch => {
                let batch = dec.read_u64()?;
                let source = read_hash(&mut dec, SECTION)?;
                if source == Hash::ZERO {
                    return Err(SaveError::Integrity("reclamation-complete/zero-identity"));
                }
                let mandatory_count = dec.read_u64()?;
                let deferred_count = dec.read_count(
                    MAX_RECLAMATION_DEFERRED_IDENTITIES,
                    SECTION,
                    "deferred-count",
                )?;
                let mut deferred = Vec::with_capacity(deferred_count as usize);
                let mut seen = std::collections::BTreeSet::new();
                for _ in 0..deferred_count {
                    let identity = read_hash(&mut dec, SECTION)?;
                    if identity == Hash::ZERO {
                        return Err(SaveError::Integrity("reclamation-complete/zero-identity"));
                    }
                    if !seen.insert(identity) {
                        return Err(SaveError::Integrity(
                            "reclamation-complete/duplicate-deferred",
                        ));
                    }
                    deferred.push(identity);
                }
                dec.finish()?;
                Ok(ReclamationComplete {
                    kind,
                    mk,
                    batch,
                    source,
                    mandatory_count,
                    deferred,
                    journal_files: Vec::new(),
                })
            }
            CompletionKind::Range => {
                let source = read_hash(&mut dec, SECTION)?;
                if source == Hash::ZERO {
                    return Err(SaveError::Integrity("reclamation-complete/zero-identity"));
                }
                let journal_count =
                    dec.read_count(MAX_RECLAMATION_JOURNAL_FILES, SECTION, "journal-count")?;
                let mut journal_files = Vec::with_capacity(journal_count as usize);
                for _ in 0..journal_count {
                    journal_files.push(
                        dec.read_str(MAX_RECLAMATION_JOURNAL_NAME, SECTION)?
                            .to_owned(),
                    );
                }
                dec.finish()?;
                Ok(ReclamationComplete {
                    kind,
                    mk,
                    batch: 0,
                    source,
                    mandatory_count: 0,
                    deferred: Vec::new(),
                    journal_files,
                })
            }
        }
    }
}

/// The v2 audit revision's deterministic idempotency key (the MK
/// and the plan-defined batch index — one v2 audit per batch, ever;
/// the W2 exclusion and the restart recognition both key on this).
pub fn reclamation_ledger_v2_idempotency_key(mk: &Hash, batch: u64) -> Vec<u8> {
    format!("{RECLAMATION_LEDGER_V2_KEY_DOMAIN}:{}:{batch}", mk.hex()).into_bytes()
}

/// The plan revision's deterministic idempotency key (the MK — one
/// plan per checkpoint, ever).
pub fn reclamation_plan_idempotency_key(mk: &Hash) -> Vec<u8> {
    format!("{RECLAMATION_PLAN_KEY_DOMAIN}:{}", mk.hex()).into_bytes()
}

// ------------------------- the V2-W F4 canonical journal-root binding
//
// The owner-ratified Option A contract: exactly ONE authoritative
// journal-root binding per newly bound campaign — a strictly
// versioned, deterministic, WRITE-ONCE canonical record binding the
// campaign's immutable founding identity to a stable journal-root ID
// and the authoritative root-location descriptor. The chain's
// duplicate-idempotency-key rule enforces the one-per-campaign
// uniqueness (a competing second root refuses at the chain
// validation); the binding is installable ONLY through the trusted
// in-crate admission after the founding authentication (a caller's
// possession of the founding hash is never authority for a second
// root); the generic append refuses the domain. The root ID is a
// STABLE NONZERO identity distinct from the filesystem path —
// carried unchanged across every rotation generation — while the
// location descriptor names the authoritative local root with the
// canonical-path normalization as its comparison rule (absolute
// binding: a relocated root is a CONTRADICTORY binding, never a
// silent rebind).

/// The canonical journal-root binding object's versioned domain.
pub const JOURNAL_ROOT_BINDING_DOMAIN: &str = "engine-persist/journal-root/1";
pub const JOURNAL_ROOT_BINDING_VERSION: u32 = 1;

/// The binding revision's deterministic idempotency-key domain
/// (one binding per founding, ever).
pub const JOURNAL_ROOT_BINDING_KEY_DOMAIN: &str = "engine-persist/journal-root-key/1";

/// The canonical journal-root binding: the immutable founding
/// record's hash, the stable journal-root identity (nonzero, never
/// the filesystem path, unchanged across rotations), and the
/// authoritative root-location descriptor (the normalized absolute
/// path — explicit comparison rule: a different location under the
/// same key is a CONTRADICTORY binding, refused, never rebound).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalRootBinding {
    pub founding: Hash,
    pub root_id: Hash,
    pub location: String,
}

impl JournalRootBinding {
    /// Canonical bytes (the binding revision's single content
    /// object) — deterministic.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut enc = Encoder::new();
        enc.write_str(JOURNAL_ROOT_BINDING_DOMAIN);
        enc.write_u32(JOURNAL_ROOT_BINDING_VERSION);
        enc.write_bytes(self.founding.as_bytes());
        enc.write_bytes(self.root_id.as_bytes());
        enc.write_str(&self.location);
        enc.into_bytes()
    }

    /// Strict decode: the domain and version gates, the exact hash
    /// widths, the nonzero-identity refusals, the nonempty location,
    /// and the trailing-data refusal.
    pub fn decode(bytes: &[u8]) -> Result<Self, SaveError> {
        let mut dec = Decoder::new(bytes);
        dec.read_domain(JOURNAL_ROOT_BINDING_DOMAIN)?;
        let version = dec.read_u32()?;
        if version != JOURNAL_ROOT_BINDING_VERSION {
            return Err(SaveError::Version {
                observed: version,
                supported: JOURNAL_ROOT_BINDING_VERSION,
            });
        }
        let founding = read_binding_hash(&mut dec)?;
        let root_id = read_binding_hash(&mut dec)?;
        if founding == Hash::ZERO || root_id == Hash::ZERO {
            return Err(SaveError::Integrity("journal-root/zero-identity"));
        }
        let location = dec.read_str(1024, "journal-root")?;
        if location.is_empty() {
            return Err(SaveError::Integrity("journal-root/empty-location"));
        }
        dec.finish()?;
        Ok(Self {
            founding,
            root_id,
            location: location.to_owned(),
        })
    }

    /// The location descriptor's normalization + comparison rule:
    /// the ABSOLUTE form WITHOUT symlink resolution (stable
    /// regardless of the target's existence — a resolution-based
    /// rule would disagree with itself across a creation boundary
    /// on symlinked temp roots such as macOS `/var`), the trailing
    /// separator trimmed. RELATIVE-LOCATION LIMITATION (documented,
    /// the owner's absolute-binding permission): the descriptor
    /// binds the absolute path as supplied; a relocated root is a
    /// contradictory binding, never a silent rebind.
    pub fn normalize_location(path: &std::path::Path) -> String {
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from("/"))
                .join(path)
        };
        let mut text = absolute.to_string_lossy().into_owned();
        while text.len() > 1 && text.ends_with('/') {
            text.pop();
        }
        text
    }
}

fn read_binding_hash(dec: &mut Decoder<'_>) -> Result<Hash, SaveError> {
    let bytes = dec.read_bytes(Hash::WIDTH, "journal-root")?;
    Hash::from_bytes(bytes).ok_or(SaveError::Integrity("journal-root/hash-width"))
}

/// The binding revision's deterministic idempotency key (the founding
/// hash — one binding per campaign, ever).
pub fn journal_root_binding_idempotency_key(founding: &Hash) -> Vec<u8> {
    format!("{JOURNAL_ROOT_BINDING_KEY_DOMAIN}:{}", founding.hex()).into_bytes()
}

/// (V2-W F4) The binding-family publication gate: no journal-root
/// binding object (any version) may enter the chain through the
/// unrestricted generic append — the binding is trusted campaign
/// authority, installable only through the in-crate admission below.
pub fn enforce_journal_root_binding_gate(objects: &[StoredObject]) -> Result<(), MediumError> {
    for object in objects {
        if object.domain().starts_with("engine-persist/journal-root/") {
            return Err(MediumError::Chain {
                rule: "journal-root/generic-append-refused",
            });
        }
    }
    Ok(())
}

/// (V2-W F4) The typed binding-publication outcome — the same
/// truthful classification discipline as the plan's.
#[derive(Debug)]
pub enum JournalRootBindingOutcome {
    Accepted { id: Hash },
    ReconciledInstalled { id: Hash },
    ReconciledNotInstalled,
    DuplicateVerified { id: Hash },
}

/// (V2-W, W3A) The minimum supported effective provider per-object
/// capacity: the retained contract's 4 MB floor, read as the binary
/// megabyte consistent with the 512 MiB canonical ceiling's
/// convention (4 × 1024 × 1024 bytes). A provider below this floor
/// is an UNSUPPORTED configuration for reclamation publication —
/// fail-closed and typed, with reads/writes otherwise unaffected (a
/// distinct refusal class from the batch-size check).
pub const MIN_SUPPORTED_EFFECTIVE_OBJECT_BYTES: u64 = 4 * 1024 * 1024;

/// (V2X-CP1) The trusted all-audits reader's outcome: the distinct
/// readiness classifications, never collapsed into one ambiguous
/// success-or-absence. The trusted positive is exactly ONE of them;
/// the corrupt, conflicting, and unsupported evidence classes are
/// typed refusals (the [`MediumError`] taxonomy), never silent
/// positives.
#[derive(Debug)]
pub enum V2ExecutionReadiness {
    /// THE FIRST-EXECUTION-READY STATE: the installed plan and EVERY
    /// plan-listed batch audit authenticated against the checkpoint's
    /// permanent metadata and the COMPLETE original retirement
    /// evidence (every retiring record present and hash-verified,
    /// every closure recomputed exactly equal, the digest and
    /// founding-derived seed recomputed equal through the accepted
    /// commitment algorithm, the ordered partition agreeing with the
    /// checkpoint index) — the only state in which a future,
    /// separately authorized execution begin could be admitted. The
    /// carried values are verified FACTS, never a capability: the
    /// future execution checkpoints re-derive everything under their
    /// own provider coordination boundaries (the recorded D3
    /// obligation); this result grants no execution authority.
    ExecutionReady {
        /// The plan revision's canonical identity — the RECORD id,
        /// the value any future V2 begin must bind as its source
        /// (distinct from the plan payload's object hash; the
        /// recorded D1 obligation).
        plan_record: Hash,
        /// The plan payload object's content hash.
        plan_object: Hash,
        /// The audits' revision identities, in the plan's
        /// deterministic batch order.
        audit_records: Vec<Hash>,
    },
    /// The legitimate non-destructive mid-publication state: the
    /// checkpoint is genuine but the plan (and/or the plan-listed
    /// batch audits) are not fully installed — nothing may execute;
    /// the publication phase simply continues. NEVER a trusted
    /// positive.
    PublicationIncomplete {
        /// The plan-listed batch indices with no installed audit
        /// (empty when the plan itself is absent).
        missing_batches: Vec<u64>,
    },
    /// The audits are complete but the original retirement range is
    /// NOT intact — retiring records or their content objects are
    /// absent (under the owner-ratified Option C policy, an absence
    /// before a begin is not recoverable by this reader). The state
    /// requires the future begin/recovery authority of the
    /// separately authorized execution checkpoints to adjudicate;
    /// this classification is diagnostic and NEVER certified
    /// execution-ready. CP1 does NOT verify V2 begin authority and
    /// makes no claim about a begin's existence either way — the
    /// classification proves only the range's degraded evidence
    /// state. (Per the recorded D2 recovery-liveness obligation, the
    /// future execution-progress verifier — not this reader — owns
    /// the resumed-state verification.)
    RangeNotIntact {
        /// The absent identities discovered (records first, then
        /// their content objects).
        absent: Vec<Hash>,
    },
}

/// (V2-W) The typed plan-publication outcome: the truthful
/// classification, never a guess. `Accepted` carries the
/// durably-acknowledged installation's record identity;
/// `ReconciledInstalled` a canonical-membership proof reconciled
/// from an Unknown outcome with the exact content verified;
/// `DuplicateVerified` an ALREADY-installed identical plan
/// (discovered by the deterministic key and exactly content-matched
/// — a conflicting content under the same key REFUSES, never an
/// equivalent duplicate); `ReconciledNotInstalled` the verified
/// absence after an Unknown outcome.
#[derive(Debug)]
pub enum PlanPublicationOutcome {
    Accepted { id: Hash },
    ReconciledInstalled { id: Hash },
    ReconciledNotInstalled,
    DuplicateVerified { id: Hash },
}

/// (V2-W) The typed v2 batch-audit publication outcome — the same
/// truthful classification discipline as the plan's.
#[derive(Debug)]
pub enum LedgerV2PublicationOutcome {
    Accepted { id: Hash },
    ReconciledInstalled { id: Hash },
    ReconciledNotInstalled,
    DuplicateVerified { id: Hash },
}

/// (F4-R3) The nonforgeable bound-journal authority capability:
/// issued ONLY by [`verify_bound_journal_authority`] after the full
/// campaign-bound verification (the chain's founding discovered,
/// the installed root binding present, the supplied root's
/// normalized location equal to the binding's authoritative
/// location). The private fields carry the verified identities the
/// authorized publication and collection use — a caller cannot
/// construct one from a path or a founding hash alone.
#[derive(Debug, Clone)]
pub struct BoundJournalAuthority {
    pub(super) founding: Hash,
    pub(super) root_id: Hash,
}

/// (F4-R11/R11-A) The creation operation's typed error: the medium
/// lifecycle verification's refusals and the journal constructor's
/// typed refusals, never conflated.
#[derive(Debug)]
pub enum BoundCreationError {
    Medium(MediumError),
    Journal(crate::journal::JournalError),
}

impl From<MediumError> for BoundCreationError {
    fn from(error: MediumError) -> Self {
        BoundCreationError::Medium(error)
    }
}

/// The completion markers' deterministic idempotency key: the batch
/// marker's (the MK and the batch index) or the range-terminal
/// marker's (the MK and the `range` tag).
pub fn reclamation_complete_idempotency_key(mk: &Hash, batch: Option<u64>) -> Vec<u8> {
    match batch {
        Some(batch) => {
            format!("{RECLAMATION_COMPLETE_KEY_DOMAIN}:{}:{batch}", mk.hex()).into_bytes()
        }
        None => format!("{RECLAMATION_COMPLETE_KEY_DOMAIN}:{}:range", mk.hex()).into_bytes(),
    }
}

/// The authenticated reclamation directive (K2-C2B P2): the audit
/// revision's identity ON THE AUTHORITATIVE CHAIN plus the caller's
/// candidate manifest. The provider re-reads and strictly decodes
/// the ledger UNDER its coordination boundary, cross-checks the
/// supplied units against the decoded manifest (any mismatch
/// refuses typed before any deletion), derives the virtual-exclusion
/// set from the decoded unit RECORDS only, verifies the audit's
/// chain membership and MK-bound idempotency key, and executes the
/// two-pass schedule against the survivor partition. A caller-
/// supplied unit list is never independent authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReclaimDirective {
    /// The audit revision's identity on the authoritative chain.
    pub audit_record: Hash,
    /// The caller's candidate deletion manifest.
    pub units: Vec<ReclaimUnit>,
}

/// The partitioned reclamation outcome: the mandatory identities
/// deleted (the unit records plus the objects no surviving root
/// requires), the deferred identities authorized to survive
/// (shared with surviving roots — the visible debt the completion
/// marker records), and the raw removal count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReclaimOutcome {
    pub removed: u64,
    pub mandatory: Vec<Hash>,
    pub deferred: Vec<Hash>,
}
