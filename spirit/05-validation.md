# 05 — Meaningful Validation

A test exists to catch a specific failure. If you can't name the
failure it would catch, don't write it. This document is the rubric
we hold validation code against.

## What counts as meaningful

A validation is meaningful when all three hold:

1. **It exercises the real path.** The code under test is the code
   that runs in production, not a simplified model, stub, or
   parallel implementation written for the test.
2. **It can fail for a reason specific to DevDev.** A broken DevDev
   invariant, a regressed API, a drifted doc claim — something that
   names a property we chose. Not something the operating system,
   the compiler, or `std` guarantees for free.
3. **The claim it proves matches the claim it appears to prove.**
   No narrowing the assertion after the fact to make a weaker test
   pass for a stronger advertised property.

If any of the three fails, the test is noise.

## Anti-patterns

### Tautologies

Tests that restate a property of the platform, the standard library,
or basic arithmetic. They pass everywhere, always, and carry no
information.

> **Example.** "Prove `exec` has no sandboxing" by spawning a
> process that writes a file outside the mount and asserting the
> file appears. This passes on every OS because that is the default
> behaviour of process spawn. It doesn't validate a DevDev property;
> it validates that Linux is Linux.

If the test would pass on a codebase that didn't exist, it isn't a
test of this codebase.

### Off-path validation

Tests that construct a fake version of the object under test and
assert properties of the fake. Changes to the real path can break
production without breaking the test.

> **Example.** A "filesystem" test that builds its own
> `HashMap<PathBuf, Vec<u8>>` and calls methods on it, instead of
> driving `Fs` through the same surface callers use.

The real path may be harder to reach — mount, PTY, IPC, subprocess.
Reach it anyway. If reaching it is genuinely impractical, the
validation belongs at a different layer, not in a faithful-looking
fake.

### Motte-and-bailey

The advertised claim is strong; the test quietly proves a weaker
one. The test passes; the strong claim remains unproven; the reader
assumes it was.

> **Example.** README says "the agent's tool calls are visible to
> the daemon." Test asserts "a tool call of type X, with a specific
> id the test itself injected, is visible." The weak form passes
> deterministically; the strong form (arbitrary agent-chosen tool
> calls, in realistic sessions) is never exercised.

If the test and the claim don't match, either strengthen the test
or weaken the claim. Don't let the gap sit.

### Shape-only assertions

Tests that check a value is of the right type or has the right
field names, without checking the value is correct.

> **Example.** Assert the IPC response is valid JSON with a
> `status` key. Don't assert what `status` is, or whether it
> reflects the daemon's actual state.

Shape checks have a place (schema contracts, serialization
round-trips), but they don't substitute for behavioural checks.

### Unreachable code as documentation

`#[cfg(target_os = "macos")] fn unsupported() { panic!(...) }` in a
repo that never runs macOS CI proves nothing. The code never
executes. The README line it "enforces" is just prose either way —
put the prose where the reader will see it, not in a dead test body.

## The rubric

Before writing a validation, answer three questions. If you can't,
don't write the test:

1. **What specific failure does this catch?** Name the regression,
   the drift, or the bug. "Something breaks" is not an answer.
2. **What real code runs when this test runs?** If the answer names
   a stub, mock, or test-only path that mirrors production, the
   test is off-path.
3. **Does the assertion match the advertised claim, or a convenient
   subset of it?** If a subset, write down the gap explicitly, in
   the test's own doc comment, so the next reader can decide
   whether to close it.

## Corollary: meaningful skipping

Skipping is not the same as failing silently. A test gated on a
dependency the host may not have (WinFSP, live Copilot, a GitHub
token) must **skip with a message that names the dependency** when
the dependency is absent. Returning early with no output, or
panicking with a generic error, masquerades as a pass and defeats
the validation.

## Corollary: meaningful manifests

When a manifest claims "this doc line is validated by this test,"
the manifest is itself subject to the rubric. A row whose `test`
field points at an existing passing test proves only that the test
exists — not that the test validates the claim. If the mapping is
worth asserting mechanically, the meta-check that enforces it must
also meet the rubric: real path, specific failure, matched claim.

## What this document is not

- Not a style guide for test code.
- Not a gate on test coverage numbers.
- Not a prohibition on fast, unit-scale tests — small tests can and
  should be meaningful under the rubric above.
- Not a claim that every property is testable. Some properties
  (performance-under-load, agent behaviour quality, security
  posture) are better served by prose, benchmarks, or external
  review. Mark those explicitly rather than writing a test that
  pretends to cover them.
