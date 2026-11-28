//! The deterministic perception core (the Era-2 NPC-AI design pass §5
//! — the sensing layer): the INVERTED QUERY primitive (which NPCs are
//! within a radius of a point), the tiered perception BUDGET table,
//! and the durable perception TRACE record with its canonical byte
//! encoding — ALL pure, fully tested, no I/O, no unordered iteration,
//! no new dependencies.
//!
//! # DETERMINISM CONTRACT
//! Every answer here is a pure function of (the query, the
//! population slice): same inputs ⇒ the same perceived set, in the
//! same fixed order, byte-identical on every platform. Iteration is
//! the caller's population order (a slice walk — never a `HashMap`'s;
//! §8.3's fixed-evaluation-order rule). The geometry is squared
//! f32 distances — pure IEEE `+`/`*`/comparisons, correctly rounded
//! and cross-platform deterministic for these operations, with a
//! FIXED operation order (dx, then dy, then dx² + dy²); NO sqrt is
//! taken (the `lod.rs` precedent: squared distances choose the
//! answer), so no libm exists anywhere in this module. The trace
//! encoding is `to_le_bytes` in a FIXED field order — the integer
//! form of the animation core's `to_bits` little-endian packing.
//!
//! # THE INVERTED QUERY (design §5.1)
//! Perception is event-side, not NPC-side: a durable-significant
//! event's commit step enumerates its witness set — the NPCs within
//! the event's perception radius — so cost is O(active NPCs in
//! radius) per significant event, zero for the population at large
//! (never N NPCs × M events). The canonical census primitive is the
//! LINEAR, exact form — [`perceive`] — and the simulation substrate
//! holds no persistent spatial partition (the §10.2 verified list;
//! the D-family index below is per-batch composition scratch, never
//! substrate state). The measured-trigger rule holds: any index must
//! preserve BOTH the inversion and the fixed order, or determinism
//! breaks. Tier gating is the caller's
//! composition: a dormant/far NPC is not in any active region's
//! census, so it witnesses nothing (§5.1 — the census this module
//! walks already excludes it).
//!
//! # THE D-FAMILY CENSUS INDEX (P2 L6 — the counted-trigger index)
//! [`perceive`] stays the linear contract and the small-workload
//! fast path, FOREVER (the P2 design §2.2/§10: the linear scan is
//! the cost/benefit optimum at every measured workload — census
//! 0.000–0.403 µs at 24–1000 records). The scaling hazard it hides:
//! census cost grows with population while the event budget does
//! not, and a galaxy-scale world cannot census per event over a
//! dense array. The index answers that hazard WITHOUT touching the
//! linear path: [`CensusIndexScratch::build`] counting-sorts the
//! population by cell (O(N + occupied cells), identity-stable
//! within cells, allocation-stable in the caller-owned scratch),
//! and [`CensusIndex::perceive`] answers the SAME inverted query
//! with the IDENTICAL output contract — population-ordered indices,
//! inclusive boundary, fail-closed radius — by walking the query's
//! cell span row-major (a LOOKUP-ONLY structure: the cell map is
//! never iterated, so its hash order is invisible), collecting the
//! candidates, RE-SORTING them to population order (each record
//! occupies exactly one cell, so the sort is over distinct indices
//! — deterministic), and running the UNCHANGED
//! [`record_in_radius`] narrow phase on each candidate in that
//! order. Because the candidate set is a conservative SUPERSET of
//! the disc's members evaluated in the linear walk's own order,
//! every yield is bit-identical to [`perceive`]'s — the same trick
//! `player.rs`'s P2-L5 static broad phase uses. The
//! composition adopts the index only above the measured gate
//! ([`CENSUS_INDEX_POPULATION_GATE`] population AND
//! [`CENSUS_INDEX_QUERIES_GATE`] census queries per tick — the
//! rebuild must amortize over queries); below either, or whenever
//! the build fail-closes (a non-finite or out-of-domain position
//! poisons the batch) or a query falls outside the index's domain
//! (non-finite center, out-of-domain extent, or the
//! [`CENSUS_CELL_WASTE_FACTOR`] crossover), the exact linear path
//! serves the query — behavior NEVER depends on the index being
//! available. The index is per-tick/per-batch scratch, never
//! persistent: the returned handle borrows both the population and
//! the scratch, so it cannot outlive either and staleness is
//! impossible by construction (drop the handle, mutate, rebuild).
//!
//! # THE POLICY BOUNDARY (design §5.2)
//! Perception classes, radii per class, and line-of-sight rules are
//! game policy. The engine ships the census mechanism (here), the
//! budget table (here), the memory-record schema (the trace, here),
//! and the retention machinery — the future packages wire them.

use std::collections::HashMap;
use std::hash::{BuildHasher, Hasher};

use crate::clock::Tick;
use crate::fingerprint::Fingerprinter;

/// One census record: the stable identity and the continuous
/// world-space position the inverted query measures against.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NpcRecord {
    /// The stable NPC identity (§3's identity facet — never reused;
    /// u32 holds any plausible population with the 1000× clause's
    /// headroom). The census the caller presents is in IDENTITY
    /// order by composition discipline (§8.3) — this module's own
    /// contract is the weaker, sufficient FIXED population order.
    pub id: u32,
    /// The continuous world-space position (§4.3's position bridge:
    /// the near tier's locomotion state carries f32 positions with
    /// fixed operation order; the committed location stays coarse).
    /// A non-finite coordinate makes the record imperceivable (the
    /// fail-closed rule below — NaN can never authorize a witness).
    pub pos: [f32; 2],
}

/// The indices [`perceive`] yields — an iterator over `population`
/// positions, in fixed population order, with NO allocation (a
/// lazily advanced cursor, nothing built).
#[derive(Debug, Clone)]
pub struct PerceivedIndices<'a> {
    population: &'a [NpcRecord],
    center: [f32; 2],
    /// The squared radius, precomputed once (the fixed-operation-
    /// order discipline: `radius · radius` happens exactly once, at
    /// iterator construction — never per record).
    radius_sq: f32,
    /// Whether the radius admits any perception at all (the
    /// fail-closed rule); `false` yields nothing.
    admissible: bool,
    cursor: usize,
}

impl<'a> Iterator for PerceivedIndices<'a> {
    type Item = usize;

    fn next(&mut self) -> Option<usize> {
        if !self.admissible {
            return None; // The fail-closed rule: an inadmissible radius yields nothing.
        }
        while self.cursor < self.population.len() {
            let index = self.cursor;
            self.cursor += 1;
            if record_in_radius(&self.population[index], self.center, self.radius_sq) {
                return Some(index);
            }
        }
        None
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.population.len() - self.cursor;
        (0, Some(remaining))
    }
}

/// Whether one record falls within the precomputed squared radius —
/// the FIXED operation order: dx = pos[0] − center[0], then
/// dy = pos[1] − center[1], then dist_sq = dx·dx + dy·dy, then the
/// single INCLUSIVE comparison `dist_sq ≤ radius_sq`. Pure IEEE f32
/// arithmetic; a non-finite dist_sq (NaN or overflow) compares false
/// — the record is imperceivable, never a panic.
fn record_in_radius(record: &NpcRecord, center: [f32; 2], radius_sq: f32) -> bool {
    let dx = record.pos[0] - center[0];
    let dy = record.pos[1] - center[1];
    let dist_sq = dx * dx + dy * dy;
    dist_sq <= radius_sq
}

/// The fail-closed radius gate, as one shared rule (P2 L6: the
/// linear and indexed census paths must gate IDENTICALLY — one
/// function, never two derivations): admissible iff the radius is
/// finite and non-negative AND its square is finite (a radius whose
/// square overflows f32 perceives nobody). Returns the gate plus
/// the precomputed square — `f32::NEG_INFINITY` when inadmissible
/// (a value that compares false against every finite dist_sq, so
/// the stored square stays well-defined and harmless regardless).
fn admissible_radius_sq(radius: f32) -> (bool, f32) {
    let finite_nonnegative = radius.is_finite() && radius >= 0.0;
    let radius_sq = if finite_nonnegative {
        radius * radius
    } else {
        f32::NEG_INFINITY
    };
    (finite_nonnegative && radius_sq.is_finite(), radius_sq)
}

