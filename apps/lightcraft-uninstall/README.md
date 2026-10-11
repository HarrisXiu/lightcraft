# MSI cleanup helper

This small pure-Rust binary is embedded in the Windows MSI, not installed or shipped in the
portable ZIP. The Remove flow offers two unchecked options: clean settings, edits and generated
data, and separately permanently delete library-owned photos. Ordinary uninstall keeps all data.

The options page shows for both ways to remove LightCraft: **Uninstall** in Windows Settings → Apps
and running the MSI again → **Remove**. Windows uninstalls an MSI product's own Apps entry with a
bare progress bar and no dialogs, so the package hides that entry (`ARPSYSTEMCOMPONENT`) and lists
its own, whose Uninstall runs the package in maintenance mode on these pages
(`MsiExec.exe /I{ProductCode} LIGHTCRAFTUNINSTALL=1`). Both end on a page saying LightCraft was
uninstalled. A silent removal (`/qn`, the entry's `QuietUninstallString`) shows no pages and keeps
all data.

The MSI invokes it as the initiating user in a checked **commit** custom action. It runs only for
an explicitly selected clean uninstall, after the uninstall script succeeded. Install, repair,
upgrade, cancellation and failed scripts never trigger it. It opens no console window.

The helper cleans the configured/default library and the current library remembered in
`ui.json` → `settings.libraryPath`. It deletes named catalog/settings/face-cache files and only
recognized thumbnail/smart-preview files. In the config folder it removes UI settings, logs,
downloaded models and camera profiles. Other libraries, environment-overridden external model
folders, unrecognized files and referenced photos outside `Originals` are kept. It never removes
an entire library root; the empty `catalog.lock` stays to avoid racing another library opener.
Without a catalog snapshot/log the next launch initializes a new library, including demo photos.

Deleting `Originals` requires the separate checkbox. Both choices warn about irreversible loss:
cleaning the catalog loses edits and deleting `Originals` loses photo copies. Symlinks and Windows
junctions are refused, including at ancestors. Paths must be absolute, recognized library markers
must exist, settings JSON is capped at 1 MiB and traversal at 200,000 entries / 64 levels. All
libraries must be available for locking. All targets are checked before the first deletion; an
unreadable/malformed settings file or an active library fails preflight without deleting data.
An I/O failure after deletion starts can leave a partial cleanup; MSI reports failure and the
helper records the path in `%TEMP%\lightcraft-uninstall.log` for manual recovery/cleanup.
When the helper fails, Windows Installer rolls the uninstall back: LightCraft stays installed, and
the error (the package's own text for error 1722) says so, to close LightCraft and uninstall again
or to uninstall without cleanup, and where the log is.

`cargo test -p lightcraft-uninstall` uses synthetic temporary folders only. The compiled MSI's
choice defaults, secure properties, maintenance routing, Apps entry, success/failure pages and
commit-action conditions are checked with `packaging/windows/test-msi.ps1 -Msi <package.msi>` without
installing anything. A full Remove/cancel/repair/upgrade run, from both Settings → Apps and the MSI,
should also be checked in a disposable Windows VM.
