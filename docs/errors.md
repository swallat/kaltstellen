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
| `KS114` | Stopping or starting a Windows service through the background helper failed, or the helper is not there (M4e). |
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
| `KS305` | `usage.json` (suggestions from use, H5) could not be read or written. |
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
| `KS801` | The update check failed: no connection, an HTTP error or a timeout (UP1). |
| `KS802` | The release cannot be used: unreadable metadata, no executable for this system, or no SHA-256 for it in `checksums.txt`; or `update --force` while no release is published. Nothing was changed. |
| `KS803` | The downloaded executable does not match its SHA-256 in `checksums.txt`. Nothing was changed. |
| `KS804` | This copy cannot update itself: installed by a package manager (scoop, winget, Chocolatey), below Program Files, in a folder that is not writable, or a development build. The message says how to update instead. |
| `KS805` | Renaming the executable failed; the old one has been put back. |
| `KS806` | The new version did not start (`--version` or the new tray failed); the old one has been put back and, if a tray ran, started again. |
| `KS807` | `update.json` (data directory) could not be read or written. |
| `KS808` | The tray could not be stopped for the update (it does not know the `quit` request, is quitting already, or did not end within 30 s). Nothing was changed. |
| `KS901` | Logging could not be set up. |
| `KS1001` | Installing, removing or controlling the background helper service failed (M4). |
| `KS1002` | The background helper is not reachable: not installed, stopped, or its pipe is not served by the system account. |
| `KS1003` | Malformed message on the helper's pipe, or another protocol version. |
| `KS1004` | `helper.toml` could not be read or is invalid. |
| `KS1005` | The action needs administrator rights and they were not granted (the UAC prompt was declined). |
| `KS1006` | The helper could not start or read its process event session (rights, or another session of the same name). |
