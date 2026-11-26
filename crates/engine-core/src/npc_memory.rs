//! The deterministic memory-lane core (the Era-2 NPC-AI design pass §5.3 —
//! the capture lane): the exactly-once memory ledger with capture-before-
//! discard semantics, live representation rebuild, and classification
//! persistence — ALL pure, fully tested, no I/O, no Vulkan, no new
//! dependencies.
//!
//! # DETERMINISM CONTRACT
//! Every answer here is a pure function of (the captured records + the
//! classification decisions): same inputs ⇒ the same state and digests on
//! every platform. Iteration is the canonical total order (BTreeMap key
//! order — never insertion order). All-integer arithmetic (i32/i64) so the
//! libm-free discipline holds trivially. The canonical encoding is fixed-
//! width little-endian at fixed offsets (the `npc_perception` trace pattern
//! extended).
//!
//! # THE ONE RETENTION ORDER (the Era-2 audit's finding 3 corrective)
//! "Newest" for episodic retention is CANONICAL EVENT-KEY ORDER — the
//! largest `budget` `MemoryKey`s (event id leading), ONE order maintained
//! identically by BOTH materialization paths: the live bounded view
//! inserts each captured episodic entry at its key-ordered position and
//! evicts the smallest key at the budget, and the activation rebuild
//! folds `witness_records`' canonical key order through the same
//! retention. A dormancy round trip (or an active-to-active tier
//! transition) therefore cannot change which episode a witness
//! remembers: execution order, reverse event ids, equal ticks and
//! held-events-released-late all collapse to the one canonical order,
//! with the event id as the free deterministic equal-tick tie-breaker.
//!
//! # THE CAPTURE DISCIPLINE
//! Capture-at-production: every produced record enters the ledger at the
//! event itself, so the deactivation's discard of the live representation
//! can never precede a record's capture — I1 by construction. The ledger's
//! local record is pending-live: captured records are authoritative for
//! rebuild in BOTH phases, pending and confirmed alike — the C1 discipline.
//! Confirmation is asynchronous and never blocks anything.
//!
//! # THE POLICY BOUNDARY
//! Which event classes produce which record kinds, the radii, and the
//! classification decisions are game policy authored above. The engine
//! ships the keyed capture/confirm/rebuild machinery, the retention
//! integration, and the shared campaign reputation fold: one global score
//! fed by ACCEPTED classified candidates only — the M-08 model (one shared
//! campaign reputation, no individual/group-average formula). WHICH classes
//! are reputation-relevant is [OPEN] design §9.2.

use crate::clock::Tick;
use crate::fingerprint::Fingerprinter;
use crate::npc_perception::PerceptionTrace;
use crate::wire::{Decoder, SaveError};
use std::collections::{BTreeMap, BTreeSet};

/// The record kind tag (the key's third field).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum MemoryKind {
    /// (a) the witness's episodic-memory entry (the existing trace,
    /// reused verbatim — never a reshaped copy).
    Episodic = 0,
    /// (b) a relationship delta (witness → actor): a signed integer in
    /// policy-authored units.
    Relationship = 1,
    /// (c) a reputation candidate: the actor, the policy-opaque class
    /// tag, and a signed integer weight. PENDING until classified.
    ReputationCandidate = 2,
}

impl MemoryKind {
    /// Parse from a tag byte. Returns `None` for unknown tags.
    #[must_use]
    pub fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            0 => Some(MemoryKind::Episodic),
            1 => Some(MemoryKind::Relationship),
            2 => Some(MemoryKind::ReputationCandidate),
            _ => None,
        }
    }

    /// Returns the tag byte (the canonical encoding).
    #[must_use]
    pub fn tag(self) -> u8 {
        self as u8
    }
}

/// One memory record's idempotency key: the EVENT's key, scoped to the
/// witness, disambiguated by kind (design §5.2: "a captured delta with
/// the event's idempotency key"; the S6-lite harness's mem:/rel:/cand:
/// key family). Derives Ord/PartialOrd over (event, witness, tag) —
/// the canonical total order, never insertion order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MemoryKey {
    /// The event's idempotency key.
    pub event: u64,
    /// The witness's stable identity.
    pub witness: u32,
    /// The record kind tag.
    pub kind: MemoryKind,
}

impl MemoryKey {
    /// One key's canonical byte size: event(u64) + witness(u32) + kind(u8).
    pub const KEY_STRIDE: usize = 13;

    /// The canonical byte encoding: event LE at 0..8, witness LE at 8..12,
    /// kind tag byte at 12.
    #[must_use]
    pub fn canonical_bytes(&self) -> [u8; Self::KEY_STRIDE] {
        let mut bytes = [0u8; Self::KEY_STRIDE];
        bytes[0..8].copy_from_slice(&self.event.to_le_bytes());
        bytes[8..12].copy_from_slice(&self.witness.to_le_bytes());
        bytes[12] = self.kind.tag();
        bytes
    }

    /// The strict decode of [`Self::canonical_bytes`] (the recovery
    /// path's reader — fixed offsets, no trailing bytes, an unknown
    /// kind tag refused, never guessed).
    pub fn decode(bytes: &[u8]) -> Result<Self, SaveError> {
        const SECTION: &str = "memory-key";
        let mut dec = Decoder::new(bytes);
        let event = dec.read_u64()?;
        let witness = dec.read_u32()?;
        let tag = dec.read_u8()?;
        let kind = MemoryKind::from_tag(tag).ok_or(SaveError::Enum {
            section: SECTION,
            discriminant: u64::from(tag),
        })?;
        dec.finish()?;
        Ok(MemoryKey {
            event,
            witness,
            kind,
        })
    }
}

/// One captured memory delta — design §5.3's three writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryDelta {
    /// (a) the witness's episodic-memory entry.
    Episodic { trace: PerceptionTrace },
    /// (b) a relationship delta (witness → actor).
    Relationship { actor: u32, delta: i32 },
    /// (c) a reputation candidate (pending until classified).
    ReputationCandidate { actor: u32, class: u32, weight: i32 },
}

impl MemoryDelta {
    /// The canonical byte size varies by kind: kind tag + fields.
    #[must_use]
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.push(self.kind().tag());
        match self {
            MemoryDelta::Episodic { trace } => {
                bytes.extend_from_slice(&trace.canonical_bytes());
            }
            MemoryDelta::Relationship { actor, delta } => {
                bytes.extend_from_slice(&actor.to_le_bytes());
                bytes.extend_from_slice(&delta.to_le_bytes());
            }
            MemoryDelta::ReputationCandidate {
                actor,
                class,
                weight,
            } => {
                bytes.extend_from_slice(&actor.to_le_bytes());
                bytes.extend_from_slice(&class.to_le_bytes());
                bytes.extend_from_slice(&weight.to_le_bytes());
            }
        }
        bytes
    }

    /// Returns the delta's kind.
    #[must_use]
    pub fn kind(&self) -> MemoryKind {
        match self {
            MemoryDelta::Episodic { .. } => MemoryKind::Episodic,
            MemoryDelta::Relationship { .. } => MemoryKind::Relationship,
            MemoryDelta::ReputationCandidate { .. } => MemoryKind::ReputationCandidate,
        }
    }

    /// Constructor for an episodic delta (builds the trace).
    #[must_use]
    pub fn episodic(observer: u32, perceived: u32, tick: Tick, class: u32) -> Self {
        MemoryDelta::Episodic {
            trace: PerceptionTrace {
                observer,
                perceived,
                tick,
                class,
            },
        }
    }

    /// The strict decode of [`Self::canonical_bytes`] (the recovery
    /// path's reader): the kind tag first, then the kind's fixed-width
    /// little-endian fields in the canonical order, no trailing bytes.
    /// The two's-complement i32 fields decode through `u32 as i32`
    /// (exact, no transcendentals, no endianness ambiguity).
    pub fn decode(bytes: &[u8]) -> Result<Self, SaveError> {
        let mut dec = Decoder::new(bytes);
        let delta = decode_delta(&mut dec)?;
        dec.finish()?;
        Ok(delta)
    }
}

/// One ledger record: the key, the delta, the logical tick of capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryRecord {
    pub key: MemoryKey,
    pub delta: MemoryDelta,
    pub tick: Tick,
}

impl MemoryRecord {
    /// The canonical byte encoding: key bytes, tick LE, delta bytes.
    #[must_use]
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&self.key.canonical_bytes());
        bytes.extend_from_slice(&self.tick.as_u64().to_le_bytes());
        bytes.extend_from_slice(&self.delta.canonical_bytes());
        bytes
    }

    /// Validates the record: delta kind equals key kind; episodic trace's
    /// observer equals key's witness.
    pub fn validate(&self) -> Result<(), MemoryLaneError> {
        if self.key.kind != self.delta.kind() {
            return Err(MemoryLaneError::KindMismatch);
        }
        if let MemoryDelta::Episodic { trace } = &self.delta
            && trace.observer != self.key.witness
        {
            return Err(MemoryLaneError::ObserverMismatch);
        }
        Ok(())
    }

    /// The strict decode of [`Self::canonical_bytes`] (the recovery
    /// path's reader): the key's fixed offsets, the tick, then the
    /// delta — validated, no trailing bytes. A kind or observer
    /// mismatch is an integrity refusal, never a reconstruction.
    pub fn decode(bytes: &[u8]) -> Result<Self, SaveError> {
        const SECTION: &str = "memory-record";
        let mut dec = Decoder::new(bytes);
        let key = MemoryKey {
            event: dec.read_u64()?,
            witness: dec.read_u32()?,
            kind: {
                let tag = dec.read_u8()?;
                MemoryKind::from_tag(tag).ok_or(SaveError::Enum {
                    section: SECTION,
                    discriminant: u64::from(tag),
                })?
            },
        };
        let tick = dec.read_tick()?;
        let delta = decode_delta(&mut dec)?;
        dec.finish()?;
        let record = MemoryRecord { key, delta, tick };
        record
            .validate()
            .map_err(|_| SaveError::Integrity("memory-record/validate"))?;
        Ok(record)
    }
}

