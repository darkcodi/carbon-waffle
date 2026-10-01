# Carbon Waffle

A Linux wireless research workbench in Rust and iced 0.14. It wraps Aircrack-ng, Hashcat, and hcxtools with a native GUI, live tool output, and cancellable jobs.

```sh
cargo run -- --demo
cargo run
```

Demo mode is selected with `--demo` at launch and uses synthetic networks and simulated jobs. It never executes the wireless tools or writes captures.

The app has **Capture** and **Recover** tabs. Capture starts on **Elevate**: click **Elevate permissions** to request desktop authorization once for radio operations, including adapter restoration. Recover opens your saved-capture library immediately, without elevation, a wireless adapter, or a new capture. Closing the app ends the elevated helper. Demo mode does not request authorization. If authorization is cancelled or the helper exits unexpectedly, return to Capture → Elevate to retry; offline recovery remains available.

## Requirements

- Linux, a Rust toolchain supporting edition 2024 and iced 0.14, and a Wayland or X11 desktop.
- `airmon-ng`, `airodump-ng`, `aireplay-ng`, and `aircrack-ng` on `PATH`.
- `nmcli` on `PATH` for automatic adapter handoff on systems using NetworkManager.
- Optional: `hcxpcapngtool` (hcxtools) for conversion and `hashcat` plus its GPU compute runtime for GPU recovery.
- `pkexec` (polkit) and a running desktop polkit authentication agent for session authorization.
- An adapter/driver supporting monitor mode; injection support for deauthentication.

The **Required dependencies** section in Elevate is collapsed by default. Expand it to see detected executables and use **Refresh** to check again after installing a tool. On NixOS, `nix-shell` provides build and GUI runtime libraries, plus the external tools. Desktop polkit configuration still comes from your system.

The UI renders on the CPU with tiny-skia; it does not require Vulkan, OpenGL, or GPU drivers for rendering. Development builds optimize rasterization and text shaping so ordinary `cargo run` avoids the large scrolling stalls of an unoptimized renderer. The first build takes longer; subsequent app-only rebuilds reuse the optimized dependencies. This keeps the rendering stack suitable for portable packaging such as AppImage.

## Workflow

The **Capture** tab has four steps: **Elevate → Monitoring → Discover → Capture**. Back and Next stay at the bottom while the content scrolls; Next requires authorization, monitor setup, and then a target selection. Back is hidden on Elevate and Next is hidden on Capture. Future steps stay disabled; use Back or an earlier step to revisit them. The **Recover** tab has one standalone screen with no step selector or Back/Next buttons. Switching tabs preserves the Capture step and recovery inputs, and does not start or stop any job. Stop an active radio job before starting an offline job. Activity and Stop all remain available in both tabs.

