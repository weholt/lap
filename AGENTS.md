# Repository instructions

<!-- pbh:pebbles:start -->
## Pebbles issue tracking

Use `pb` for issue tracking from the repository root. Use this reference before calling help;
only use `pb <command> --help` for syntax not covered here or an installed-version mismatch.
Never edit `.pebbles/events.jsonl` directly or substitute Markdown task lists for Pebbles.

### Find and inspect work

```text
pb ready                         # Open issues with no blocking dependencies
pb list                          # Open and in-progress issues
pb list --all                    # Include closed issues
pb list --status open
pb list --blocked
pb show <id>                     # Description, dependencies, and comments
pb show <id> --json
pb ready --json
pb list --json --all
```

Use real IDs returned by Pebbles in place of `<id>` and the other angle-bracket placeholders.
Read the issue and its acceptance criteria before changing code.

### Record work

```text
pb create --title "Title" --type task --priority P2 --description "Context and acceptance criteria"
pb update <id> --title "New title" --description "Complete replacement description"
pb update <id> --type bug --priority P1
pb comment <id> --body "What changed; tests and results; next steps"
```

Quote values containing spaces. `--description` replaces the description; use comments to
append progress, decisions, and exact failing/passing validation commands. File unrelated
discoveries as new issues. Priorities are `P0` through `P4` (highest to lowest); common types
are `task`, `bug`, `feature`, and `epic`.

### Issue status and harness ownership

For manual work outside `pbh run`, start and finish the issue with:

```text
pb update <id> --status in_progress
pb close <id>
pb reopen <id>                   # Return a closed issue to open
```

Statuses are `open`, `in_progress`, and `closed`. Close only after the acceptance criteria
and required validation pass; record the evidence and remaining work in a comment first.

When invoked by `pbh run`, work on the assigned issue.
Do not close, reopen, or update the assigned issue; the harness owns tracker state.
Use comments for evidence and leave status transitions to the harness. This harness-specific rule
takes precedence over a manual start/close workflow elsewhere in this file.

### Dependencies and hierarchy

```text
pb dep add <issue> <depends-on>              # First issue is blocked by the second
pb dep rm <issue> <depends-on>
pb dep tree <issue>
pb dep add <child> <parent> --type parent-child
```

Hierarchy changes can rename child IDs. Re-read `pb list --all` after adding a parent and use
the current IDs for subsequent commands. Blocking dependencies and parent-child links are distinct.

If the repository has no tracker yet, initialize it once with `pb init --prefix <prefix>`.
<!-- pbh:pebbles:end -->


## RAW development extraction workflow

The governing specification is `docs/raw-development/spec.md`; the importable
contract and logical-to-native ID map are beside it. The spec is copied verbatim
from RapidRAW: its original relative RapidRAW source links refer to the isolated
RapidRAW checkout below, not same-named Lap files.

Work in these two dedicated local checkouts only:

- Lap: `C:/Users/Thomas/Desktop/lap`, branch `feature/raw-development`.
  The previous `codex/rapidraw-development-panel` branch was renamed to this
  branch. `pebbles-harness/raw-development` remains historical, not the working
  branch for new panel changes.
- Engine: `C:/Users/Thomas/Desktop/RapidRAW-engine`, branch
  `feature/lap-engine-extraction`.

`C:/Users/Thomas/Desktop/RapidRAW` and any other existing Lap checkout are
reference-only. Do not modify them. Do not push, publish, deploy, merge into main,
or create external messages. Local commits with issue IDs are authorized.

Lap coordinates the imported plan. For extraction changes, create or reuse a
linked issue in RapidRAW-engine using pb and record the coordinating Lap ID.
Obey that checkout's AGENTS.md RED-GREEN workflow, record actual test evidence,
and commit separately in each repository. Never edit a Pebbles event log by hand.
Do not run the historical RapidRAW backlog; work only on the assigned Lap issue
and its explicitly linked extraction issue. The harness owns the assigned Lap
issue status; comments are allowed and encouraged.

Prepend `$env:USERPROFILE/.cargo/bin` before every Rust invocation. This machine
has an older standalone GNU compiler earlier on its default PATH. Use rustup
1.98/MSVC for the extraction. Never use `--all-features` on Windows. Machine-local
toolchain configuration, targets, dependency caches and logs must stay untracked.

Write meaningful failing behavior regressions before implementation, record RED
evidence, then implement and record GREEN. Run the issue's actual verification,
not only the harness's initial `git diff --check`. During TASK-101 establish the
Lap test command and extend harness validation commands with executable focused
gates as appropriate. Existing baseline errors must be distinguished from new
regressions and never represented as passing checks.

Treat missing fixtures, unsupported hardware, interactive-verification access and
distribution decisions honestly. Dependencies must pass before downstream work.
Do not claim real RAW or GPU parity using mocks or synthetic images. Do not claim
cross-platform qualification from unexecuted CI definitions. An unmet required
implementation criterion blocks completion; only the explicitly scoped final
readiness assessment may complete with a BLOCKED release result.

Epics are completion reviews, explicitly dependent on all children. Do not use
an epic assignment to implement uncompleted child work or bypass dependencies.
The initial writable adjacent-sidecar behavior is the selected design. Preserve
read-only-library failures with retry/retained session state; do not invent a
second recipe authority. Preserve original bytes and all acknowledged recipes.