/// The delta fields off an in-progress decoder (the shared reader of
/// [`MemoryDelta::decode`] and [`MemoryRecord::decode`] — one field
/// order, one place).
fn decode_delta(dec: &mut Decoder<'_>) -> Result<MemoryDelta, SaveError> {
    const SECTION: &str = "memory-delta";
    let tag = dec.read_u8()?;
    let delta = match MemoryKind::from_tag(tag).ok_or(SaveError::Enum {
        section: SECTION,
        discriminant: u64::from(tag),
    })? {
        MemoryKind::Episodic => MemoryDelta::Episodic {
            trace: PerceptionTrace {
                observer: dec.read_u32()?,
                perceived: dec.read_u32()?,
                tick: dec.read_tick()?,
                class: dec.read_u32()?,
            },
        },
        MemoryKind::Relationship => MemoryDelta::Relationship {
            actor: dec.read_u32()?,
            delta: dec.read_u32()? as i32,
        },
        MemoryKind::ReputationCandidate => MemoryDelta::ReputationCandidate {
            actor: dec.read_u32()?,
            class: dec.read_u32()?,
            weight: dec.read_u32()? as i32,
        },
    };
    Ok(delta)
}

/// Error variants for memory lane operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryLaneError {
    /// The delta's kind does not match the key's kind.
    KindMismatch,
    /// The episodic trace's observer does not equal the key's witness.
    ObserverMismatch,
    /// The record is already classified (terminal candidate).
    AlreadyClassified,
    /// Checked i64 overflow in relationship accumulation.
    ArithmeticOverflow,
}

impl std::fmt::Display for MemoryLaneError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MemoryLaneError::KindMismatch => {
                write!(f, "KindMismatch: delta kind does not equal key kind")
            }
            MemoryLaneError::ObserverMismatch => {
                write!(
                    f,
                    "ObserverMismatch: episodic trace observer != key witness"
                )
            }
            MemoryLaneError::AlreadyClassified => {
                write!(f, "AlreadyClassified: candidate is already terminal")
            }
            MemoryLaneError::ArithmeticOverflow => {
                write!(f, "ArithmeticOverflow: i64 accumulation overflowed")
            }
        }
    }
}

impl std::error::Error for MemoryLaneError {}

/// Capture outcome for [`MemoryLedger::capture`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureOutcome {
    /// New record captured into pending.
    Captured,
    /// Record already in pending (idempotent — no-op).
    AlreadyPending,
    /// Record already in confirmed (idempotent — no-op).
    AlreadyConfirmed,
}

/// Confirm outcome for [`MemoryLedger::confirm`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmOutcome {
    /// Record moved from pending to confirmed.
    Confirmed,
    /// Record already in confirmed.
    AlreadyConfirmed,
    /// Key not found (fail-closed).
    Unknown,
}

/// Classification outcome for [`WitnessMemory::classify`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassifyOutcome {
    /// Candidate classified as Accepted or Rejected.
    Classified,
    /// Candidate already terminal.
    AlreadyClassified,
    /// Key not found.
    Unknown,
}

/// One candidate record in a witness's memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateRecord {
    pub actor: u32,
    pub class: u32,
    pub weight: i32,
    pub phase: CandidatePhase,
}

/// The candidate's classification phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidatePhase {
    /// Pending: not yet classified.
    Pending,
    /// Accepted: classified as positive.
    Accepted,
    /// Rejected: classified as negative.
    Rejected,
}

impl CandidatePhase {
    /// Convert to a tag byte for canonical encoding.
    #[must_use]
    pub fn tag(self) -> u8 {
        match self {
            CandidatePhase::Pending => 0,
            CandidatePhase::Accepted => 1,
            CandidatePhase::Rejected => 2,
        }
    }
}

/// One retained episodic entry in a witness's live bounded view: the
/// record's EVENT ID (the retention order's sort key — the witness and
/// kind are fixed inside one witness's view, so the event id alone IS
/// the canonical `MemoryKey` order here) beside its trace. The entry
/// exists because a bare `PerceptionTrace` does not carry the event id
/// — the one retention order needs the key visible in the live view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EpisodicMemory {
    /// The record's event id (the sort key).
    pub event: u64,
    /// The witnessed trace (the payload).
    pub trace: PerceptionTrace,
}

/// One witness's live memory — the REPRESENTATION (discarded at
/// deactivation, rebuilt from the ledger at activation).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WitnessMemory {
    episodic: Vec<EpisodicMemory>,
    relationships: BTreeMap<u32, i64>,
    candidates: BTreeMap<MemoryKey, CandidateRecord>,
}

/// THE ONE RETENTION ORDER'S LIVE HALF: insert the captured episodic
/// entry at its event-ordered position and evict the SMALLEST event id
/// at the budget — the live bounded view maintains canonical event-key
/// order, exactly the order the activation rebuild folds (the module's
/// determinism contract; the audit's finding 3 corrective). A budget of
/// zero retains nothing; a duplicate event id is a no-op
/// (defense-in-depth — the ledger's capture discipline refuses
/// duplicates before `apply` ever runs).
fn retain_episodic(
    memory: &mut Vec<EpisodicMemory>,
    event: u64,
    trace: PerceptionTrace,
    budget: usize,
) {
    if budget == 0 {
        return;
    }
    match memory.binary_search_by(|entry| entry.event.cmp(&event)) {
        Ok(_) => {}
        Err(pos) => {
            memory.insert(pos, EpisodicMemory { event, trace });
            // Over budget: evict from the FRONT — the smallest event id
            // (a newly captured smallest key that does not fit the
            // budget evicts ITSELF here, never a larger key).
            while memory.len() > budget {
                memory.remove(0);
            }
        }
    }
}

impl WitnessMemory {
    /// Create a new empty WitnessMemory.
    #[must_use]
    pub fn new() -> Self {
        WitnessMemory {
            episodic: Vec::new(),
            relationships: BTreeMap::new(),
            candidates: BTreeMap::new(),
        }
    }

    /// Apply a ledger record to this witness's live memory.
    /// Returns `Err(ArithmeticOverflow)` on checked i64 overflow in
    /// relationship accumulation.
    pub fn apply(
        &mut self,
        record: &MemoryRecord,
        episodic_budget: usize,
    ) -> Result<(), MemoryLaneError> {
        match &record.delta {
            MemoryDelta::Episodic { trace } => {
                retain_episodic(
                    &mut self.episodic,
                    record.key.event,
                    *trace,
                    episodic_budget,
                );
            }
            MemoryDelta::Relationship { actor, delta } => {
                let current = self.relationships.entry(*actor).or_insert(0i64);
                *current = current
                    .checked_add(*delta as i64)
                    .ok_or(MemoryLaneError::ArithmeticOverflow)?;
            }
            MemoryDelta::ReputationCandidate {
                actor,
                class,
                weight,
            } => {
                // A duplicate key insert is a no-op — the ledger's capture
                // discipline already guards this (defense-in-depth).
                let key = MemoryKey {
                    event: record.key.event,
                    witness: record.key.witness,
                    kind: MemoryKind::ReputationCandidate,
                };
                self.candidates
                    .entry(key)
                    .or_insert_with(|| CandidateRecord {
                        actor: *actor,
                        class: *class,
                        weight: *weight,
                        phase: CandidatePhase::Pending,
                    });
            }
        }
        Ok(())
    }

    /// Classify a reputation candidate.
    pub fn classify(&mut self, key: &MemoryKey, accept: bool) -> ClassifyOutcome {
        let candidate = match self.candidates.get_mut(key) {
            Some(c) => c,
            None => return ClassifyOutcome::Unknown,
        };

        match candidate.phase {
            CandidatePhase::Pending => {
                candidate.phase = if accept {
                    CandidatePhase::Accepted
                } else {
                    CandidatePhase::Rejected
                };
                ClassifyOutcome::Classified
            }
            CandidatePhase::Accepted | CandidatePhase::Rejected => {
                ClassifyOutcome::AlreadyClassified
            }
        }
    }

    /// Read-only access to the retained episodic entries, in canonical
    /// event-key order (the one retention order — ascending event id).
    #[must_use]
    pub fn episodic(&self) -> &[EpisodicMemory] {
        &self.episodic
    }

    /// Read-only access to relationships.
    #[must_use]
    pub fn relationships(&self) -> &BTreeMap<u32, i64> {
        &self.relationships
    }

    /// Read-only access to candidates.
    #[must_use]
    pub fn candidates(&self) -> &BTreeMap<MemoryKey, CandidateRecord> {
        &self.candidates
    }

    /// FNV-1a digest over:
    /// - episodic traces' canonical_bytes in canonical event order
    /// - each relationship (actor LE, delta LE) in map order
    /// - each candidate (key bytes, actor, class, weight, phase byte) in map order
    ///
    /// (The episodic entries' event ids are already covered by the
    /// LEDGER digest; the witness digest pins the retained view's
    /// traces and their order.)
    #[must_use]
    pub fn digest(&self) -> u64 {
        let mut hasher = Fingerprinter::new();

        // Episodic traces in canonical event order
        for entry in &self.episodic {
            hasher.write_bytes(&entry.trace.canonical_bytes());
        }

        // Relationships in BTreeMap order
        for (actor, delta) in &self.relationships {
            hasher.write_u32(*actor);
            // The i64's two's-complement little-endian bytes (the
            // `write_u64(as u64)` shape, written via the byte form so
            // this module never extends `Fingerprinter`'s API).
            hasher.write_bytes(&delta.to_le_bytes());
        }

        // Candidates in BTreeMap order
        for (key, candidate) in &self.candidates {
            hasher.write_bytes(&key.canonical_bytes());
            hasher.write_u32(candidate.actor);
            hasher.write_u32(candidate.class);
            hasher.write_bytes(&candidate.weight.to_le_bytes());
            hasher.write_u8(candidate.phase.tag());
        }

        hasher.finish()
    }
}

impl Default for WitnessMemory {
    fn default() -> Self {
        Self::new()
    }
}

/// The capture-before-discard, exactly-once memory ledger: every produced
/// record's local home in BOTH phases. PURE DATA — BTreeMaps keyed by
/// MemoryKey, so iteration is the canonical total order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryLedger {
    pending: BTreeMap<MemoryKey, MemoryRecord>,
    confirmed: BTreeMap<MemoryKey, MemoryRecord>,
}

impl MemoryLedger {
    /// Create a new empty ledger.
    #[must_use]
    pub fn new() -> Self {
        MemoryLedger {
            pending: BTreeMap::new(),
            confirmed: BTreeMap::new(),
        }
    }
}

impl Default for MemoryLedger {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryLedger {
    /// Capture a record: validates, then the idempotent append. THE
    /// FIRST CAPTURE WINS — a duplicate delivery (same key, any
    /// content) is a no-op and the stored record is untouched (the
    /// idempotency-key discipline).
    pub fn capture(&mut self, record: MemoryRecord) -> Result<CaptureOutcome, MemoryLaneError> {
        record.validate()?;

        let key = record.key;
        if self.confirmed.contains_key(&key) {
            return Ok(CaptureOutcome::AlreadyConfirmed);
        }
        if self.pending.contains_key(&key) {
            return Ok(CaptureOutcome::AlreadyPending);
        }
        self.pending.insert(key, record);
        Ok(CaptureOutcome::Captured)
    }