1. In Elevate, expand **Required dependencies** to review tools, click **Elevate permissions**, and complete the desktop prompt. Once authorized, click **Next** to open Monitoring. Simply opening the app or changing steps does not request permissions.
2. Select the research adapter and enable monitor mode. If NetworkManager manages it, the app temporarily releases only that adapter with `nmcli device set <adapter> managed no` before starting `airmon-ng`. This disconnects that adapter from Wi-Fi. Success requires a new monitor interface on its PHY; a successful command exit alone is insufficient. Interface names are discovered rather than assuming a `mon` suffix.
3. Start discovery and select a network. Every named SSID appears as a collapsed group, including those with only one BSSID. Expand the group and select a specific BSSID. The selected BSSID stays visible in the group header when collapsed. Each BSSID shows signal, channel, authentication, and associated clients; it updates from airodump's CSV files. Hidden networks are grouped in a **Hidden networks** section, collapsed by default. Expand it to select one. Choosing a different target resets that Capture workflow's recording input; Recover keeps its own independent inputs, and existing files remain on disk.
4. In Capture, click **Start recording**, then reconnect a phone or laptop to the selected Wi-Fi. Discovery stops automatically first; recording stays on the selected BSSID and channel. The screen shows a recording timer and how many associated devices have been seen.
5. If manual reconnection is inconvenient, expand **Need help reconnecting a device?** during recording. Select a device or enter its MAC, then explicitly send a finite disconnect request. An empty MAC addresses all clients of that AP. The burst count is aireplay's count, not an exact packet count; devices may ignore the request.
6. Click **Stop & check**. The app stops recording and any active disconnect request, waits for the capture to be saved, then checks it with Aircrack-ng. Capture shows **Handshake found**, **No handshake found yet**, or an error with retry options. **Open in Recover** transfers a checked WPA/WPA2 PSK recording and its network into the Recover tab. It rechecks that the file still exists and has not changed. Checks happen after recording stops, not continuously during capture.
7. In Recover, choose a recording from the library. Its WPA networks and handshake counts are read automatically for Aircrack-ng; a single usable network is selected automatically. Choosing Hashcat prepares the selected capture automatically using hcxtools. Existing `.hc22000` and `.22000` files can also be imported. Choose **Dictionary** for complete passwords or **Pattern / regex** to combine words, digits, and literals, then use the file chooser to select a wordlist. Aircrack-ng uses the network selected in Recover; Hashcat processes every record in its input file. Recovery starts only when you click **Start recovery**.
8. Restore managed mode when finished. The app stops the monitor interface and returns the original adapter to NetworkManager if it released it. Failed or cancelled setup also attempts this rollback. If restoration fails, **Restore managed mode** remains available to retry. Normal window closure stops jobs and attempts restoration. Interfaces already in monitor mode before launch are left under your control.

Open **Activity** at the bottom to inspect command output. Every command retains its own full-width panel, independent scrollbar, and top-right **Copy** button for its command and output. Active jobs can be stopped from their screen or Activity; **Stop all** remains available while any job is running. Failed commands and inconclusive checks open Activity automatically; successful handshake checks appear directly on Capture.

New recordings are saved automatically under `~/.carbon-waffle/captures/session-…/`, regardless of the launch directory. Recover lists nonempty capture and imported WPA hash files, newest first, with network names when available, age, and size. Conversion and recovery outputs go into a session’s `recovery/` subdirectory and do not clutter the capture list. Directories are private. Use **Import capture** to copy older or external files into the library without changing the originals; **Previous captures** in the file chooser opens the old `captures/` folder when launching from a previous working directory. The built-in iced file chooser also selects wordlists and needs no extra dialog packages. Directory reading and imports run in the background. Capture files created by elevated tools may be root-owned. Recovered secrets may appear in CLI output and result files. Nothing is uploaded. Hashcat uses a session-local potfile; the GUI reports its exit status and result path rather than interpreting all progress/result formats.

Recovery settings are saved before offline jobs, when switching tabs, and on normal closure. On reopening, the app restores the last tab, capture/hash paths, BSSID, engine, wordlist, and pattern. Paths are stored as absolute paths in `$XDG_STATE_HOME/carbon-waffle/recovery.json` (or `~/.local/state/carbon-waffle/recovery.json`), with private file permissions. Demo mode does not load or save this state. Recover opens on the library after reopening; selecting a capture restores access to the recovery options. Restoration never authorizes the app or starts a job automatically. This remembers the setup, **not the exact cracking position**: starting again begins a new search. Hashcat restore files and pattern checkpoints are not implemented.

The adapter handoff does not run `airmon-ng check kill` or restart system networking services; other adapters stay managed. Adapters that were already unmanaged are not assigned to NetworkManager afterward. Without `nmcli`, the selected adapter must already be free of network management. Independent supplicants or other network managers can still interfere; Activity exposes the tool diagnostics. A separate research adapter is useful when you need to keep a network connection.

## Pattern recovery

