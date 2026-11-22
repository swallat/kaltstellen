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
| `KS205` | The main window could not write the configuration: the result would be invalid, the file changed on disk meanwhile, or a file error. |
| `KS301` | State file could not be read or written. |
| `KS302` | State file was corrupt and has been renamed (S9). |
| `KS303` | Another switch holds the state lock (exit code 5). |
| `KS304` | `seen.json` (unassigned programs, N1) could not be read or written. |
| `KS401` | Unknown profile (exit code 4). |
| `KS402` | Request superseded by a later one (W3, exit code 3). |
| `KS403` | Switch interrupted (Ctrl+C, Ctrl+Break, console closed); what was frozen is resumed (exit code 1). |
| `KS501` | Tray instance not reachable. |
| `KS502` | Single-instance mutex of the tray could not be created (L1). |
| `KS503` | Tray window or message loop could not be set up. |
| `KS504` | The tray's named pipe could not be created or served (L1). |
| `KS505` | Malformed pipe message, or another protocol version (exit code 5). |
| `KS601` | The Run key for start at sign-in could not be read or written. |
| `KS701` | The main window could not be set up. |
| `KS702` | The Microsoft Edge WebView2 Runtime is not installed; the main window cannot open (exit code 1). |
| `KS901` | Logging could not be set up. |
