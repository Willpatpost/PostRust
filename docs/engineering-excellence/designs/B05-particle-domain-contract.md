# B05 — The Particle Numerical Domain Contract: Design

**Finding:** B05 (DIRECTIVE §6) — finite particle arithmetic still needs a
complete domain contract.
**State (FINDINGS.md):** CONFIRMED (orchestrator-adjudicated) — the
auditor's "combined Y-excursion is checked" label was wrong; the direct
read shows particle.rs:286-291 bounds position+velocity per axis and
:293-294 checks gravity×life² SEPARATELY — the combined sum is never
bounded.
**Status of this document:** DESIGN ONLY. No implementation in this task.
Read-only over `crates/**`; this file is the deliverable.
**Author:** design subagent (glm-5.3-w4a8), 2026-11-26, branch
`refactor/engineering-excellence`.
**Sources read:** `crates/engine-core/src/particle.rs` (implementation
:63-618 and the full test module), `crates/engine-core/src/weather.rs`,
`crates/engine-core/src/ripple.rs` (campaign composition),
`crates/engine-slice/src/corner_weather.rs`,
`crates/engine-slice/tests/render_weather.rs`,
`crates/engine-core/tests/spine_allocations.rs` (allocation-gate
context). All citations line-exact against the working tree at read
time.

---

## 1. The adjudicated defects, restated precisely

### 1a. The combined Y excursion is never bounded (particle.rs:284-295)

`EmitterConfig::validate` ends with two separate kinematic gates:

```rust
// particle.rs:284-292 — per-axis excursion (position + velocity)
let life = self.lifetime[1];
for axis in 0..3 {
    let speed = self.base_velocity[axis].abs() + self.velocity_jitter[axis].abs();
    let excursion =
        speed * life + self.volume.max[axis].abs() + self.volume.min[axis].abs();
    if !excursion.is_finite() {
        return Err("particle/kinematic-extent-non-finite");
    }
}
// particle.rs:293-294 — gravity displacement, SEPARATELY
if !(self.gravity * life * life).is_finite() {
    return Err("particle/kinematic-extent-non-finite");
}
```

The Y axis's true worst-case excursion is the **sum** of three parts:
the spawn endpoint magnitude, the velocity×life displacement, and the
gravity displacement. Each part is bounded separately; the sum is not.
A configuration with every part finite near the f32 ceiling
(`f32::MAX ≈ 3.4028e38`) passes both gates and then integrates to
±inf.

**A concrete all-gates-green witness** (hand-derived from the pinned
integration order; to be executed as the discriminating test):

- `volume` min = max = `[0.0; 3]`; `base_velocity = [0.0, -3e38, 0.0]`;
  `velocity_jitter = [0.0; 3]`; `lifetime = [1.0, 1.0]`;
  `gravity = 3e38`; `drag_factor = 1.0`; `kill_y = None`; `rate ≥ 2`.
- Gates today: Y-axis excursion = `3e38·1 + 0 + 0 = 3e38` — finite ✓.
  Gravity gate: `3e38·1·1 = 3e38` — finite ✓. All other gates green.
- Integration at `dt = 0.5` (the pinned order, particle.rs:459-469):
  step 1 applies `v[1] -= g·dt` → `-3e38 - 1.5e38 = -4.5e38` → f32
  rounds to `-inf` (magnitude above `f32::MAX`); then
  `p[1] += v[1]·dt` → `-inf`; `life -= 0.5` → `0.5 > 0` — the
  particle **survives the tick with nonfinite state**, exposed
  through `Emitter::particles()` (particle.rs:329-331) to the
  renderer's `ProxyInstance` pack (corner_weather.rs:431-445) and
  through `fold_state` (particle.rs:510-517) into every digest that
  folds the pool.

### 1b. A second, distinct unbounded term: velocity accumulation