In Recover, choose **Aircrack-ng** or **Hashcat** using the selector at the top, select **Pattern / regex**, supply a source wordlist (one word per line), and enter a pattern. **Preview & count** shows sample passwords and the number of candidate combinations without expanding the entire search. **Start recovery** streams candidates directly into the selected tool. No combined dictionary file is created, and no additional external generator is required. Source loading, counting, and generation run off the UI thread; Stop cancels the generator and the tool. The selected recording and wordlist appear by name; **Change** opens the library or file chooser. No capture, hash, or wordlist paths need to be typed.

| Pattern | Candidates |
| --- | --- |
| `{Word}{Word}[0-9]{3}` | Two capitalized words followed by exactly three digits, such as `AlphaBeta123` |
| `{word}{5}` | Five words joined as written in the source list |
| `({word}-){4}{word}` | Five words separated by hyphens |
| `{WORD}[0-9]{2,4}` | One uppercase word followed by two to four digits |
| `(summer\|winter)-[0-9]{4}` | A literal alternative followed by a hyphen and four digits; no wordlist needed |

`{word}` preserves each entry; `{Word}` lowercases ASCII letters and capitalizes the first letter; `{WORD}` uppercases them; `{lower}` lowercases them. Each word slot is an independent choice, including repeats and both word orders. Source duplicates remain duplicates. Spaces in source words are preserved, CRLF line endings are accepted, and blank, overlong, or NUL-containing entries are skipped. Wordlists and retained word variants are limited to 256 MiB for pattern mode; ordinary dictionary mode still lets the tool read its source file directly.

