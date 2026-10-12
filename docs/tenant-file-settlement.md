# Observe the original tenant gateway settlement

This revision changes tests and evidence only. All107 production inputs and the
frozen/target binary retain the PR131 compiled source bytes. Reader, permission,
hold, owner and deadline contracts remain unchanged; the fixture neither retries
a failed backend observation nor releases an unknown hold.

## Original failure and controlled mechanism

PR131 head `6de964a35e73876d2daf7bcb73aa38a2e67a4f74`, merge
`b3e8ed9b42dca64e05bebc7dd3353f49eaa1cb76`, Release qualification
`38200658796`, Linux ARM job `114653118260`, failed in file group2. The two model
POSTs had occurred and the fixture's GET accepted a completed receipt; its
subsequent original15-second SQLite hold assertion timed out. The original log
has no private wire/admission receipts or hold row. Its unique historical cause
therefore remains unknown. Later candidate packaging/archive/install checks did
not execute. Original snapshots/same-head successes remain separate in
`/tmp/jiaclaw-oct12-tenant-file-delivery/official-round59-failed`.

The original frozen binary's public seven groups passed locally. A controlled
real gateway/backend/model task nevertheless reproduced the exact hold symptom:
a test-only loopback relay holds a GET150ms and returns503 to an overlapping GET.
Extra fixture polling can interfere with the separate original gateway observer.
Two model POSTs complete, later GETs show completed/session_committed/active=false,
but the gateway correctly retains needs_review/backend_outcome_unknown. The
original hold assertion still fails. This is one proven controlled mechanism,
not proof that the historical ARM runner had the same wire failure.

Persistent pre-fix source `479cc19517cc6e12014a66e513e2f2e2448194f3` captured exit1
on that actual call pattern, preserving the original order and15-second assertion.
The relay has no production hook, paid credential or supplier claim.

## Revised success observation

Both successful file tasks require the original gateway worker to remove its
matching hold before the fixture adds a receipt GET. The hold query, full reply,
captured tool enum, two model POSTs, two users, readOnly/crossuser facts, unsafe
sources, cancellation and actual eight-reader assertions remain. The original
15-second hold observation budget is also checked after its query returns.
Logical rejection is not preemption of blocking SQLite/OS work.

Alice's success case retains the controlled relay; actual private GET receipts
must be nonempty, all200 and nonoverlapping. Bob uses the direct native endpoint.
Checkpoint records bind both hold observations to the before-client-GET phase.
After successful settlement, the gateway stops, the relay is fully retired and
the same direct backend URL is restored before other HTTP/browser/read-owner
matrices. Production's known/unknown outcome and retained-capacity policy do not
change. A completed later GET still cannot clear an unknown/restarted hold.

The original public HTTP suite and four browser groups must pass independently
on the revision. The new fixed remote head must obtain its own seven actual jobs,
two platforms63 mandatory Python steps, Linux11 actual Chromium suites, four
candidates each13 unique archive/source/install checks, authenticated artifact
API digests and149 audit sites. The extra site requires the actual settlement
checkpoints/relay receipts. The old audit remains pinned to the old head without
requiring this new marker.

The new receipt helper is also included in the release workflow's explicit
pull-request path filter, so future helper-only edits trigger all four candidate
jobs. The main CI has no such path filter. This closes the fresh Standards P3;
the original omission/report remains separate from the historical ARM failure.

## Evidence limits

Compiled `da8b942f`1302 Rust plus one ignored, build/fmt/required Clippy and other
process qualifications are inherited, not rerun by this fixture revision. Fresh
source/delivery/publication/official Standards and Spec remain separate; history
since cumulative4b is inherited, not exhaustively re-reviewed. Original failures,
local controlled RED/GREEN and new official qualification are separate facts.

No real supplier, public TLS, release installation, business quality, Agent
browser, physical failure, independent ACL or StateKnot durable qualification is
inferred. Original ZIPs lack complete local physical digests; official evidence
uses authenticated artifact API digests and actual logs. No merge/public release.
