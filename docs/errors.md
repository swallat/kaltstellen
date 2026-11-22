# Error codes

Stable error codes (spec E1). Every error in the log and on the command line
has the form `KSnnn [step] program: reason`; step and program are present
when they apply. Codes are never renumbered or reused. The source of truth is
`src/error.rs`; a unit test checks that every code is listed here.

| Code | Meaning |
| --- | --- |
| `KS101` | Taking the process snapshot failed. |
| `KS102` | Needs administrator: the process runs at a higher integrity level (P7). |
| `KS103` | Protected process (PPL); cannot be opened (P7). |
| `KS104` | Hard-protected program; never touched (P6, A6). |
| `KS105` | Suspending a process failed. |
| `KS106` | Resuming a process failed. |
| `KS107` | Trimming the working set failed. |
| `KS108` | Program did not close in time and `on_timeout = "keep"` (A2). |
| `KS109` | Terminating a process failed. |
| `KS110` | Starting a program failed. |
| `KS111` | Starting a program failed because it requires elevation (P7). |
| `KS112` | Minimising the windows of a process failed (W7). |
| `KS113` | Warn-list program selected without confirmation (`--force`) (P6). |
| `KS199` | Other operating-system error while handling a process. |
| `KS201` | Configuration file could not be read. |
| `KS202` | Configuration file is not valid TOML or has unknown keys (C3). |
| `KS203` | Configuration violates a rule C1 to C7. |
| `KS204` | `settings.json` could not be read or written. |
| `KS301` | State file could not be read or written. |
| `KS302` | State file was corrupt and has been renamed (S9). |
| `KS303` | Another switch holds the state lock (exit code 5). |
| `KS401` | Unknown profile (exit code 4). |
| `KS402` | Request superseded by a later one (W3, exit code 3). |
| `KS501` | Tray instance not reachable. |
| `KS901` | Logging could not be set up. |