    /// Confirm a pending record: moves it to confirmed.
    pub fn confirm(&mut self, key: &MemoryKey) -> ConfirmOutcome {
        if self.confirmed.contains_key(key) {
            return ConfirmOutcome::AlreadyConfirmed;
        }
        if let Some(record) = self.pending.remove(key) {
            self.confirmed.insert(*key, record);
            return ConfirmOutcome::Confirmed;
        }
        ConfirmOutcome::Unknown
    }

    /// Get a record from either phase.
    #[must_use]
    pub fn record(&self, key: &MemoryKey) -> Option<&MemoryRecord> {
        self.pending.get(key).or_else(|| self.confirmed.get(key))
    }

    /// All records for a witness from BOTH phases, in the CANONICAL
    /// GLOBAL KEY ORDER — a merge of the two key-ordered maps, never
    /// a phase-partitioned concatenation (the rebuild folds these in
    /// order, and the episodic retention keeps the LAST records
    /// applied: only the event order makes "last" mean "newest").
    #[must_use]
    pub fn witness_records(&self, witness: u32) -> Vec<&MemoryRecord> {
        let mut merged: Vec<(MemoryKey, &MemoryRecord)> = self
            .pending
            .iter()
            .chain(self.confirmed.iter())
            .filter(|(k, _)| k.witness == witness)
            .map(|(k, r)| (*k, r))
            .collect();
        merged.sort_unstable_by_key(|(k, _)| *k);
        merged.into_iter().map(|(_, r)| r).collect()
    }

    /// The witness's pending (captured, unconfirmed) keys in canonical
    /// order — the pipeline ride's episode material (which keys a
    /// commit claims).
    #[must_use]
    pub fn pending_keys(&self, witness: u32) -> Vec<MemoryKey> {
        self.witness_records(witness)
            .iter()
            .filter(|r| self.pending.contains_key(&r.key))
            .map(|r| r.key)
            .collect()
    }

    /// Get (pending count, confirmed count) for a witness.
    #[must_use]
    pub fn witness_counts(&self, witness: u32) -> (usize, usize) {
        let pending = self
            .pending
            .values()
            .filter(|r| r.key.witness == witness)
            .count();
        let confirmed = self
            .confirmed
            .values()
            .filter(|r| r.key.witness == witness)
            .count();
        (pending, confirmed)
    }

    /// Check if a key is captured in either phase.
    #[must_use]
    pub fn is_captured(&self, key: &MemoryKey) -> bool {
        self.pending.contains_key(key) || self.confirmed.contains_key(key)
    }

    /// Check if a key is captured in the CONFIRMED phase (the recovery
    /// comparison's per-key phase predicate — `false` for pending and
    /// for absent alike, distinguishing the two needs `is_captured`).
    #[must_use]
    pub fn is_confirmed(&self, key: &MemoryKey) -> bool {
        self.confirmed.contains_key(key)
    }

    /// FNV-1a digest over every record (pending first then confirmed, each
    /// map in key order): key bytes, phase byte (0 pending, 1 confirmed),
    /// then record's canonical bytes.
    #[must_use]
    pub fn digest(&self) -> u64 {
        let mut hasher = Fingerprinter::new();
        for (key, record) in &self.pending {
            hasher.write_bytes(&key.canonical_bytes());
            hasher.write_u8(0); // pending phase
            hasher.write_bytes(&record.canonical_bytes());
        }
        for (key, record) in &self.confirmed {
            hasher.write_bytes(&key.canonical_bytes());
            hasher.write_u8(1); // confirmed phase
            hasher.write_bytes(&record.canonical_bytes());
        }
        hasher.finish()
    }

    /// Iterator over ALL ReputationCandidate records (both phases) in
    /// the canonical global key order — the shared reputation fold's
    /// source (the merge discipline of [`Self::witness_records`], at
    /// lane scope).
    pub fn candidate_records(&self) -> Vec<&MemoryRecord> {
        let mut merged: Vec<(&MemoryKey, &MemoryRecord)> = self
            .pending
            .iter()
            .chain(self.confirmed.iter())
            .filter(|(_, r)| matches!(r.delta, MemoryDelta::ReputationCandidate { .. }))
            .collect();
        merged.sort_unstable_by_key(|(k, _)| *k);
        merged.into_iter().map(|(_, r)| r).collect()
    }
}

/// The memory lane: the ledger (durable, both phases), the durable
/// classification decisions, and the ACTIVE witnesses' live representations.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MemoryLane {
    ledger: MemoryLedger,
    classifications: BTreeMap<MemoryKey, bool>,
    live: BTreeMap<u32, WitnessMemory>,
    /// The ACTIVE witness set: a first-sight capture inserts (the
    /// implicit activation), [`Self::activate`] inserts; membership is
    /// the materialization gate.
    active: BTreeSet<u32>,
    /// The DORMANT witness set: [`Self::deactivate`] moves the witness
    /// here (and out of `active`); [`Self::activate`] moves it back. A
    /// dormant witness's records still capture into the ledger (I1)
    /// but NEVER materialize live — materialization is the rebuild,
    /// so the live view can never silently regress to a ledger subset.
    dormant: BTreeSet<u32>,
    produced: BTreeMap<u32, u64>,
}

impl MemoryLane {
    /// Create a new empty lane.
    #[must_use]
    pub fn new() -> Self {
        MemoryLane::default()
    }

    /// Whether the witness is currently active (live representation
    /// present, or awaiting its first record — the implicit
    /// activation). A dormant witness is not active even though its
    /// records keep capturing.
    #[must_use]
    pub fn is_active(&self, witness: u32) -> bool {
        self.active.contains(&witness)
    }

    /// Whether the witness is currently dormant (deactivated; its
    /// records accumulate in the ledger until [`Self::activate`]).
    #[must_use]
    pub fn is_dormant(&self, witness: u32) -> bool {
        self.dormant.contains(&witness)
    }

    /// THE SINGLE-SOURCED EXACTLY-ONCE EVENT PATH: capture into the
    /// ledger; ONLY ON `Captured` also (i) increment the witness's
    /// produced counter (CHECKED — the conservation's left side never
    /// saturates), (ii) materialize into the live representation IF
    /// the witness is active (first sight inserts — the implicit
    /// activation; a DORMANT witness accumulates in the ledger,
    /// materialized at [`Self::activate`]). A duplicate delivery
    /// (either already-phase outcome) changes NOTHING — live state
    /// included. This single function is I1's exactly-once guard.
    pub fn capture_and_apply(
        &mut self,
        record: MemoryRecord,
        episodic_budget: usize,
    ) -> Result<CaptureOutcome, MemoryLaneError> {
        let key = record.key;
        let witness = key.witness;

        match self.ledger.capture(record)? {
            CaptureOutcome::Captured => {
                let count = self.produced.entry(witness).or_insert(0u64);
                *count = count
                    .checked_add(1)
                    .ok_or(MemoryLaneError::ArithmeticOverflow)?;

                // The materialization gate: active (or first sight —
                // a never-seen witness becomes implicitly active); a
                // dormant witness's record stays ledger-only.
                if !self.dormant.contains(&witness) {
                    self.active.insert(witness);
                    let witness_mem = self.live.entry(witness).or_default();
                    let captured = self.ledger.record(&key).expect("just captured");
                    witness_mem.apply(captured, episodic_budget)?;
                }
                Ok(CaptureOutcome::Captured)
            }
            already @ (CaptureOutcome::AlreadyPending | CaptureOutcome::AlreadyConfirmed) => {
                Ok(already)
            }
        }
    }

    /// Confirm a pending record (the async acknowledgment; never touches live).
    pub fn confirm(&mut self, key: &MemoryKey) -> ConfirmOutcome {
        self.ledger.confirm(key)
    }

    /// Discard the live representation for a witness (the discard
    /// half of capture-before-discard; the capture half happened at
    /// production by construction). The witness moves to the dormant
    /// set. Returns whether a representation existed.
    pub fn deactivate(&mut self, witness: u32) -> bool {
        self.active.remove(&witness);
        self.dormant.insert(witness);
        self.live.remove(&witness).is_some()
    }

    /// THE REBUILD: fold `ledger.witness_records(witness)` in canonical
    /// order into a FRESH WitnessMemory, re-applying the durable
    /// classification decisions to still-Pending candidates (a
    /// classified candidate never regresses to Pending). FAILS CLOSED:
    /// an apply overflow propagates (never swallowed). The witness
    /// leaves the dormant set.
    pub fn activate(
        &mut self,
        witness: u32,
        episodic_budget: usize,
    ) -> Result<usize, MemoryLaneError> {
        let mut fresh = WitnessMemory::new();
        let mut count = 0usize;
        for record in self.ledger.witness_records(witness) {
            fresh.apply(record, episodic_budget)?;
            count += 1;
        }

        // Re-apply the durable classification decisions.
        for (key, candidate) in &mut fresh.candidates {
            if candidate.phase == CandidatePhase::Pending
                && let Some(&accepted) = self.classifications.get(key)
            {
                candidate.phase = if accepted {
                    CandidatePhase::Accepted
                } else {
                    CandidatePhase::Rejected
                };
            }
        }

        self.dormant.remove(&witness);
        self.active.insert(witness);
        self.live.insert(witness, fresh);
        Ok(count)
    }

    /// The durable classification decision for one key, if any (the
    /// journal sync and the recovery comparison's reader).
    #[must_use]
    pub fn classification(&self, key: &MemoryKey) -> Option<bool> {
        self.classifications.get(key).copied()
    }

    /// One witness's durable classification decisions in canonical key
    /// order — the journal's classification-sync section (the
    /// applicable classification history carried with each episode).
    #[must_use]
    pub fn witness_classifications(&self, witness: u32) -> Vec<(MemoryKey, bool)> {
        self.classifications
            .iter()
            .filter(|(key, _)| key.witness == witness)
            .map(|(key, &decision)| (*key, decision))
            .collect()
    }

