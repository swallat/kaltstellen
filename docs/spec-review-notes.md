# Review notes: core-semantics vs draft v1

Problems found and how the consolidated draft resolves them.

1. **Last-profile storage contradiction.** S5 said the tray offers the last profile; S7 said recovery clears the state file. Resolved: last profile name lives in `settings.json`.
2. **W1 wasteful for shared programs.** "Resume everything, then freeze" would unfreeze and refreeze (and re-trim) programs frozen by both profiles. Resolved: resume skips what the target suspends again.
3. **Undefined cases in W1.** Stopping a frozen process, `start` for a suspended program, programs absent from the target. Resolved explicitly.
4. **Missing single-instance and CLI path.** No definition of how the CLI reaches the tray or runs without it. Added L1 (mutex, named pipe, state lock).
5. **Untested exit behaviour.** Quit vs sign-out differed. Added L2.
6. **`restart_on_neutral` scope.** Unclear if honoured on switches to other profiles. Limited to `neutral`.
7. **Forced kill risks data loss.** A2 terminates unconditionally. Added `on_timeout`.
8. **A6 vs warn list.** "Never touched" conflicted with confirmable categories. Split into two tiers.
9. **Race and relaunch cases.** Children spawned during freeze, apps relaunched by the user: added W6 with explicit non-goal (no watcher).
10. **Untestable requirements.** Q1 ("few MB"), no exit codes, no error contract. Added thresholds, E2, test table.
11. **Config edge cases.** Hot reload with an active profile removed, duplicate hotkeys, invalid option combinations: added C4-C6.
12. **Not covered, left for later:** multi-user machines (only current session considered), fast-user-switching, per-monitor GUI scaling, localisation of the GUI (German strings vs English default), config schema versioning (suggest a `version = 1` key).
