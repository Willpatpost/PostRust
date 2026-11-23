# CYCLE 6-6D — SESSION & REPLICATION BOUNDARIES (the in-session architecture design investigation)

> **Status:** Design investigation v1.0 · 2026-11-23 · **UNRATIFIED DESIGN — no implementation authorized.** This document designs the *in-session* multiplayer architecture boundaries — what runs where during play, how 1–4 players' inputs order into the deterministic spine, what state crosses the wire, and how divergence detects. Every recommendation herein is a **PROPOSAL** awaiting owner ratification; the open-decision list (§9) is the owner's menu, and nothing is settled silently.
>
> **Ownership:** [`MULTIPLAYER-SPEC.md`](MULTIPLAYER-SPEC.md) owns the *player-facing policy* (ratified — referenced, never overridden here). [`MULTIPLAYER-ARCHITECTURE-RESEARCH.md`](MULTIPLAYER-ARCHITECTURE-RESEARCH.md) owns the *campaign-level* authority/persistence/durability research (unapproved — built on, not duplicated). **This document owns the *in-session* boundary design** — the layer the CURRENT-STATUS capability table lists as "networking and replication: not yet designed."
>
> **Evidence key:** **[ESTABLISHED]** = external literature or documented external capability; **[PROJECT EVIDENCE]** = demonstrated in this repository; **[HYPOTHESIS]** = this design's unproven adaptation; **[OPEN]** = unresolved decision or question.

---

## 0. What this document is — and is not

**The gap.** Three layers of the multiplayer territory exist today:

| Layer | Document | Status |
|---|---|---|
| Player-facing policy (rosters, votes, suspension, handoff, persistence semantics) | `MULTIPLAYER-SPEC.md` | **Ratified** (M-01–M-35) |
| Campaign-level authority/persistence (canonical medium, leases, durability tiers, availability) | `MULTIPLAYER-ARCHITECTURE-RESEARCH.md` | Researched, **unapproved** (Architecture E recommended) |
| **In-session architecture** (sim placement, input ordering, replication scope, divergence detection, session lifecycle mechanics) | **this document** | **Designed here, unratified** |

The third layer is what every eventual line of networking code hangs from, and it is the layer that touches the engine's untouchable property: **determinism**. Designing it before any transport or provider selection is deliberate — the boundaries below are transport-agnostic by construction, so the owner's deferred T-06 decisions (Steam Networking Sockets, P2P, relay services) remain open without invalidating anything here.

**Is not:** a transport selection, a provider selection, a protocol specification, a ratification of any SPEC policy, or an implementation authorization. The staged implementation path (§10) is owner-gated at every step.

---

## 1. The constraint set — what any in-session design must honor

| # | Constraint | Source | Consequence for this design |
|---|---|---|---|
| C1 | 1–4 registered players, world-bound campaigns | SPEC §0, M-01 | Tiny scale; protocol simplicity beats sophistication; bandwidth is never the binding constraint on residential links **[ESTABLISHED-class]** |
| C2 | One authoritative session per campaign | M-28 | Exactly one *authority* shape; everything else must be provably non-authoritative |
| C3 | Player-hosted listen-server is the **working preference** (unratified) | SPEC §2.3, M-03 | Design for host-on-a-player-machine; do not require developer-funded infrastructure (the hard constraint) |
| C4 | Fail-closed authority; no silent divergent-history merging | M-35 | Divergence *detects loudly* and suspends; it never negotiates or merges client-side |
| C5 | 15-minute authored-world suspension on disconnect; exact resume within a live suspension; encounter-boundary resume after termination | M-18, SPEC §9.1 | The game is **stall-friendly by policy** — an enormous simplification the design should spend deliberately |
| C6 | Frontier subset play continues while a member is absent | SPEC §3.2, M-19 | The session must tick with an incomplete input roster — absent members are a first-class state, not an error |
| C7 | The spine's byte-identical fingerprints are untouchable | The engine contract (every historical fingerprint pinned in CI) | No multiplayer mechanism may alter canonical logical state, event ordering, or execution semantics; divergence detection must *read* fingerprints, never *influence* them (the D3 clause generalized) |
| C8 | Planned host handoff required; crash-time seamless migration is not a launch requirement | M-31, SPEC §9.3 | Handoff is a designed transition; crash is fail-closed with emergency copies |
| C9 | Threat model: cooperative friends; integrity against accidents/drift/bugs, not adversarial anti-cheat | Research §1 | No anti-tamper arms race; signed/validated records suffice; trust-but-verify |
| C10 | RPG pacing (Cyberpunk-2077-class co-op), not esports | VISION | Input-latency tolerance is measured in tens of milliseconds, not single digits; correctness and consistency outrank per-tick latency minimization |
| C11 | Engine/game separation: reusable primitives below, this RPG's policy above | Research §10, SPEC §1 | Every boundary below is stated twice: the engine capability and the configured policy that rides it |
| C12 | Sim/presentation separation (the standing engine discipline) | The engine architecture | Client-side smoothing/prediction, if any, lives **entirely in presentation** — never in the spine, never in fingerprints |