/// The INVERTED QUERY: which NPCs in `population` are within
/// `radius` of `query_center` — indices into `population`, in FIXED
/// POPULATION ORDER, no allocation.
///
/// THE BOUNDARY IS INCLUSIVE (pinned): a record at squared distance
/// exactly `radius²` IS perceived (`≤`, not `<`) — an NPC standing
/// precisely on the event's perception radius witnesses it. The
/// known-answer tests pin the choice with the exact 3-4-5 diagonal
/// (25.0 = 5.0² bit-exactly in f32), so a flip to exclusive fails a
/// named test, not a witness count.
///
/// THE FAIL-CLOSED RULE: a radius that is negative, non-finite, or
/// whose square overflows to non-finite perceives NOBODY — a
/// degenerate query is a caller's validation bug, never a panic and
/// never an over-perception here (the `lod.rs` non-finite policy:
/// NaN/Infinity cannot authorize an authoritative outcome; the
/// squared-threshold finiteness check is its precedent). Likewise a
/// record at a non-finite position never perceives (its dist_sq
/// compares false).
///
/// THE ORDER RULE: yields are in population order — the caller
/// presents the census in identity order (§8.3), and whatever index
/// later accelerates the census must preserve that order (§5.1's
/// determinism constraint). Indices, not references, for the same
/// one-borrow reason [`crate::npc_schedule::due_commitments`]
/// documents.
pub fn perceive(
    query_center: [f32; 2],
    radius: f32,
    population: &[NpcRecord],
) -> PerceivedIndices<'_> {
    let (admissible, radius_sq) = admissible_radius_sq(radius);
    PerceivedIndices {
        population,
        center: query_center,
        radius_sq,
        admissible,
        cursor: 0,
    }
}

impl PerceivedIndices<'_> {
    /// Whether the query admitted any perception at all (the
    /// fail-closed rule above); exposed so a caller can distinguish
    /// "radius too small" from "radius invalid" at its own policy
    /// layer — the iterator itself simply yields nothing either way.
    #[must_use]
    pub fn is_admissible(&self) -> bool {
        self.admissible
    }
}

// ---------------------------------------------------------------------
// P2 L6 — the D-family census index (the per-tick counting-sort
// compact grid over the census population; see the module docs for
// the bit-identity argument and the design's §2.2.2 for the shape).
// ---------------------------------------------------------------------

/// The census index's population gate (the design's §2.2.3 initial
/// gate, a measurement-pinned placeholder — §9.2's crossover curve
/// is the contract, not this number): below this population the
/// linear census is the cost/benefit optimum FOREVER (the E-8
/// census measurements: 0.000–0.403 µs at 24–1000 records — at the
/// 24-record demo the linear walk is sub-timer-resolution), so the
/// index exists only for the scaling slope the flat E-8 line hides.
/// The rebuild costs one O(N) counting sort; it amortizes only
/// when the population is large enough that the per-query linear
/// walk dominates the per-tick budget.
pub const CENSUS_INDEX_POPULATION_GATE: usize = 512;

/// The census index's queries-per-tick gate (the other half of the
/// design's §2.2.3 rule): the index pays only when
/// (queries per tick) × (linear cost − indexed cost) > rebuild
/// cost. The linear and rebuild costs both grow ∝ N, so the RATIO
/// improves with query count, not population — 4 census queries
/// per tick is the design's initial crossover estimate at the
/// population gate (the measured curve replaces it).
pub const CENSUS_INDEX_QUERIES_GATE: u64 = 4;

/// The census index's cell edge (m) — "cell size ~ query extent"
/// over the census family's radius census (the encounter table's
/// meter-scale radii, the same ~one-decade extent span the P2
/// design records). A power of two, so the
/// `coordinate / CENSUS_CELL_SIZE` division is EXACT in f32 for
/// every normal input (no rounding in the cell mapping — the
/// determinism discipline; the weak-monotonicity of divide-then-
/// floor is what actually guarantees the span's conservative
/// coverage, exactness just keeps it clean). The value is a cost
/// knob only: the index is conservative for ANY cell size, because
/// insertion keys each record's position cell and every query walks
/// the full conservative extent of its own disc.
pub const CENSUS_CELL_SIZE: f32 = 4.0;

/// The cell-index domain bound: |cell| < 2²¹ on every axis (the
/// design's §2.2.6 numerical-domain contract, mirroring the
/// fail-closed radius gate). With [`CENSUS_CELL_SIZE`] this admits
/// world coordinates out to ±8,388,608 m — far beyond any
/// region-local world. A record whose position is non-finite or
/// maps outside this domain poisons the whole batch's build
/// (fail-closed): the composition keeps the exact linear census,
/// so behavior is bit-identical by fallback rather than silently
/// degrading.
pub const CENSUS_CELL_LIMIT: i32 = 1 << 21;

/// The cell-walk crossover factor (the design's §2.6 waste trigger,
/// the same number and argument `player.rs`'s L5 static broad phase
/// carries): when a query's conservative extent spans more than this many
/// cells PER RECORD in the population, the cell walk alone
/// outcosts the linear walk's per-record evaluations, so the query
/// takes the exact linear path. This guard is ALSO what makes the
/// f32 span arithmetic safe in the regime it admits: a perceived
/// record can sit at most ~radius·2⁻²² beyond the span endpoint
/// (the rounding slack of the dx² + dy² ≤ r² comparison chain), a
/// record beyond the span escapes the walk only if that slack
/// crosses a whole cell edge — radius > ~2²²·cell — and the guard
/// confines the grid arm to radius = O(√population), five orders
/// below the escape regime for any realizable population. Beyond
/// the guard, the query falls back to the linear walk,
/// bit-identical by construction.
pub const CENSUS_CELL_WASTE_FACTOR: u64 = 4;

/// The composition's per-batch adoption gate (the design's §2.2.3
/// rule, both halves): the census index pays only when the
/// population reaches [`CENSUS_INDEX_POPULATION_GATE`] AND the
/// expected census query count for the batch reaches
/// [`CENSUS_INDEX_QUERIES_GATE`]. Below either, the exact linear
/// census is the fast path (the charter's retained small-workload
/// path). `queries` is the caller's expected count for the batch
/// the index would serve (per tick, or per population-change batch
/// — the cadence the composition owns).
#[must_use = "a gate that is never checked adopts nothing"]
pub fn census_index_advisable(population_len: usize, queries: u64) -> bool {
    population_len >= CENSUS_INDEX_POPULATION_GATE && queries >= CENSUS_INDEX_QUERIES_GATE
}

/// One census cell key: `floor(x/s), floor(y/s)` with
/// s = [`CENSUS_CELL_SIZE`] (the census is a 2D disc query).
type CensusCellKey = [i32; 2];

/// The cell index of one coordinate: floor(coord /
/// [`CENSUS_CELL_SIZE`]). `None` when the coordinate is non-finite
/// or the cell lies outside the [`CENSUS_CELL_LIMIT`] domain — the
/// caller's fail-closed signal (build: poison; query: linear
/// fallback).
fn census_cell_of(coordinate: f32) -> Option<i32> {
    if !coordinate.is_finite() {
        return None;
    }
    let cell = (coordinate / CENSUS_CELL_SIZE).floor();
    // The f32 compare is exact (integer-valued cells, |cell| < 2^24
    // in-range, and the limit is exactly representable), so the i32
    // conversion below never saturates.
    if cell.abs() >= CENSUS_CELL_LIMIT as f32 {
        return None;
    }
    Some(cell as i32)
}

/// A trivially-cheap deterministic hasher for cell keys (FNV-1a
/// over the key bytes — the same hasher choice `player.rs`'s L5
/// static broad phase carries). The index is lookup-only — never
/// iterated — so the hash choice is behavior-invisible by
/// construction; it exists purely to keep per-cell lookups cheap.
#[derive(Clone, Default)]
struct CensusHasher(u64);

impl CensusHasher {
    /// The FNV-1a offset basis.
    const BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    /// The FNV-1a prime.
    const PRIME: u64 = 0x0000_0100_0000_01b3;
}

impl Hasher for CensusHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            // FNV-1a is DEFINED over wrapping arithmetic (the prime
            // multiply intentionally exceeds u64).
            self.0 = (self.0 ^ u64::from(byte)).wrapping_mul(Self::PRIME);
        }
    }
}

/// Builds [`CensusHasher`]s (see its docs for why the default
/// hasher is replaced).
#[derive(Clone, Default)]
struct CensusHasherBuilder;

impl BuildHasher for CensusHasherBuilder {
    type Hasher = CensusHasher;

    fn build_hasher(&self) -> CensusHasher {
        CensusHasher(CensusHasher::BASIS)
    }
}

/// Caller-owned reusable buffers for the per-tick census index (the
/// F-02 caller-held-scratch discipline: once warm, the per-tick
/// rebuild allocates nothing — every buffer below is cleared and
/// refilled in place, capacities retained). The composition keeps
/// ONE of these for the lifetime of its census lane and calls
/// [`build`](CensusIndexScratch::build) (or
/// [`build_if_advisable`](CensusIndexScratch::build_if_advisable))
/// per tick or per population-change batch.
#[derive(Debug, Default)]
pub struct CensusIndexScratch {
    /// The counting sort's build buffers.
    build: CensusBuildBuffers,
    /// The per-query candidate scratch (the eager walk's output,
    /// re-sorted to population order before the lazy narrow phase).
    candidates: Vec<usize>,
}

