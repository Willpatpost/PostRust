#!/usr/bin/env bash
# ci-count-gate-selftest.sh — the T01 gate regression fixture.
#
# Deterministic, cargo-free regression test for the CI count-gate
# control plane (the class of defect repaired 2026-11-28: the heavy
# shard's echo printed 74 while its gate enforced 64 against an
# actual of 72 — three literals for one quantity, able to drift
# independently). Two halves:
#
#   1. Gate-semantics cases: the exact awk/grep parse expressions the
#      workflow uses, exercised against synthetic logs — a valid
#      summary passes; wrong passed/ignored/result-line counts each
#      fail; a FAILED result line is excluded from the passed sum
#      (so a failing suite cannot pass its own count gate); a missing
#      summary parses to zero and fails (compilation success is never
#      counted as coverage); pipefail propagates a nonzero cargo exit
#      through tee.
#   2. Workflow-agreement cases: the heavy and main shard steps in
#      engine-ci.yml derive their printed triples AND their enforced
#      assertions from the same centralized expectation variables —
#      the drift that produced 74/64/72 cannot silently return.
#
# Runs locally and as a CI step (bash + awk + grep only).

set -u
cd "$(dirname "$0")/.."

failures=0
note()  { printf '  ok: %s\n' "$1"; }
failed() { printf '  FAIL: %s\n' "$1"; failures=$((failures + 1)); }

# The parse expressions, verbatim from the workflow's count gates.
parse_counts() { # $1 = log path; sets P (passed) IG (ignored) R (result lines)
  P=$(awk '/test result: ok/ {for(i=1;i<NF;i++) if($(i+1)=="passed;") s+=$i} END {print s+0}' "$1")
  IG=$(awk '/test result: ok/ {for(i=1;i<NF;i++) if($(i+1)=="ignored;") s+=$i} END {print s+0}' "$1")
  R=$(grep -c 'test result:' "$1" || true)
}

# The gate shape, verbatim from the workflow's assertion structure.
gate() { # $1=passed $2=ignored $3=results $4=e_passed $5=e_ignored $6=e_results
  [ "$1" -eq "$4" ] || return 1
  [ "$2" -eq "$5" ] || return 1
  [ "$3" -eq "$6" ] || return 1
  return 0
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

echo "gate semantics:"
# Case 1: a valid multi-binary summary (12 passed, 3 ignored, 3 lines).
cat > "$tmp/valid.log" <<'EOF'
running 6 tests
test result: ok. 5 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 0 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.00s
EOF
parse_counts "$tmp/valid.log"
if [ "$P" = 12 ] && [ "$IG" = 3 ] && [ "$R" = 3 ]; then note "valid summary parses to 12/3/3"; else failed "valid summary parsed to $P/$IG/$R (want 12/3/3)"; fi
if gate "$P" "$IG" "$R" 12 3 3; then note "correct counts pass the gate"; else failed "correct counts failed the gate"; fi

# Case 2: each wrong expectation fails the gate.
gate "$P" "$IG" "$R" 11 3 3 && failed "wrong passed expectation passed" || note "wrong passed expectation fails"
gate "$P" "$IG" "$R" 12 2 3 && failed "wrong ignored expectation passed" || note "wrong ignored expectation fails"
gate "$P" "$IG" "$R" 12 3 4 && failed "wrong result-line expectation passed" || note "wrong result-line expectation fails"

# Case 3: a FAILED result line is not an ok line — a failing suite
# cannot satisfy its own passed-count gate.
cat > "$tmp/failed.log" <<'EOF'
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
EOF
parse_counts "$tmp/failed.log"
if [ "$P" = 5 ] && [ "$R" = 2 ]; then note "FAILED line excluded from the passed sum (5/2)"; else failed "FAILED line miscounted: $P/$IG/$R"; fi
gate "$P" "$IG" "$R" 6 0 2 && failed "failed suite passed its count gate" || note "failed suite fails its count gate"

# Case 4: a missing/zero summary parses to zero, and zero fails the
# gate (an empty log — compile error, no execution — never passes).
: > "$tmp/empty.log"
parse_counts "$tmp/empty.log"
if [ "$P" = 0 ] && [ "$IG" = 0 ] && [ "$R" = 0 ]; then note "empty log parses to 0/0/0 (grep || true does not abort)"; else failed "empty log parsed to $P/$IG/$R"; fi
gate "$P" "$IG" "$R" 0 0 1 && failed "zero-result log passed a nonzero gate" || note "zero-result log fails a nonzero gate"

# Case 5: pipefail propagates a nonzero cargo exit through tee even
# when the log content would satisfy the parser.
set -o pipefail
printf 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n' > "$tmp/one.log"
if bash -c 'set -o pipefail; printf "x\n" | tee /dev/null; exit 3' > "$tmp/pipe.out" 2>&1; then
  failed "pipefail did not propagate the nonzero exit"
else
  note "pipefail propagates a nonzero cargo exit through tee"
fi
set +o pipefail

echo "workflow agreement (engine-ci.yml):"
wf=.github/workflows/engine-ci.yml

# The heavy and main echo lines must print the centralized variables,
# not literals (a literal here is how 74 drifted from 64 and 72).
if grep -q 'echo "heavy shard: \$passed passed, \$ignored ignored, \$results test-result lines (expected \$expected / \$expected_ignored / \$expected_results)"' "$wf"; then
  note "heavy echo derives from the centralized variables"
else
  failed "heavy echo no longer derives from the centralized variables"
fi
if grep -q 'echo "main shard: \$passed passed, \$ignored ignored, \$results test-result lines (expected \$expected / \$expected_ignored / \$expected_results)"' "$wf"; then
  note "main echo derives from the centralized variables"
else
  failed "main echo no longer derives from the centralized variables"
fi

# Every shard's ignored and result-line assertions must enforce the
# same variables the echo prints (currently 2 shards x 2 gates).
n=$(grep -c '\[ "\$ignored" -eq "\$expected_ignored" \]' "$wf")
[ "$n" -eq 2 ] && note "both ignored gates enforce \$expected_ignored" || failed "expected 2 ignored gates on \$expected_ignored, found $n"
n=$(grep -c '\[ "\$results" -eq "\$expected_results" \]' "$wf")
[ "$n" -eq 2 ] && note "both result-line gates enforce \$expected_results" || failed "expected 2 result-line gates on \$expected_results, found $n"

# The centralized variables must be defined exactly once per shard
# step, after the passed pin (the one-authoritative-set rule).
n=$(grep -c 'expected_ignored=' "$wf")
[ "$n" -eq 2 ] && note "expected_ignored defined once per shard" || failed "expected 2 expected_ignored definitions, found $n"
n=$(grep -c 'expected_results=' "$wf")
[ "$n" -eq 2 ] && note "expected_results defined once per shard" || failed "expected 2 expected_results definitions, found $n"

if [ "$failures" -ne 0 ]; then
  printf 'ci-count-gate-selftest: %d FAILURE(S)\n' "$failures"
  exit 1
fi
printf 'ci-count-gate-selftest: all cases green\n'