---

## 2. The layer boundary — where multiplayer code is allowed to live

**PROPOSAL (the crate map):**

```text
  Game policy layer (this RPG — configured data, not engine code)
    grace windows · full-roster rule · encounter definitions · vote thresholds
  ────────────────────────────────────────────────────────────────────────────
  engine-session  (FUTURE crate — the composition layer this design specifies)
    session state machine · input collection + canonical ordering ·
    replication-scope computation · fingerprint checkpoint exchange ·
    suspension/handoff protocols · divergence monitoring
  ────────────────────────────────────────────────────────────────────────────
  engine-core     (UNCHANGED in scope: stays network-free and deterministic)
    the ONLY multiplayer-relevant addition ever needed: the ordered
    input-frame seam (§3) — pure data + validation, zero transport
  engine-persist (UNCHANGED: the durable lane — canonical medium, commit
    pipeline, journal, encounter transactions; already designed for
    exclusive authority via conditional append)
  engine-renderer / engine-slice (presentation consumers of replicated state)
```

**The load-bearing rule: `engine-core` never learns that networks exist.** The spine stays a pure deterministic event machine — sockets, sessions, peers, and transport are categorically beneath it. This is not stylistic; it is the determinism guarantee's structural defense (C7): nothing asynchronous can reach canonical logical state because the crate that owns that state has no asynchronous surface at all. Multiplayer enters only as *pre-ordered, pre-validated data* through the same event-scheduling seam the single-player demo uses today **[PROJECT EVIDENCE — `combat_demo::tick(intent, fire)`]**.

`engine-session` is the only new crate this design requires. It is the analogue of what `engine-persist` is to storage: a policy-free composition layer the game configures. Its creation is batch-gated exactly as Stage 5 was (§10).

---

## 3. The input boundary — the determinism-critical seam

### 3.1 The problem

The spine is deterministic given an identical event stream in identical order **[PROJECT EVIDENCE — every scenario-trace pin]**. Player inputs arrive from up to four machines at uncontrolled times over uncontrolled delays. Naive forwarding makes event order a function of network arrival — which makes fingerprints a function of network weather. **This seam is where multiplayer either preserves determinism or destroys it.**

### 3.2 The design (PROPOSAL)

**The host-serial input lane.** Inputs are already pure data — `MoveIntent` plus fire/interaction edges, with the T-17 D1 semantics (one-shot consumption, buffered to the next executed tick, never dropped) **[PROJECT EVIDENCE — ratified behavior]**. The lane:

1. **Collection window.** The host opens a collection window for tick N; each connected client submits its input intents for tick N (a signed, sequence-numbered input frame).
2. **Canonical order — by rule, not by arrival.** Within any tick, input events order **player-id-major, then per-player sequence number** — a fixed rule independent of arrival time. Arrival time decides only *which tick* an input lands in (N or N+1), never the order within a tick. **This is the single most important sentence in this document**: it is what makes the event stream — and therefore every fingerprint — a function of *what* was submitted, not *when* it arrived.
3. **Explicit no-op fill.** A connected player with no submitted input for tick N contributes an explicit no-op input frame (or is marked absent per §7's frontier rules). Absence is represented, never implicit — an omitted frame is indistinguishable from a lost frame, and the fail-visible discipline (C4) refuses that ambiguity.
4. **Validation before the spine.** Input frames validate at the lane boundary (finite values, well-formed edges, sequence continuity) — the R4-style input contracts **[PROJECT EVIDENCE — `combat.rs`/`player.rs` refusal discipline]**. Malformed input is refused loudly before any event is scheduled; it is never repaired (the standing decode discipline).
5. **Scheduling through the existing seam.** Ordered, validated inputs schedule through `Spine::schedule` exactly as single-player inputs do today. The spine is unaware anything changed.

### 3.3 The late-input policy (owner decision, §9-D2)

When a client's tick-N input has not arrived by the window's close, three candidate policies:

| Policy | Behavior | Fits | Cost |
|---|---|---|---|
| **(a) Stall the shared clock** | Tick N waits for all connected players' inputs | The SPEC's shared-clock/waiting philosophy (C5); simplest correctness story | Everyone's latency = slowest player's; a lagging player stalls the group (acceptable for authored-world co-op; hostile for frontier action) |
| **(b) No-op fill + next-tick application** (PROPOSAL default) | The absent input becomes tick N's no-op; the real input applies at tick N+1 (D1's buffered-to-next-executed-tick semantics, already ratified) | Frontier subset play (C6); the latency-tolerant default | The affected player sees one tick of input delay under lag; never a stall cascade |
| **(c) Per-player input delay** (lockstep-style) | Every player's inputs apply uniformly K ticks after submission | Competitive fairness of feel | Always-on latency for everyone; hard to justify at RPG pacing (C10) |