/// The counting sort's working memory (all cleared-and-refilled in
/// place per build; the flat layout is a pure function of the
/// population — bucket ids are assigned in FIRST-APPEARANCE order
/// over the population, and the scatter iterates the population in
/// order, so the layout is deterministic and pinnable; the cell map
/// is only ever looked up, never iterated — the module's
/// no-unordered-iteration contract holds).
#[derive(Debug, Default)]
struct CensusBuildBuffers {
    /// Per-record bucket id (pass 1 scratch: population order).
    buckets: Vec<u32>,
    /// Per-bucket `(start, len)` slice into `sorted` — counts
    /// during pass 1, write cursors during the scatter (restored
    /// to starts by the fix-up pass).
    spans: Vec<(u32, u32)>,
    /// The cell-sorted population indices, identity-stable within
    /// each cell's slice.
    sorted: Vec<u32>,
    /// Cell key → bucket id. LOOKUP ONLY.
    map: HashMap<CensusCellKey, u32, CensusHasherBuilder>,
}

impl CensusIndexScratch {
    /// An empty scratch (buffers allocate lazily on first build and
    /// are retained thereafter).
    #[must_use]
    pub fn new() -> CensusIndexScratch {
        CensusIndexScratch::default()
    }

    /// Unconditionally builds the census index over `population`
    /// (the test/battery control — the composition's gated entry is
    /// [`build_if_advisable`](Self::build_if_advisable)). `None`
    /// when any record's position is non-finite or outside the
    /// [`CENSUS_CELL_LIMIT`] domain — the fail-closed poison rule:
    /// the caller keeps the exact linear census for this batch, so
    /// behavior never depends on the index being available. The
    /// returned handle borrows BOTH this scratch and `population`,
    /// so it cannot outlive either — the grid is per-batch, never
    /// persistent, and staleness is impossible by construction
    /// (drop the handle, mutate the population, rebuild). This is
    /// the borrowed-population ownership contract Q02 generalized
    /// across the spatial machinery (`CombatIndex` carries the same
    /// shape); the census landed it first.
    pub fn build<'p, 's>(&'s mut self, population: &'p [NpcRecord]) -> Option<CensusIndex<'p, 's>> {
        let Self { build, candidates } = self;
        if !build_grid(build, population) {
            return None;
        }
        Some(CensusIndex {
            population,
            sorted: &build.sorted,
            spans: &build.spans,
            map: &build.map,
            candidates,
        })
    }

    /// The composition's gated build: [`build`](Self::build) only
    /// when [`census_index_advisable`] says the batch clears both
    /// gate halves (population AND expected queries); otherwise
    /// `None` and the caller runs the exact linear census — the
    /// charter's retained small-workload fast path.
    pub fn build_if_advisable<'p, 's>(
        &'s mut self,
        population: &'p [NpcRecord],
        queries: u64,
    ) -> Option<CensusIndex<'p, 's>> {
        if !census_index_advisable(population.len(), queries) {
            return None;
        }
        self.build(population)
    }

    /// The number of distinct occupied cells in the last successful
    /// build (observability: the resident shape of the index —
    /// occupied cells only, the galaxy rule; no dense world-space
    /// allocation exists anywhere in this structure).
    #[must_use]
    pub fn occupied_cells(&self) -> usize {
        self.build.map.len()
    }
}

/// The counting-sort build over the population. Three passes, all
/// in place over the caller's buffers:
///
/// 1. **Keys and buckets** — per record (in population order):
///    the position's cell key (None poisons the build,
///    fail-closed), a bucket id assigned in FIRST-APPEARANCE order
///    (deterministic; the map is only ever read by `get`), and the
///    per-bucket count.
/// 2. **Prefix starts** — the counts become `(start, len)` slices.
/// 3. **Scatter** — per record (in population order, which makes
///    each cell's slice IDENTITY-STABLE — the counting sort's
///    stability), the population index lands in its cell's slice at
///    the running cursor; the fix-up restores the starts the
///    scatter consumed.
///
/// The flat layout is therefore a pure function of the population
/// slice — no HashMap iteration anywhere (the module contract), so
/// the layout is deterministic on every run and platform.
///
/// Q01 ASSESSMENT (the spatial-machinery hardening lane): this build
/// has NO cell-amplification path — each record occupies exactly
/// ONE cell (the counting sort's premise), so a record's resident
/// contribution is one entry regardless of any extent: the
/// oversized-collider mechanism the static and combatant grids
/// carry is structurally unnecessary here.
fn build_grid(build: &mut CensusBuildBuffers, population: &[NpcRecord]) -> bool {
    build.buckets.clear();
    build.spans.clear();
    build.sorted.clear();
    build.map.clear();
    build.buckets.reserve(population.len());
    build.sorted.reserve(population.len());

    // Pass 1: cell keys, first-appearance bucket ids, counts.
    for record in population {
        // Fail-closed poison: a non-finite or out-of-domain
        // coordinate refuses the whole build.
        let Some(key_x) = census_cell_of(record.pos[0]) else {
            return false;
        };
        let Some(key_y) = census_cell_of(record.pos[1]) else {
            return false;
        };
        let key = [key_x, key_y];
        let bucket = match build.map.get(&key) {
            Some(&bucket) => bucket,
            None => {
                let bucket = build.spans.len() as u32;
                build.map.insert(key, bucket);
                build.spans.push((0, 0));
                bucket
            }
        };
        build.buckets.push(bucket);
        build.spans[bucket as usize].1 += 1;
    }

    // Pass 2: prefix starts from the counts.
    let mut running = 0u32;
    for span in &mut build.spans {
        span.0 = running;
        running += span.1;
    }
    build.sorted.resize(population.len(), 0);

    // Pass 3: the identity-stable scatter (population order), using
    // the starts as write cursors...
    for (index, &bucket) in build.buckets.iter().enumerate() {
        let span = &mut build.spans[bucket as usize];
        build.sorted[span.0 as usize] = index as u32;
        span.0 += 1;
    }
    // ...then the fix-up restores the starts (each cursor now sits
    // at its slice's end; end − len == start).
    for span in &mut build.spans {
        span.0 -= span.1;
    }
    true
}

/// The per-batch census index handle: the LOOKUP-ONLY compact grid
/// over one population snapshot. Queries walk the cells under
/// their own conservative extent in canonical row-major order,
/// collect the candidate indices, re-sort them to POPULATION order
/// (each record occupies exactly one cell and each cell is visited
/// once, so the candidates are distinct and the sort is
/// deterministic), and run the UNCHANGED [`record_in_radius`]
/// narrow phase on each — the design's bit-identity trick: the
/// candidate sequence is exactly the linear walk's iteration order
/// restricted to a conservative SUPERSET of the disc's members.
///
/// The handle borrows the population and the caller's scratch, so
/// it cannot outlive either (per-batch by construction — never
/// persistent, never stale). Every out-of-domain query falls back
/// to the exact linear walk over the SAME population, internally —
/// the caller never branches per query.
#[derive(Debug)]
pub struct CensusIndex<'p, 's> {
    population: &'p [NpcRecord],
    sorted: &'s [u32],
    spans: &'s [(u32, u32)],
    map: &'s HashMap<CensusCellKey, u32, CensusHasherBuilder>,
    /// The per-query candidate scratch (eagerly filled, then read
    /// by the lazy iterator).
    candidates: &'s mut Vec<usize>,
}

impl CensusIndex<'_, '_> {
    /// The population this index was built over (the caller's
    /// census slice — the same slice it passed to
    /// [`CensusIndexScratch::build`], enforced by the borrow).
    #[must_use]
    pub fn population(&self) -> &[NpcRecord] {
        self.population
    }

    /// The number of distinct occupied cells (the resident shape —
    /// observability for tests and measurement).
    #[must_use]
    pub fn occupied_cells(&self) -> usize {
        self.map.len()
    }

    /// The eager half of the query: gathers the candidate set into
    /// this handle's scratch and re-sorts it to POPULATION order,
    /// then reports the shared fail-closed gate so the lazy half
    /// can enforce it. The grid arm walks the query's cell span
    /// row-major (the canonical order — any fixed order would do,
    /// the re-sort restores the contract order) and collects the
    /// candidates; any out-of-domain query drains the EXACT linear
    /// iterator into the same scratch instead (the narrow phase's
    /// re-filter is idempotent over it — the same comparison on
    /// the same values).
    fn gather(&mut self, query_center: [f32; 2], radius: f32) -> (bool, f32) {
        let (admissible, radius_sq) = admissible_radius_sq(radius);
        self.candidates.clear();
        if !admissible {
            return (admissible, radius_sq);
        }
        if let Some(([(lo_cx, hi_cx), (lo_cy, hi_cy)], _)) =
            census_query_span(self.population.len(), query_center, radius)
        {
            for cy in lo_cy..=hi_cy {
                for cx in lo_cx..=hi_cx {
                    if let Some(&bucket) = self.map.get(&[cx, cy]) {
                        let (start, len) = self.spans[bucket as usize];
                        let slice = &self.sorted[start as usize..(start + len) as usize];
                        // Disjoint field borrows: the candidate
                        // scratch is written while the grid fields
                        // are read.
                        self.candidates
                            .extend(slice.iter().map(|&index| index as usize));
                    }
                }
            }
            // Population order restored. Distinct by construction
            // (one cell per record, one visit per cell), so the
            // unstable sort is deterministic.
            self.candidates.sort_unstable();
        } else {
            // The per-query fallback: the exact linear walk.
            self.candidates
                .extend(perceive(query_center, radius, self.population));
        }
        (admissible, radius_sq)
    }

