# LIFECYCLE — the renderer lifecycle cluster design (T02/T03/T06/T16 under the T22 lens)

**Status:** DESIGN RECORD (read-only analysis lane `ee/overnight-lifecycle-analysis`,
base `d2fcbf2`). No source under `crates/` was modified. All line numbers refer to
the worktree head `d2fcbf2` (which includes T05 `fd3f7da`, the population-stamp /
indirect invalidation at submission time — the audit `096f8d9` predates it, and the
difference is load-bearing for the T02 verdict).

**Cluster mandate (strict audit IV, the directive's ONE-TEAM table):** T02 (frame.rs
stamps), T03 (renderer.rs/corner.rs/staging.rs), T06/T16 (vulkan.rs/frame.rs/
staging.rs) serialize as one lane because R3/R2/CA-10/CA-13 interact on the same
state. T22 is the LENS, not a lane: RAII scoped owners and release transactions
generalized (audit §4 T22; the directive's "treat it as the DESIGN LENS for those
fixes").

---

## 0. Thesis

The four findings are one defect class with four surfaces: **state that vouches for
device content (generation stamps, published scene handles, pool readiness,
the device-loss latch) is advanced or consulted on the strength of a *call
sequence* rather than the *resource identity it describes*.** The cluster fix is
therefore one architectural move applied at four sites, per T22:

1. **T02** — stamps must be bound to the **allocation epoch of the mapped carve**
   they vouch for, not to a per-frame counter delta observed at one point in the
   phase ladder.
2. **T03** — attach must be a **PreparedScene RAII transaction** that commits the
   public handle only on proven upload readiness, with in-flight destinations
   **quarantined under their ticket**, never destroyed on timeout.
3. **T06** — the renderer needs an explicit **lifecycle enum** with
   observe-on-every-exit (disposals included), stuck fences classified through a
   device-loss check before being called stuck, and the acquire-to-submit
   disposition's first-use present made legal by construction or refused.
4. **T16** — staging availability/poll/teardown must be **typed and classified**
   (unsupported vs retryable vs terminal), polled as an ordered prefix with no
   all-fence allocation, and submitted-but-unwaited work must retain ownership of
   its destinations.

Every landed guarantee is preserved: CA-01's rollback, CA-10's deferred fence
commit, CA-11's scoped replacement guards, CA-13's latch, R3's replay-log growth
safety and ordered-prefix retirement, R2/T05's generation+identity gates. No new
dependencies; no new enabled device extensions (see §4.4 for the one candidate and
why it stays out by default).

---

## 1. The interaction map (shared state and cross-path interactions, traced at `d2fcbf2`)

### 1.1 The shared state

| State | Where | Written by | Read by |
|---|---|---|---|
| `FrameState.population_stamps` — per-slot `instance`/`pose`/`indirect` generations + `indirect_counts` | `vulkan/frame.rs:133`, struct `:195–203` | `clear()` `:228`, `clear_instance()` `:249`, `clear_pose()` `:259`; the upload site `:1467/:1500–1501/:1531` (write-with-bytes) | `generation_write_needed` gates at `:1449/:1487/:1519` |
| `ctx.arena_growth_events: Cell<u64>` | `vulkan/context.rs:114`; bumped by `record_arena_growth` `:1074` | region `pipelines.rs:470`, instance `passes/proxy_draw.rs:595`, pose `passes/skinned_draw.rs:317` — each **after** a successful carve swap | the frame baseline `frame.rs:667` and the delta clear `:743`; the outcome stat `:794` |
| `Renderer.population_generations` + retained `instance_bytes`/`pose_bytes` | `renderer.rs:476–489` | `adapt_frame` (`submission.rs:435–706`): the T05 token gate `:505–518`, the repack gates `proxy_instances.rs:449–472` (count guard `:461`) and animation's `repack_poses_generation` | `population_repacked` signals `submission.rs:612–615` → the submission-time clears `renderer.rs:1135–1140` |
| `Renderer.scene: Option<AttachedScene>` + `ctx.corner` | `renderer.rs:461`, `context.rs` | `attach_scene` `renderer.rs:930–1011` (install `context.rs:586–600`, publish `renderer.rs:988`) | `submit_frame`'s coherence gate `:1060–1070`; the frame flow's `ctx.corner` reads |
| `Renderer.device_lost: DeviceLossLatch` | `renderer.rs:512`; `vulkan.rs:320–371` | `observe` on every device-touching entry's error exit (`renderer.rs:892/957/981/1007/1160/1214/1238/1478`) | the entry gates (`:829/934/1050/1202/1458/1472`) |
| `RendererError::vk` — the loss classification choke point | `vulkan.rs:302–317` | every flattened Vulkan conversion | `DeviceLost { operation }` `:295`, `FrameSlotUnrecoverable { source, release }` `:273` |
| `ctx.staging: RefCell<StagingPool>` | `context.rs:196`; `staging.rs:197–215` | `stage` `:506`, `flush` `:574` (in-flight push `:676`), `retire_signaled` `:260`, `drain` `:301`, `ensure_space` `:349`, teardown `:418` | `begin_staging_batch` `:841` (sticky `unavailable` `:863`), `drain_staging` `:899`, `has_in_flight` `:242` |
| CA-10 acquire-to-submit slot state — `in_flight` fences, `images_in_flight`, per-image `present_semaphores`, `image_available` | `frame.rs:118–174` | `commit_slot_fence` `:1118` (the deferred reset), `guard_image_and_read_timestamps` `:1063` (ownership staging `:1091`), `dispose_failed_acquisition` `:1207` (heal/replace) | `wait_slot_fence` `:918`, `acquire_with_retry` `:954`, `submit_command_buffer` `:2127` |

### 1.2 The cross-path interactions the audit names

**R2 ↔ CA-10 (stamps inside the post-acquire ladder).** The stamp writes happen in
`upload_frame_data` (`frame.rs:1449–1534`), phase 6 of the CA-10 ladder — after the
acquire, before the fence commit at `:859`. Within a frame the order is correct
(bytes written at `:1466` before the stamp at `:1467`, per region); a later phase
error routes through `dispose_failed_acquisition`, which touches fences/semaphores/
command buffers but never stamps — correctly, because the bytes *were* written. The
hazard is exclusively **cross-frame against carve replacement** (T02, §2).

**R2 ↔ swapchain recreation.** `recreate_swapchain` (`swapchain.rs:422–444`) clears
all stamps at `:442` **before** its first fallible step (the idle at `:443`) — a
failed recreation still cleared. The replacement `PipelineState::new` re-creates
the instance/indirect carves with no content copy.

**R2 ↔ arena growth (the T02 site).** The growth-delta clear at `frame.rs:743–745`
compares the counter against the baseline taken at entry `:667`. Between baseline
and clear sit three `?`-propagating phases: `prepare_frame` (`:678` — the ensure
ladder `frame_prepare.rs:153` where growth happens), `wait_slot_fence` (`:706`),
`acquire_with_retry` (`:710`). Growth events recorded before any of those errors
are **swallowed by the next frame's baseline**.

**R2 ↔ T05 (the submission-time invalidation, NEW since the audit).**
`adapt_frame` derives `InstanceSourceToken`/`PoseSourceToken`
(`proxy_instances.rs:163–196`; pointer+count provenance), resets the retained
generation records on mismatch (`submission.rs:511–518`), and the repack gates
(`repack_populations_generation`, count guard at `proxy_instances.rs:461`) report
`population_repacked` (`submission.rs:612–615`). `submit_frame` consumes it
**before the frame flow** (`renderer.rs:1135–1140`): any regeneration zeroes the
corresponding stamp half. This postdates audit SHA `096f8d9` and is decisive for
T02 (§2).

**R3 ↔ CA-01 (the attach ordering, T03's site).** `CornerResources::build` opens
one staging batch (`corner.rs:155`), stages the mesh VBO and every texture, and
flushes **Deferred** as the build's last statement (`:303–305`). Back in
`attach_scene`: `ensure_corner_pipelines` (`renderer.rs:973`) → publish
`self.scene = Some(attached)` (`:988`) → **then** `drain_staging()` (`:1004`). On
a drain error the function returns `Err` with the scene already published
(`renderer.rs:988–1009`). The CA-01 rollback (`release_staged_scene_resources`,
`context.rs:624–637`) is only reachable from the pipeline-ensure failure (`:978`),
not from the drain failure — the committed pipeline group would orphan.

**R3 ↔ staging growth and teardown.** `ensure_space` drains the in-flight list
before replacing the backing (`staging.rs:365`); a drain error propagates through
`stage()` into the caller's partial-failure cleanup. Teardown (`destroy`,
`:418–451`) idles the device first when anything is in flight but **ignores the
idle's result** (`:422`). The InlineWait arm (`:685–711`) pushes the in-flight
entry **before** the wait (`:676` vs `:688`), so a wait timeout correctly leaves
the regions bound — but the caller then destroys the *destination* on that error
(`textures.rs:41–48`, and the same shape through every `?` on
`create_device_local_buffer_staged` + `flush(InlineWait)` — `pipelines.rs:371`,
`passes/proxy_draw.rs:502`): a submitted copy may still reference the destroyed
image/buffer. This is T03's "in-flight destinations never destroyed on timeout"
in its cleanest existing form.

**CA-13 ↔ CA-10 (the latch and the release).** `DeviceLossLatch::observe`
(`vulkan.rs:358–370`) latches a top-level `DeviceLost`, or a
`FrameSlotUnrecoverable` **whose source** is one. The `release` half of the FSU is
never inspected: a heal submit that fails with `DeviceLost` while the source is
ordinary (the exact composition `dispose_failed_acquisition` can produce — fence
discriminator `frame.rs:1226` takes the heal arm on a status *error*, the heal
submit at `:1280` fails lossy) does **not** latch. Non-loss FSU (the stranded-slot
terminal class) latches nothing at all: the next `submit_frame` passes the gate at
`renderer.rs:1050` and reuses the slot the terminal error declared unrecoverable.
`Renderer::drop` (`renderer.rs:1501–1510`) and the pool teardown ignore their idle
results entirely (`renderer.rs:1503`, `staging.rs:422`, `context.rs:632`).

**CA-13/T06 ↔ T16 (loss masking).** `retire_signaled` collects
`wait_for_fences(...,0).is_ok()` into a fresh `Vec<bool>` (`staging.rs:266–270`)
— every error, including `ERROR_DEVICE_LOST`, reads "unsignaled"; the ordered
prefix (`retire_leading_signaled`, `:119`) then correctly refuses to retire, but
the loss is invisible. `drain`'s non-timeout arm does classify through
`RendererError::vk` (`:336`); the timeout arm (`:326–335`) returns a plain
`Vulkan(String)` — a device loss that manifests as fence timeouts is never
consulted. `begin_staging_batch`'s `None` (`:863`) is sticky over *any* first
backing failure, permanently disabling the pool after e.g. a transient OOM.

**T06's WSI premise (first-use present).** `dispose_failed_acquisition` returns
the acquired image by presenting it waiting only `image_available[slot]`
(`frame.rs:1237–1247`). The doc (`:1146–1162`) claims the acquire's implicit
layout transition orders this. For an image that has been presented before, the
presentation engine hands it back effectively in `PRESENT_SRC_KHR` and the
release-present is the canonical idiom (the rig's sync-validation catches proved
the semaphore-wait half). For a **never-rendered image** — acquired for the first
time after a recreation, frame failing before recording — the image's layout is
`UNDEFINED` (a swapchain image's initial layout), and presenting it in that state
is exactly the premise the audit refuses to trust (§4.4).

---

## 2. T02 — adjudication: reproduce or refute

**Verdict: CONFIRMED at the audited SHA `096f8d9`; REFUTED-as-unreachable at the
current head `d2fcbf2` by a traceable mechanism (T05's submission-time
invalidation closes every population-carve arm) — and the delayed invalidation is
still eliminated as a fragile invariant, per the audit's own instruction to
"record the proof and still eliminate delayed invalidation."**

### 2.1 Why the hypothesis was real at `096f8d9`

The stamp-write/carve-replacement causality: a stamp only ever vouches for bytes
written into the carve that existed at its write (`frame.rs:1466–1467` — write and
stamp are adjacent). A stale skip therefore needs (i) a carve replacement, (ii) a
frame whose submission generation equals the surviving stamp, and (iii) no
intervening clear. At `096f8d9` the only growth-path clear was the delta site at
`frame.rs:743`, which sits after three `?`-propagating phases; the next frame's
baseline (`:667`) swallows the recorded event. The audit's repro sketch ("populate
stamp at g, inject instance growth then a downstream failure; retry with g")
constructs (ii) through the re-admission arm: frame W writes stamp g (counts N);
frame B submits larger counts (demand > capacity → growth) and fails downstream
(pre-acquire ladder fault, slot-fence timeout, acquire error); frame C re-submits
generation g. At `096f8d9` nothing between B's growth and C's upload clears —
C skips and the draw reads the replacement carve's uninitialized storage.

### 2.2 The head's closure mechanism (cite-precise)

Every path is mapped; each closes for a named reason:

**(a) Growth → later error in the same preparation (the audit's primary arm).**
At the head, the growth frame B **necessarily repacked**, and the repack cleared
the stamps before B's frame flow ran:

- A carve replacement at B requires `demand_B > capacity`. Every frame that
  passed the ensure ladder on this carve had `demand ≤ capacity` — in particular
  W, the frame that wrote the surviving stamp (it passed the ladder, then wrote).
- `demand` is a pure function of the submission's counts (instance demand =
  npc+vehicle+rain; pose demand = pose blocks) — the same facts the retained
  buffers' repack gates track.
- So `demand_B > demand_W` ⟹ counts changed ⟹ the count guard
  (`proxy_instances.rs:461`, `buffer.len() == total * INSTANCE_STRIDE`) or the
  generation compare (`:460`) or the T05 token reset (`submission.rs:511–518`)
  forces `GenerationDecision::Repacked` at B ⟹ `population_repacked` is true
  (`submission.rs:612–615`) ⟹ `clear_instance`/`clear_pose` ran at
  `renderer.rs:1135–1140`, **before** `vulkan::render_frame` — hence before the
  ladder growth and the error. The stamps are zero when the frame dies; frame C
  writes (`generation_write_needed(0, g)` is always true,
  `proxy_instances.rs:213–218`).
- The same argument covers a *reverting* frame C (generation equal to the stamp
  but content different): C's retained-buffer record holds B's repack, so C
  either reuses B's bytes (token+generation+count match — then C's demand equals
  B's, which exceeded the old capacity but fits the *new* carve the growth just
  built; the stamp was already cleared, so C writes anyway) or repacks (clears
  again). There is no residue where a nonzero stamp meets a replaced carve.

**(b) Growth → deferred acquire.** Both deferral arms latch `needs_recreate`:
the double-out-of-date arm (`frame.rs:1037–1040`) and the probe-deferred arm
inside `prepare_after_recreation` (`frame_prepare.rs:250–259`,
`mark_suboptimal` at `:256`). The next *proceeding* frame enters the recreation
block (`frame_prepare.rs:127–142`, `consume_recreate` at `:158`) and
`recreate_swapchain` clears at `swapchain.rs:442` **before** its fallible idle —
so the clear happens even if the recreation itself fails. No proceeding frame
can draw between the deferral and the clear.

**(c) Swapchain recreation.** Unconditional clear at `swapchain.rs:442`, before
the first fallible step (`:443`). Closed at both SHAs.

**(d) Generation-zero.** `generation_write_needed` writes unconditionally at
generation 0 and resets the stamp to 0 (`proxy_instances.rs:213–218`).
Closed.

**(e) Retry across both slots.** All clears are whole-`PopulationStamps` (or
whole-half) operations; the per-slot analysis above applies uniformly. The
other slot's stamp cannot survive what the writing slot's analysis closed.

**(f) Region-only growth.** A region growth increments the counter and the
delta clear conservatively clears the *population* stamps
(`frame.rs:736–742` documents the conservatism). A missed region-growth clear
therefore leaves population stamps vouching for population carves that were
**not** replaced — still valid — and region bytes carry no stamps at all
(`upload_layers` runs unconditionally every frame). Harmless in both
directions.

**(g) Device-loss error arm.** If the downstream failure was a loss, the latch
(`renderer.rs:1160` → `vulkan.rs:358`) refuses every later entry; the stale
stamps die with the renderer (fresh construction is the supported path). Moot.

**(h) New indirect resources.** `ensure_indirect` is create-only
(`passes/proxy_draw.rs:603–606`, no growth path); the indirect stamp starts 0 and
the first indirect frame writes; pipeline replacement rides the recreation clear.
Closed.

**Does T05 close part of the window? Yes — the entire population-carve error arm
(a), which is the only arm that was ever open at `096f8d9` beyond the
recreation/deferral arms already closed by (b)/(c).** The pose half is identical
(`PoseSourceToken` + the pose repack gate + `clear_pose`).

### 2.3 Why the fix still lands (the fragile-invariant ruling)

The closure proof spans four invariants in four files: the count guard inside
`repack_populations_generation`, the T05 token reset, the demand-carve
inequality (demand is a pure function of repack-tracked facts), and the
`renderer.rs:1135–1140` wiring. None of them is *local* to the stamp protocol; a
future drift in any (a new demand source, a repack gate that reuses on matching
generation+length against a shrunken carve, a caller moving storage in a way the
token can't see — the documented allocator-recycling residual,
`proxy_instances.rs:149–158`) reopens the window silently. The audit's solution
is stronger and simpler: **bind the stamps to the resource allocation epoch.**

**Fix shape (T02-F):**

- Each mapped carve gains an `allocation_epoch: u64` — minted from a new
  context-monotonic counter (`VulkanContext::resource_epochs: Cell<u64>`,
  bumped once per *publication of a resource identity*), written at exactly the
  sites that replace a carve's identity: the three growth commits
  (`pipelines.rs:457`, `passes/proxy_draw.rs:582`, `passes/skinned_draw.rs:284`
  — the `replacement.commit()` lines), the initial carve publications, and the
  pipeline replacement (`PipelineState::new`'s carves — equivalently: the
  recreation path assigns fresh epochs because the handles are fresh).
- `PopulationStamps` grows to `(epoch, generation)` per slot per region
  (`instance`/`pose`/`indirect` each carry the epoch of the carve they vouch
  for; the indirect block rides the instance carve's epoch).
- The gate at `frame.rs:1449/:1487/:1519` becomes: write iff
  `generation == 0 || stamp.epoch != carve.epoch || stamp.generation != generation`
  — `generation_write_needed` stays the pure decision function (extended, still
  unit-pinned), the carve comparison happens at the call site where the live
  epoch is readable. Resource creation, replacement, and reset can never inherit
  a previous identity: a fresh epoch never equals a stamp's epoch.
- The delta clear at `frame.rs:743–745` is **demoted to diagnostic** (it becomes
  redundant; keep it as the belt with a comment naming the epoch gate as the
  correctness bearer, or delete it — the design keeps it, one redundant rewrite
  per growth event, because defense-in-depth on a correctness path is cheap and
  the growth events are log-bounded).
- T05's submission-time clears stay exactly as landed (they close the
  *identity* window; the epoch closes the *resource* window; they compose).
- The existing oracle harness at `frame.rs:2405+` extends with epoch arms
  (replace-epoch → write; same-epoch+generation → skip; generation-0 → write).

**Witness (rig-deferred, written now):** warm both slots at generation g; arm
`LifecycleFaultSite::PoseGrowthIdle` (or a later pipeline-creation fault) on a
frame whose demand grows the instance carve then fails downstream; retry the
same population; assert `population_writes_skipped == 0` on the retry (the
forced rewrite) and the debug missed-bump verify stays green. Against the
pre-fix tree the same script yields `population_writes_skipped > 0` (the red
witness; equivalently constructible at `096f8d9`). The pure half — the epoch
gate's decision table — is CPU-runnable now.

---

## 3. T03 — the PreparedScene RAII transaction

### 3.1 The audit's three acceptable outcomes (restated as the contract)

`(a)` scene fully ready, handle published; `(b)` failure leaves the previous
usable scene and **no reachable partial resource, with retry supported**;
`(c)` a typed terminal poisoned renderer with every device-touching entry
refused, no successful handle reported, safe teardown. An upload in flight
retains every destination until actual completion or terminal device-loss
teardown; **a timeout is not completion.** Never publish first and drain later;
never roll back over pipelines that retain destroyed resources.

### 3.2 The transaction shape

New type (renderer-side, `renderer.rs` or a small `attach` module):

```rust
struct PreparedScene {
    resources: CornerResources,      // staged-installed, CA-01's staged half
    ticket: Option<UploadTicket>,    // the deferred batch's identity (T16-F)
    // the pipeline group assembles against ctx.corner, so the transaction
    // drives the context's staged install (below), not a detached copy
}
```

`attach_scene` becomes:

1. **Gate** on the lifecycle state (§4) — device loss / poison refuses here.
2. **Build** `CornerResources` (unchanged — `corner.rs:118–307`; the deferred
   flush at `:303–305` returns/records its `UploadTicket` instead of being
   anonymous).
3. **Staged install + pipeline group** (unchanged order — the group binds the
   staged resources' descriptor layout): `ctx.attach_scene_resources` +
   `ensure_corner_pipelines`. On failure here (nothing submitted has to
   survive): the existing CA-01 rollback `release_staged_scene_resources`,
   with its idle result **audited** (§3.4). Outcome (b): no previous scene is
   disturbed (one-scene budget unconsumed), retry clean. *Preserved verbatim
   from CA-01.*
4. **Drain — MOVED BEFORE the publish.** `ctx.drain_staging()` runs while
   `self.scene` is still `None` and the resources are still transaction-local.
   On success: install the retained facts and publish the handle together
   (`self.scene = Some(attached)` + `Ok(handle)` — the one visibility point,
   as today at `renderer.rs:984–987`). Outcome (a).
5. **On drain failure — classify, then (b) or (c):**
   - `DeviceLost` → the lifecycle latch takes it (outcome (c): every later
     device-touching entry refused; the quarantine and teardown become the
     device-loss teardown below).
   - TIMEOUT (stuck fence) → **quarantine, do not destroy**: move the staged
     `CornerResources` into `ctx.quarantined_scenes` (a `Vec`, normally empty;
     bounded by the one-scene budget — at most one live quarantined attach per
     renderer since a second attach is refused while any quarantine exists),
     tagged with the batch's `UploadTicket`. The one-scene budget frees
     (`ctx.corner` is `None` again), `self.scene` never published → outcome
     (b): retry supported, previous state (scene-less) intact. The
     quarantined resources are re-drained at the next attach attempt (the
     fence may have signaled in the meantime — the ticket's completion check
     is the T16 typed poll) and released on completion; at teardown the
     stuck-fence protocol (device idle, `staging.rs:419–423`'s shape) makes
     the destroys legal. **In-flight destinations are never destroyed on
     timeout — the quarantine is tied to the ticket.**
   - Other Vulkan errors → treat as (b) with the same quarantine (the copy may
     or may not have executed; the ticket decides at the next drain).

The deferral's *performance* value is preserved: the pipeline-group build still
overlaps the copies (steps 2–3 unchanged); only the publication moves behind
the drain (µ-scale tail, the R3 lane's own honest assessment).

### 3.3 The in-flight-destination rule, generalized (InlineWait callers)

The same hazard exists at every `flush(FlushMode::InlineWait)` caller:
`textures.rs:41–48` destroys the texture on a flush error *even when the submit
succeeded and only the wait timed out* (the entry is already in flight,
`staging.rs:676` precedes `:688`); the `?` at `pipelines.rs:371` and
`passes/proxy_draw.rs:502` route into partial-failure destroys of destination
VBOs the same way. Fix: `flush` returns a **typed disposition** —

```rust
enum FlushDisposition {
    Completed,                                    // inline wait succeeded
    NotSubmitted(RendererError),                  // begin/end/submit failed — nothing pending
    SubmittedUnwaited { error: RendererError, ticket: UploadTicket }, // in flight
}
```

— and the callers branch: `NotSubmitted` → destroy (today's behavior, correct:
a failed `queue_submit` attaches nothing, `staging.rs:657–673`);
`SubmittedUnwaited` → hand the destination to the ticket's quarantine (a
context-owned retirement list keyed by the ticket, drained by
`drain_staging`/teardown — the same mechanism §3.2's scene quarantine uses, one
implementation). `UploadTicket` is the in-flight entry's identity (the fence
handle plus a pool-minted monotonic batch id — the `InFlight` struct
`staging.rs:183–189` already carries everything but the id).

### 3.4 The `release_staged_scene_resources` idle audit

`context.rs:628–633` currently swallows the fallback idle's result
(`device_wait_idle().ok()`). Audit and fix:

- On the drain error, **classify before idling** (T16-F): if the drain error is
  `DeviceLost`, skip the idle entirely (a lost device fails the idle with the
  same loss — no information gained) and proceed with the destroys: after
  `ERROR_DEVICE_LOST`, `vkDestroy*`/`vkFree*` calls remain the supported
  teardown path (the spec requires `vkDestroyDevice` to be callable; the
  pending-work abandon is the loss's own semantics). Latch the loss (§4's
  observe-on-exit — this is a disposal observing).
- If the idle itself returns `ERROR_DEVICE_LOST`: same — latch, proceed.
- Any other idle error: today's `.ok()` silently converts "we could not prove
  the device idle" into destroys that may race pending copies. Keep the
  rollback infallible-by-contract (the ORIGINAL error propagates) but record
  the idle failure as a typed diagnostic on the context
  (`last_release_idle_failure: Option<RendererError>`) and surface it through
  `last_capture_refusal`-style observability; document the residual risk in
  the unsafe invariant block (the alternative — propagating — would convert a
  CA-01 rollback into an unrecoverable renderer, which the audit's outcome (b)
  forbids for a *cleanup* failure).

**Unsafe invariants documented per site:** the quarantine's destroys are legal
only under (i) the ticket's fence signaled (the drain path) or (ii) a completed
or loss-abandoned device idle (the teardown path); the transaction's publish
point is the single place `self.scene` and `ctx.corner` become frame-visible;
the staged install's destroys (CA-01 arm) are legal because the deferred batch
was either never submitted (pipeline failure precedes nothing-in-flight) or was
drained/idled first (§3.4's classification).

---

## 4. T06 — the renderer lifecycle enum and the terminal-state discipline

### 4.1 The enum

```rust
pub(crate) enum RendererLifecycle {
    Ready,
    RecreateRequired,               // needs_recreate latched — the frame flow's own
                                   // deferred arm; NOT an error state (unchanged behavior)
    DeviceLost { operation: String },   // CA-13's latch, absorbed (the field stays; the
                                        // enum wraps it — refusal() delegates)
    Poisoned { reason: PoisonReason },  // terminal, non-loss: every device-touching
                                        // entry refused with the typed reason
    Destroying,                      // Drop entered — observability for the disposal
                                     // observations (§4.2)
}
enum PoisonReason { FrameSlotUnrecoverable(Box<RendererError>, Box<RendererError>),
                    StuckFence { site: &'static str },
                    ReleaseFailed(/* §3.4's non-loss idle failure, if ever escalated */) }
```

- `DeviceLost` keeps exactly CA-13's semantics (first observation wins, stable
  operation context) — the latch struct remains the implementation; the enum is
  the state machine around it. **CA-13 preserved.**
- `Poisoned` is new terminality: **any** `FrameSlotUnrecoverable` (ordinary
  source included — the audit's "non-device-loss terminal slot error is not
  evidently permanently latched") and the stuck-fence class (§4.3). Every
  public entry (`render_frame`/`render_layers`, `submit_frame`, `attach_scene`,
  `request_*`, `take_*`) consults the lifecycle first; `RecreateRequired` is
  internal-only (frames still run; the recreation block consumes it).

### 4.2 Observe-on-every-exit, disposals included

- `DeviceLossLatch::observe` extends to inspect the FSU's **`release`** half
  too (`vulkan.rs:358–370` currently matches `source` only): a loss that first
  appears in the release ladder latches with the release's operation context.
  The pure latch test (`vulkan.rs:588+`) gains that arm.
- `dispose_failed_acquisition` observes on its own exits: the terminal FSU
  construction sites (`frame.rs:1283/:1293/:1320`) route through the renderer's
  observe (they already do via `submit_frame`'s error exit — but the FSU with
  ordinary source must now *also* poison, §4.1).
- Disposals: `Renderer::drop` (`renderer.rs:1503`) replaces
  `device_idle(&self.ctx).ok()` with observe-and-record (the lifecycle enters
  `Destroying`; a loss observed at the idle is recorded — nothing further is
  refused, drop must complete — but the destroy sequence proceeds on the
  loss-abandon semantics of §3.4); `StagingPool::destroy` (`staging.rs:418–451`)
  classifies its idle (§5.4) instead of `.ok()`; `release_staged_scene_resources`
  per §3.4. The rule: **no device call's error is discarded without either a
  classification or a recorded diagnostic.**

### 4.3 Stuck-fence classification (device loss consulted first)

Every bounded fence wait that times out — `wait_slot_fence` (`frame.rs:930–934`),
`guard_image_and_read_timestamps` (`:1071–1080`), the staging drain
(`staging.rs:305–308`), the InlineWait (`:688–692`) — currently reports "stuck"
without consulting the device. Fix: on `TIMEOUT`, issue **one**
`get_fence_status(fence)`:

- `Ok(SUCCESS)` — the fence signaled between the wait and the poll (treat as
  signaled; proceed).
- `Ok(NOT_READY)` — genuinely unsignaled: **stuck fence** → `Poisoned {
  PoisonReason::StuckFence }` (the frame-slot class) / the typed staging
  timeout (the pool class, retaining its regions), never a plain string.
- `Err(ERROR_DEVICE_LOST)` — the loss surfaces: `DeviceLost` latches (the
  classification the audit demands: "stuck-fence timeout that's device-loss
  masquerading as timeout").
- `Err(other)` — propagate typed.

`vkGetFenceStatus` is the spec's cheap device-health probe (a lost device
returns `ERROR_DEVICE_LOST` from it); no idle wait, no new submits. Pure
decision function (`classify_fence_timeout(vk::Result) -> FenceTimeoutClass`)
unit-pinned CPU-side; the real-loss arms are rig-only.

### 4.4 The acquire-to-submit disposition and the first-use present premise

The CA-10 disposition (`frame.rs:1134–1329`) is preserved wholesale — returned
image + consumed acquire semaphore via the release-present, fence heal under
the discriminator, drain proof, semaphore replacement on rejection, FSU
escalation. Two changes:

1. **First-use tracking.** `FrameState` gains `image_ever_presented:
   [bool; _]` (grown with the per-image tables, `resize_images`/`
   sync_present_semaphores` sites), set at every successful present
   (`present_image`'s Ok arm) and at every completed render submission of that
   image (the submit at `:2153` — a rendered image ends the frame in
   `PRESENT_SRC` via the render pass's final layout).
2. **The first-use arm of `dispose_failed_acquisition`.** When the acquired
   image was **never presented or rendered** and the frame failed before
   recording, the release-present presents an `UNDEFINED`-layout image. The
   Vulkan WSI rule at stake: a presentable image begins life in
   `IMAGE_LAYOUT_UNDEFINED` (the swapchain's images are created by the
   implementation; the application's `initialLayout` rights over them are the
   WSI chapter's), and `vkQueuePresentKHR` consumes the image in
   `IMAGE_LAYOUT_PRESENT_SRC_KHR` — the transition to `PRESENT_SRC` is the
   application's to perform (the acquire performs no implicit transition *into*
   presentability; the "implicit transition" the code comment at
   `frame.rs:1148–1151` leans on is the presentation engine's *release* side,
   which applies to previously-presented images). **The premise is to be
   settled by the rig's core validation on the never-presented arm, not by this
   document's reading** — the design carries both dispositions:
   - **Primary (layout-legal by construction):** submit the minimal transition
     — record a one-barrier command buffer
     (`UNDEFINED → PRESENT_SRC_KHR`, `srcAccessMask = 0`,
     `dstAccessMask = 0` — the WSI-acquire idiom; general queue families are
     in scope since the barrier's stages are `TOP_OF_PIPE→BOTTOM_OF_PIPE`),
     submit it waiting `image_available[slot]` at `COLOR_ATTACHMENT_OUTPUT`
     and signaling the present semaphore, then present as today. This
     replaces the direct release-present **only** on the never-presented arm.
   - **Fallback (if validation objects to even this, or the owner prefers
     no new submits on the error path):** hold the image (the existing
     hard-present-failure policy — pool shrinkage, fail-visible as the typed
     acquire timeout) and replace the semaphore as today. Documented, typed,
     never silent.
   - `VK_EXT_swapchain_maintenance1`'s `vkReleaseSwapchainImagesEXT` is the
     modern no-present release; it is **not enabled**
     (`context.rs:1341–1347` selects only `KHR_swapchain` + portability
     subset) and enabling a device extension is a capability claim this design
     does not make — noted as a future owner decision, not a dependency.

**Witness:** the rig battery constructs a fresh swapchain (recreation), arms a
post-acquire fault on the first acquisition of a never-presented image, and
runs core validation over the disposition — CPU-untestable by construction
(the WSI engine is the arbiter). The previously-presented arm (the current
behavior) stays green under the same battery.

---

## 5. T16 — typed staging availability, ordered-prefix poll, ticket RAII, teardown classification

### 5.1 Typed availability (replacing the sticky `bool` and the `Option`)

```rust
enum StagingAvailability {
    Available,
    UnsupportedMemory { detail: String },   // no HOST_VISIBLE|HOST_COHERENT type —
                                            // permanent, the pre-R3 fallback contract
    Pressure(RendererError),                // allocation failed (OOM-class) — RETRYABLE:
                                            // the next begin_staging_batch retries the
                                            // backing; callers may fall back per-object
                                            // WITHOUT permanently disabling the pool
    Terminal(RendererError),                // device loss observed at the pool
}
```

`begin_staging_batch` (`staging.rs:841–893`) returns the typed state; the
sticky `unavailable: bool` (`:202`, set at `:863`) becomes
`availability: StagingAvailability` with only `UnsupportedMemory` sticky
(the audit's "permanently disable pooling after transient conditions"
corrected: a transient allocation failure degrades one call to the fallback
and retries the pool next time). The per-object fallback stays the
`UnsupportedMemory`/`Pressure` arm's behavior — R3's fallback contract
preserved.

### 5.2 The ordered-prefix poll (no all-fence Vec, classified)

`retire_signaled` (`staging.rs:260–292`) becomes: iterate `in_flight` **in
submission order**, `get_fence_status(entry.fence)` per entry —

- `SUCCESS` → retire this entry (the existing retire body: destroy fence, free
  command buffer, advance watermark — R3's ordered-prefix retirement and
  replay-log growth safety untouched);
- `NOT_READY` → **stop** (the prefix property `retire_leading_signaled`
  pins — the pure function stays, now fed classified statuses);
- `ERROR_DEVICE_LOST` → return the typed loss (propagates to the caller's
  observe/latch — T06's composition);
- other `Err` → propagate typed.

No `Vec<bool>` allocation (the audit's "no all-fence allocation; check only
ordered prefix, stop at first pending"); no `.is_ok()` masking. The pure seam
(`retire_leading_signaled`) gains a classified-input twin
(`classify_fence_status(vk::Result) -> FencePoll`) — CPU-pinned.

### 5.3 The guard/ticket RAII for recorded/submitted commands and destinations

`UploadTicket` (§3.3) + the context-owned **retirement list**: every
destination created beside a batch registers with the open batch; on flush the
registration transfers to the ticket's in-flight entry; on retire/teardown the
destinations release. The `SubmittedUnwaited` disposition (§3.3) moves them to
the quarantine instead of the caller's destroy. The "check
failure-to-publish queue entry (allocation pressure) before submit" item: the
flush's pre-submit allocations (command buffer, fence) are the queue-entry
publications — on their failure the disposition is `NotSubmitted` (today's
cleanup is correct); the pool's own census already counts them. The
`StagingFenceNeverSignals` seam (`staging.rs:638–646`) is reused unchanged as
the quarantine's red witness (the battery proves destinations survive the
never-signaling ticket and release at teardown's idle).

### 5.4 Teardown classification

`StagingPool::destroy` (`staging.rs:418–451`): the idle's result is classified
— `Ok` → destroys legal (today); `ERROR_DEVICE_LOST` → destroys proceed on the
loss-abandon semantics, the classification recorded (§4.2's observe) — **the
audit's "don't assert device_idle succeeded when result was ignored"**; other
`Err` → the destroys proceed (teardown must complete) with the failure
recorded on the diagnostic surface. `drain`'s timeout arm (§4.3) consults the
device first; its error keeps the typed staging-timeout shape (regions stay
bound — the honest state, unchanged).

**Unsafe invariants documented per site:** the backing replacement's
copy-forward (`staging.rs:383–400`) still requires the pre-drain (unchanged);
the quarantine/retirement destroys require ticket completion or classified
idle; the mapped pointer's single-thread confinement comments stand.

---

## 6. The staged landing plan (serialized commits, per-stage gates, the witness split)

The existing fault seams are **reused, not duplicated**: `LifecycleFaultSite`
(`config.rs:178–229`: CA-01's 13 attach sites, CA-10's four frame boundaries,
CA-11's three growth idles, `StagingFenceNeverSignals`), `TargetFaultSite`/
`TargetFaultStage` (`config.rs:116–143`), `inject_device_loss`
(`renderer.rs:1430` → the acquire site, `context.rs:884–903`). New sites are
added only where a boundary has no knob — one: `LifecycleFaultSite::
StagingDrainTimeout` (fires in `drain` before the wait, making the T03
quarantine arm drivable without `StagingFenceNeverSignals`'s permanent stick),
and optionally `AcquireSemaphoreCreate` if the rig wants the
release-replacement arm isolated (the existing generic release-error arms
already cover it — default: not added).

| Stage | Commit | Content | Gate to pass before the next | CPU-runnable now | Rig-deferred |
|---|---|---|---|---|---|
| **L1** | lifecycle substrate | `RendererLifecycle` + FSU-any-source poisons + release-half latch observation + observe-on-exit at drop/teardown/release paths (§4.1–4.2); the pure `classify_fence_timeout`/`classify_fence_status` | the latch policy tests extended (release-arm, poison transitions) — all pure; no behavior change on non-terminal paths (the existing CA-13 batteries unchanged) | the pure classifier/transition tables; the extended latch tests | the real-loss propagation battery (`inject_device_loss` + first-use-present arm needs the rig; the loss-from-get_fence_status arm needs a real or rig-injected loss) |
| **L2** | T16 staging typing | `StagingAvailability` + ordered-prefix classified poll (no Vec) + `UploadTicket` + `FlushDisposition` + teardown classification (§5) | the staging seam tests extended (classified prefix, availability transitions); `StagingFenceNeverSignals` battery re-run green on the rig at batch close | the pure prefix/classification/availability tests | the fault battery on a live queue (never-signaling fence, poll-error arms) |
| **L3** | T03 transaction | `PreparedScene` order swap (drain before publish) + quarantine + the InlineWait destination rule (§3) + the `release_staged_scene_resources` idle audit (§3.4) | the CA-01 fault battery (all 13 sites) green — the rollback arm unchanged; the new `StagingDrainTimeout` witness: attach fails, `scene()` is `None`, submit with scene refuses typed, retry succeeds on a healthy device | the transaction's state machine (pure half: the outcome table (a)/(b)/(c) per error class) | `StagingFenceNeverSignals` during attach → scene never published, quarantine survives, teardown releases; first-render after each fault |
| **L4** | T02 epochs | carve `allocation_epoch` + `(epoch, generation)` stamps + the extended pure gate + the `frame.rs:743` clear demoted to belt/diagnostic (§2.3) | the oracle harness extended (epoch arms); the existing stamp battery green; **the interrupted-growth witness script written** (§2.3) | the pure epoch-gate decision table | the interrupted-growth red/green pair (fault-arm the ladder after a growth, retry, `population_writes_skipped == 0`); the always-upload byte oracle against mapped bytes |
| **L5** | T06 first-use disposition | `image_ever_presented` + the minimal transition submit on the never-presented arm (§4.4) | the CA-10 battery green on previously-presented images (unchanged path); the never-presented arm green under **core validation** | none of the WSI arm (a device-decided premise) | **the deciding run**: fresh swapchain → first-acquire fault → validation clean on the transition submit (or the fallback arm taken and documented); the WSI spec premise settled with the layer's actual verdict recorded |

Serialization: L1 → L2 → L3 → L4 → L5 (L1 is the error/state substrate L2–L5
consume; L3's ticket is L2's; L4 is independent but rides the same files; L5
is last because its evidence is rig-gated and it must not block the CPU
chain). Each stage lands as one commit with its tests; the rig batteries run
at batch close (the lane's CPU work completes without the rig).

**Count-gate note (the T01 lesson):** each stage's new tests move the
renderer shard counts; the pins are corrected in the same commit through the
centralized expected-count mechanism, never by editing echo lines at offsets.

---

## 7. Honest limits — what only the rig can prove

1. **The WSI first-use premise (T06/L5).** Whether the release-present of a
   never-presented `UNDEFINED` image is accepted (and by which implementations)
   is a validation-layer and driver question. The design carries the
   layout-legal transition as primary precisely so it does not depend on the
   answer, but the *current* comment's claim is only falsifiable on the rig.
2. **Real device-loss propagation.** Every `ERROR_DEVICE_LOST` arm here
   (get_fence_status classification, the release-half latch, the teardown
   classification, loss-at-drain) is designed from the spec's error contract;
   `inject_device_loss` models only the acquire site. A real loss's actual
   error surface at each call site (which calls return the loss vs
   `VK_ERROR_DEVICE_LOST` vs stale handles) is rig/owner-validation territory —
   the audit's own boundary.
3. **The never-signaling quarantine.** `StagingFenceNeverSignals` is a faithful
   structural injection, but "the fence eventually signals and the quarantine
   drains" (the retry-supported half of outcome (b)) is only observable with a
   real queue's timing.
4. **Byte-equality after forced rewrites.** The epoch-forced rewrite path must
   be pixel-identical to the always-write oracle on device — the CPU half pins
   the *decision table*, not the bytes.
5. **The T02 refutation is a trace, not an execution.** §2.2's closure argument
   is source-level; the rig's red/green pair (pre-fix tree vs post-fix tree,
   same script) is what turns "refuted by trace" into "reproduced-or-proven."
   Until then the finding's correct status is *refuted-at-head-by-mechanism,
   fix landed as invariant hardening*.
6. **T05's allocator-recycling residual** (`proxy_instances.rs:149–158`)
   remains release-open and out of this cluster's scope (the audit's
   revision-scoped population resource is the named future hardening); the
   epoch gate does not close it (it is an identity, not a resource, hazard).
7. **No exact-head CI claim is made here** — this lane is read-only design;
   the implementation lane's gates (including the rig batteries and the count
   pins) are specified in §6 for the dispatching orchestrator.