**(b) is the PROPOSAL default** because it reuses ratified semantics (D1), never stalls the group, and degrades per-player. **(a)** is proposed as the *authored-world opt-in* (where C5's stall-friendliness is policy anyway) — the two presets can differ; that split is an owner decision.

### 3.4 The input-frame wire format

Extends the Stage 5 B1 canonical-wire discipline **[PROJECT EVIDENCE — `engine-core::wire`, validated full-fidelity round-trip]**: versioned, structurally validated, refuse-never-repair. The frame is tiny (a movement vector, a few edges, a sequence number, a player id, a signature) — at 60 Hz × 4 players it is kilobits per second, rounding error on any residential link **[ESTABLISHED-class arithmetic]**.

---

## 4. The simulation boundary — what runs where

### 4.1 The options

| Model | Shape | Authority fit | Determinism fit | Cost profile |
|---|---|---|---|---|
| **S1 — host-only sim + state replication** (the standard listen-server) | Host runs the one authoritative spine; clients send inputs and receive presentation state | Direct M-28 fit | Determinism helps the host (reproducibility, recovery) but clients cannot self-verify | Full presentation-state streaming per client per tick; client is a dumb renderer |
| **S2 — host-relayed deterministic lockstep** | All peers run identical spines; the host relays ordered input sets; state never replicates except for late join | Weakest M-28 fit (peers compute authority, host only orders it) | **Maximal** — byte-identical fingerprints across peers make divergence detection built-in | Every client pays full sim CPU; the group ticks at the slowest input; late join needs a state transfer anyway |
| **S3 — host-authoritative sim + optional client mirror** (PROPOSAL direction) | S1's authority shape: the host runs the *canonical* spine. Clients *may* additionally run a **mirror spine** in validation mode, fed the same ordered input sets | Direct M-28 fit — the mirror is explicitly **non-authoritative** (presentation + verification only, C2) | Full: mirrors must produce byte-identical fingerprints or divergence is declared | Mirror CPU per client (cheap — see below); inputs + checkpoints + late-join snapshots are the only traffic |

### 4.2 Why S3 is the recommended direction

**The engine's efficiency makes S3 affordable — that is not true of most engines.** A mirror spine is the full simulation running per client; on this engine the demonstrated costs are: sub-millisecond spine ticks at 1,000–5,000 entities **[PROJECT EVIDENCE — the sustained-load matrix]**, 0.073 ms CPU per frame for the full lit-scene render path **[PROJECT EVIDENCE — the 6-B rig measurement]**. A co-op client running mirror + presentation is a fraction of one core. In a heavier engine, S2/S3 mirroring would be a non-starter; here it is nearly free **[PROJECT EVIDENCE-grounded, per-client claim HYPOTHESIS until measured on real client hardware]**.

**S3's traffic shape is minimal and self-verifying.** Per tick: ordered input sets (KB/s). Periodically: fingerprint checkpoints (§6). On late join: one spine snapshot — which already exists as the validated full-fidelity wire round-trip **[PROJECT EVIDENCE — Stage 5 B1]**. No continuous presentation-state streaming is *required* (a replicating presentation-delta channel remains an option for clients that opt out of the mirror; §5 keeps that seam).

**S3 degrades to S1 gracefully.** A client that cannot or will not run the mirror (weak machine, debug build, owner choice) falls back to receiving replication-scope state. The canonical spine never depends on the mirror's existence — the mirror is a *consumer* of the ordered input stream, never a participant in authority. Divergence in a mirror flags loudly and the client re-syncs from authority (C4); it can never corrupt the session.

**What S3 deliberately does not do:** client-side simulation authority of any kind, client-initiated rollback, or authoritative prediction. The single-writer discipline (M-28) is preserved end to end.

### 4.3 Prediction (the C12 line in the sand)

Player-movement feel under input latency is the one place prediction tempts. The design's answer: **local movement extrapolation lives entirely in the client's presentation layer** — a visual smoothing that renders the local player's submitted intent ahead of the authoritative echo, reconciled (never rolled back through the spine) when authoritative state arrives. It is invisible to the event stream, to the ledger, and to every fingerprint, exactly as the glyph-atlas rebuild and camera interpolation are today **[PROJECT EVIDENCE — the sim/presentation separation]**. Its absence is also acceptable: at 1–4 players on residential links with C10's pacing tolerance, round-trip input echo is tens of milliseconds; whether extrapolation is *needed* at all is an owner decision after feel-testing (§9-D3), not a prerequisite.

---

## 5. The replication-scope boundary — what crosses the wire

For mirror clients (S3 default), simulation state does not replicate — the mirror derives it. What still crosses:

| Channel | Contents | Cadence | Consumer |
|---|---|---|---|
| **Ordered input sets** | §3's canonical per-tick input frames for all players | Per tick | Mirror spines; presentation |
| **Fingerprint checkpoints** | The spine fingerprint at checkpoint ticks (§6) | Per N ticks + encounter boundaries | Mirror verification |
| **Late-join snapshot** | The full-fidelity spine state via the B1 wire format | Once, on join/rejoin | Mirror initialization (catch-up can replay the input log from a snapshot — both paths exist as PROJECT EVIDENCE primitives) |
| **Presentation deltas (optional, non-mirror clients)** | Region-scoped entity presence/transforms at LOD-appropriate rates, effect cues, encounter-phase state, the pending/confirmed commit UI state | Per tick, budgeted | S1-fallback clients |
| **The durable lane** | Milestone commits, encounter transactions, journal frames | On commit | **Not session traffic at all** — `engine-persist`'s canonical-medium channel (research §5–§6); the session layer only *triggers* it |

**The interest-management unit already exists: the Stage 4 region director.** Eight regions, four LOD levels, presence samples, activation/reconstruction **[PROJECT EVIDENCE]** map one-to-one onto replication scoping for presentation-delta clients: a client receives region-scoped proxies at the LOD its view actually consumes. At 1–4 players, per-client region interest is the camera's region neighborhood — the same computation the LOD director already performs per frame. No new spatial infrastructure is required **[HYPOTHESIS — the reuse claim needs the batch-1 loopback demo to confirm]**.

**The rate discipline is D3's, unchanged.** Replication cadence is a bounded, measured budget with dual-regime evidence, not ad hoc per-frame allocation — the zero-avoidable-per-frame-allocation contract applies to the session layer as it does to the renderer.

**What never replicates:** spine internals (the event ledger, waiter lists, budget counters beyond diagnostics), AI micro-state beyond the LOD-relevant projection, the durable lane (engine-persist's channel), and any per-tick fingerprint other than checkpoints.

---

## 6. The divergence boundary — detection, loud failure, never negotiation

**The fingerprint chain is the divergence detector, and it is already built.** The host publishes spine-fingerprint checkpoints (every N ticks and at every encounter boundary — the transaction anchors). Each mirror client compares its locally computed fingerprint at the same tick against the host's checkpoint:

- **Match:** byte-identical state, by construction **[PROJECT EVIDENCE — the fingerprint contract]**. No traffic, no state sync needed.
- **Mismatch:** the mirror diverged (a non-determinism regression, a build mismatch, or data corruption). The client **fails loudly** (the R4 visible-failure discipline), declares itself desynced, and re-syncs from authority: snapshot + input-log replay from the last matching checkpoint. It never merges, never rolls back authority, never continues silently — M-35's fail-closed policy expressed at the session layer.

**What fingerprints do not do (the honest limits):** a checkpoint mismatch identifies *that* divergence exists, not *which side* is wrong; with one authority (C2) that question is settled by policy — the host is right, the mirror re-syncs. If the *host's* build is the buggy one, every mirror flags simultaneously: a four-way alarm that no single-client engine can produce. That alarm is S3's quiet superpower.

**Version discipline:** mirror clients must run byte-identical engine builds; the session handshake compares engine/schema versions before play (the medium's compatibility-version discipline, research §3's campaign metadata, applied to the session) **[HYPOTHESIS — the handshake protocol is a batch-2+ design]**.

---

## 7. The session-lifecycle boundary — ratified policy as a state machine

The SPEC's policy moments map onto session states. Each state fixes three things: **who ticks, what replicates, what persists.**

| State | Entered by | Tick authority | Replication | Persistence |
|---|---|---|---|---|
| **Hub** | Session open (M-27: every session begins here) | Host ticks the hub spine | Inputs + checkpoints (mirrors); hub-scoped deltas | None (durable lane idle) |
| **AuthoredSession** | Full roster present + destination vote | Host ticks the authored-world spine | Inputs + checkpoints | Milestone commits via the pipeline, gated per M-29 |
| **FrontierSession** | Any nonempty subset (C6) | Host ticks the frontier spine; **absent members are explicit absent-roster state, not errors** | Inputs + checkpoints (connected only) | Frontier-scoped commits |
| **Suspended** | Authored-world disconnect (M-18) | **Nobody ticks** — the authoritative freeze is a *host session-state*, not a sim feature; the exact suspended state is captured via the full-fidelity wire snapshot at suspension time (the SPEC §9.1 snapshot-timing recommendation) | Suspension state to all clients; reconnect countdown on rejoin | The suspension snapshot is safeguarded immediately **[PROJECT EVIDENCE — the B1 round-trip is the exact-resume mechanism the policy requires]** |
| **Terminated → RecoverySnapshot** | 15-minute expiry | Nobody | Recovery composition to clients | Committed durable state + encounter checkpoint — the `engine-persist` checkpoint/journal machinery **[PROJECT EVIDENCE — the Stage 5 B3 encounter transactions]**; group returns to Hub |
| **Handoff** | Planned host handoff (M-31: volunteer → nominate → accept) | Old host halts at a checkpoint boundary; successor validates the campaign revision + spine snapshot and assumes the lease (the research §5 lease/epoch machinery); session *continues* — distinct from departure (SPEC §2.3) | Full snapshot to the successor; clients re-anchor | The lease transfer record; the durable lane is untouched |
| **CrashFailClosed** | Unexpected host loss | Nobody; session ends (no seamless migration at launch, M-31) | Each client preserves its local emergency recovery copy | Fail-closed; resumption requires renewed validation |

Two mappings worth naming explicitly, because the policy language was written before the machinery existed and matches it exactly:

- **The SPEC §9.1 "exact suspended authoritative gameplay state" is the Stage 5 B1 full-fidelity spine round-trip** — the wire format was built and validated for precisely this class of fidelity **[PROJECT EVIDENCE]**.
- **The SPEC's "recovery snapshot — committed durable state plus the encounter checkpoint" is the Stage 5 B3 commit pipeline + encounter-journal machinery** **[PROJECT EVIDENCE]**. The ratified policy's recovery model composes from existing parts without new persistence concepts.

**Absent-member avatars in frontier play** (their character stands? despawns? AI-escorts?) is a policy question the SPEC leaves OPEN (§6.2/§9.2) — this design marks it OPEN here too (§9-D5); the input lane supports all answers (absent = explicit no-op frames or absent-roster state).

---

## 8. The engine/game boundary — the reuse table (C11)

| `engine-session` primitive (game-agnostic) | This RPG's configured policy riding it |
|---|---|
| Ordered input lane (collection, canonical ordering, no-op fill, validation) | Which players are connected-eligible; late-input policy per destination (§3.3) |
| Session state machine (Hub/Authenticated/Suspended/Handoff/...) | The 15-minute window; full-roster rule; subset rules; grace windows |
| Fingerprint checkpoint publication + mirror verification | Checkpoint cadence; divergence UX |
| Late-join snapshot + input-log replay | Rejoin rules; frontier mid-join eligibility (SPEC §6.2, OPEN) |
| Lease/handoff protocol (over `engine-persist`'s medium) | Handoff eligibility; volunteer/nominate/accept UX |
| Replication-scope computation (region/LOD projection) | Per-client interest policy; presentation-delta budgets |
| Divergence fail-closed handling | Re-sync UX; emergency-copy policy |

The layering rule from research §10 applies verbatim: the engine never hardcodes full-roster-authored-city or equal-XP rules; those are data above `engine-session`.

---

## 9. Open decisions for the owner — do not silently settle

| # | Decision | The menu | This design's lean |
|---|---|---|---|
| **D1** | Simulation placement (§4) | S1 host-only / S2 lockstep / S3 host-authoritative + optional mirror | **S3** |
| **D2** | Late-input policy (§3.3) | Stall / no-op fill / input delay; possibly different per destination | **No-op fill default; stall opt-in for authored world** |
| **D3** | Client movement prediction (§4.3) | None / presentation-layer local extrapolation | **None until feel-testing says otherwise** |
| **D4** | Fingerprint checkpoint cadence (§6) | Per-tick / per-N-ticks / encounter-boundaries-only | **Per-N-ticks + encounter boundaries** (N measured, not guessed) |
| **D5** | Absent-member avatar policy in frontier play | Despawn / AI-escort / frozen-proxy | **OPEN** (SPEC owns it) |
| **D6** | Transport/provider research authorization | Steam Networking Sockets / raw UDP / relay services (T-06 deferral) | **Not this cycle** — the boundaries are transport-agnostic by construction |
| **D7** | `engine-session` batch gating (§10) | Approve the staged path + batch 1 scope | **Approve batch 1 = the input-ordering seam + loopback session** |
| **D8** | Mirror spine opt-in policy | Always-on / opt-out / off-by-default | **Opt-out** (on where the machine allows; S1 fallback always available) |

---

## 10. The staged implementation path — NOT authorized by this document

Each batch is independently owner-gated, reviewed, and CI-pinned, exactly as Stage 5 was:

1. **Batch 1 — the input-ordering seam (engine-core, pure data):** the ordered input-frame type, canonical-order rule, no-op fill, and validation — with the determinism regression: identical input sets in shuffled arrival orders produce byte-identical fingerprints. No transport. **The single most load-bearing primitive; small and exhaustively testable.**
2. **Batch 2 — `engine-session` skeleton + single-process loopback:** the session state machine over the existing spine + persist machinery, one process playing all roles; the lifecycle table (§7) becomes executable tests.
3. **Batch 3 — two-process loopback over an abstract transport:** ordered input sets, checkpoints, late-join snapshot, divergence fail-closed — over IPC/localhost with the transport behind an interface (D6 still open).
4. **Batch 4 — handoff + suspension drills:** lease transfer, exact-resume, recovery snapshot — the §7 hard states under fault injection.
5. **Batch 5+ — real transport, real hosts, feel-testing:** D3/D6 resolved with rig evidence; only then does anything touch a network.

**Nothing in this document authorizes any of it.** The path exists so the owner can gate precisely.

---

## 11. Relationship to the standing documents

- `MULTIPLAYER-SPEC.md` — every policy statement here defers to it; where this design *mechanizes* a policy (the suspension freeze, the recovery snapshot, handoff), it cites the SPEC section and adds only the mechanism.
- `MULTIPLAYER-ARCHITECTURE-RESEARCH.md` — this design assumes nothing from it beyond its *ratified inputs* (M-28/M-31/M-35); Architecture E's canonical medium remains unapproved research that Batches 2+ would build on *if* ratified.
- `DECISIONS.md` — this document adds no registry entry because it ratifies nothing; the owner's §9 rulings become registry entries when made.
- `CURRENT-STATUS.md` — the capability row "Networking and replication" moves from "not yet designed" to "in-session boundaries designed (unratified); transport/provider selection open."

---

*This document intentionally includes open questions so agents do not mistake design proposals for adopted rules. No code, saves, or assets are modified or authorized by this file.*