    /// The INDEXED census — the drop-in sibling of [`perceive`]
    /// with the IDENTICAL output contract: which NPCs in the
    /// index's population are within `radius` of `query_center`,
    /// as indices in FIXED POPULATION ORDER (bit-identical to the
    /// linear walk's yields, order included). Eagerly gathers and
    /// re-sorts the candidate set into this handle's scratch (no
    /// per-query allocation), then returns the lazy iterator that
    /// runs the unchanged narrow phase. An inadmissible radius
    /// yields nothing (the shared fail-closed gate); every
    /// out-of-domain query falls back to the exact linear walk,
    /// INTERNALLY — the caller never branches per query.
    pub fn perceive(&mut self, query_center: [f32; 2], radius: f32) -> IndexedPerceived<'_> {
        let (admissible, radius_sq) = self.gather(query_center, radius);
        IndexedPerceived {
            population: self.population,
            candidates: &*self.candidates,
            center: query_center,
            radius_sq,
            admissible,
            cursor: 0,
        }
    }
}

/// The query's walked-cell span and total: the disc's conservative
/// bounding square, per axis `floor((c−r)/s)..=floor((c+r)/s)`
/// (inclusive — the boundary-tight class the inclusive `≤`
/// comparison admits). `None` (the per-query linear fallback
/// signal) when the extent is non-finite, outside the cell domain,
/// absurdly large (the checked-multiply guard), or when the
/// [`CENSUS_CELL_WASTE_FACTOR`] crossover fires — the cell walk
/// would outcost the linear per-record evaluations (see that
/// constant's docs for the f32-span safety argument).
fn census_query_span(
    population_len: usize,
    center: [f32; 2],
    radius: f32,
) -> Option<([(i32, i32); 2], u64)> {
    let lo_x = center[0] - radius;
    let hi_x = center[0] + radius;
    let lo_y = center[1] - radius;
    let hi_y = center[1] + radius;
    // Finiteness on the RAW extent values BEFORE anything else (a
    // NaN center must fall back, never silently clamp).
    if !(lo_x.is_finite() && hi_x.is_finite() && lo_y.is_finite() && hi_y.is_finite()) {
        return None;
    }
    let lo_cx = census_cell_of(lo_x)?;
    let hi_cx = census_cell_of(hi_x)?;
    let lo_cy = census_cell_of(lo_y)?;
    let hi_cy = census_cell_of(hi_y)?;
    let walked_x = (hi_cx as i64 - lo_cx as i64 + 1).max(0) as u64;
    let walked_y = (hi_cy as i64 - lo_cy as i64 + 1).max(0) as u64;
    let walked = walked_x.checked_mul(walked_y)?;
    if walked > CENSUS_CELL_WASTE_FACTOR * population_len as u64 {
        return None;
    }
    Some(([(lo_cx, hi_cx), (lo_cy, hi_cy)], walked))
}

/// The indexed census iterator — [`CensusIndex::perceive`]'s lazy
/// half, mirroring [`PerceivedIndices`]: a cursor over the
/// re-sorted candidate slice, yielding each index that passes the
/// UNCHANGED [`record_in_radius`] narrow phase (the fixed
/// dx/dy/dx²+dy²/≤ operation order — the same code the linear walk
/// runs, so the only possible difference between the paths is WHICH
/// records run it, in what order, and both are the linear walk's
/// own order over a conservative superset).
#[derive(Debug)]
pub struct IndexedPerceived<'a> {
    population: &'a [NpcRecord],
    candidates: &'a [usize],
    center: [f32; 2],
    radius_sq: f32,
    admissible: bool,
    cursor: usize,
}

impl Iterator for IndexedPerceived<'_> {
    type Item = usize;

    fn next(&mut self) -> Option<usize> {
        if !self.admissible {
            return None; // The fail-closed rule: an inadmissible radius yields nothing.
        }
        while self.cursor < self.candidates.len() {
            let index = self.candidates[self.cursor];
            self.cursor += 1;
            if record_in_radius(&self.population[index], self.center, self.radius_sq) {
                return Some(index);
            }
        }
        None
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.candidates.len() - self.cursor;
        (0, Some(remaining))
    }
}

/// The per-tier perception budget table, indexed by LOD level:
/// 0 = Full, 1 = Simplified, 2 = Event-driven, 3 = Dormant (the
/// `lod.rs` [`crate::lod::LodLevel`] ordering, most to least
/// detailed). PURE TABLE — a lookup, never a computation.
///
/// THE VALUES: Full 32, Simplified 8, Event-driven 2, Dormant 0.
/// Dormant = 0 is pinned by design §5.1 ("a dormant/far NPC is not
/// in any active region's witness census, so it witnesses nothing")
/// — a dormant NPC's perception budget is storage, nothing else.
/// The other three are [ASSUMPTION] workload guesses pending the
/// design §8.5 measurement plan (witness enumeration cost per
/// significant event); revisitable only with rig evidence, never
/// silently (the `MAX_INSTANCES` discipline). An out-of-range level
/// (anything past 3) is 0 — fail closed, the budget table's form of
/// the perception fail-closed rule.
pub const PERCEPTION_BUDGETS: [usize; 4] = [32, 8, 2, 0];

/// The perception budget at a LOD level: how many perception
/// records a tier's NPC may produce per opportunity — the cap that
/// keeps a perception storm (a riot) degrading to deferred work
/// within the lane budget, never dropped consequences (design §5.2,
/// the corrected acceptance discipline). See [`PERCEPTION_BUDGETS`]
/// for the tier mapping and each value's evidence key.
#[must_use = "a budget that is never checked caps nothing"]
pub fn perception_budget(lod_level: u8) -> usize {
    PERCEPTION_BUDGETS
        .get(usize::from(lod_level))
        .copied()
        .unwrap_or(0)
}

/// One durable perception trace — WHAT a perception durably records
/// (design §5.3's witness memory entry, the engine-schema half): the
/// observer, the perceived, the logical tick, and the perception
/// class. PURE DATA — all integers, so the libm-free discipline
/// holds trivially and the canonical encoding is exact.
///
/// The trace is the record's episodic-memory lane input; what
/// happens to it (relationship deltas, reputation candidates,
/// retention) is the future capture-ledger package's composition —
/// this module only fixes its shape and its bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PerceptionTrace {
    /// The perceiving (witnessing) NPC's stable identity.
    pub observer: u32,
    /// The perceived NPC's stable identity (the witnessed actor —
    /// for event-side witnesses, the NPC the significant event
    /// concerned).
    pub perceived: u32,
    /// The logical campaign tick the perception was recorded at
    /// (never wall-clock time — the `clock.rs` law).
    pub tick: Tick,
    /// The perception class: an opaque u32 game-policy tag (§5.2 —
    /// which classes are durable-significant, and which are
    /// reputation-relevant, are the owner's open decisions §9.2;
    /// the engine gives the field no meaning).
    pub class: u32,
}

/// One trace's canonical byte size: four little-endian fixed-width
/// integers — observer (4), perceived (4), tick (8), class (4).
pub const TRACE_STRIDE: usize = 20;

impl PerceptionTrace {
    /// The canonical byte encoding: `observer`, `perceived`, `tick`,
    /// `class` — each little-endian at its fixed offset (0, 4, 8,
    /// 16). THE FIXED FIELD ORDER IS THE WIRE CONTRACT: the integer
    /// form of the animation core's `to_bits` little-endian packing;
    /// a reordering is a versioned layout change, never a silent
    /// reshuffle. Byte-identical on every platform for the same
    /// trace (all fields are fixed-width integers).
    #[must_use]
    pub fn canonical_bytes(&self) -> [u8; TRACE_STRIDE] {
        let mut bytes = [0u8; TRACE_STRIDE];
        bytes[0..4].copy_from_slice(&self.observer.to_le_bytes());
        bytes[4..8].copy_from_slice(&self.perceived.to_le_bytes());
        bytes[8..16].copy_from_slice(&self.tick.as_u64().to_le_bytes());
        bytes[16..20].copy_from_slice(&self.class.to_le_bytes());
        bytes
    }
}

