# Tally roadmap execution log

One line per merged PR, appended by the orchestrator after merge (see
[PROMPT_PLAYBOOK.md](./PROMPT_PLAYBOOK.md) §7.1). This log is the
orientation input for phase selection: the current phase is the lowest-
numbered phase whose exit criterion is not yet evidenced here.

Format:

```
| date | PR | phase | invariant established | evidence |
```

Evidence must name a real artifact: a test (crate::module::test_name), a
signed compatibility-matrix receipt id, a migration version, or a demo
scenario transcript reference. "Done" is not evidence.

| Date | PR | Phase | Invariant established | Evidence |
| --- | --- | --- | --- | --- |
| _(none yet — Phase 1 has not started)_ | | | | |