This is a **finite regex subset**, not a general regex matcher: character classes, `\d`, `.`, groups, alternatives, `?`, and bounded repeats such as `{3}` or `{1,5}` are supported. Escape regex punctuation with `\` when it should be literal. Candidates match the entire pattern; outer `^` and `$` are optional. Unbounded `*`, `+`, and `{n,}`, lookarounds, and backreferences are rejected. Character classes expand over printable ASCII, and case conversions affect ASCII letters. Only complete WPA passphrases of 8–63 bytes are sent; raw 64-digit PSKs are outside this mode.

The count includes duplicate generation paths and excludes candidates outside the length limit. Very large counts saturate at the displayed `≥` value. With N usable source words, two words plus three digits can mean N² × 1,000 combinations; five words can mean N⁵. Streaming avoids the expanded file, but does not reduce the number of guesses. The on-screen generated count measures candidates supplied to the pipe, not passwords already checked by the tool. Activity records the pattern, source, command, and tool output. Aircrack-ng receives `-w -`; Hashcat uses straight mode with stdin and literal candidate handling.

## Implementation

- `src/app.rs`: workflow state, adapter ownership, and job coordination.
- `src/app/library.rs`: home-directory capture storage, metadata, background discovery, and private imports.
- `src/app/browser.rs`: built-in capture and wordlist file chooser.
- `src/app/recovery.rs`: private, atomic recovery-settings persistence and restoration without starting jobs.
- `src/app/view.rs` and `src/app/appearance.rs`: step-based screens and shared visual styles.
- `src/model.rs`: sysfs adapter discovery, airodump CSV parsing, and input validation.
- `src/command.rs`: typed operations and argument builders. Commands use `Command::args`, never a shell; absolute paths keep file names from becoming options.
- `src/runner.rs`: worker IPC, concurrent output readers, snapshot delivery, and process-group cancellation.
- `src/runner/session.rs`: button-triggered authorization, session helper lifecycle, and concurrent privileged job dispatch.
- `src/runner/inspection.rs`: bounded capture inspection, offline capture network lists, selected-BSSID handshake results, and reliable delivery separate from Activity text.
- `src/pattern.rs`: finite pattern parsing, length-aware combination counts, previews, and cancellable candidate streaming.
- `src/monitor.rs`: adapter-specific NetworkManager handoff, mode verification, and rollback.

During scanning, the worker checks for CSV changes every 100 ms and delivers changed results reliably even when Activity output fills its queue. Identical snapshots do not trigger additional UI updates. Airodump is configured to write its CSV once per second, so new discoveries still depend on that upstream write interval.

The desktop UI runs as your user, retaining normal X11/Wayland access. When you click **Elevate permissions**, `pkexec` launches this executable's `--session-worker` mode once. It receives typed start/cancel requests over private stdin/stdout pipes and runs each radio tool in its own process group; concurrent jobs retain separate output and cancellation. Offline operations use an unprivileged `--worker`, without another authorization prompt. No passwords are collected or stored by the application, and no system daemon or custom authorization policy is installed.

Normal closure stops jobs, restores the session's adapter, and waits for the elevated helper to exit. Closing its control pipe also cancels active jobs inside the helper, including root-owned descendants. It sends SIGINT first to let capture files flush, then SIGKILL after a grace period. Monitoring setup and cleanup commands have a 30-second timeout; requested cancellation does not interrupt rollback. A GUI crash closes its pipes and stops tool processes, but cannot guarantee adapter restoration after setup has finished. If needed, use `sudo airmon-ng stop <monitor-interface>` and then `sudo nmcli device set <restored-interface> managed yes` for an adapter previously managed by NetworkManager.

This is a local development helper, not an installed privileged service. Keep polkit's interactive session authorization: do not make the executable setuid or create a passwordless policy for a user-writable build. The helper executes the tool paths discovered in your environment, including Nix wrappers.

## Scope and validation

The initial workflow covers 2.4/5 GHz discovery and WPA/WPA2 PSK dictionary and finite-pattern recovery. It does not implement WPA3 SAE recovery, enterprise authentication, 6 GHz survey controls, native Hashcat masks/rules, persistent job resume, or live handshake detection while recording. Tool status and errors remain visible in the activity panel; handshake quality and recovery success depend on the input and tools.

```sh
cargo fmt --all -- --check
cargo test
cargo clippy --all-targets -- -D warnings
```

To measure scrolling without opening a window or touching the radio, run `cargo test scroll_rendering -- --ignored --nocapture`. This opt-in benchmark scrolls the actual Elevate and Discover screens, including 250 synthetic BSSIDs and 500 clients, both with unique names and expanded SSID groups, into an in-memory software framebuffer. It reports warmed layout and full-redraw timings, excluding compositor presentation. The app polls job output while jobs run and sleeps between input events when idle. For visual review of Capture states at both supported window sizes, run `cargo test capture_previews -- --ignored`; it writes software-rendered PPM previews to `target/capture-previews/` using synthetic data.

Tests use synthetic scan data, simulated adapters, and fake local subprocesses. They cover deferred authorization, elevation retries and duplicate clicks, session authorization reuse, concurrent jobs, authorization cancellation, helper failure and shutdown, adapter handoff, partial setup failure, restoration retries, misleading tool exit codes, prompt scan updates, scan delivery under heavy Activity output, capture/check sequencing and cancellation, inspection results, and invalidation when the selected file or target changes. They do not request real desktop authorization, scan, transmit packets, change interfaces, or attempt password recovery against real captures. Hardware behavior needs validation on a research adapter and test AP.

Pattern tests cover combination completeness, word casing and separators, byte-length filtering, huge counts, stale preview cancellation, stdin delivery and EOF, early tool exit, and cancellation while a candidate pipe is full. For visual review of Recover states, run `cargo test recover_previews -- --ignored`; previews are written to `target/recover-previews/`.

Flow tests cover independent tabs and inputs, Capture-to-Recover handoff, offline operation without authorization or adapters, capture network selection, and reopening saved settings without launching a process. Invalid saved settings leave the app usable; file persistence tests use temporary directories.

Command formats follow the [Aircrack-ng documentation](https://www.aircrack-ng.org/documentation.html) and [Hashcat WPA recovery documentation](https://hashcat.net/wiki/doku.php?id=cracking_wpawpa2). UI APIs follow [iced 0.14](https://docs.rs/iced/0.14.0/iced/).

Capture-library tests cover discovery, ignored empty files and generated outputs, symlink loops, import collisions, original-file preservation, private permissions, and file chooser filtering. UI previews include the library, empty state, and file chooser.