/// The FNV-1a 64 digest over a trace sequence: each trace's
/// [`PerceptionTrace::canonical_bytes`] concatenated in slice order,
/// folded through the repo's [`Fingerprinter`] constants. Fixed
/// 20-byte records make the concatenation unambiguous (no
/// length prefixes needed).
///
/// THE CROSS-PLATFORM BYTE PIN: a fixed trace sequence hashes to one
/// u64 everywhere — integer encoding only, no libm exists to vary.
/// A divergence from the pinned test value is an investigation,
/// never a silent re-pin (the frozen-pin discipline).
#[must_use = "a digest that is never compared proves nothing"]
pub fn trace_sequence_digest(traces: &[PerceptionTrace]) -> u64 {
    let mut hasher = Fingerprinter::new();
    for trace in traces.iter() {
        hasher.write_bytes(&trace.canonical_bytes());
    }
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The five-NPC known-answer population: distances 0, 5 (the
    /// 3-4-5 diagonal — exact in f32), 5 (axis point — exact), 10,
    /// and 1 from the origin. Every coordinate is exactly
    /// representable, so every boundary case lands bit-exactly.
    fn population_of_five() -> Vec<NpcRecord> {
        vec![
            NpcRecord {
                id: 0,
                pos: [0.0, 0.0],
            }, // distance 0
            NpcRecord {
                id: 1,
                pos: [3.0, 4.0],
            }, // distance 5 (3-4-5)
            NpcRecord {
                id: 2,
                pos: [5.0, 0.0],
            }, // distance 5 (axis)
            NpcRecord {
                id: 3,
                pos: [10.0, 0.0],
            }, // distance 10
            NpcRecord {
                id: 4,
                pos: [-1.0, 0.0],
            }, // distance 1
        ]
    }

    /// KNOWN ANSWER: radii that include and exclude each of the five
    /// — full inclusion at 10, the boundary pair at 5, near-field at
    /// 2, the exact center at 0 (inclusive at distance zero).
    #[test]
    fn perceive_known_answers() {
        let population = population_of_five();
        assert_eq!(
            perceive([0.0, 0.0], 10.0, &population).collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 4],
            "radius 10 perceives all five"
        );
        assert_eq!(
            perceive([0.0, 0.0], 5.0, &population).collect::<Vec<_>>(),
            vec![0, 1, 2, 4],
            "radius 5 perceives all but the distance-10 record"
        );
        assert_eq!(
            perceive([0.0, 0.0], 2.0, &population).collect::<Vec<_>>(),
            vec![0, 4],
            "radius 2 perceives distances 0 and 1 only"
        );
        assert_eq!(
            perceive([0.0, 0.0], 0.0, &population).collect::<Vec<_>>(),
            vec![0],
            "radius 0 perceives the exact center only (inclusive at 0)"
        );
    }

    /// THE BOUNDARY PIN: an NPC exactly ON the radius IS perceived —
    /// the inclusive choice, pinned by name. The 3-4-5 diagonal and
    /// the axis point both sit at squared distance exactly 25.0 =
    /// 5.0² in f32, so the `≤` comparison is exercised bit-exactly;
    /// a flip to exclusive fails HERE, visibly, not as a witness
    /// count drift.
    #[test]
    fn perceive_boundary_on_radius_is_inclusive() {
        let population = population_of_five();
        let perceived: Vec<usize> = perceive([0.0, 0.0], 5.0, &population).collect();
        assert!(
            perceived.contains(&1) && perceived.contains(&2),
            "records at exactly distance 5 ARE perceived (inclusive boundary): {perceived:?}"
        );
        // Just inside and just outside stay unambiguous: 4.9 excludes
        // the boundary pair, 5.1 includes them.
        assert_eq!(
            perceive([0.0, 0.0], 4.9, &population).collect::<Vec<_>>(),
            vec![0, 4]
        );
        assert_eq!(
            perceive([0.0, 0.0], 5.1, &population).collect::<Vec<_>>(),
            vec![0, 1, 2, 4]
        );
    }

    /// THE ORDER PIN: yields follow POPULATION order, not distance
    /// order — a census authored far-near-mid yields ascending
    /// indices regardless of geometry (§5.1's ordered-iteration
    /// constraint; the identity-order discipline is the caller's
    /// composition, this module's fixedness is its foundation).
    #[test]
    fn perceive_order_is_population_order_not_distance_order() {
        let population = vec![
            NpcRecord {
                id: 7,
                pos: [10.0, 0.0],
            }, // far
            NpcRecord {
                id: 2,
                pos: [0.0, 0.0],
            }, // at center
            NpcRecord {
                id: 9,
                pos: [3.0, 4.0],
            }, // mid (distance 5)
            NpcRecord {
                id: 4,
                pos: [20.0, 0.0],
            }, // farther
        ];
        assert_eq!(
            perceive([0.0, 0.0], 6.0, &population).collect::<Vec<_>>(),
            vec![1, 2],
            "population order [1, 2], never distance order"
        );
    }

    /// THE FAIL-CLOSED PIN: a negative, NaN, infinite, or
    /// square-overflowing radius perceives NOBODY (and reports
    /// inadmissible); a record at a non-finite position is never
    /// perceived even under a huge finite radius. Never a panic,
    /// never an over-perception.
    #[test]
    fn perceive_fail_closed_on_degenerate_radii_and_positions() {
        let population = population_of_five();
        for radius in [-1.0, f32::NAN, f32::INFINITY, 1e30] {
            let mut iter = perceive([0.0, 0.0], radius, &population);
            assert!(
                iter.next().is_none(),
                "radius {radius} perceives nobody (fail closed)"
            );
            assert!(!iter.is_admissible(), "radius {radius} is inadmissible");
        }
        // The in-admissible flag distinguishes nothing-wrong from
        // invalid: a small valid radius is admissible, just empty.
        let quiet = perceive([0.0, 0.0], 0.25, &population);
        assert!(quiet.is_admissible());
        assert_eq!(quiet.collect::<Vec<_>>(), vec![0]);

        // A non-finite position never perceives (NaN compares false;
        // the rule is the comparison's own, not a special case).
        let poisoned = vec![
            NpcRecord {
                id: 0,
                pos: [0.0, 0.0],
            },
            NpcRecord {
                id: 1,
                pos: [f32::NAN, 0.0],
            },
            NpcRecord {
                id: 2,
                pos: [f32::INFINITY, 0.0],
            },
        ];
        assert_eq!(
            perceive([0.0, 0.0], 1e10, &poisoned).collect::<Vec<_>>(),
            vec![0],
            "non-finite positions are imperceivable even at a huge radius"
        );
    }

    /// The query is pure: same inputs, same yields, any number of
    /// runs (and the empty population is a valid, empty census).
    #[test]
    fn perceive_is_pure_and_empty_population_is_quiet() {
        let population = population_of_five();
        let first: Vec<usize> = perceive([1.0, 0.0], 3.0, &population).collect();
        for _ in 0..3 {
            assert_eq!(
                perceive([1.0, 0.0], 3.0, &population).collect::<Vec<_>>(),
                first,
                "byte-identical yields across runs"
            );
        }
        // Center shifted to (1, 0): distances 1, sqrt(20)≈4.47, 4,
        // 9, 2 — radius 3 perceives indices 0 and 4 only.
        assert_eq!(first, vec![0, 4]);
        assert!(
            perceive([0.0, 0.0], 5.0, &[]).next().is_none(),
            "the empty census perceives nobody"
        );
    }

    /// THE BUDGET TABLE: the four tier values, and the fail-closed
    /// out-of-range level (anything past Dormant is 0).
    #[test]
    fn perception_budget_table_values() {
        assert_eq!(perception_budget(0), 32, "Full");
        assert_eq!(perception_budget(1), 8, "Simplified");
        assert_eq!(perception_budget(2), 2, "Event-driven");
        assert_eq!(perception_budget(3), 0, "Dormant — §5.1's pinned zero");
        assert_eq!(PERCEPTION_BUDGETS, [32, 8, 2, 0]);
        for out_of_range in [4u8, 100, 255] {
            assert_eq!(
                perception_budget(out_of_range),
                0,
                "level {out_of_range} fails closed"
            );
        }
    }

    /// THE BYTE-LAYOUT PIN: a trace with every byte distinct encodes
    /// to exactly these 20 little-endian bytes — observer at 0..4,
    /// perceived at 4..8, tick at 8..16 (little-endian u64: the
    /// 0x…88 byte lands FIRST), class at 16..20. A moved byte is an
    /// investigation (a layout change is versioned, never silent).
    #[test]
    fn trace_canonical_byte_layout_pinned() {
        let trace = PerceptionTrace {
            observer: 0x0302_0100,
            perceived: 0x0706_0504,
            tick: Tick(0x1122_3344_5566_7788),
            class: 0x0A0B_0C0D,
        };
        let expected: [u8; TRACE_STRIDE] = [
            // observer 0x03020100, little-endian.
            0x00, 0x01, 0x02, 0x03, // perceived 0x07060504, little-endian.
            0x04, 0x05, 0x06, 0x07, // tick 0x1122334455667788, little-endian.
            0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11,
            // class 0x0A0B0C0D, little-endian.
            0x0D, 0x0C, 0x0B, 0x0A,
        ];
        assert_eq!(trace.canonical_bytes(), expected);
        assert_eq!(trace.canonical_bytes().len(), TRACE_STRIDE);
    }

    /// The digest fixture: three traces spanning the field ranges —
    /// a distinct-byte trace, a small-values trace, and a zero-tick
    /// maximal-class trace (enough shape to make a field-order or
    /// encoding bug visible in the digest).
    fn trace_fixture() -> Vec<PerceptionTrace> {
        vec![
            PerceptionTrace {
                observer: 3,
                perceived: 11,
                tick: Tick(0x1122_3344_5566_7788),
                class: 0x0A0B_0C0D,
            },
            PerceptionTrace {
                observer: 11,
                perceived: 3,
                tick: Tick(99),
                class: 1,
            },
            PerceptionTrace {
                observer: 0,
                perceived: 999,
                tick: Tick(0),
                class: u32::MAX,
            },
        ]
    }

    /// THE DIGEST PIN: the fixture sequence hashes to this FNV-1a 64
    /// value on every platform — fixed-width integer encoding in the
    /// fixed field order, so there is nothing to vary. A moved value
    /// is an investigation (an encoding or order change), never a
    /// silent re-pin.
    #[test]
    fn trace_sequence_digest_pin() {
        assert_eq!(trace_sequence_digest(&trace_fixture()), PINNED_TRACE_DIGEST);
    }

    /// The measured-actual FNV-1a of the fixture trace sequence.
    const PINNED_TRACE_DIGEST: u64 = 10_766_371_085_850_726_021;

    /// The digest is a pure function of the sequence: same traces ⇒
    /// same digest; the empty sequence ⇒ the FNV offset basis; a
    /// reordered sequence differs (the order IS the content — §5.1's
    /// ordered-iteration constraint made assertable).
    #[test]
    fn trace_sequence_digest_is_pure_and_order_sensitive() {
        let traces = trace_fixture();
        assert_eq!(
            trace_sequence_digest(&traces),
            trace_sequence_digest(&trace_fixture()),
            "same sequence, same digest"
        );
        assert_eq!(
            trace_sequence_digest(&[]),
            Fingerprinter::new().finish(),
            "the empty sequence is the offset basis"
        );
        let mut reordered = trace_fixture();
        reordered.swap(0, 1);
        assert_ne!(
            trace_sequence_digest(&reordered),
            trace_sequence_digest(&traces),
            "trace order is significant"
        );
    }

    // -----------------------------------------------------------------
    // P2 L6 — the census-index battery (the design's L6 acceptance
    // row: the equivalence battery below/above the gate, the
    // boundary/on-radius/region-edge classes, the candidate-superset
    // property, the population-order property, the gate boundary,
    // the fail-closed domains, and the scaling evidence).
    // -----------------------------------------------------------------

    /// A deterministic xorshift64* stream for fixture generation
    /// (std-only, seeded per population, so every run of the battery
    /// builds the IDENTICAL population — the no-RNG discipline,
    /// same shape as the L5 battery's).
    struct FixtureRng(u64);

    impl FixtureRng {
        fn next_u32(&mut self) -> u32 {
            debug_assert!(self.0 != 0, "a zero state would freeze the stream");
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 33) as u32
        }

        /// [0, 1) at 24-bit granularity.
        fn unit(&mut self) -> f32 {
            (self.next_u32() >> 8) as f32 / 16_777_216.0
        }

        /// Uniform in [lo, hi].
        fn range(&mut self, lo: f32, hi: f32) -> f32 {
            lo + (hi - lo) * self.unit()
        }
    }

    /// The battery's population shapes (the design's §9.1 density
    /// rows, reduced to the discriminating classes).
    #[derive(Clone, Copy)]
    enum CensusShape {
        /// A jittered 4 m lattice over a square extent that GROWS
        /// with the population (constant density — the "bigger
        /// city" scaling shape; the jitter keeps records off the
        /// cell boundaries, which the boundary shape covers).
        Uniform,
        /// Every record within ±1.5 m of the origin — a handful of
        /// occupied cells regardless of N (the clustered row; a
        /// 100%-candidate query class).
        Clustered,
        /// Eight octant rays of exact 3-4-5 triangles: record t on
        /// each ray sits at distance EXACTLY 5t (bit-exact in f32),
        /// so a radius of exactly 5K puts the whole K-ring ON the
        /// boundary — the boundary-saturation row.
        Ring,
    }

    /// The deterministic fixture population for one (seed, count,
    /// shape).
    fn fixture_population(seed: u64, count: usize, shape: CensusShape) -> Vec<NpcRecord> {
        let mut rng = FixtureRng(seed);
        let mut population = Vec::with_capacity(count);
        for index in 0..count {
            let pos = match shape {
                CensusShape::Uniform => {
                    // Constant density (1 record per 16 m²): the
                    // lattice column count grows ∝ √count, so the
                    // populated extent grows with the population —
                    // the "bigger city" scaling shape.
                    let cols = ((count as f64).sqrt().ceil() as usize).max(1);
                    let x = (index % cols) as f32 * 4.0 + rng.range(-1.5, 1.5);
                    let y = (index / cols) as f32 * 4.0 + rng.range(-1.5, 1.5);
                    [x, y]
                }
                CensusShape::Clustered => [rng.range(-1.5, 1.5), rng.range(-1.5, 1.5)],
                CensusShape::Ring => {
                    let t = (index / 8 + 1) as f32; // distances 5t, bit-exact
                    match index % 8 {
                        0 => [3.0 * t, 4.0 * t],
                        1 => [-3.0 * t, 4.0 * t],
                        2 => [3.0 * t, -4.0 * t],
                        3 => [-3.0 * t, -4.0 * t],
                        4 => [4.0 * t, 3.0 * t],
                        5 => [-4.0 * t, 3.0 * t],
                        6 => [4.0 * t, -3.0 * t],
                        _ => [-4.0 * t, -3.0 * t],
                    }
                }
            };
            population.push(NpcRecord {
                id: index as u32,
                pos,
            });
        }
        population
    }

    /// The query classes the battery runs per population: street
    /// points over the populated extent, exact record positions
    /// (the distance-0 class), the boundary-saturating radius for
    /// ring populations, a cover-everything radius (the waste-guard
    /// regime), the degenerate radii (fail-closed), a NaN center,
    /// and a domain-edge center (the per-query fallback regime).
    fn fixture_queries(
        seed: u64,
        population: &[NpcRecord],
        shape: CensusShape,
    ) -> Vec<([f32; 2], f32)> {
        let mut rng = FixtureRng(seed ^ 0x9E37_79B9_7F4A_7C15);
        let extent = match shape {
            CensusShape::Uniform => ((4.0 * (population.len() as f64).sqrt()) as f32).max(4.0),
            CensusShape::Clustered => 3.0,
            CensusShape::Ring => 5.0 * (population.len() / 8 + 1) as f32,
        };
        let mut queries = Vec::new();
        for _ in 0..24 {
            let center = [rng.range(-extent, extent), rng.range(-extent, extent)];
            for radius in [0.25, 2.0, 5.0, extent / 4.0] {
                queries.push((center, radius));
            }
        }
        // The distance-0 class: query from exact record positions.
        for index in (0..population.len()).step_by(population.len().max(8) / 8) {
            queries.push((population[index].pos, 3.0));
            queries.push((population[index].pos, 0.0));
        }
        // The boundary-saturation class: the exact on-radius ring.
        if let CensusShape::Ring = shape {
            let mid = (population.len() / 16).max(1);
            queries.push(([0.0, 0.0], 5.0 * mid as f32));
            queries.push(([0.0, 0.0], 5.0 * (population.len() / 8) as f32));
        }
        // The cover-everything class (waste-guard regime: the cell
        // walk would outcost the linear walk — internal fallback).
        queries.push(([0.0, 0.0], 1e6));
        // The degenerate radii (fail-closed: nobody, on both paths).
        for radius in [-1.0, f32::NAN, f32::INFINITY, 1e30] {
            queries.push(([0.0, 0.0], radius));
        }
        // The NaN center (the linear walk's own NaN comparison rule).
        queries.push(([f32::NAN, 0.0], 5.0));
        // The domain-edge center (out-of-domain extent: per-query
        // linear fallback).
        let edge = CENSUS_CELL_LIMIT as f32 * CENSUS_CELL_SIZE;
        queries.push(([edge, 0.0], 10.0));
        queries.push(([edge * 0.5, edge * 0.5], 1e5));
        queries
    }

    /// THE L6 EQUIVALENCE BATTERY (the design's acceptance row, the
    /// L5 template): populations BELOW and ABOVE the gate, three
    /// seeds, three distribution shapes — every query through the
    /// indexed census vs the exact linear walk on the IDENTICAL
    /// population, bit-for-bit, order included. The below-gate arm
    /// force-builds the index (the gate otherwise keeps those
    /// populations linear — the composition's documented default);
    /// the built-in per-query fallbacks (waste guard, degenerate
    /// radii, NaN center, domain edge) are exercised INSIDE the
    /// same loop, so every arm of the decision tree is compared.
    #[test]
    fn indexed_census_equivalence_battery() {
        let mut scratch = CensusIndexScratch::new();
        for shape in [
            CensusShape::Uniform,
            CensusShape::Clustered,
            CensusShape::Ring,
        ] {
            for count in [24usize, 125, 300, 512, 2048] {
                for seed in [0x5EED_0001u64, 0x5EED_0010, 0x5EED_0110] {
                    let population = fixture_population(seed, count, shape);
                    let label = format!("{count} @ {seed}");
                    let mut index = scratch
                        .build(&population)
                        .expect("the fixture population is grid-usable");
                    let queries = fixture_queries(seed, &population, shape);
                    for (center, radius) in queries {
                        let expected: Vec<usize> = perceive(center, radius, &population).collect();
                        let got: Vec<usize> = index.perceive(center, radius).collect();
                        assert_eq!(
                            got, expected,
                            "indexed census {label} at {center:?} r{radius}"
                        );
                    }
                }
            }
        }
    }

    /// THE ORDER PIN (the L6 acceptance row's property test): the
    /// indexed census yields POPULATION order, never distance or
    /// cell order — a far-near-mid authored population yields
    /// ascending indices, exactly like the linear pin
    /// `perceive_order_is_population_order_not_distance_order`.
    /// The clustered shape is the adversarial: all records share
    /// cells, so only the candidate re-sort holds the order.
    #[test]
    fn indexed_census_order_is_population_order() {
        let population = vec![
            NpcRecord {
                id: 7,
                pos: [10.0, 0.0],
            }, // far
            NpcRecord {
                id: 2,
                pos: [0.25, 0.25],
            }, // at center
            NpcRecord {
                id: 9,
                pos: [3.0, 4.0],
            }, // mid (distance 5)
            NpcRecord {
                id: 4,
                pos: [20.0, 0.0],
            }, // farther
            NpcRecord {
                id: 1,
                pos: [0.9, 0.1],
            }, // near center, same cell as the mid-near
        ];
        let mut scratch = CensusIndexScratch::new();
        let mut index = scratch.build(&population).expect("grid-usable");
        assert_eq!(
            index.perceive([0.0, 0.0], 6.0).collect::<Vec<_>>(),
            vec![1, 2, 4],
            "population order [1, 2, 4], never distance or cell order"
        );
        // The same pin through a clustered population (one cell).
        let clustered = fixture_population(0xABCD, 64, CensusShape::Clustered);
        let mut index = scratch.build(&clustered).expect("grid-usable");
        let got: Vec<usize> = index.perceive([0.0, 0.0], 2.2).collect();
        let expected: Vec<usize> = perceive([0.0, 0.0], 2.2, &clustered).collect();
        assert_eq!(got, expected, "the clustered shape: identical, ordered");
        assert!(
            got.windows(2).all(|pair| pair[0] < pair[1]),
            "strictly ascending population indices: {got:?}"
        );
    }

    /// THE SUPERSET PROPERTY (the L6 acceptance row): every record
    /// the linear walk yields appears in the indexed path's
    /// candidate set — the property that makes bit-identity work
    /// (the narrow phase then runs the UNCHANGED comparison over
    /// the linear walk's own order). Checked directly against the
    /// handle's internal candidate scratch.
    #[test]
    fn indexed_census_candidate_superset_property() {
        let mut scratch = CensusIndexScratch::new();
        for (count, seed) in [(512usize, 0x5EED_0001u64), (2048, 0x5EED_0110)] {
            let population = fixture_population(seed, count, CensusShape::Uniform);
            let mut index = scratch.build(&population).expect("grid-usable");
            for (center, radius) in fixture_queries(seed, &population, CensusShape::Uniform) {
                let expected: Vec<usize> = perceive(center, radius, &population).collect();
                let _ = index.perceive(center, radius).count();
                for index_yielded in expected {
                    assert!(
                        index.candidates.contains(&index_yielded),
                        "count {count} at {center:?} r{radius}: yielded {index_yielded} \
                         must be a candidate"
                    );
                }
            }
        }
    }

    /// THE LAYOUT PIN: the counting sort's flat layout is a pure
    /// function of the population — bucket ids in FIRST-APPEARANCE
    /// order, each cell's slice identity-stable, spans exact. A
    /// HashMap-iteration-dependent build (the forbidden unordered
    /// iteration) could not pin these values.
    #[test]
    fn indexed_census_counting_sort_layout_pinned() {
        let population = vec![
            NpcRecord {
                id: 0,
                pos: [0.5, 0.5],
            }, // cell (0, 0) — bucket 0, first appearance
            NpcRecord {
                id: 1,
                pos: [4.2, 0.1],
            }, // cell (1, 0) — bucket 1
            NpcRecord {
                id: 2,
                pos: [0.1, 4.1],
            }, // cell (0, 1) — bucket 2
            NpcRecord {
                id: 3,
                pos: [-0.5, -0.5],
            }, // cell (−1, −1) — bucket 3
            NpcRecord {
                id: 4,
                pos: [0.9, 0.9],
            }, // cell (0, 0) — bucket 0 again
        ];
        let mut scratch = CensusIndexScratch::new();
        let index = scratch.build(&population).expect("grid-usable");
        assert_eq!(
            index.sorted,
            vec![0, 4, 1, 2, 3],
            "identity-stable cell slices"
        );
        assert_eq!(
            index.spans,
            vec![(0, 2), (2, 1), (3, 1), (4, 1)],
            "first-appearance buckets, exact starts"
        );
        assert_eq!(index.occupied_cells(), 4);
        assert_eq!(
            index.map.get(&[0, 0]).copied(),
            Some(0),
            "the cell map is lookup-only, keyed correctly"
        );
        // The handle's borrow ends at its last use (NLL): the
        // scratch observation below is legal without an explicit
        // drop — the handle is per-batch by construction.
        assert_eq!(
            scratch.occupied_cells(),
            4,
            "the scratch outlives the handle"
        );
    }

    /// THE GATE BOUNDARY (the L6 acceptance row): the composition's
    /// adoption gate needs BOTH halves — population AND expected
    /// queries. Below either, `build_if_advisable` declines and the
    /// exact linear census is the path (the charter's retained
    /// small-workload fast path — the 24-record demo lane stays
    /// linear forever). At or above both, the build succeeds and
    /// answers bit-identically.
    #[test]
    fn indexed_census_gate_boundary() {
        assert!(
            !census_index_advisable(511, 1_000_000),
            "population 511: below"
        );
        assert!(
            census_index_advisable(512, 1_000_000),
            "population 512: at the gate"
        );
        assert!(!census_index_advisable(512, 3), "3 queries: below");
        assert!(census_index_advisable(512, 4), "4 queries: at the gate");
        assert!(census_index_advisable(100_000, 4));

        let mut scratch = CensusIndexScratch::new();
        // The demo-lane shape (24 records, any query load): linear.
        let small = fixture_population(1, 24, CensusShape::Uniform);
        assert!(
            scratch.build_if_advisable(&small, 1_000_000).is_none(),
            "the 24-record population never adopts the index"
        );
        // Above both halves: the build serves, bit-identically (the
        // handle is scoped so the scratch frees for the next arm).
        let large = fixture_population(2, 600, CensusShape::Uniform);
        {
            let mut index = scratch
                .build_if_advisable(&large, 4)
                .expect("600 population, 4 queries: advisable");
            let expected: Vec<usize> = perceive([0.0, 0.0], 3.0, &large).collect();
            assert_eq!(
                index.perceive([0.0, 0.0], 3.0).collect::<Vec<_>>(),
                expected,
                "the gated build answers bit-identically"
            );
        }
        // The unconditional build serves BELOW the gate too (the
        // battery's force control — the composition's test hook).
        {
            let mut forced = scratch
                .build(&small)
                .expect("the small population is usable");
            assert_eq!(
                forced.perceive([0.0, 0.0], 3.0).collect::<Vec<_>>(),
                perceive([0.0, 0.0], 3.0, &small).collect::<Vec<_>>(),
                "the forced build is bit-identical below the gate"
            );
        }
    }

    /// THE FAIL-CLOSED DOMAINS (the design's §2.2.6 contract): a
    /// non-finite or out-of-domain POSITION poisons the batch's
    /// build (the composition keeps the exact linear census);
    /// out-of-domain QUERIES on a healthy index fall back per
    /// query; and the per-tick cadence — rebuild after population
    /// mutation, scratch reused — stays bit-identical. Behavior
    /// never depends on the index being available.
    #[test]
    fn indexed_census_failclosed_domains_and_rebuild_cadence() {
        let clean = fixture_population(0xFACE, 600, CensusShape::Uniform);
        let mut scratch = CensusIndexScratch::new();
        assert!(scratch.build(&clean).is_some(), "the clean control builds");

        // The build poison classes: one bad record refuses the batch.
        for poison in [
            [f32::NAN, 0.0],
            [0.0, f32::INFINITY],
            [CENSUS_CELL_LIMIT as f32 * CENSUS_CELL_SIZE, 0.0],
            [-CENSUS_CELL_LIMIT as f32 * CENSUS_CELL_SIZE * 2.0, 0.0],
        ] {
            let mut poisoned = clean.clone();
            poisoned[7].pos = poison;
            assert!(
                scratch.build(&poisoned).is_none(),
                "position {poison:?} poisons the build (fail-closed)"
            );
        }
        // The poisoned population still censuses through the linear
        // path — the composition's documented fallback — and the
        // linear answers are the authority: the poisoned record was
        // perceivable before (the control) and never perceives
        // after, with everyone else unaffected.
        let clean_yields: Vec<usize> = perceive([0.0, 0.0], 3.0, &clean).collect();
        assert!(
            clean_yields.contains(&0),
            "the control: record 0 is perceivable before the poison"
        );
        let mut poisoned = clean.clone();
        poisoned[0].pos = [f32::NAN, 0.0];
        let expected: Vec<usize> = perceive([0.0, 0.0], 3.0, &poisoned).collect();
        assert!(
            !expected.contains(&0),
            "the NaN-position record never perceives"
        );
        assert_eq!(
            expected,
            clean_yields
                .iter()
                .copied()
                .filter(|&index| index != 0)
                .collect::<Vec<_>>(),
            "everyone else is unaffected — the linear census is the fallback and the authority"
        );

        // The per-query fallback classes on a HEALTHY index: each
        // answers bit-identically to the linear walk.
        let mut index = scratch.build(&clean).expect("the clean control builds");
        for (center, radius) in [
            ([f32::NAN, 0.0], 5.0),                                     // NaN center
            ([0.0, f32::NAN], 5.0),                                     // NaN center, other axis
            ([0.0, 0.0], 1e6),                                          // waste-guard regime
            ([CENSUS_CELL_LIMIT as f32 * CENSUS_CELL_SIZE, 0.0], 10.0), // domain-edge extent
            ([0.0, 0.0], -0.5),                                         // negative radius
            ([0.0, 0.0], 1e30),                                         // square-overflow radius
        ] {
            assert_eq!(
                index.perceive(center, radius).collect::<Vec<_>>(),
                perceive(center, radius, &clean).collect::<Vec<_>>(),
                "the fallback class {center:?} r{radius} answers identically"
            );
        }

        // THE REBUILD CADENCE (the per-tick rule): the handle's
        // borrow ends at its last use above (NLL), the population
        // mutates (the locomotion analogue), and the rebuild over
        // the SAME scratch (buffers retained) answers
        // bit-identically to the linear walk on the MUTATED
        // population — never the stale one.
        let mut moved = clean;
        for record in &mut moved {
            record.pos[0] += 8.5;
            record.pos[1] -= 0.25;
        }
        let mut rebuilt = scratch.build(&moved).expect("the moved population builds");
        assert_eq!(
            rebuilt.perceive([8.5, -0.25], 3.0).collect::<Vec<_>>(),
            perceive([8.5, -0.25], 3.0, &moved).collect::<Vec<_>>(),
            "the rebuilt index tracks the mutated population"
        );
        let expected: Vec<usize> = perceive([0.0, 0.0], 3.0, &moved).collect();
        assert_eq!(
            rebuilt.perceive([0.0, 0.0], 3.0).collect::<Vec<_>>(),
            expected,
            "the old center answers for the moved population, identically"
        );
    }

    /// THE L6 SCALING EVIDENCE (the design's §9.2 measurement
    /// shape, observational): constant-density populations at
    /// multiple sizes, the indexed query vs the linear walk vs the
    /// warm per-tick rebuild, with least-squares slopes (ns per 1k
    /// records) and the amortized crossover query count. Printed,
    /// not gated: generous sanity bounds only (the F-06
    /// no-thresholds-before-variance rule). CAVEAT, stated honestly:
    /// this runs under the UNOPTIMIZED test profile — the slopes
    /// and the indexed/linear RATIO are the evidence here; absolute
    /// constants belong to the release-mode rig matrix (§9, the
    /// measurement owner). The claim being evidenced: the indexed
    /// query cost is FLAT in population at constant density (the
    /// linear walk's cost grows ∝ N), so the index exists for the
    /// scaling slope the flat E-8 tick line hides. Every timed
    /// query is also bit-compared against the linear path inline.
    #[test]
    fn census_index_scaling_evidence() {
        fn mean_ns(runs: u64, mut run: impl FnMut()) -> f64 {
            let start = std::time::Instant::now();
            for _ in 0..runs {
                run();
            }
            start.elapsed().as_secs_f64() / runs as f64 * 1e9
        }
        // Least-squares slope, ns per 1k records.
        let slope = |points: &[(usize, f64)]| -> f64 {
            let n = points.len() as f64;
            let sum_x: f64 = points.iter().map(|(x, _)| *x as f64).sum();
            let sum_y: f64 = points.iter().map(|(_, y)| *y).sum();
            let sum_xy: f64 = points.iter().map(|(x, y)| *x as f64 * y).sum();
            let sum_xx: f64 = points.iter().map(|(x, _)| (*x as f64) * (*x as f64)).sum();
            (n * sum_xy - sum_x * sum_y) / (n * sum_xx - sum_x * sum_x) * 1000.0
        };

        let mut scratch = CensusIndexScratch::new();
        let sizes = [512usize, 2048, 8192, 32768, 131072];
        let mut indexed_series: Vec<(usize, f64)> = Vec::new();
        let mut linear_series: Vec<(usize, f64)> = Vec::new();
        let mut rebuild_series: Vec<(usize, f64)> = Vec::new();
        for size in sizes {
            let population =
                fixture_population(0xC1E5_0000 + size as u64, size, CensusShape::Uniform);
            // The timed query: a near-origin street point, radius
            // 2 m (the encounter-table scale).
            let center = [4.0, 4.0];
            let radius = 2.0;

            // The warm per-tick rebuild (the counting sort over the
            // population, buffers retained — one cold build outside
            // the timer, then the per-tick cadence: build, drop).
            let _ = scratch.build(&population).expect("grid-usable");
            let rebuilds = (400_000 / size as u64).clamp(4, 500);
            let rebuild_ns = mean_ns(rebuilds, || {
                let _ = scratch.build(&population).expect("rebuild stays usable");
            });

            // The query handle for the timed queries.
            let mut index = scratch.build(&population).expect("grid-usable");

            // Inline bit-identity on the timed query (the battery's
            // claim, restated on the timed population).
            let expected: Vec<usize> = perceive(center, radius, &population).collect();
            assert_eq!(
                index.perceive(center, radius).collect::<Vec<_>>(),
                expected,
                "size {size}: the timed query is bit-identical"
            );

            let queries = (600_000 / size as u64).clamp(50, 2000);
            let indexed_ns = mean_ns(queries, || {
                let _ = index.perceive(center, radius).count();
            });
            let linear_ns = mean_ns(queries, || {
                let _ = perceive(center, radius, &population).count();
            });
            let witnesses = expected.len();
            let candidates = index.candidates.len();
            println!(
                "[census-index-scaling] pop {size:6} · cells {} · witnesses {witnesses} · \
                 candidates {candidates} · ns/query indexed {:9.1} vs linear {:9.1} · \
                 rebuild {:9.1}",
                index.occupied_cells(),
                indexed_ns,
                linear_ns,
                rebuild_ns
            );
            indexed_series.push((size, indexed_ns));
            linear_series.push((size, linear_ns));
            rebuild_series.push((size, rebuild_ns));
            // Sanity bounds only (characterization, not a gate). The
            // query arms are flat and keep the absolute bound; the
            // rebuild arm is linear in population (measured ~230 ns
            // per record serial, ~520 ns under concurrent load on
            // the rig, 2026-11-27), so its bound carries a per-record
            // term: a fixed bound trips on the arm's own scaling
            // (68 ms at 131,072 under load) while a quadratic blowup
            // would still clear it by orders of magnitude.
            for ns in [indexed_ns, linear_ns] {
                assert!(
                    ns.is_finite() && ns < 50_000_000.0,
                    "size {size}: a query arm is absurdly slow: {ns} ns"
                );
            }
            assert!(
                rebuild_ns.is_finite() && rebuild_ns < 5_000_000.0 + size as f64 * 2_000.0,
                "size {size}: the rebuild arm is absurdly slow: {rebuild_ns} ns"
            );
        }

        let indexed_slope = slope(&indexed_series);
        let linear_slope = slope(&linear_series);
        let rebuild_slope = slope(&rebuild_series);
        println!(
            "[census-index-scaling] slope per 1k records — indexed query {indexed_slope:9.1} \
             ns · linear query {linear_slope:9.1} ns · rebuild {rebuild_slope:9.1} ns"
        );
        // The amortized crossover at each size: the query count
        // where rebuild + q·indexed < q·linear (the gate's shape).
        for ((size, indexed), (_, linear)) in indexed_series.iter().zip(&linear_series) {
            let (_, rebuild) = rebuild_series
                .iter()
                .find(|&&(s, _)| s == *size)
                .expect("the rebuild series is parallel");
            if linear > indexed {
                let crossover = rebuild / (linear - indexed);
                println!(
                    "[census-index-scaling] pop {size:6} · crossover ≈ {crossover:7.1} \
                     queries per batch (rebuild {rebuild:8.1} ns / saved {per_query:8.1} ns)",
                    per_query = linear - indexed
                );
            }
        }
        // The directional claim, generously bounded (not a gate):
        // at constant density the indexed query is flat in
        // population while the linear walk grows — the indexed
        // slope must sit far below the linear slope.
        assert!(
            indexed_slope < linear_slope / 4.0,
            "the indexed query slope ({indexed_slope} ns/1k) must sit far below the \
             linear slope ({linear_slope} ns/1k) at constant density"
        );
    }
}