The gravity *velocity* gain over a lifetime is `g·life` (each step
subtracts `g·dt`; over elapsed time `t` the accumulated magnitude is
`g·t`, damped by `drag ≤ 1` between steps — never amplified). The
checked term is `g·life²`. **For `life < 1` these differ by a factor
`1/life`**: the velocity can overflow while the checked displacement
term stays finite, and the overflowed velocity then writes ±inf into
the position on the next `p += v·dt`.

Witness: `base_velocity[1] = -3e38`, `gravity = 6e38`,
`lifetime = [0.5, 0.5]`, volume 0, `dt = 0.25`:
- Gates today: Y excursion = `3e38·0.5 = 1.5e38` ✓; gravity =
  `6e38·0.25 = 1.5e38` ✓ — all green.
- Integration: `v[1] = -3e38 - 6e38·0.25 = -4.5e38` → `-inf`;
  `p[1] = -inf·0.25 = -inf`; `life = 0.25` — again a nonfinite
  survivor.

Note this witness's *combined position excursion* would be finite
(`1.5e38 + 1.5e38 = 3e38`) — so even a correct combined-position gate
alone does not close the defect; the velocity bound
`|v0| + g·life ≤ f32::MAX` must be gated too. (Derivation of both
bounds: §3.)

### 1c. The dt domain: any finite nonnegative dt is admitted
(particle.rs:429-431)

```rust
if !dt.is_finite() || dt < 0.0 {
    return Err("particle/tick-duration-non-finite-or-negative");
}
```