    /// THE SESSION-REOPEN CAPTURE (the recovery path): capture into the
    /// ledger and hold the witness DORMANT — counted, never
    /// materialized (the materialization is the [`Self::activate`]
    /// rebuild, exactly the captured-before-discard discipline a fresh
    /// session starts from). THE EXACTLY-ONCE GUARD is the ledger's
    /// own: a duplicate key is the matching `Already*` no-op.
    pub fn capture_dormant(
        &mut self,
        record: MemoryRecord,
    ) -> Result<CaptureOutcome, MemoryLaneError> {
        let witness = record.key.witness;
        match self.ledger.capture(record)? {
            CaptureOutcome::Captured => {
                let count = self.produced.entry(witness).or_insert(0u64);
                *count = count
                    .checked_add(1)
                    .ok_or(MemoryLaneError::ArithmeticOverflow)?;
                self.active.remove(&witness);
                self.dormant.insert(witness);
                Ok(CaptureOutcome::Captured)
            }
            already @ (CaptureOutcome::AlreadyPending | CaptureOutcome::AlreadyConfirmed) => {
                Ok(already)
            }
        }
    }

    /// THE DURABLE CLASSIFICATION: record the decision AND apply it to
    /// the live candidate if present. FAILS CLOSED on unknown keys —
    /// a decision is only ever recorded for a CAPTURED candidate (a
    /// phantom decision for a not-yet-captured key must never
    /// pre-classify a future record), and the record must BE a
    /// candidate. Exactly-once: a terminal decision (live phase or
    /// durable) is [`ClassifyOutcome::AlreadyClassified`].
    pub fn classify(&mut self, key: &MemoryKey, accept: bool) -> ClassifyOutcome {
        // The existence gate: the ledger's record (either phase) must
        // be the candidate this key names.
        let Some(record) = self.ledger.record(key) else {
            return ClassifyOutcome::Unknown;
        };
        if record.key.kind != MemoryKind::ReputationCandidate {
            return ClassifyOutcome::Unknown;
        }

        // Already durably decided? Exactly-once, first decision wins.
        if self.classifications.contains_key(key) {
            return ClassifyOutcome::AlreadyClassified;
        }

        self.classifications.insert(*key, accept);
        if let Some(witness_mem) = self.live.get_mut(&key.witness) {
            // The live candidate exists (the ledger gate above plus the
            // materialization discipline: every captured record of an
            // active witness applied live); the outcome propagates
            // anyway — never swallowed, an invariant break is visible.
            return witness_mem.classify(key, accept);
        }
        // The inactive-witness shape: the decision is durably recorded;
        // the rebuild at activate re-applies it.
        ClassifyOutcome::Classified
    }

    /// THE SHARED CAMPAIGN REPUTATION (M-08: one shared score, fed by
    /// classified candidates only — design §5.2/§5.3). The fold over
    /// the LEDGER's ReputationCandidate records in canonical key order: a
    /// candidate whose durable classification decision is ACCEPTED
    /// contributes its weight (checked i64 accumulation — overflow fails
    /// closed); pending, rejected, and never-classified candidates
    /// contribute NOTHING. Durable by construction: the fold reads the
    /// ledger + the classification decisions, never the live
    /// representations — a deactivation cannot change the score.
    pub fn reputation_score(&self) -> Result<i64, MemoryLaneError> {
        let mut score: i64 = 0;
        for record in self.ledger.candidate_records() {
            if self.classifications.get(&record.key) == Some(&true) {
                let weight = match &record.delta {
                    MemoryDelta::ReputationCandidate { weight, .. } => i64::from(*weight),
                    _ => unreachable!("candidate_records filters by kind"),
                };
                score = score
                    .checked_add(weight)
                    .ok_or(MemoryLaneError::ArithmeticOverflow)?;
            }
        }
        Ok(score)
    }

    /// THE I1 ORPHAN IDENTITY: (produced, pending, confirmed) per witness.
    #[must_use]
    pub fn witness_conservation(&self, witness: u32) -> (u64, usize, usize) {
        let produced = *self.produced.get(&witness).unwrap_or(&0);
        let (pending, confirmed) = self.ledger.witness_counts(witness);
        (produced, pending, confirmed)
    }

    /// Access to the underlying ledger.
    #[must_use]
    pub fn ledger(&self) -> &MemoryLedger {
        &self.ledger
    }

    /// Access to the live memory for a witness.
    #[must_use]
    pub fn live(&self, witness: u32) -> Option<&WitnessMemory> {
        self.live.get(&witness)
    }

    /// FNV-1a digest over the whole lane: the ledger digest (u64 LE),
    /// each classification (key bytes, decision byte), each live
    /// witness (witness u32 LE, its digest) — all in map order.
    #[must_use]
    pub fn digest(&self) -> u64 {
        let mut hasher = Fingerprinter::new();
        hasher.write_u64(self.ledger.digest());
        for (key, &decision) in &self.classifications {
            hasher.write_bytes(&key.canonical_bytes());
            hasher.write_u8(u8::from(decision));
        }
        for (&witness, mem) in &self.live {
            hasher.write_u32(witness);
            hasher.write_u64(mem.digest());
        }
        hasher.finish()
    }

