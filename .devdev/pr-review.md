# PR review

What to flag:

- **Correctness gaps.** The change does the wrong thing, or the
  right thing only for the happy path. Concrete repro > vibes.
- **Public API churn.** New pub items, breaking signatures, new
  required deps. Worth a sentence even when the change is fine.
- **Unjustified scope creep.** "Drive-by refactors" inside an
  otherwise tight PR. Ask whether they belong in their own commit.
- **Test debt.** New behaviour without coverage, or a fix without
  a regression test.

What to skip:

- Style nits the formatter would catch. `cargo fmt` exists.
- Renaming preferences. The name in the diff is fine.
- Speculative "what if someday" objections. Cross that bridge later.
- Restating what the diff already says.

Tone:

- One thread per concern. Don't pile observations into a single
  comment.
- Quote the line you mean. Anchor the comment to the code.
- If approving, say so plainly. No ceremony.
- Sign off with the takeaway, not with apologies.
