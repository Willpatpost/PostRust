//! (P3-L5, B02) THE STREAMING READER'S DIFFERENTIAL ORACLE — the
//! L5 lane's mandatory test shape: the production streaming
//! scan/open/refresh paths proven BYTE-IDENTICAL to the whole-file
//! reader over an adversarial corpus. The whole-file reader is
//! preserved in-tree, compile-time isolated to tests, as the
//! reference (`scan_file_whole_file`, `scan_bound_file_whole_file`,
//! and the whole-file `classify`/`classify_bound`/
//! `strict_bound_state` they drive); this module is the corpus
//! generator and the equivalence assertions.
//!
//! **Why in-module:** the corpus builds its fixtures through the
//! writer's own encoders and the crate-private bound-generation
//! constructor (whose crate privacy is a documented trust contract
//! — no external caller can manufacture a generation), so the
//! oracle and the generator both live inside the crate's test
//! build; the production API grows by nothing (the streaming
//! reader itself is crate-private).
//!
//! **The corpus:** valid journals (legacy and bound, built through
//! the writer — real chains, real digests), then the damage
//! families the design names: truncation at EVERY frame boundary
//! and on a stride across every offset (torn length prefixes, torn
//! records, header-torn), zero-filled tails (the documented
//! filesystem artifact), single byte flips across the post-header
//! region (record-chain damage, decode failures, damaged envelopes,
//! and every fail-closed commit-field rule — sequence, count,
//! digest, identity set, identity derivation, duplicate id),
//! garbage-frame splices at frame boundaries (damage within
//! committed history), trailing-garbage and wrap-window tails (a
//! `u64::MAX` declaration), giant declared frames above
//! [`MAX_JOURNAL_RECORD_BYTES`] with the bytes PRESENT (the
//! unbuffered skip path, including the fail-closed
//! damage-within-committed-history shape through it), and the
//! header-length corruption window where a shorter probe would
//! have changed the refusal's detail string. Every fixture runs
//! through BOTH readers; states compare by equality, errors by
//! their complete debug and display surfaces.

use super::*;
use crate::consequence::{AwardCheckpoint, Coverage, OccurrenceSpan};
use crate::envelope::{ClassifiedSpan, CounterValues, SignalLane, SourceBaseline};
use std::sync::atomic::{AtomicU64, Ordering};

/// One corpus fixture: the raw journal bytes (the scan paths are
/// read-only, so both readers observe identical bytes after one
/// write).
#[derive(Clone)]
struct Fixture {
    label: String,
    bytes: Vec<u8>,
}

impl Fixture {
    fn new(label: impl Into<String>, bytes: Vec<u8>) -> Self {
        Fixture {
            label: label.into(),
            bytes,
        }
    }
}

