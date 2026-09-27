# Restart after model correction and frozen-config acceptance failure

The user explicitly requires zai-coding-plan/glm-5.3-flash. The prior run completed
this task locally but could not be accepted because the harness configuration
changed after dispatch. This is a fresh verification attempt under the corrected
profile and the committed Windows validation wrapper. Review the existing task
comments, commits, corpus manifests and prior GPU evidence before doing work.

Preserve completed implementation and frozen baseline outputs. Do not regenerate
or replace baseline outputs merely to make a comparison pass. Re-run the focused
Lap tests, engine corpus gates and existing baseline comparison without update
mode; record the exact evidence and any real unresolved criteria. Repair only a
reproduced task-scoped failure. An absent or failing required check is not a pass.

The active harness configuration is frozen for the duration of this attempt.
Do not edit config.yml, change models/validation commands, or install more global
command shims. The checked-in harness-validation.ps1 already invokes the real
npm.cmd and propagates its exit code. Report a genuine configuration problem via
the managed question/checkpoint mechanism so the owner can prepare a new attempt.
The harness owns this assigned issue's status. Comment with evidence; do not close
or reopen the assigned issue yourself. Reuse engine issue rapidraw-39c and existing
proof rather than recreating the corpus. The old read-only lap-control-source
snapshot is provenance, not the location of the current runtime configuration.