`dt > lifetime[1]` (and `dt` above any particle's remaining life) is
admitted, and the integration (:452-488) runs **before** the expiry
test (:483, after `life -= dt` at :469). The life-based admission
gates bound `v·life` and `g·life²` — they say nothing about `v·dt` or
`g·dt` when `dt > life`. Consequences, worked through the actual
observables:

1. **A `dt > lifetime[1]` step is necessarily every particle's LAST
   step** (`life ≤ lifetime[1] < dt` ⇒ `life − dt < 0` ⇒ dead at
   :483). So big-`dt` alone cannot create a nonfinite *survivor* —
   the poisoned particle is compacted away the same tick. What it
   does create is a nonfinite **kill-death observation**:
2. **`on_kill_death` can receive NaN/±inf** through
   `kill_plane_impact` (particle.rs:584-608). The NaN path: with
   `pre` finite and `post` nonfinite (e.g. `post[0] = +inf` from
   `v_x·dt` overflow — the X gate bounds `v_x·life`, not `v_x·dt`),
   `span = pre[1] − post[1] = +inf` ⇒ `alpha = (pre[1]−k)/inf = +0`
   ⇒ `impact[0] = pre[0] + (post[0]−pre[0])·alpha = finite + inf·0 =
   NaN`. The ripple field then spawns a ring at NaN XZ
   (ripple.rs:285-309 `observe_death`).
   Witness: `volume` 0; `base_velocity = [3e38, -2e38, 0.0]`;
   `lifetime = [1.0, 1.0]`; `gravity = 0.0`; `drag_factor = 1.0`;
   `kill_y = Some(0.0)`; `rate = 1`. All admission gates green today
   (X excursion `3e38` ✓, Y `2e38` ✓, gravity `0` ✓). `tick(2.0)`:
   `post = [6e38→+inf, -4e38→-inf, 0]`; kill test fires
   (`-inf ≤ 0`); `pre = [0,0,0]`; `alpha = 0`;
   `impact = [0 + (inf−0)·0, 0, 0] = [NaN, 0, 0]` → the observer
   closure receives NaN.
3. **`dt·rate` can overflow to +inf before the cap**
   (particle.rs:435-438): `spawn_accumulator += dt·rate` → `inf`;
   `inf > SPAWN_ACCUMULATOR_CAP` → clamped to `1000.0`. Boundedness
   is preserved (the drain loop is structurally capped — good), but
   the **discard is silent**: `dropped` (:445) counts only
   pool-bound refusals; the spawn units discarded by the accumulator
   cap (`dt·rate − 1000`, which itself may be `inf`) are counted
   NOWHERE. The charter's "budget exhaustion must be observable" is
   met for pool overflow but not for accumulator saturation.
4. **`overflow_total` is a wrapping u32 lifetime accumulator**
   (particle.rs:490: `self.overflow_total += dropped;`): at the
   structural maximum 1000 drops/tick × 60 ticks/s the u32 wraps in
   ≈ 19.9 hours of sustained pathological saturation — a silent loss
   of the observable the field exists to be.
5. `life -= dt` with huge `dt` produces a large-negative **finite**
   life (|life|, |dt| ≤ f32::MAX; their difference cannot overflow) —
   harmless, the death test is `life <= 0.0`. No action needed.

### 1d. `set_volume` revalidates the whole configuration
(particle.rs:359-375)

`set_volume` rebuilds the entire `EmitterConfig` and runs the full
`validate()` — including every volume-INdependent gate (rate,
gravity, drag, velocities, jitter, kill_y, lifetime, and the gravity
extent). The invariant that would justify a split **does hold**
(`Emitter::config` is private; nothing but `set_volume` mutates it,
and it mutates only `.volume`), so the immutable-half revalidation is
redundant — but the cost is ~30 comparisons on a call that fires only
when the quantized listener box changes (corner_weather.rs:363-368:
the 4 m cell-crossing cadence, not per frame). See §6 for the
disposition.

## 2. The contracts that constrain any fix

1. **The pinned integration order is byte-sacred**
   (particle.rs:22-37, the module contract): gravity → drag →
   position → life → death test, fixed axis order, no
   transcendental. The trajectory digest pin
   `PINNED_CAMPAIGN_DIGEST = 16_840_467_954_493_462_700`
   (particle.rs:1560, pinned by `digest_pin` :1534-1556 over the
   rain-like campaign, seed `0xC0FFEE`, 600 ticks at dt = 1/60,
   folded every 60th) must hold **digit-for-digit for every config
   that remains admitted**. The same for the ripple campaign digests
   (ripple.rs:1133-1152's composed `step`, campaign at :1171+) and
   the whole render_weather gate battery (ink floors at
   render_weather.rs:332-356), all of which ride the demo rain
   emitter's exact trajectories.
2. **A tightened admission must not refuse any config the ordinary
   campaigns use.** The margins are enormous (§5.2): every ordinary
   config's excursion is ~10² m against a ~3.4e38 ceiling.
3. **Admission-only changes are digest-free by construction**:
   refusing a config at `validate()` never changes the arithmetic of
   an admitted one. The one hot-path-adjacent change in this design
   (the impact totality guard, §4.3) is branch-guarded to alter
   behavior only when a post-integration endpoint is nonfinite —
   which today is exactly the defective path no pin covers.
4. **The fail-visible refusal discipline**: refusals are typed
   `&'static str`, refusal never mutates state
   (particle.rs:382-388, pinned by
   `tick_duration_refusals_advance_nothing` :1861-1914 and
   `refused_refresh_preserves_state` :1567-1596).
5. **The allocation gates**: the particle tick path allocates
   nothing by design (particle.rs:15-16, the fixed-array pool); the
   crowd-path allocation pins in `render_instanced.rs` and the spine
   pins (`spine_allocations.rs:178,186`) must not move. All proposed
   changes are branch/compare additions on existing paths — zero
   allocation.

## 3. The admission proof shape (defects 1a + 1b)

### 3.1 The two bounds, derived against the pinned order

Per step (particle.rs:459-467): `v[1] -= g·dt`; `v[1] *= d`
(`d = drag ≤ 1`); `p[1] += v[1]·dt`. Also
`p[axis] += v[axis]·dt` for X/Z with no gravity.

**Velocity bound.** Each gravity impulse `g·dt_i` is applied once and
then only multiplied by `d ≤ 1` in later steps — it never grows. So
at any time, `|v[1]| ≤ |v0[1]| + g·(elapsed)`. A particle survives a
tick iff `life_after > 0`, and `life` decreases by exactly `dt` per
tick, so **a survivor's total elapsed time since spawn is ≤ its
drawn lifetime ≤ `lifetime[1]`**. The initial draw is
`v0[1] = base[1] + (draw·2−1)·jitter[1]` (particle.rs:533), so
`|v0[1]| ≤ |base[1]| + |jitter[1]|`. Therefore for every state a
surviving particle can ever hold:

```
|v[1]| ≤ |base[1]| + |jitter[1]| + g·lifetime[1]
```

**Position bound.** `|p[1]| ≤ |y0| + Σ|v_i|·dt_i ≤ |y0| +
max|v|·(elapsed) ≤ |y0| + (|base[1]|+|jitter[1]|+g·life)·life`.
Using the existing conservative endpoint form (`|min|+|max| ≥ |y0|`
for any draw) and keeping the existing `g·life²` (not the tighter
continuous `½g·life²` — the discrete bound is velocity×life, and
conservatism is free at admission):

```
|p[1]| ≤ (|base[1]|+|jitter[1]|)·life + |min[1]| + |max[1]| + g·life²
```

X and Z carry no gravity; their existing per-axis form is already the
combined bound for those axes and is **unchanged**.

### 3.2 The gate arithmetic — overflow-safe by construction

Both bounds are sums of nonnegative f32 terms. Computing the sum in
f32 and refusing when `!sum.is_finite()` is the correct direction:
if the exact sum exceeds `f32::MAX`, the f32 sum is `+inf` (a sum of
nonnegatives cannot round to a *smaller* finite value except within
~1 ulp of the ceiling). The residual 1-ulp boundary window (exact
sum in `(MAX, MAX·(1+2⁻²⁴)]` rounding down to `MAX`) is
symmetric with the integration's own rounding at the same threshold
and sits 36 orders of magnitude above every ordinary config (§5.2) —
documented, not slacked. An explicit slack constant
(e.g. `MAX/2`) would additionally refuse
currently-admitted-but-truly-finite configs — a semantic narrowing
with no defect class to close — and is **not recommended**.

### 3.3 The exact code shape (design, not implementation)

In `validate` (particle.rs:284-295), replace the separate gravity
check with:

1. Keep the per-axis loop (:285-292) verbatim for axes 0 and 2.
2. For axis 1 (Y), fold the gravity term into the excursion sum:
   `speed·life + |max[1]| + |min[1]| + gravity·life²`, refuse
   `!finite` with the SAME string
   `"particle/kinematic-extent-non-finite"`.
3. Add one velocity gate (the 1b bound):
   `|base[1]| + |jitter[1]| + gravity·life`, refuse `!finite` with
   the same string (a distinct string — e.g.
   `particle/kinematic-velocity-non-finite` — is worth considering
   for diagnosis, but the existing test battery pins the
   extent string for gravity-origin refusals (see §5.1); keeping one
   string preserves every pin; the design recommends one string and
   notes the alternative).
4. Delete :293-294.

The gravity term joins the SAME loop the volume endpoints are
checked in, so `set_volume` (which revalidates through the same
function) enforces the tightened Y gate on refreshed volumes
automatically — no divergence between construction and refresh.

**Why this refuses exactly the defective region:** the gates bound
the worst case over all draws and all admissible step partitions
(dt ≤ remaining life, per the survivor argument). A config the
combined gate refuses has a worst-case nonfinite trajectory — the
same worst-case discipline the existing gates already use
(`|min|+|max|` counts both endpoints though one suffices per spawn).
A config the gates admit cannot produce a nonfinite **survivor**
state, whatever the draw and whatever the dt sequence (§3.1's
elapsed ≤ life argument). The only remaining nonfinite window is the
dying step itself (dt > remaining life), whose sole observable is
the kill-death impact — closed by §4.3.

## 4. The dt domain decision (defect 1c)

### 4.1 Options considered

**(i) Admission-time dt bound / typed refusal of `dt > lifetime[1]`.**
Rejected: a `dt > lifetime[1]` tick is exactly an "everything dies
this step" tick — a legitimate catch-up shape (and A24's bounded
catch-up discipline makes such steps reachable in principle). Worse,
it outlaws a **valid ordinary policy shape**: short-lived particles
(e.g. lifetime `[0.005, 0.01]` s sparks) at a 60 Hz tick have
`dt = 1/60 > lifetime[1]` on **every** tick; today each spark spawns,
integrates one step, and dies — valid and finite. A blanket refusal
makes such emitters unusable. Determinism cost of the alternative
"refuse only when survivors exist": the refusal would depend on pool
state — an admission rule that varies per tick — hostile to the
determinism contract's simplicity. Rejected.

**(ii) Bounded substep policy** (split `dt` into chunks ≤ some
threshold, integrate per chunk). Deterministic (fixed partition
rule) but **changes arithmetic for large-dt inputs** (different
rounding per partition), adds a substep-count bound of its own
(`dt = 1e38` at chunk `lifetime[1]` ⇒ ~10³⁸ substeps — needs a cap,
which reintroduces the discard-observability question), and changes
kill-death observation order across substeps. No pin covers
large-dt behavior today, but the churn buys nothing once (iii)
closes every observable channel. Rejected.

**(iii) Keep the dt domain `[0, +∞)` finite-nonnegative; make the
dying step's only observable total.** Chosen. The survivor argument
(§3.1) plus the config gates (§3) already guarantee every SURVIVING
particle is always finite. A `dt`-oversized step's nonfinite
intermediates live exactly one sweep and are exposed solely through
`kill_plane_impact`'s return value. Guard that one pure helper
(§4.3) and the contract is complete: **no nonfinite observable
output exists anywhere in the admitted domain — not in pool state,
not in fold_state, not in reports, not in the death observer.**

### 4.2 What stays as-is

- The dt admission test at :429-431 (NaN/negative refusal,
  no-state-advance) — unchanged, already pinned
  (`tick_duration_refusals_advance_nothing` :1861-1914, which also
  pins zero-dt-as-paused-clock).
- `life -= dt` producing large-negative finite life — harmless
  (§1c.5).
- The spawn phase order (accumulator → clamp → drain → integrate)
  — unchanged; known-answer test 1 pins that a particle spawned this
  tick is integrated this tick (particle.rs:648-652, :674-689).

### 4.3 The impact totality guard (the one hot-path-adjacent edit)

In `kill_plane_impact` (particle.rs:584-608), the straddling branch
computes `[pre[0] + (post[0]−pre[0])·alpha, k, pre[2] +
(post[2]−pre[2])·alpha]`. Add a totality condition: **if either
interpolation endpoint (`post[0]`, `post[2]`) is nonfinite, return
`[pre[0], k, pre[2]]`** — the last finite position's XZ at the
plane, the honest in-step estimate when the step left the finite
domain (mirroring the existing already-below degenerate branch's
honesty at :604-606, which returns `post` when `post` is the honest
finite estimate).

- Cost: two `is_finite` tests per kill death (~480/s in the rain
  campaign) — negligible; no allocation; no change to the finite
  path's arithmetic or evaluation order ⇒ **all digests and ink pins
  hold byte-identically** (the guard's branch is unreachable for
  every finite `post`, i.e. for every pinned campaign).
- `pre` is always finite when the helper is invoked: a kill death
  fires the first sweep `p[1] ≤ kill_y` holds (a prior `-inf`
  position would have fired it earlier and compacted the particle),
  and spawn positions are finite by the endpoint/span gates — so
  guarding `post` suffices; the alpha computation itself is total
  (`span > 0` guard at :593-597, `pre[1]` finite).
- Document the totality law in the helper's doc comment: "the
  returned impact is always finite for any finite `pre`" — the
  observer-facing contract the ripple layer consumes.

### 4.4 Counter observability decisions (defects 1c.3, 1c.4)

1. **Accumulator-cap discard must be observable.** Extend the
   per-tick report: compute the whole-unit discard **before** the
   clamp as a saturating count (form: the pre-clamp accumulator
   minus `SPAWN_ACCUMULATOR_CAP`, floored at 0, computed in f32 and
   saturated into a u32 — the overflowed `inf` case saturates to
   `u32::MAX`, which is the honest "more than 4.29e9 units were
   discarded" report). Carry it beside `overflow` in `TickReport`
   (e.g. `cap_discard: u32`), keeping `TickReport` `Copy +
   PartialEq`. Construction-site impact: exactly one literal
   construction outside particle.rs
   (`corner_weather.rs:1102-1105`, the census saturation fixture)
   plus the one whole-struct `assert_eq!` inside particle.rs
   (:1902-1905) — both mechanical updates, no behavioral pin moves
   (the value is 0 on every ordinary tick, including every existing
   test's ticks).
2. **`overflow_total` becomes saturating**: `overflow_total =
   overflow_total.saturating_add(dropped)` at particle.rs:490. The
   lifetime semantic changes from "wraps silently after 4.29e9
   drops" to "sticks at u32::MAX, fail-visible saturation" — the
   repo's established counter discipline. Note the per-tick
   `SpawnOverflow` carry and the census identity
   `active_before + spawned == deaths + active` (particle.rs:494-497)
   are untouched.
3. **The saturation is not end-to-end testable at reasonable cost**
   (wrapping needs ~4.3e6 fully-saturating ticks ≈ tens of seconds);
   the test plan (§7) covers it with a focused form instead.

## 5. Compatibility audit against every existing pin

### 5.1 Refusal-string pins (must hold with the SAME strings)

- `validation_refusals` (particle.rs:1385-1478): pins the
  ordering/endpoint/span/rate/gravity/drag/velocity/jitter/kill_y
  strings — none of those gates change.
- `nonfinite_configurations_are_refused_typed`
  (particle.rs:1700-1852): pins the kinematic-extent string for four
  witnesses —
  - `base_velocity = f32::MAX`, life 10, gravity 0 (:1760-1774): X
    axis `MAX·10 → inf` — my gates refuse on the X loop, unchanged,
    same string ✓.
  - base+jitter at `MAX/2` each (:1777-1786): X excursion `MAX·1.4 →
    inf` — unchanged ✓.
  - gravity 1, lifetime `[√MAX, 2√MAX]` (:1789-1796): today refused
    by the separate :293-294 (`g·life² = 4·MAX → inf`); under the
    design it is refused by the Y-loop's folded gravity term (same
    sum overflowing, same string) ✓ — **this is the one test whose
    refusal *site* moves; its expectation is byte-identical**.
  - extreme lifetime `[MIN_POSITIVE, MAX]` with zero motion
    (:1829-1837) asserts `validate().is_ok()`: velocity gate
    `0 + 0 + 0·MAX = 0` ✓; Y excursion `0 + 0 + 0 + 0·MAX² = 0` ✓ —
    still admitted ✓. The moving variant (:1840-1846) refuses via X
    (`2·MAX → inf`) — unchanged ✓.
- `set_volume` refusal equality (:1801-1819, :1567-1596): the
  refresh path runs the same single `validate` — the tightened Y gate
  applies to refreshed volumes identically ✓.

### 5.2 Ordinary-campaign margins (must remain admitted)

The demo rain policy (`corner_weather.rs:79-89`: fall 9.0, jitters
0.6/0.15, lifetime [1.65, 2.15], gravity 9.81, band XZ ±20-ish over
the quantized listener box, receiver at ground) and the test
campaign (`rain_like_campaign`, particle.rs:628-643: volume
[-20,12,-20]..[20,16,20], v [0,-9,0], life [0.9,1.4], g 9.81):

- Combined Y excursion ≈ `(9+0.15)·2.15 + 16+12 + 9.81·2.15²` ≈
  **92 m**; velocity gate ≈ `9.15 + 9.81·2.15` ≈ **30 m/s**; X/Z ≈
  `0.6·2.15 + 20 + 20` ≈ **41 m** — against a 3.4e38 ceiling:
  **>36 orders of magnitude of margin**. Rooftop/climb variants move
  the band by tens of meters — nothing changes materially.
- The saturation census fixture (particle.rs:1938-1951: rate 1e5,
  life 100, zero motion, zero gravity): all gate sums are exactly
  `0.0` ✓ admitted.
- `known_answer_kill_plane` (:742-798: v −20, life 100, volume y 21):
  Y excursion `20·100 + 21+21 + 0` = 2042 ✓; velocity `20 + 0` ✓.
- The dt=1.0 spawn tick in that test: `dt=1.0 < lifetime[1]=100` ✓
  (and §4 keeps the dt domain unchanged anyway).

### 5.3 Trajectory pins (must hold digit-for-digit)

`digest_pin` (16_840_467_954_493_462_700), the known-answer trio
(:654-798), `fixed_seed_sequence_byte_identical` (:1189),
`reset_replays_identically` (:1485),
`n_emitters_compose_byte_identically_with_standalone` (:1639), the
ripple campaign digests (ripple.rs:1171+), the weather steady-state
test (weather.rs:448-469), and the render_weather ink floors
(:332-356): all ride admitted configs at ordinary dt — the admission
gates do not touch their arithmetic, and the impact guard is
unreachable for finite `post` ⇒ **all hold byte-identically**. The
counter additions are report-side only and zero on every ordinary
tick ⇒ the census identities and report pins hold.

## 6. `set_volume` split decision (defect 1d)

**Recommendation: no-change (keep the single full `validate`), with
the invariant documented.** The measured shape:

- The immutable-half revalidation costs ~30 scalar comparisons per
  call; the call fires on 4 m listener-box crossings only
  (corner_weather.rs:363-368), not per frame — nanoseconds per
  crossing against a frame budget of milliseconds.
- The single path is load-bearing for correctness: the tightened Y
  gate (§3.3) mixes volume endpoints with immutable terms (gravity,
  base velocity, lifetime); a split validation would have to
  reproduce exactly that composition in the volume-half, creating a
  second refusal vocabulary whose divergence from construction
  would be a new defect class. The existing pins deliberately
  assert construction/refresh refusal equality
  (particle.rs:1801-1819).
- The charter's own conditional — "split ... **only if invariant
  holds**" — is satisfied by the invariant but the split's value is
  absent at the measured call rate. Revisit only if a future caller
  refreshes per-frame (then: extract the volume-dependent gates into
  a private `validate_volume(&SpawnVolume, &ImmutableHalves)` used
  by BOTH construction and refresh — one vocabulary, two entry
  points — and re-pin the refusal-equality tests).

## 7. Test / evidence plan

**Existing pins that must hold unchanged** (the regression bar for
any implementing lane): every test enumerated in §5 — the full
particle battery, the weather derivation tests, the ripple campaign
digests, the render_weather ink gates, and (via corner_weather's
census) the render_instanced composition pins. The lane must run the
engine-core battery plus `render_weather`; the orchestrator's batch
gate adds the rig-level Vulkan pins (A24's discipline).

**New discriminating tests** (each fails pre-fix, passes post-fix —
the anti-"completion-claim" requirement from the calibration
record):

1. `combined_y_excursion_is_refused` — the §1a witness (v −3e38,
   g 3e38, life [1,1], volume 0): assert
   `validate() == Err("particle/kinematic-extent-non-finite")`.
   Pre-fix: `Ok(())` AND (as the demonstration half of the same
   test, before the fix lands) a driven tick at dt 0.5 leaves a
   survivor whose `position[1].to_bits() == 0xFF800000`. The test
   should be written in two commits or assert only the refusal, with
   the nonfinite-survivor demonstration recorded in the lane's
   report (a test that shows today's defect cannot coexist with the
   fix).
2. `velocity_accumulation_is_refused` — the §1b witness (v −3e38,
   g 6e38, life [0.5,0.5]): asserts the velocity gate specifically
   (the combined position excursion is finite here — 3e38 — so this
   discriminates the NEW gate, not the folded gravity term).
3. `dt_over_lifetime_impact_is_finite` — the §1c.2 witness
   (v [3e38, −2e38, 0], life [1,1], g 0, kill_y 0, rate 1,
   `tick_observing_kill_deaths(2.0, …)`): assert every component of
   the observed impact `is_finite()`. Pre-fix: the observer receives
   `[NaN, 0, 0]`.
4. `accumulator_cap_discard_is_observable` — rate 1e10 (or a large
   finite dt·rate product): assert the new `TickReport` carry
   reports the saturated discard and `overflow` still reports
   pool-bound drops separately; assert the carry is `0` on an
   ordinary rain tick (the negative pin).
5. `overflow_total_saturates` — the end-to-end wrap is too slow to
   drive (§4.4.3); cover the accumulation form with a focused unit
   test (drive `dropped`-shaped increments through a
   `#[cfg(test)]`-visible seam or assert the
   `saturating_add` semantics on the counter's update expression
   via a small extracted helper). Honest limitation: this is the
   weakest of the five tests; the change is two tokens
   (`+ dropped` → `.saturating_add(dropped)`) and the review burden
   is the diff itself.
6. **A rain-policy admission smoke**: `RAIN_POLICY`-derived configs
   (street eye, rooftop, climbing variants — reuse weather.rs's
   observed-volume fixtures) assert `validate().is_ok()` — the
   "tightening refuses nothing ordinary" pin, guarding against a
   future over-tightening regression.

**Measurement owed by the implementing lane**: none beyond the
gates — every change is admission-time or branch-guarded; the
hot-path cost delta is two `is_finite` tests per kill death
(~480/s), below measurement noise, and can be stated by inspection
in the handoff.

## 8. Summary of recommendations

| Defect | Disposition | Change |
|---|---|---|
| 1a combined Y excursion | FIX at admission | fold `gravity·life²` into the Y-axis excursion sum; delete the separate :293-294 check; same refusal string |
| 1b velocity accumulation | FIX at admission | new gate `|base[1]|+|jitter[1]|+gravity·lifetime[1]` finite |
| 1c dt domain | KEEP `[0,∞)` finite-nonneg; close the observer | totality guard in `kill_plane_impact` (nonfinite `post` XZ ⇒ `[pre[0], k, pre[2]]`) |
| 1c.3 cap discard | FIX observability | saturating per-tick discard carry in `TickReport` (0 on ordinary ticks) |
| 1c.4 overflow_total | FIX silently-wrapping u32 | `saturating_add` |
| 1d set_volume revalidation | NO-CHANGE | document the immutability invariant; single `validate` stays (§6) |

All changes are admission-time or guard-branch; no pinned trajectory
arithmetic changes; the digest 16_840_467_954_493_462_700 and every
ordinary-campaign admission hold.

## 9. Coverage limits (honest)

- The witnesses in §1 are hand-derived from the pinned integration
  order by IEEE-754 f32 round-to-nearest reasoning, not executed
  (design-only task, no cargo). Each is shaped to be far from
  rounding boundaries (overflow margins of ≥ 30%, not 1-ulp), so
  execution should reproduce them exactly; the implementing lane
  must confirm before claiming the fix.
- The 1-ulp boundary analysis (§3.2) is argued, not formally proven;
  the design's position is that the window is unreachable by
  ordinary configs and symmetric with integration rounding — a
  formal proof is out of scope and unnecessary at 36 orders of
  margin.
- The render_weather ink pins were read at their floor form
  (:332-356); the rig-level pinned values named in the A24 row
  (ink 17041 etc.) live in the gate harness/run records outside this
  read — the design's claim is conditional on those riding admitted
  configs at ordinary dt, which their provenance (the same rain
  campaign) supports.
- `RippleField::observe_death`'s own handling of a NaN input
  (pre-guard) was not traced beyond ripple.rs:285-309; with the
  guard, NaN never arrives, so the trace is unnecessary for this
  design but would be needed for any historical-defect writeup.
- The bench/measurement side (`behavior_measurement.rs`) does not
  exercise the particle module; no interaction was audited beyond
  confirming no cross-calls exist (grep).