/// A disposable per-test directory (the batteries' `s7-*` temp-dir
/// discipline; never owner data).
struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("s7-j3-diff-{label}-{}-{seq}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create differential scratch root");
        Scratch(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The bound corpus's identities (arbitrary but fixed).
fn bound_identities() -> (Hash, Hash) {
    (
        hash_object(b"differential-founding"),
        hash_object(b"differential-journal-root"),
    )
}

/// One capture envelope (the writer's own validation applies).
fn capture_envelope(npc: u32, tick: u64) -> ConsequenceEnvelope {
    capture_envelope_with_class(npc, tick, DurabilityClass::DurableEligible)
}

/// One capture envelope under an explicit per-span class (the
/// transaction lane composes with mixed spans).
fn capture_envelope_with_class(npc: u32, tick: u64, class: DurabilityClass) -> ConsequenceEnvelope {
    ConsequenceEnvelope::capture(
        ConsequenceId::new(1, 1, npc),
        vec![ClassifiedSpan {
            span: OccurrenceSpan::new(0, 2).expect("a valid span"),
            class,
        }],
        SignalLane::Captured,
        CounterValues::default(),
        SourceBaseline {
            counters: CounterValues::default(),
            award: AwardCheckpoint {
                durable: Coverage::new(),
                provisional: Coverage::new(),
            },
        },
        SourcePosition { head: None, tick },
    )
    .expect("a valid envelope")
}

/// One transaction input (unique identity per index).
fn tx_input(index: u64) -> TransactionInput {
    TransactionInput {
        envelopes: vec![capture_envelope(
            u32::try_from(index).expect("a small index"),
            100 + index,
        )],
        position: SourcePosition {
            head: None,
            tick: 100 + index,
        },
        classification: DurabilityClass::DurableEligible,
        dependencies: Vec::new(),
        frontier: vec![FrontierEntry {
            region: 1,
            episode: index,
        }],
        plan: format!("plan-{index}").into_bytes(),
    }
}

/// The legacy bases, built through the writer (real chains, real
/// digests): the header-only journal; three transactions plus a
/// cancellation; a multi-envelope transaction with dependencies,
/// frontier entries, and a large plan; and an aborted-capture tail
/// (the incomplete-preparation crash window).
fn legacy_bases(dir: &Path) -> Vec<Fixture> {
    let mut fixtures = Vec::new();
    {
        let path = dir.join("base-empty.t1");
        T1Journal::open(&path).expect("open the empty base");
        fixtures.push(Fixture::new("legacy-empty", fs::read(&path).unwrap()));
    }
    {
        let path = dir.join("base-mixed.t1");
        let mut journal = T1Journal::open(&path).unwrap();
        for index in 1..=3u64 {
            journal.append_transaction(&tx_input(index)).unwrap();
        }
        let second = journal.transactions()[1].id;
        journal
            .append_cancellation(second, CancellationReason::ApplicationPolicy, b"policy")
            .unwrap();
        drop(journal);
        fixtures.push(Fixture::new("legacy-mixed", fs::read(&path).unwrap()));
    }
    {
        let path = dir.join("base-rich.t1");
        let mut journal = T1Journal::open(&path).unwrap();
        let first = journal.append_transaction(&tx_input(1)).unwrap();
        let mut input = tx_input(2);
        input.envelopes.push(capture_envelope_with_class(
            3,
            102,
            DurabilityClass::EncounterProvisional,
        ));
        input.dependencies = vec![first.id];
        input.frontier = vec![
            FrontierEntry {
                region: 1,
                episode: 2,
            },
            FrontierEntry {
                region: 3,
                episode: 7,
            },
        ];
        input.plan = vec![7u8; 512];
        journal.append_transaction(&input).unwrap();
        drop(journal);
        fixtures.push(Fixture::new("legacy-rich", fs::read(&path).unwrap()));
    }
    {
        let path = dir.join("base-aborted.t1");
        let input = tx_input(1);
        let mut journal = T1Journal::open_with(
            &path,
            Some(
                JournalFaultControl::new()
                    .arm(JournalFaultPoint::CapturesWritten, FaultAction::InjectError),
            ),
        )
        .unwrap();
        assert!(matches!(
            journal.append_transaction(&input),
            Err(JournalError::AbortedBeforeCommit { .. })
        ));
        drop(journal);
        fixtures.push(Fixture::new(
            "legacy-aborted-tail",
            fs::read(&path).unwrap(),
        ));
    }
    fixtures
}

/// The bound bases, through the rotation's own generation creator
/// (crate-private by contract — why this differential lives
/// in-module).
fn bound_bases(dir: &Path, founding: &Hash, root_id: &Hash) -> Vec<Fixture> {
    let mut fixtures = Vec::new();
    {
        let path = dir.join("bound-empty.t1");
        T1Journal::create_bound_generation(&path, founding, root_id)
            .expect("create the bound empty base");
        fixtures.push(Fixture::new("bound-empty", fs::read(&path).unwrap()));
    }
    {
        let path = dir.join("bound-mixed.t1");
        let mut journal = T1Journal::create_bound_generation(&path, founding, root_id).unwrap();
        for index in 1..=2u64 {
            journal.append_transaction(&tx_input(index)).unwrap();
        }
        let first = journal.transactions()[0].id;
        journal
            .append_cancellation(first, CancellationReason::GovernedResolution, b"governed")
            .unwrap();
        drop(journal);
        fixtures.push(Fixture::new("bound-mixed", fs::read(&path).unwrap()));
    }
    fixtures
}

/// The bare header shapes (no writer involved): the empty file, the
/// valid header alone, a torn-creation prefix, garbage, and the
/// corrupted-domain-length field (the error-identity window — see
/// [`LEGACY_HEADER_PROBE_LEN`]).
fn header_fixtures() -> Vec<Fixture> {
    let header = header_bytes();
    vec![
        Fixture::new("legacy-empty-file", Vec::new()),
        Fixture::new("legacy-header-only", header.clone()),
        Fixture::new("legacy-header-torn-10", header[..10].to_vec()),
        Fixture::new("legacy-header-garbage", b"not-a-header".to_vec()),
        Fixture::new(
            "legacy-header-plus-garbage",
            [header.as_slice(), b"x"].concat(),
        ),
        Fixture::new("legacy-header-schema-flip", {
            let mut variant = header.clone();
            let schema_at = 8 + JOURNAL_DOMAIN.len();
            variant[schema_at] ^= 0xff;
            variant
        }),
        Fixture::new("legacy-header-claim-huge", {
            let mut variant = header.clone();
            variant[7] = 0xff;
            variant
        }),
    ]
}

/// The bare bound header shapes: the empty file, the header alone, a
/// torn prefix, garbage, and a legacy header where the bound header
/// belongs (the strict never-reinterpret refusal).
fn bound_header_fixtures(founding: &Hash, root_id: &Hash) -> Vec<Fixture> {
    let header = bound_header_bytes(founding, root_id);
    vec![
        Fixture::new("bound-empty-file", Vec::new()),
        Fixture::new("bound-header-only", header.clone()),
        Fixture::new("bound-header-torn", header[..20].to_vec()),
        Fixture::new(
            "bound-header-garbage",
            b"not-a-bound-header-at-all".to_vec(),
        ),
        Fixture::new("bound-legacy-shape", header_bytes()),
    ]
}

/// The byte offsets where a frame starts (the `[u64 length][record]`
/// boundaries — the corpus's truncation, zero-fill, and splice
/// sites), walking the same frame arithmetic the readers do.
fn frame_starts(bytes: &[u8], header_len: usize) -> Vec<usize> {
    let mut starts = Vec::new();
    let mut offset = header_len;
    while offset + 8 <= bytes.len() {
        starts.push(offset);
        let declared = u64::from_le_bytes(bytes[offset..offset + 8].try_into().expect("8 bytes"));
        if declared == 0 {
            offset += 8;
            continue;
        }
        let Some(end) = (offset as u64 + 8).checked_add(declared) else {
            break;
        };
        if end > bytes.len() as u64 {
            break;
        }
        offset = end as usize;
    }
    starts
}

/// The damage corpus over one base. `giant` adds the
/// giant-declared-frame fixtures (the unbuffered skip path).
fn damaged(base: &Fixture, header_len: usize, giant: bool) -> Vec<Fixture> {
    let bytes = &base.bytes[..];
    let mut out = Vec::new();

    // Truncation at every frame boundary: the torn boundary, the
    // missing-commit shape, the clean shorter journal.
    for (n, start) in frame_starts(bytes, header_len).into_iter().enumerate() {
        out.push(Fixture::new(
            format!("{}#truncate-boundary-{n}", base.label),
            bytes[..start].to_vec(),
        ));
    }
    // Truncation on a stride across every offset (mid-prefix,
    // mid-record, and — below the header's own length — the
    // header-torn shapes).
    for cut in (1..bytes.len()).step_by(13) {
        out.push(Fixture::new(
            format!("{}#truncate-stride-{cut}", base.label),
            bytes[..cut].to_vec(),
        ));
    }
    // Zero-fill: the documented filesystem artifact, at frame
    // boundaries and over the whole post-header region.
    for (n, start) in frame_starts(bytes, header_len).into_iter().enumerate() {
        for fill in [8usize, 64] {
            let mut variant = bytes[..start].to_vec();
            variant.resize(start + fill, 0);
            out.push(Fixture::new(
                format!("{}#zerofill-{n}-{fill}", base.label),
                variant,
            ));
        }
    }
    {
        let mut variant = bytes[..header_len].to_vec();
        variant.resize(header_len + 96, 0);
        out.push(Fixture::new(
            format!("{}#zerofill-all", base.label),
            variant,
        ));
    }
    // Single byte flips across the post-header region: record-chain
    // damage, undecodable records, damaged envelopes, and the
    // fail-closed commit/cancellation field rules.
    for at in (header_len..bytes.len()).step_by(5) {
        let mut variant = bytes.to_vec();
        variant[at] ^= 0xff;
        out.push(Fixture::new(format!("{}#flip-{at}", base.label), variant));
    }
    // The header-length corruption window: a domain-length claim
    // between the fixed header's own tag and the decoder's
    // `MAX_TAG_LEN` (where a short probe would have turned a
    // `Domain`/`Utf8` refusal into a `Limit` refusal with a
    // different detail string).
    for delta in [1u64, 2, 40] {
        let mut variant = bytes.to_vec();
        let claim = u64::from_le_bytes(variant[..8].try_into().expect("8 bytes")) + delta;
        variant[..8].copy_from_slice(&claim.to_le_bytes());
        out.push(Fixture::new(
            format!("{}#header-claim-window-{delta}", base.label),
            variant,
        ));
    }
    // Garbage-frame splices at frame boundaries: damage within
    // committed history, and the undecodable-record walk-past.
    for (n, start) in frame_starts(bytes, header_len).into_iter().enumerate() {
        let mut variant = bytes[..start].to_vec();
        variant.extend_from_slice(&frame(b"garbage record"));
        variant.extend_from_slice(&bytes[start..]);
        out.push(Fixture::new(format!("{}#splice-{n}", base.label), variant));
    }
    // Trailing tails: plain garbage; the wrap-window declaration
    // (u64::MAX — the audit's empirical repro); the zero-fill
    // artifact; a declared frame with only a partial payload.
    {
        let mut variant = bytes.to_vec();
        variant.extend_from_slice(b"trailing garbage");
        out.push(Fixture::new(
            format!("{}#tail-garbage", base.label),
            variant,
        ));
    }
    {
        let mut variant = bytes.to_vec();
        variant.extend_from_slice(&[0xFF; 8]);
        variant.extend_from_slice(b"tail-garbage");
        out.push(Fixture::new(
            format!("{}#tail-wrap-window", base.label),
            variant,
        ));
    }
    {
        let mut variant = bytes.to_vec();
        variant.extend_from_slice(&[0u8; 16]);
        out.push(Fixture::new(format!("{}#tail-zero", base.label), variant));
    }
    {
        let mut variant = bytes.to_vec();
        variant.extend_from_slice(&100u64.to_le_bytes());
        variant.extend_from_slice(b"only-partial");
        out.push(Fixture::new(
            format!("{}#tail-partial-frame", base.label),
            variant,
        ));
    }
    // The giant declared frames: a declaration above
    // MAX_JOURNAL_RECORD_BYTES with the bytes PRESENT (the
    // unbuffered skip path — the whole-file reader's decode-failure
    // classification, without the frame ever landing in memory),
    // both as the trailing tail and BETWEEN transactions (the
    // fail-closed damage-within-committed-history rule through the
    // skip path).
    if giant {
        let declared = MAX_JOURNAL_RECORD_BYTES + 1;
        {
            let mut variant = bytes.to_vec();
            variant.extend_from_slice(&declared.to_le_bytes());
            variant.resize(variant.len() + declared as usize, 0x5a);
            out.push(Fixture::new(
                format!("{}#tail-giant-complete", base.label),
                variant,
            ));
        }
        let starts = frame_starts(bytes, header_len);
        if starts.len() >= 3 {
            let mid = starts[2];
            let mut variant = bytes[..mid].to_vec();
            variant.extend_from_slice(&declared.to_le_bytes());
            variant.resize(variant.len() + declared as usize, 0x5a);
            variant.extend_from_slice(&bytes[mid..]);
            out.push(Fixture::new(
                format!("{}#splice-giant", base.label),
                variant,
            ));
        }
    }
    out
}

/// The full legacy corpus: header shapes, the writer-built bases,
/// and every damage family over them.
fn legacy_corpus(dir: &Path) -> Vec<Fixture> {
    let mut fixtures = header_fixtures();
    for base in legacy_bases(dir) {
        let giant = !matches!(base.label.as_str(), "legacy-empty" | "legacy-aborted-tail");
        fixtures.push(base.clone());
        fixtures.extend(damaged(&base, header_bytes().len(), giant));
    }
    fixtures
}

/// The full bound corpus.
fn bound_corpus(dir: &Path, founding: &Hash, root_id: &Hash) -> Vec<Fixture> {
    let mut fixtures = bound_header_fixtures(founding, root_id);
    for base in bound_bases(dir, founding, root_id) {
        let giant = base.label != "bound-empty";
        fixtures.push(base.clone());
        fixtures.extend(damaged(
            &base,
            bound_header_bytes(founding, root_id).len(),
            giant,
        ));
    }
    fixtures
}

/// The differential's one assertion: the production streaming scan
/// and the whole-file oracle over the SAME bytes — states by
/// equality, errors by their complete debug and display surfaces.
fn assert_scan_equivalence(path: &Path, label: &str, bound: Option<(&Hash, &Hash)>) {
    let streaming = bound
        .map(|(founding, root_id)| scan_bound_file(path, founding, root_id))
        .unwrap_or_else(|| T1Journal::scan_file(path));
    let whole = bound
        .map(|(founding, root_id)| scan_bound_file_whole_file(path, founding, root_id))
        .unwrap_or_else(|| scan_file_whole_file(path));
    match (streaming, whole) {
        (Ok(streamed), Ok(reference)) => {
            assert_eq!(
                streamed,
                reference,
                "scan state divergence: {label} ({})",
                path.display()
            );
        }
        (Err(streamed), Err(reference)) => {
            assert_eq!(
                format!("{streamed:?}"),
                format!("{reference:?}"),
                "scan error-surface divergence: {label} ({})",
                path.display()
            );
            assert_eq!(
                streamed.to_string(),
                reference.to_string(),
                "scan error-display divergence: {label} ({})",
                path.display()
            );
        }
        (streamed, reference) => panic!(
            "scan outcome divergence: {label} ({}) — streaming {streamed:?}, whole-file {reference:?}",
            path.display()
        ),
    }
}

/// Runs every fixture through both readers (one reused path; the
/// scan paths are read-only).
fn run_scan_corpus(label: &str, fixtures: &[Fixture], bound: Option<(&Hash, &Hash)>) {
    let scratch = Scratch::new(label);
    let path = scratch.0.join("journal.t1");
    for fixture in fixtures {
        fs::write(&path, &fixture.bytes).expect("write the fixture");
        assert_scan_equivalence(&path, &fixture.label, bound);
    }
}

// ------------------------------------------------------------ the oracle

/// The legacy differential: the whole corpus through the production
/// `scan_file` (streaming) and the whole-file oracle, plus the
/// missing file (both absorb it into the empty scan) and the cross
/// shape (the legacy reader over a bound journal — the strict
/// domain refusal).
#[test]
fn scan_file_matches_the_whole_file_oracle_over_the_corpus() {
    let scratch = Scratch::new("legacy-scan");
    let fixtures = legacy_corpus(&scratch.0);
    run_scan_corpus("legacy-scan", &fixtures, None);
    let missing = scratch.0.join("missing.t1");
    assert_scan_equivalence(&missing, "legacy-missing-file", None);
    let (founding, root_id) = bound_identities();
    let bound = bound_bases(&scratch.0, &founding, &root_id)
        .pop()
        .expect("a bound base");
    let path = scratch.0.join("cross-bound.t1");
    fs::write(&path, &bound.bytes).unwrap();
    assert_scan_equivalence(&path, &bound.label, None);
}

/// The bound differential: the whole bound corpus through the
/// production `scan_bound_file` (streaming) and the whole-file
/// oracle, plus the missing file (both refuse with the same I/O
/// surface) and the cross shape (the bound reader over a legacy
/// journal — the strict never-reinterpret refusal).
#[test]
fn scan_bound_file_matches_the_whole_file_oracle_over_the_corpus() {
    let scratch = Scratch::new("bound-scan");
    let (founding, root_id) = bound_identities();
    let fixtures = bound_corpus(&scratch.0, &founding, &root_id);
    let bound = Some((&founding, &root_id));
    run_scan_corpus("bound-scan", &fixtures, bound);
    let missing = scratch.0.join("missing.t1");
    assert_scan_equivalence(&missing, "bound-missing-file", bound);
    let legacy = legacy_bases(&scratch.0).pop().expect("a legacy base");
    let path = scratch.0.join("cross-legacy.t1");
    fs::write(&path, &legacy.bytes).unwrap();
    assert_scan_equivalence(&path, &legacy.label, bound);
}

/// The open-differential panics' outcome descriptor (the journal
/// handle and state are not `Debug`; the ERROR side is).
fn describe_outcome<T>(result: &Result<T, JournalError>) -> String {
    match result {
        Ok(_) => "Ok(..)".to_string(),
        Err(error) => format!("Err({error:?})"),
    }
}

/// The legacy OPEN differential: every corpus fixture through the
/// production `T1Journal::open` (streaming classification plus the
/// tail-truncation and re-initialization effects) against the
/// whole-file oracle's classification of the same pristine bytes.
/// The oracle runs first, on the in-memory bytes — open mutates its
/// file (torn-tail truncation, crashed-creation re-initialization).
#[test]
fn open_matches_the_whole_file_classification() {
    let scratch = Scratch::new("legacy-open");
    let path = scratch.0.join("journal.t1");
    for fixture in legacy_corpus(&scratch.0) {
        // The whole-file oracle over the pristine bytes.
        let oracle = match read_header(&fixture.bytes) {
            Ok(HeaderState::Valid { header_hash }) => Some(classify(&fixture.bytes, header_hash)),
            Ok(HeaderState::Fresh | HeaderState::TornCreation { .. }) => None,
            Err(error) => Some(Err(error)),
        };
        fs::write(&path, &fixture.bytes).expect("write the fixture");
        match (T1Journal::open(&path), oracle) {
            (Ok(journal), Some(Ok(state))) => {
                assert_eq!(
                    journal.transactions(),
                    state.committed,
                    "open state divergence (committed): {}",
                    fixture.label
                );
                assert_eq!(
                    journal.cancellations(),
                    state.cancellations,
                    "open state divergence (cancellations): {}",
                    fixture.label
                );
                assert_eq!(
                    journal.tail(),
                    state.observed_tail,
                    "open state divergence (tail): {}",
                    fixture.label
                );
                assert_eq!(
                    journal.next_sequence(),
                    state.next_sequence,
                    "open state divergence (sequence): {}",
                    fixture.label
                );
                assert_eq!(
                    fs::metadata(&path).unwrap().len(),
                    state.append_offset,
                    "the tail-truncation boundary: {}",
                    fixture.label
                );
            }
            (Ok(journal), None) => {
                // Fresh or torn-creation: the re-initialization (the
                // header is one write + one fsync; nothing could ever
                // have been journaled).
                assert!(
                    journal.transactions().is_empty(),
                    "re-initialized journal is empty: {}",
                    fixture.label
                );
                assert_eq!(
                    journal.tail(),
                    TailState::Clean,
                    "re-initialized journal is clean: {}",
                    fixture.label
                );
                assert_eq!(
                    fs::metadata(&path).unwrap().len(),
                    header_bytes().len() as u64,
                    "re-initialized journal is header-only: {}",
                    fixture.label
                );
            }
            (Err(error), Some(Err(reference))) => {
                assert_eq!(
                    format!("{error:?}"),
                    format!("{reference:?}"),
                    "open error-surface divergence: {}",
                    fixture.label
                );
                assert_eq!(
                    error.to_string(),
                    reference.to_string(),
                    "open error-display divergence: {}",
                    fixture.label
                );
            }
            (outcome, expected) => panic!(
                "open outcome divergence: {} — streaming {}, whole-file {}",
                fixture.label,
                describe_outcome(&outcome),
                match expected {
                    Some(inner) => describe_outcome(&inner),
                    None => "re-initialization".to_string(),
                }
            ),
        }
    }
}

/// The bound OPEN differential: every bound corpus fixture through
/// the production `T1Journal::open_bound` (streaming strict
/// classification plus the tail-truncation effect) against the
/// whole-file `strict_bound_state` oracle over the same pristine
/// bytes.
#[test]
fn open_bound_matches_the_whole_file_classification() {
    let scratch = Scratch::new("bound-open");
    let (founding, root_id) = bound_identities();
    let path = scratch.0.join("journal.t1");
    for fixture in bound_corpus(&scratch.0, &founding, &root_id) {
        let oracle = strict_bound_state(&fixture.bytes, &founding, &root_id);
        fs::write(&path, &fixture.bytes).expect("write the fixture");
        match (T1Journal::open_bound(&path, &founding, &root_id), oracle) {
            (Ok(journal), Ok(state)) => {
                assert_eq!(
                    journal.transactions(),
                    state.committed,
                    "bound open state divergence (committed): {}",
                    fixture.label
                );
                assert_eq!(
                    journal.cancellations(),
                    state.cancellations,
                    "bound open state divergence (cancellations): {}",
                    fixture.label
                );
                assert_eq!(
                    journal.tail(),
                    state.observed_tail,
                    "bound open state divergence (tail): {}",
                    fixture.label
                );
                assert_eq!(
                    journal.next_sequence(),
                    state.next_sequence,
                    "bound open state divergence (sequence): {}",
                    fixture.label
                );
                assert_eq!(
                    fs::metadata(&path).unwrap().len(),
                    state.append_offset,
                    "the bound tail-truncation boundary: {}",
                    fixture.label
                );
            }
            (Err(error), Err(reference)) => {
                assert_eq!(
                    format!("{error:?}"),
                    format!("{reference:?}"),
                    "bound open error-surface divergence: {}",
                    fixture.label
                );
                assert_eq!(
                    error.to_string(),
                    reference.to_string(),
                    "bound open error-display divergence: {}",
                    fixture.label
                );
            }
            (outcome, expected) => panic!(
                "bound open outcome divergence: {} — streaming {}, whole-file {}",
                fixture.label,
                describe_outcome(&outcome),
                describe_outcome(&expected)
            ),
        }
    }
}

/// The ambiguous-write REREAD differential: the recovery refresh
/// (`reconcile_write` → `refresh_from_disk`, now streaming) against
/// the whole-file oracle over the pristine post-fault bytes — the
/// legacy and bound forms, with a torn tail beyond the committed
/// frame (the refresh must also resolve it to the oracle's
/// append boundary), plus the lost-header refusal.
#[test]
fn refresh_from_disk_matches_the_whole_file_classification() {
    let scratch = Scratch::new("refresh");

    // (a) The legacy ambiguous commit: the commit frame is on disk,
    // the receipt never returned; a torn tail follows it.
    let path = scratch.0.join("legacy.t1");
    let input = tx_input(1);
    let mut journal = T1Journal::open_with(
        &path,
        Some(
            JournalFaultControl::new()
                .arm(JournalFaultPoint::CommitWritten, FaultAction::InjectError),
        ),
    )
    .unwrap();
    assert!(matches!(
        journal.append_transaction(&input),
        Err(JournalError::AmbiguousCommit { .. })
    ));
    {
        // Written past the journal's own handle — the data file's
        // bytes are never range-locked.
        let mut tail = OpenOptions::new().append(true).open(&path).unwrap();
        tail.write_all(&[0xFF; 8]).unwrap();
        tail.write_all(b"torn").unwrap();
    }
    let pristine = fs::read(&path).unwrap();
    let header_hash = match read_header(&pristine).unwrap() {
        HeaderState::Valid { header_hash } => header_hash,
        _ => panic!("the post-fault file keeps its header"),
    };
    let oracle = classify(&pristine, header_hash).expect("the oracle classifies the file");
    assert!(matches!(
        journal.reconcile_write(&input).unwrap(),
        WriteReconciliation::Committed { .. }
    ));
    assert_eq!(
        journal.transactions(),
        oracle.committed,
        "refresh state divergence (legacy, committed)"
    );
    assert_eq!(
        journal.cancellations(),
        oracle.cancellations,
        "refresh state divergence (legacy, cancellations)"
    );
    assert_eq!(
        journal.tail(),
        oracle.observed_tail,
        "refresh state divergence (legacy, tail)"
    );
    assert_eq!(
        journal.next_sequence(),
        oracle.next_sequence,
        "refresh state divergence (legacy, sequence)"
    );
    assert_eq!(
        fs::metadata(&path).unwrap().len(),
        oracle.append_offset,
        "the refresh's tail-resolution boundary (legacy)"
    );
    drop(journal);

    // (b) The bound form.
    let (founding, root_id) = bound_identities();
    let bound_path = scratch.0.join("bound.t1");
    let mut bound = T1Journal::create_bound_generation(&bound_path, &founding, &root_id).unwrap();
    bound.fault = Some(
        JournalFaultControl::new().arm(JournalFaultPoint::CommitWritten, FaultAction::InjectError),
    );
    assert!(matches!(
        bound.append_transaction(&tx_input(1)),
        Err(JournalError::AmbiguousCommit { .. })
    ));
    let pristine = fs::read(&bound_path).unwrap();
    let oracle =
        strict_bound_state(&pristine, &founding, &root_id).expect("the bound oracle classifies");
    assert!(matches!(
        bound.reconcile_write(&tx_input(1)).unwrap(),
        WriteReconciliation::Committed { .. }
    ));
    assert_eq!(
        bound.transactions(),
        oracle.committed,
        "refresh state divergence (bound, committed)"
    );
    assert_eq!(
        bound.cancellations(),
        oracle.cancellations,
        "refresh state divergence (bound, cancellations)"
    );
    assert_eq!(
        bound.tail(),
        oracle.observed_tail,
        "refresh state divergence (bound, tail)"
    );
    assert_eq!(
        bound.next_sequence(),
        oracle.next_sequence,
        "refresh state divergence (bound, sequence)"
    );
    assert_eq!(
        fs::metadata(&bound_path).unwrap().len(),
        oracle.append_offset,
        "the refresh's tail-resolution boundary (bound)"
    );
    drop(bound);

    // (c) The lost-header refusal: an open journal whose file lost
    // its complete header refreshes to the typed refusal.
    let lost_path = scratch.0.join("lost.t1");
    let mut lost = T1Journal::open(&lost_path).unwrap();
    lost.append_transaction(&tx_input(1)).unwrap();
    fs::write(lost.path(), &header_bytes()[..10]).unwrap();
    assert!(
        matches!(
            lost.reconcile_write(&tx_input(2)),
            Err(JournalError::CorruptHeader { .. })
        ),
        "the lost header refuses the refresh"
    );
}

/// The working-set bound's observable: a giant COMPLETE declared
/// frame (above [`MAX_JOURNAL_RECORD_BYTES`]) classifies exactly as
/// the whole-file oracle's decode failure did — and its bytes never
/// land in the streaming reader's buffer. The walk is driven
/// directly so the reader's peak buffer is observable.
#[test]
fn the_streaming_reader_buffers_at_most_one_bounded_frame() {
    let scratch = Scratch::new("bounded-frame");
    let path = scratch.0.join("journal.t1");
    let mut journal = T1Journal::open(&path).unwrap();
    journal.append_transaction(&tx_input(1)).unwrap();
    drop(journal);
    let declared = MAX_JOURNAL_RECORD_BYTES + 1;
    let mut bytes = fs::read(&path).unwrap();
    bytes.extend_from_slice(&declared.to_le_bytes());
    bytes.resize(bytes.len() + declared as usize, 0x5a);
    fs::write(&path, &bytes).unwrap();

    let mut reader = FrameReader::open(&path).unwrap();
    let header_hash =
        match read_header(&reader.read_probe(LEGACY_HEADER_PROBE_LEN).unwrap()).unwrap() {
            HeaderState::Valid { header_hash } => header_hash,
            _ => panic!("the base keeps its header"),
        };
    let state = classify_legacy_streaming(&mut reader, header_hash).unwrap();
    assert_eq!(
        state.committed.len(),
        1,
        "the committed transaction survives the giant frame"
    );
    assert!(
        matches!(
            state.observed_tail,
            TailState::Inert {
                kind: TailKind::Undecodable,
                ..
            }
        ),
        "the giant frame is the undecodable inert tail: {:?}",
        state.observed_tail
    );
    assert!(
        reader.peak_frame_bytes <= MAX_JOURNAL_RECORD_BYTES,
        "the per-frame buffer stayed bounded: {}",
        reader.peak_frame_bytes
    );
    assert!(
        reader.peak_frame_bytes < declared,
        "the giant frame was skipped, never buffered"
    );
    // And the whole-file oracle agrees on the same fixture.
    assert_scan_equivalence(&path, "giant-complete", None);
}

/// The 4 GiB ceiling is the SHARED fail-closed length check both
/// readers consult at open, before any read (the design's "mocked
/// via the length check": a real over-cap fixture is a
/// multi-gigabyte file — the check itself is the pinned surface,
/// and both readers route through it in the same position in their
/// open sequence).
#[test]
fn the_file_ceiling_is_the_shared_fail_closed_length_check() {
    let over = check_journal_length(MAX_JOURNAL_FILE_BYTES + 1).unwrap_err();
    assert!(
        matches!(
            &over,
            JournalError::Format(SaveError::Limit {
                kind: "file-length",
                value,
                max: MAX_JOURNAL_FILE_BYTES,
                ..
            }) if *value == MAX_JOURNAL_FILE_BYTES + 1
        ),
        "the over-cap refusal is the typed file-length limit: {over:?}"
    );
    assert_eq!(
        check_journal_length(MAX_JOURNAL_FILE_BYTES).unwrap(),
        MAX_JOURNAL_FILE_BYTES,
        "exactly at the cap is admissible"
    );
}
