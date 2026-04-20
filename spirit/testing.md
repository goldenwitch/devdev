# Testing Standards

## Every test must be falsifiable

A test that cannot fail is not a test. If there is no input or state that would cause the assertion to produce a different result, the test is tautological and must be rewritten or removed.

## Every test must document its intent

Each test states what property it verifies and what regression it guards against. If the intent cannot be stated concisely, the test is doing too much.

## Assert both the positive and negative case

A test that only checks the happy path is half a test. After a successful operation, verify the expected side effects. After a failed operation, verify nothing changed.

## Use distinct values to prove change

When verifying that content was replaced, use values that differ between old and new state. Identical values only prove metadata changed — distinct values prove the substance changed too.

## Test helpers follow production rules

Test utilities must obey the same invariants as production code. A helper that silently violates a system-wide convention will produce tests that pass for the wrong reasons.

## One test, one property

A test verifies a single behaviour. If a second property matters, write a second test.
