# Security

## Scope

DevDev runs agent-driven processes against your real filesystem under
your user account. It does **not** claim to be a sandbox. Virtual
workspaces are a convenience — a friendly path for tools that expect
a real directory — not a containment boundary.

If you're evaluating DevDev for security-sensitive work: today,
processes launched through `Workspace::exec` can read and write host
files outside the mount if they want to, use your network, and
execute as your user. Real containment (Linux namespaces + seccomp,
Windows job objects) is on the [roadmap](ROADMAP.md), not shipped.

## Reporting a vulnerability

Please **do not** open a public GitHub issue for security reports.

Instead, use GitHub's private vulnerability reporting:

**<https://github.com/goldenwitch/devdev/security/advisories/new>**

Include:

- The affected version or commit SHA.
- A reproducer or minimal proof-of-concept.
- Your assessment of impact.

We'll acknowledge within a week, keep you posted on remediation, and
credit you in the fix announcement unless you prefer otherwise.

## What counts as a security issue

In scope:

- A path escape from the virtual workspace that is *unintended* by
  the current design (i.e., not the `exec` subprocess behavior we
  already disclose).
- A remote-triggerable code path (IPC, MCP server) that executes
  arbitrary commands.
- Credential leakage: DevDev reading, logging, or transmitting
  `GH_TOKEN` / gh-CLI credentials beyond their intended use.

Out of scope (but still welcome as regular issues):

- Anything covered by the non-containment disclosure above.
- Dependencies' vulnerabilities where DevDev is not actually exposed.
- Missing hardening on features that are explicitly
  "roadmap / aspirational".