    /// THE VERSIONED DIAGNOSTIC DIGEST (finding 7's correction — a
    /// diagnostic BESIDE the compatibility `digest` above, never a
    /// replacement for it; versioned like R1's journal, so a future
    /// field list is a new version tag, never a silent move of this
    /// one): folds everything the compatibility digest omits but
    /// future behavior depends on — the ledger, the classifications,
    /// the live maps (as above), PLUS the ACTIVE and DORMANT
    /// membership sets and the produced counters. Deactivating a
    /// never-seen witness changes later capture MATERIALIZATION
    /// while leaving the compatibility digest identical (the
    /// audit's witness); this digest distinguishes those worlds.
    /// Field order is fixed: the version tag, the ledger digest,
    /// each classification, each live witness, each active member,
    /// each dormant member, each produced counter — map order
    /// throughout.
    #[must_use]
    pub fn diagnostic_digest(&self) -> u64 {
        let mut hasher = Fingerprinter::new();
        hasher.write_str("engine-core/npc-memory-lane-diagnostic/1");
        hasher.write_u64(self.ledger.digest());
        for (key, &decision) in &self.classifications {
            hasher.write_bytes(&key.canonical_bytes());
            hasher.write_u8(u8::from(decision));
        }
        for (&witness, mem) in &self.live {
            hasher.write_u32(witness);
            hasher.write_u64(mem.digest());
        }
        for &witness in &self.active {
            hasher.write_u32(witness);
        }
        for &witness in &self.dormant {
            hasher.write_u32(witness);
        }
        for (&witness, &produced) in &self.produced {
            hasher.write_u32(witness);
            hasher.write_u64(produced);
        }
        hasher.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The canonical Tick constant used in tests.
    const TEST_TICK: Tick = Tick(0x1122_3344_5566_7788);

    /// Helper to create an episodic delta with known trace bytes.
    fn make_episodic(witness: u32, perceived: u32, tick: Tick, class: u32) -> MemoryDelta {
        MemoryDelta::episodic(witness, perceived, tick, class)
    }

    /// Helper to create a relationship delta.
    fn make_relationship(_witness: u32, actor: u32, delta: i32) -> MemoryDelta {
        MemoryDelta::Relationship { actor, delta }
    }

    /// Helper to create a reputation candidate.
    fn make_candidate(_witness: u32, actor: u32, class: u32, weight: i32) -> MemoryDelta {
        MemoryDelta::ReputationCandidate {
            actor,
            class,
            weight,
        }
    }

    /// Create a MemoryKey for testing.
    fn make_key(event: u64, witness: u32, kind: MemoryKind) -> MemoryKey {
        MemoryKey {
            event,
            witness,
            kind,
        }
    }

    /// Create a MemoryRecord for testing.
    fn make_record(
        event: u64,
        witness: u32,
        kind: MemoryKind,
        delta: MemoryDelta,
        tick: Tick,
    ) -> MemoryRecord {
        MemoryRecord {
            key: make_key(event, witness, kind),
            delta,
            tick,
        }
    }

    /// THE DORMANCY DISCRIMINANT (finding 7's deciding test): two
    /// lanes identical except ONE deactivated never-seen witness.
    /// The compatibility `digest` cannot tell them apart (the
    /// ledger, classifications, and live maps are all unchanged —
    /// the audit's witness); the VERSIONED diagnostic digest can.
    /// And the materialization consequence the diagnostic predicts
    /// is real: a subsequent capture for that witness materializes
    /// live in one lane and stays ledger-only in the other.
    #[test]
    fn diagnostic_digest_distinguishes_dormancy_the_compatibility_digest_cannot() {
        let record = |witness: u32| {
            make_record(
                1,
                witness,
                MemoryKind::Episodic,
                make_episodic(witness, 5, TEST_TICK, 7),
                TEST_TICK,
            )
        };
        let mut plain = MemoryLane::new();
        let mut deactivated = MemoryLane::new();
        // A never-seen witness 9 is deactivated in the second lane:
        // no representation existed (the honest `false`), but the
        // dormant membership is now state a future capture obeys.
        assert!(
            !deactivated.deactivate(9),
            "no representation existed to discard"
        );
        // The compatibility digest is IDENTICAL — the exact blind
        // spot the audit named (asserted as the demonstrated BEFORE
        // state, never re-derived anywhere as an equality gate on
        // future behavior).
        assert_eq!(
            plain.digest(),
            deactivated.digest(),
            "the compatibility digest cannot see dormancy membership (the audit's \
             witness — preserved untouched)"
        );
        // The versioned diagnostic digest distinguishes the worlds.
        assert_ne!(
            plain.diagnostic_digest(),
            deactivated.diagnostic_digest(),
            "the diagnostic sees the dormant bit"
        );
        // Purity: the diagnostic is a pure function of the state.
        assert_eq!(
            plain.diagnostic_digest(),
            MemoryLane::new().diagnostic_digest(),
            "two identically-constructed lanes agree"
        );
        // THE MATERIALIZATION CONSEQUENCE the diagnostic predicts:
        // a capture for witness 9 materializes live in the plain
        // lane and stays ledger-only in the deactivated lane.
        plain
            .capture_and_apply(record(9), 8)
            .expect("the record validates");
        deactivated
            .capture_and_apply(record(9), 8)
            .expect("the record validates");
        assert!(plain.live(9).is_some(), "the active witness materializes");
        assert!(
            deactivated.live(9).is_none(),
            "the dormant witness stays ledger-only (materialization waits for \
             activation)"
        );
        assert!(deactivated.is_dormant(9));
        // Post-capture the two lanes differ on the live maps too —
        // both digests now distinguish; the diagnostic did it FIRST,
        // before any capture made the difference visible.
        assert_ne!(plain.digest(), deactivated.digest());
    }

    /// Test 1: key canonical byte layout pinned
    #[test]
    fn key_canonical_byte_layout_pinned() {
        // All-distinct bytes for clear verification
        let key = MemoryKey {
            event: 0x0807_0605_0403_0201,
            witness: 0x0C0B_0A09,
            kind: MemoryKind::Episodic,
        };
        let expected: [u8; MemoryKey::KEY_STRIDE] = [
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, // event LE
            0x09, 0x0A, 0x0B, 0x0C, // witness LE
            0x00, // kind tag (Episodic)
        ];
        assert_eq!(key.canonical_bytes(), expected);
        assert_eq!(key.canonical_bytes().len(), MemoryKey::KEY_STRIDE);

        // Test Relationship
        let key = MemoryKey {
            event: 0x0807_0605_0403_0201,
            witness: 0x0C0B_0A09,
            kind: MemoryKind::Relationship,
        };
        let expected: [u8; MemoryKey::KEY_STRIDE] = [
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C,
            0x01, // kind tag (Relationship)
        ];
        assert_eq!(key.canonical_bytes(), expected);

        // Test ReputationCandidate
        let key = MemoryKey {
            event: 0x0807_0605_0403_0201,
            witness: 0x0C0B_0A09,
            kind: MemoryKind::ReputationCandidate,
        };
        let expected: [u8; MemoryKey::KEY_STRIDE] = [
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C,
            0x02, // kind tag (ReputationCandidate)
        ];
        assert_eq!(key.canonical_bytes(), expected);
    }

    /// Test 2: key order is event then witness then kind
    #[test]
    fn key_order_is_event_then_witness_then_kind() {
        let k1 = make_key(1, 2, MemoryKind::Relationship);
        let k2 = make_key(1, 1, MemoryKind::Episodic);
        let k3 = make_key(2, 1, MemoryKind::ReputationCandidate);

        // k2 < k1 (same event, witness 1 < 2)
        let mut keys = [k1, k2, k3];
        keys.sort();
        assert_eq!(keys[0], k2);
        assert_eq!(keys[1], k1);
        assert_eq!(keys[2], k3);

        // Equal keys are equal
        let k4 = make_key(1, 2, MemoryKind::Relationship);
        assert_eq!(k1, k4);
        assert_ne!(k1, k2);

        // Kind breaks ties
        let k5 = make_key(1, 2, MemoryKind::Episodic);
        let k6 = make_key(1, 2, MemoryKind::Relationship);
        assert!(k5 < k6);
    }

    /// Test 3: delta canonical bytes pinned
    #[test]
    fn delta_canonical_bytes_pinned() {
        // Use the trace from npc_perception's pinned test
        let trace = PerceptionTrace {
            observer: 0x0302_0100,
            perceived: 0x0706_0504,
            tick: Tick(0x1122_3344_5566_7788),
            class: 0x0A0B_0C0D,
        };

        // Episodic delta - kind tag + trace bytes
        let delta = MemoryDelta::Episodic { trace };
        let expected: Vec<u8> = vec![
            0x00, // kind tag (Episodic)
            0x00, 0x01, 0x02, 0x03, // observer LE
            0x04, 0x05, 0x06, 0x07, // perceived LE
            0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11, // tick LE
            0x0D, 0x0C, 0x0B, 0x0A, // class LE
        ];
        assert_eq!(delta.canonical_bytes(), expected);

        // Relationship delta
        let delta = MemoryDelta::Relationship {
            actor: 0x0302_0100,
            delta: 0x0706_0504,
        };
        let expected: Vec<u8> = vec![
            0x01, // kind tag (Relationship)
            0x00, 0x01, 0x02, 0x03, // actor LE
            0x04, 0x05, 0x06, 0x07, // delta LE (i32)
        ];
        assert_eq!(delta.canonical_bytes(), expected);

        // ReputationCandidate delta
        let delta = MemoryDelta::ReputationCandidate {
            actor: 0x0302_0100,
            class: 0x0706_0504,
            weight: 0x0B0A_0908,
        };
        let expected: Vec<u8> = vec![
            0x02, // kind tag (ReputationCandidate)
            0x00, 0x01, 0x02, 0x03, // actor LE
            0x04, 0x05, 0x06, 0x07, // class LE
            0x08, 0x09, 0x0A, 0x0B, // weight LE (i32)
        ];
        assert_eq!(delta.canonical_bytes(), expected);
    }

    /// Test 4: record validation refuses kind and observer mismatches
    #[test]
    fn record_validation_refuses_kind_and_observer_mismatches() {
        let trace = PerceptionTrace {
            observer: 1,
            perceived: 2,
            tick: TEST_TICK,
            class: 3,
        };

        // Valid Episodic record
        let valid = MemoryRecord {
            key: make_key(10, 1, MemoryKind::Episodic),
            delta: MemoryDelta::Episodic { trace },
            tick: TEST_TICK,
        };
        assert!(valid.validate().is_ok());

        // Kind mismatch: Episodic trace under Relationship key
        let kind_mismatch = MemoryRecord {
            key: make_key(10, 1, MemoryKind::Relationship),
            delta: MemoryDelta::Episodic { trace },
            tick: TEST_TICK,
        };
        assert_eq!(kind_mismatch.validate(), Err(MemoryLaneError::KindMismatch));

        // Observer mismatch: trace observer ≠ key witness
        let trace2 = PerceptionTrace {
            observer: 99,
            perceived: 2,
            tick: TEST_TICK,
            class: 3,
        };
        let observer_mismatch = MemoryRecord {
            key: make_key(10, 1, MemoryKind::Episodic),
            delta: MemoryDelta::Episodic { trace: trace2 },
            tick: TEST_TICK,
        };
        assert_eq!(
            observer_mismatch.validate(),
            Err(MemoryLaneError::ObserverMismatch)
        );
    }

    /// Test 5: capture is idempotent and first wins
    #[test]
    fn capture_is_idempotent_and_first_wins() {
        let mut ledger = MemoryLedger::new();

        let trace = PerceptionTrace {
            observer: 1,
            perceived: 2,
            tick: TEST_TICK,
            class: 3,
        };
        let record = MemoryRecord {
            key: make_key(10, 1, MemoryKind::Episodic),
            delta: MemoryDelta::Episodic { trace },
            tick: TEST_TICK,
        };

        // First capture
        assert_eq!(ledger.capture(record).unwrap(), CaptureOutcome::Captured);

        // Same key, different content - idempotent, first wins
        let different = MemoryRecord {
            key: make_key(10, 1, MemoryKind::Episodic),
            delta: MemoryDelta::Episodic {
                trace: PerceptionTrace {
                    observer: 1,
                    perceived: 99,
                    tick: TEST_TICK,
                    class: 3,
                },
            },
            tick: TEST_TICK,
        };
        assert_eq!(
            ledger.capture(different).unwrap(),
            CaptureOutcome::AlreadyPending
        );

        // The stored record is still the first
        let stored = ledger
            .record(&make_key(10, 1, MemoryKind::Episodic))
            .unwrap();
        assert_eq!(
            stored.delta.canonical_bytes(),
            record.delta.canonical_bytes()
        );

        // Confirm
        assert_eq!(
            ledger.confirm(&make_key(10, 1, MemoryKind::Episodic)),
            ConfirmOutcome::Confirmed
        );

        // Third delivery to confirmed
        assert_eq!(ledger.capture(record), Ok(CaptureOutcome::AlreadyConfirmed));
    }

    /// Test 6: confirm moves the record byte-identically
    #[test]
    fn confirm_moves_the_record_byte_identically() {
        let mut ledger = MemoryLedger::new();

        let trace = PerceptionTrace {
            observer: 1,
            perceived: 2,
            tick: TEST_TICK,
            class: 3,
        };
        let record = MemoryRecord {
            key: make_key(10, 1, MemoryKind::Episodic),
            delta: MemoryDelta::Episodic { trace },
            tick: TEST_TICK,
        };
        let key = make_key(10, 1, MemoryKind::Episodic);

        // Capture first
        ledger.capture(record).unwrap();

        // Before confirm
        let before_bytes = record.canonical_bytes();
        assert_eq!(ledger.witness_counts(1), (1, 0));

        // Confirm
        assert_eq!(ledger.confirm(&key), ConfirmOutcome::Confirmed);

        // After confirm - same bytes in confirmed phase
        let stored = ledger.record(&key).unwrap();
        assert_eq!(stored.canonical_bytes(), before_bytes);
        assert_eq!(ledger.witness_counts(1), (0, 1));

        // Re-confirm
        assert_eq!(ledger.confirm(&key), ConfirmOutcome::AlreadyConfirmed);

        // Unknown key
        assert_eq!(
            ledger.confirm(&make_key(99, 1, MemoryKind::Episodic)),
            ConfirmOutcome::Unknown
        );
    }

    /// Test 7: witness records span both phases in key order
    #[test]
    fn witness_records_span_both_phases_in_key_order() {
        let mut ledger = MemoryLedger::new();
        let witness = 7;
        let other_witness = 8;

        // Capture 3 keys for witness 7 (one of each kind)
        let r1 = make_record(
            1,
            witness,
            MemoryKind::Episodic,
            make_episodic(7, 1, TEST_TICK, 1),
            TEST_TICK,
        );
        let r2 = make_record(
            2,
            witness,
            MemoryKind::Relationship,
            make_relationship(7, 1, 5),
            TEST_TICK,
        );
        let r3 = make_record(
            3,
            witness,
            MemoryKind::ReputationCandidate,
            make_candidate(7, 2, 1, 10),
            TEST_TICK,
        );
        let r4 = make_record(
            1,
            other_witness,
            MemoryKind::Episodic,
            make_episodic(8, 1, TEST_TICK, 1),
            TEST_TICK,
        );

        ledger.capture(r1).unwrap();
        ledger.capture(r2).unwrap();
        ledger.capture(r3).unwrap();
        ledger.capture(r4).unwrap();

        // Confirm one
        assert_eq!(
            ledger.confirm(&make_key(2, witness, MemoryKind::Relationship)),
            ConfirmOutcome::Confirmed
        );

        // witness_records(7) is exactly 3 in CANONICAL GLOBAL KEY ORDER —
        // the confirmed r2 (event 2) interleaves BETWEEN the pending r1
        // (event 1) and r3 (event 3): never a phase-partitioned
        // concatenation (the rebuild's retention depends on the event
        // order, not the phase order — THE INTERLEAVE PIN).
        let records = ledger.witness_records(witness);
        assert_eq!(records.len(), 3);
        assert_eq!(records[0].key, r1.key);
        assert_eq!(records[1].key, r2.key);
        assert_eq!(records[2].key, r3.key);

        // Counts: (2 pending, 1 confirmed)
        assert_eq!(ledger.witness_counts(witness), (2, 1));

        // record() finds either phase
        assert!(ledger.record(&r1.key).is_some());
        assert!(ledger.record(&r2.key).is_some());
        assert!(ledger.record(&r3.key).is_some());
    }

    /// Test 8: capture always materializes live (on first sight)
    #[test]
    fn capture_always_materializes_live() {
        let mut lane = MemoryLane::new();
        let witness = 7;

        // First capture creates live memory
        let trace = PerceptionTrace {
            observer: witness,
            perceived: 1,
            tick: TEST_TICK,
            class: 1,
        };
        let r1 = make_record(
            1,
            witness,
            MemoryKind::Episodic,
            MemoryDelta::Episodic { trace },
            TEST_TICK,
        );
        let r2 = make_record(
            2,
            witness,
            MemoryKind::Relationship,
            make_relationship(witness, 1, 5),
            TEST_TICK,
        );
        let r3 = make_record(
            3,
            witness,
            MemoryKind::ReputationCandidate,
            make_candidate(witness, 2, 1, 10),
            TEST_TICK,
        );

        // Capture and apply with references
        assert_eq!(
            lane.capture_and_apply(r1, 16).unwrap(),
            CaptureOutcome::Captured
        );
        assert_eq!(
            lane.capture_and_apply(r2, 16).unwrap(),
            CaptureOutcome::Captured
        );
        assert_eq!(
            lane.capture_and_apply(r3, 16).unwrap(),
            CaptureOutcome::Captured
        );

        // Live memory has all three
        let live = lane.live(witness).unwrap();
        assert_eq!(live.episodic().len(), 1);
        assert_eq!(live.relationships().get(&1), Some(&5));
        assert_eq!(live.candidates().len(), 1);

        // produced counter
        assert_eq!(lane.witness_conservation(witness).0, 3);

        // Duplicate re-delivery: no change
        let digest_before = lane.digest();
        assert_eq!(
            lane.capture_and_apply(r1, 16).unwrap(),
            CaptureOutcome::AlreadyPending
        );
        assert_eq!(
            lane.capture_and_apply(r2, 16).unwrap(),
            CaptureOutcome::AlreadyPending
        );
        assert_eq!(
            lane.capture_and_apply(r3, 16).unwrap(),
            CaptureOutcome::AlreadyPending
        );

        assert_eq!(lane.digest(), digest_before);
    }

    /// Test 9: a DORMANT witness's records accumulate ledger-only and
    /// materialize at activation — THE I1 CAPTURE-BEFORE-DISCARD SHAPE
    /// (the spec's named gate: the discard can never lose a record,
    /// and the live view can never regress to a ledger subset).
    #[test]
    fn inactive_witness_accumulates_and_materializes_at_activation() {
        let mut lane = MemoryLane::new();
        let witness = 7;

        // First sight: implicitly active, materializes live.
        let trace = PerceptionTrace {
            observer: witness,
            perceived: 1,
            tick: TEST_TICK,
            class: 1,
        };
        let r1 = make_record(
            1,
            witness,
            MemoryKind::Episodic,
            MemoryDelta::Episodic { trace },
            TEST_TICK,
        );
        lane.capture_and_apply(r1, 16).unwrap();
        assert!(lane.is_active(witness));
        assert_eq!(lane.live(witness).unwrap().episodic().len(), 1);

        // Deactivate: the representation is discarded, the witness is
        // dormant.
        assert!(lane.deactivate(witness));
        assert!(!lane.is_active(witness));
        assert!(lane.is_dormant(witness));
        assert!(lane.live(witness).is_none());

        // A record delivered while DORMANT: captured (I1 — the local
        // record), counted, but NEVER materialized live.
        let r2 = make_record(
            2,
            witness,
            MemoryKind::Relationship,
            make_relationship(witness, 1, 5),
            TEST_TICK,
        );
        lane.capture_and_apply(r2, 16).unwrap();
        assert!(lane.live(witness).is_none(), "no silent partial live view");
        let (prod, pend, conf) = lane.witness_conservation(witness);
        assert_eq!(prod, 2);
        assert_eq!(pend + conf, 2, "the dormant capture is in the ledger");

        // Activation: the rebuild materializes EVERYTHING — both the
        // pre-deactivation and the dormant-window records.
        let count = lane.activate(witness, 16).unwrap();
        assert_eq!(count, 2);
        assert!(lane.is_active(witness));
        assert!(!lane.is_dormant(witness));
        let live = lane.live(witness).unwrap();
        assert_eq!(live.episodic().len(), 1);
        assert_eq!(live.relationships().get(&1), Some(&5));
    }

    /// Test 10: episodic retention bounds the rebuild
    #[test]
    fn episodic_retention_bounds_the_rebuild() {
        let mut lane = MemoryLane::new();
        let witness = 7;
        let budget = 2;

        // Activate first to create live memory
        lane.activate(witness, budget).unwrap();

        // Capture 3 episodic records (live)
        for i in 0..3 {
            let trace = PerceptionTrace {
                observer: witness,
                perceived: i,
                tick: Tick(i as u64),
                class: 1,
            };
            let r = make_record(
                i as u64,
                witness,
                MemoryKind::Episodic,
                MemoryDelta::Episodic { trace },
                Tick(i as u64),
            );
            lane.capture_and_apply(r, budget).unwrap();
        }

        // Live holds last 2 (oldest evicted)
        let live = lane.live(witness).unwrap();
        assert_eq!(live.episodic().len(), 2);
        assert_eq!(live.episodic()[0].trace.perceived, 1);
        assert_eq!(live.episodic()[1].trace.perceived, 2);

        // Deactivate + activate: rebuild has same last 2
        lane.deactivate(witness);
        let count = lane.activate(witness, budget).unwrap();
        assert_eq!(count, 3); // ledger still has all 3

        let live = lane.live(witness).unwrap();
        assert_eq!(live.episodic().len(), 2);
        assert_eq!(live.episodic()[0].trace.perceived, 1);
        assert_eq!(live.episodic()[1].trace.perceived, 2);

        // Conservation: produced = pending + confirmed
        // We captured 3 records, none confirmed, so all 3 are pending
        let (prod, pend, conf) = lane.witness_conservation(witness);
        assert_eq!(prod, 3);
        assert_eq!(pend, 3);
        assert_eq!(conf, 0);
        assert_eq!(prod as usize, pend + conf); // conservation law
    }

    /// Test 11: relationship accumulation is checked and ordered
    #[test]
    fn relationship_accumulation_is_checked_and_ordered() {
        let mut lane = MemoryLane::new();
        let witness = 7;

        // Activate first to create live memory
        lane.activate(witness, 16).unwrap();

        // Two deltas for same actor accumulate
        let r1 = make_record(
            1,
            witness,
            MemoryKind::Relationship,
            make_relationship(witness, 1, 10),
            TEST_TICK,
        );
        let r2 = make_record(
            2,
            witness,
            MemoryKind::Relationship,
            make_relationship(witness, 1, 20),
            TEST_TICK,
        );

        lane.capture_and_apply(r1, 16).unwrap();
        lane.capture_and_apply(r2, 16).unwrap();

        let live = lane.live(witness).unwrap();
        assert_eq!(live.relationships().get(&1), Some(&30));

        // Different actors stay separate
        let r3 = make_record(
            3,
            witness,
            MemoryKind::Relationship,
            make_relationship(witness, 2, 5),
            TEST_TICK,
        );
        lane.capture_and_apply(r3, 16).unwrap();

        let live = lane.live(witness).unwrap();
        assert_eq!(live.relationships().len(), 2);
        assert_eq!(live.relationships().get(&1), Some(&30));
        assert_eq!(live.relationships().get(&2), Some(&5));

        // Negative delta
        let r4 = make_record(
            4,
            witness,
            MemoryKind::Relationship,
            make_relationship(witness, 1, -5),
            TEST_TICK,
        );
        lane.capture_and_apply(r4, 16).unwrap();

        let live = lane.live(witness).unwrap();
        assert_eq!(live.relationships().get(&1), Some(&25));

        // BTreeMap order: keys in ascending order
        let keys: Vec<_> = live.relationships().keys().copied().collect();
        assert_eq!(keys, vec![1, 2]);
    }

    /// Test 12: deactivate then activate rebuilds byte-identically
    #[test]
    fn deactivate_then_activate_rebuilds_byte_identically() {
        let mut lane = MemoryLane::new();
        let witness = 7;

        // Activate first to create live memory
        lane.activate(witness, 16).unwrap();

        // All three kinds
        let trace = PerceptionTrace {
            observer: witness,
            perceived: 1,
            tick: TEST_TICK,
            class: 1,
        };
        let r1 = make_record(
            1,
            witness,
            MemoryKind::Episodic,
            MemoryDelta::Episodic { trace },
            TEST_TICK,
        );
        let r2 = make_record(
            2,
            witness,
            MemoryKind::Relationship,
            make_relationship(witness, 1, 5),
            TEST_TICK,
        );
        let r3 = make_record(
            3,
            witness,
            MemoryKind::ReputationCandidate,
            make_candidate(witness, 2, 1, 10),
            TEST_TICK,
        );

        lane.capture_and_apply(r1, 16).unwrap();
        lane.capture_and_apply(r2, 16).unwrap();
        lane.capture_and_apply(r3, 16).unwrap();

        // Classify as Accepted
        let cand_key = make_key(3, witness, MemoryKind::ReputationCandidate);
        assert_eq!(lane.classify(&cand_key, true), ClassifyOutcome::Classified);

        // Digest before deactivate
        let live_before = lane.live(witness).unwrap();
        let digest_before = live_before.digest();

        // Deactivate
        assert!(lane.deactivate(witness));

        // Activate
        lane.activate(witness, 16).unwrap();

        // Digest after activate equals before
        let live_after = lane.live(witness).unwrap();
        assert_eq!(live_after.digest(), digest_before);

        // Candidate is Accepted, not Pending
        let candidate = live_after.candidates().get(&cand_key);
        assert_eq!(candidate.unwrap().phase, CandidatePhase::Accepted);

        // Conservation: produced = pending + confirmed
        let (prod, pend, conf) = lane.witness_conservation(witness);
        assert_eq!(
            prod as usize,
            pend + conf,
            "conservation law: produced = pending + confirmed"
        );
    }

    /// Test 13: rebuild never invents — an empty ledger rebuilds an
    /// empty memory (and the empty lane's shared reputation is 0);
    /// a one-record ledger rebuilds exactly that one.
    #[test]
    fn rebuild_never_invents() {
        let mut lane = MemoryLane::new();
        let witness = 7;

        // Empty ledger
        let count = lane.activate(witness, 16).unwrap();
        assert_eq!(count, 0);

        let live = lane.live(witness).unwrap();
        assert_eq!(live.candidates().len(), 0);
        assert_eq!(
            lane.reputation_score().unwrap(),
            0,
            "the empty lane's shared reputation is zero"
        );

        // One record
        let trace = PerceptionTrace {
            observer: witness,
            perceived: 1,
            tick: TEST_TICK,
            class: 1,
        };
        let r1 = make_record(
            1,
            witness,
            MemoryKind::Episodic,
            MemoryDelta::Episodic { trace },
            TEST_TICK,
        );
        lane.capture_and_apply(r1, 16).unwrap();

        lane.deactivate(witness);
        lane.activate(witness, 16).unwrap();

        let live = lane.live(witness).unwrap();
        assert_eq!(live.episodic().len(), 1);
        assert_eq!(live.relationships().len(), 0);
        assert_eq!(live.candidates().len(), 0);
    }

    /// Test 14: classification is exactly once and durable
    #[test]
    fn classification_is_exactly_once_and_durable() {
        let mut lane = MemoryLane::new();
        let witness = 7;
        let cand_key = make_key(1, witness, MemoryKind::ReputationCandidate);
        let cand = make_record(
            1,
            witness,
            MemoryKind::ReputationCandidate,
            make_candidate(witness, 1, 1, 10),
            TEST_TICK,
        );
        lane.capture_and_apply(cand, 16).unwrap();

        // First classify → Classified
        assert_eq!(lane.classify(&cand_key, true), ClassifyOutcome::Classified);

        // Re-classify → AlreadyClassified
        assert_eq!(
            lane.classify(&cand_key, false),
            ClassifyOutcome::AlreadyClassified
        );
        assert_eq!(
            lane.classify(&cand_key, true),
            ClassifyOutcome::AlreadyClassified
        );

        // Decision survives deactivate/reactivate
        lane.deactivate(witness);
        lane.activate(witness, 16).unwrap();

        let live = lane.live(witness).unwrap();
        let candidate = live.candidates().get(&cand_key);
        assert_eq!(candidate.unwrap().phase, CandidatePhase::Accepted);

        // Unknown key
        assert_eq!(
            lane.classify(
                &make_key(99, witness, MemoryKind::ReputationCandidate),
                true
            ),
            ClassifyOutcome::Unknown
        );
    }

    /// Test 15: shared campaign reputation folds accepted only (M-08: one
    /// shared score, fed by classified candidates only). THE §8.4-ITEM-4 GATE.
    #[test]
    fn reputation_score_folds_accepted_only() {
        let mut lane = MemoryLane::new();

        // TWO witnesses: w3 has +5 and -3, w9 has +2
        let w3 = 3;
        let w9 = 9;

        // w3: two candidates
        let c1_w3 = make_record(
            1,
            w3,
            MemoryKind::ReputationCandidate,
            make_candidate(w3, 1, 1, 5),
            TEST_TICK,
        );
        let c2_w3 = make_record(
            2,
            w3,
            MemoryKind::ReputationCandidate,
            make_candidate(w3, 2, 1, -3),
            TEST_TICK,
        );

        // w9: one candidate
        let c1_w9 = make_record(
            1,
            w9,
            MemoryKind::ReputationCandidate,
            make_candidate(w9, 3, 1, 2),
            TEST_TICK,
        );

        lane.capture_and_apply(c1_w3, 16).unwrap();
        lane.capture_and_apply(c2_w3, 16).unwrap();
        lane.capture_and_apply(c1_w9, 16).unwrap();

        // None classified → shared score 0
        assert_eq!(lane.reputation_score().unwrap(), 0);

        // Classify w3 +5 as accepted
        lane.classify(&make_key(1, w3, MemoryKind::ReputationCandidate), true);
        assert_eq!(lane.reputation_score().unwrap(), 5);

        // Classify w3 -3 as rejected
        lane.classify(&make_key(2, w3, MemoryKind::ReputationCandidate), false);
        assert_eq!(lane.reputation_score().unwrap(), 5);

        // Classify w9 +2 as accepted
        lane.classify(&make_key(1, w9, MemoryKind::ReputationCandidate), true);
        assert_eq!(lane.reputation_score().unwrap(), 7);

        // A never-classified 4th changes nothing
        let c4_w3 = make_record(
            4,
            w3,
            MemoryKind::ReputationCandidate,
            make_candidate(w3, 4, 1, 100),
            TEST_TICK,
        );
        lane.capture_and_apply(c4_w3, 16).unwrap();
        assert_eq!(lane.reputation_score().unwrap(), 7);

        // Score unchanged by deactivate/reactivate (the fold is representation-independent)
        lane.deactivate(w3);
        lane.activate(w3, 16).unwrap();
        lane.deactivate(w9);
        lane.activate(w9, 16).unwrap();
        assert_eq!(
            lane.reputation_score().unwrap(),
            7,
            "shared score unchanged by deactivate/reactivate"
        );
    }

    /// Test 16: conservation produced equals pending plus confirmed
    #[test]
    fn conservation_produced_equals_pending_plus_confirmed() {
        let mut lane = MemoryLane::new();
        let w1 = 7;
        let w2 = 8;

        // Mixed script
        // w1: 3 captures, 1 confirm
        let r1 = make_record(
            1,
            w1,
            MemoryKind::Episodic,
            make_episodic(w1, 1, TEST_TICK, 1),
            TEST_TICK,
        );
        let r2 = make_record(
            2,
            w1,
            MemoryKind::Relationship,
            make_relationship(w1, 1, 5),
            TEST_TICK,
        );
        let r3 = make_record(
            3,
            w1,
            MemoryKind::ReputationCandidate,
            make_candidate(w1, 2, 1, 10),
            TEST_TICK,
        );
        lane.capture_and_apply(r1, 16).unwrap();
        lane.capture_and_apply(r2, 16).unwrap();
        lane.capture_and_apply(r3, 16).unwrap();

        assert_eq!(
            lane.confirm(&make_key(1, w1, MemoryKind::Episodic)),
            ConfirmOutcome::Confirmed
        );

        // w2: 1 capture, 0 confirm
        let r5 = make_record(
            1,
            w2,
            MemoryKind::Episodic,
            make_episodic(w2, 1, TEST_TICK, 1),
            TEST_TICK,
        );
        lane.capture_and_apply(r5, 16).unwrap();

        // Duplicate to w1 (no-op)
        lane.capture_and_apply(r1, 16).unwrap();

        // Inactive w3: 1 capture
        let r6 = make_record(
            1,
            9,
            MemoryKind::Episodic,
            make_episodic(9, 1, TEST_TICK, 1),
            TEST_TICK,
        );
        lane.capture_and_apply(r6, 16).unwrap();

        // Check conservation for each witness
        // w1: produced=3, pending=2 (r2, r3), confirmed=1 (r1 confirmed)
        let (prod, pend, conf) = lane.witness_conservation(w1);
        assert_eq!(prod, 3);
        assert_eq!(pend + conf, 3);

        // w2: produced=1, pending=1, confirmed=0
        let (prod, pend, conf) = lane.witness_conservation(w2);
        assert_eq!(prod, 1);
        assert_eq!(pend + conf, 1);

        // w3: produced=1, pending=1, confirmed=0 (inactive)
        let (prod, pend, conf) = lane.witness_conservation(9);
        assert_eq!(prod, 1);
        assert_eq!(pend + conf, 1);
    }

    /// Test 17: lane digest is pure and order sensitive
    #[test]
    fn lane_digest_is_pure_and_order_sensitive() {
        let mut lane = MemoryLane::new();
        let witness = 7;

        let trace = PerceptionTrace {
            observer: witness,
            perceived: 1,
            tick: TEST_TICK,
            class: 1,
        };
        let r1 = make_record(
            1,
            witness,
            MemoryKind::Episodic,
            MemoryDelta::Episodic { trace },
            TEST_TICK,
        );

        lane.capture_and_apply(r1, 16).unwrap();
        let digest1 = lane.digest();

        // Same script twice
        lane.deactivate(witness);
        lane.activate(witness, 16).unwrap();
        assert_eq!(lane.digest(), digest1, "same script produces same digest");

        // One extra record differs
        let r2 = make_record(
            2,
            witness,
            MemoryKind::Relationship,
            make_relationship(witness, 1, 5),
            TEST_TICK,
        );
        lane.capture_and_apply(r2, 16).unwrap();
        assert_ne!(lane.digest(), digest1, "extra record differs");

        // Empty lane digest - measured after ledger digest is included
        // The empty lane's digest includes the ledger's digest (hash of empty pending+confirmed)
        let empty = MemoryLane::new();
        assert_eq!(empty.digest(), 17648789916381521356, "empty lane digest");
    }

    /// Test 18: THE DIGEST PIN — the fixed fixture script's lane
    /// digest, both live-witness digests, and the shared reputation,
    /// each pinned to its measured-actual value (a divergence is an
    /// investigation, never a silent re-pin — the frozen-pin
    /// discipline).
    #[test]
    fn lane_digest_pin() {
        let lane = pin_fixture_lane();

        // The measured-actual FNV-1a values of the fixture script.
        const PINNED_LANE_DIGEST: u64 = 8_539_250_605_216_440_289;
        const PINNED_W1_DIGEST: u64 = 5_025_866_800_409_966_096;
        const PINNED_W2_DIGEST: u64 = 7_657_767_680_175_182_496;
        const PINNED_SHARED_REPUTATION: i64 = 10;

        assert_eq!(lane.digest(), PINNED_LANE_DIGEST, "the lane digest pin");
        assert_eq!(
            lane.live(7).unwrap().digest(),
            PINNED_W1_DIGEST,
            "witness 7's live digest pin"
        );
        assert_eq!(
            lane.live(8).unwrap().digest(),
            PINNED_W2_DIGEST,
            "witness 8's live digest pin"
        );
        assert_eq!(
            lane.reputation_score().unwrap(),
            PINNED_SHARED_REPUTATION,
            "the shared reputation pin (w1's accepted +10 candidate only)"
        );
    }

    /// The pin fixture: two witnesses, five records across all three
    /// kinds, one confirm, one classification, one deactivate/
    /// reactivate cycle — enough shape that any field-order, phase,
    /// or fold-order change moves a digest.
    fn pin_fixture_lane() -> MemoryLane {
        let mut lane = MemoryLane::new();
        let (w1, w2) = (7u32, 8u32);

        let r1 = make_record(
            1,
            w1,
            MemoryKind::Episodic,
            MemoryDelta::Episodic {
                trace: PerceptionTrace {
                    observer: w1,
                    perceived: 1,
                    tick: TEST_TICK,
                    class: 1,
                },
            },
            TEST_TICK,
        );
        let r2 = make_record(
            2,
            w1,
            MemoryKind::Relationship,
            make_relationship(w1, 1, 5),
            TEST_TICK,
        );
        let r3 = make_record(
            3,
            w1,
            MemoryKind::ReputationCandidate,
            make_candidate(w1, 2, 1, 10),
            TEST_TICK,
        );
        let r4 = make_record(
            1,
            w2,
            MemoryKind::Episodic,
            MemoryDelta::Episodic {
                trace: PerceptionTrace {
                    observer: w2,
                    perceived: 2,
                    tick: TEST_TICK,
                    class: 2,
                },
            },
            TEST_TICK,
        );
        let r5 = make_record(
            2,
            w2,
            MemoryKind::ReputationCandidate,
            make_candidate(w2, 3, 2, -3),
            TEST_TICK,
        );

        lane.capture_and_apply(r1, 16).unwrap();
        lane.capture_and_apply(r2, 16).unwrap();
        lane.capture_and_apply(r3, 16).unwrap();
        lane.capture_and_apply(r4, 16).unwrap();
        lane.capture_and_apply(r5, 16).unwrap();

        // One confirm (w1's episodic record — the phase interleave).
        assert_eq!(
            lane.confirm(&make_key(1, w1, MemoryKind::Episodic)),
            ConfirmOutcome::Confirmed
        );

        // One classification: w1's candidate ACCEPTED (the only score
        // contribution); w2's -3 candidate stays pending.
        assert_eq!(
            lane.classify(&make_key(3, w1, MemoryKind::ReputationCandidate), true),
            ClassifyOutcome::Classified
        );

        // One deactivate/reactivate cycle for w1 (the rebuild must
        // reproduce the live digest byte-identically).
        assert!(lane.deactivate(w1));
        assert_eq!(lane.activate(w1, 16).unwrap(), 3);

        lane
    }

    /// Test 19: THE CANONICAL DECODE ROUND-TRIP — every kind's
    /// `canonical_bytes` decodes back to the identical record, and the
    /// strict reader refuses trailing bytes and a foreign kind tag
    /// (the recovery path's reader, pinned at the unit boundary).
    #[test]
    fn canonical_bytes_decode_round_trips_and_refuses() {
        let records = [
            make_record(
                0x0807_0605_0403_0201,
                0x0C0B_0A09,
                MemoryKind::Episodic,
                make_episodic(0x0C0B_0A09, 0x0706_0504, TEST_TICK, 0x0A0B_0C0D),
                TEST_TICK,
            ),
            make_record(
                2,
                7,
                MemoryKind::Relationship,
                make_relationship(7, 0x0302_0100, -0x0706_0504i32),
                TEST_TICK,
            ),
            make_record(
                3,
                7,
                MemoryKind::ReputationCandidate,
                make_candidate(7, 0x0302_0100, 0x0706_0504, -0x0706_0504i32),
                TEST_TICK,
            ),
        ];
        for record in &records {
            let bytes = record.canonical_bytes();
            assert_eq!(
                &MemoryRecord::decode(&bytes).unwrap(),
                record,
                "the round-trip"
            );
            // Trailing bytes are corruption, never noise.
            let mut trailing = bytes.clone();
            trailing.push(0);
            assert!(MemoryRecord::decode(&trailing).is_err(), "trailing refused");
        }
        // A foreign kind tag is refused, never guessed.
        let mut foreign = records[0].canonical_bytes();
        foreign[12] = 0x7F;
        assert!(
            MemoryRecord::decode(&foreign).is_err(),
            "a foreign kind tag refused"
        );
        // A truncated buffer is refused.
        assert!(
            MemoryRecord::decode(&records[0].canonical_bytes()[..5]).is_err(),
            "truncated refused"
        );
    }

    /// Test 20: THE ONE RETENTION ORDER (the audit's finding 3
    /// corrective, at the unit boundary): the live bounded view
    /// maintains canonical event-key order, so a dormancy round trip
    /// cannot change which episode a witness remembers — reverse
    /// event ids, equal ticks and duplicate delivery all hold the one
    /// order at budgets one and two.
    #[test]
    fn the_one_retention_order_survives_the_dormancy_round_trip() {
        for budget in [1usize, 2] {
            // REVERSE IDS: event 20 executes first, event 10 second
            // (the execution stream may deliver an unseen lower id).
            // The canonical order keeps 20 at budget one — the LARGEST
            // key, not the last execution.
            let mut lane = MemoryLane::new();
            let witness = 7u32;
            let e20 = make_record(
                20,
                witness,
                MemoryKind::Episodic,
                make_episodic(witness, 1, Tick(5), 1),
                Tick(5),
            );
            let e10 = make_record(
                10,
                witness,
                MemoryKind::Episodic,
                make_episodic(witness, 2, Tick(6), 1),
                Tick(6),
            );
            lane.capture_and_apply(e20, budget).unwrap();
            lane.capture_and_apply(e10, budget).unwrap();
            let live_before: Vec<EpisodicMemory> = lane.live(witness).unwrap().episodic().to_vec();
            assert_eq!(
                live_before.iter().map(|e| e.event).collect::<Vec<_>>(),
                if budget == 1 { vec![20] } else { vec![10, 20] },
                "budget {budget}: the live view is in canonical key order"
            );
            // The duplicate delivery of event 10 changes nothing.
            lane.capture_and_apply(e10, budget).unwrap();
            assert_eq!(
                lane.live(witness).unwrap().episodic(),
                live_before.as_slice(),
                "budget {budget}: the duplicate delivery is a no-op"
            );
            // The round trip rebuilds the IDENTICAL retained set.
            lane.deactivate(witness);
            lane.activate(witness, budget).unwrap();
            assert_eq!(
                lane.live(witness).unwrap().episodic(),
                live_before.as_slice(),
                "budget {budget}: reverse ids — the rebuild keeps the same episode"
            );

            // EQUAL TICKS: events 31 and 30 both at tick 7, delivered
            // 31 first. The event id is the deterministic tie-breaker
            // in BOTH paths.
            let mut lane = MemoryLane::new();
            let e31 = make_record(
                31,
                witness,
                MemoryKind::Episodic,
                make_episodic(witness, 3, Tick(7), 1),
                Tick(7),
            );
            let e30 = make_record(
                30,
                witness,
                MemoryKind::Episodic,
                make_episodic(witness, 4, Tick(7), 1),
                Tick(7),
            );
            lane.capture_and_apply(e31, budget).unwrap();
            lane.capture_and_apply(e30, budget).unwrap();
            let live_before: Vec<EpisodicMemory> = lane.live(witness).unwrap().episodic().to_vec();
            assert_eq!(
                live_before.iter().map(|e| e.event).collect::<Vec<_>>(),
                if budget == 1 { vec![31] } else { vec![30, 31] },
                "budget {budget}: equal ticks tie-break by event id"
            );
            lane.deactivate(witness);
            lane.activate(witness, budget).unwrap();
            assert_eq!(
                lane.live(witness).unwrap().episodic(),
                live_before.as_slice(),
                "budget {budget}: equal ticks — the rebuild keeps the same episode"
            );

            // THE HELD EVENT RELEASED LATE: event 5 executes at tick
            // 100, long after event 50 at tick 50. Execution-time
            // retention would keep 5; the canonical order keeps 50 in
            // BOTH paths — the divergence this gate pins away.
            let mut lane = MemoryLane::new();
            let e50 = make_record(
                50,
                witness,
                MemoryKind::Episodic,
                make_episodic(witness, 5, Tick(50), 1),
                Tick(50),
            );
            let e5 = make_record(
                5,
                witness,
                MemoryKind::Episodic,
                make_episodic(witness, 6, Tick(100), 1),
                Tick(100),
            );
            lane.capture_and_apply(e50, budget).unwrap();
            lane.capture_and_apply(e5, budget).unwrap();
            let live_before: Vec<EpisodicMemory> = lane.live(witness).unwrap().episodic().to_vec();
            assert_eq!(
                live_before.iter().map(|e| e.event).collect::<Vec<_>>(),
                if budget == 1 { vec![50] } else { vec![5, 50] },
                "budget {budget}: the late-held low id is the smallest key"
            );
            lane.deactivate(witness);
            lane.activate(witness, budget).unwrap();
            assert_eq!(
                lane.live(witness).unwrap().episodic(),
                live_before.as_slice(),
                "budget {budget}: the late release cannot change the remembered episode"
            );
        }
    }

    /// Test 21: the session-reopen capture holds the witness dormant —
    /// counted in the ledger and conservation, never materialized
    /// until activate (the recovery path's capture shape).
    #[test]
    fn capture_dormant_accumulates_ledger_only() {
        let mut lane = MemoryLane::new();
        let witness = 7u32;
        let r1 = make_record(
            1,
            witness,
            MemoryKind::Episodic,
            make_episodic(witness, 1, TEST_TICK, 1),
            TEST_TICK,
        );
        let r2 = make_record(
            2,
            witness,
            MemoryKind::Relationship,
            make_relationship(witness, 1, 5),
            TEST_TICK,
        );
        assert_eq!(lane.capture_dormant(r1).unwrap(), CaptureOutcome::Captured);
        assert_eq!(lane.capture_dormant(r2).unwrap(), CaptureOutcome::Captured);
        assert!(
            lane.is_dormant(witness),
            "the recovered witness starts dormant"
        );
        assert!(lane.live(witness).is_none(), "no silent partial live view");
        assert_eq!(
            lane.capture_dormant(r1).unwrap(),
            CaptureOutcome::AlreadyPending
        );
        let (produced, pending, confirmed) = lane.witness_conservation(witness);
        assert_eq!((produced, pending, confirmed), (2, 2, 0));
        // The rebuild materializes everything, in the one order.
        assert_eq!(lane.activate(witness, 16).unwrap(), 2);
        let live = lane.live(witness).unwrap();
        assert_eq!(live.episodic().len(), 1);
        assert_eq!(live.relationships().get(&1), Some(&5));
    }
}
