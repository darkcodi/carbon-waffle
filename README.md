# Carbon Waffle

A Linux wireless research workbench in Rust and iced 0.14. It wraps Aircrack-ng, Hashcat, and hcxtools with a native GUI, live tool output, and cancellable jobs.

```sh
cargo run -- --demo
cargo run
```

Demo mode uses synthetic networks and simulated jobs. It never executes the wireless tools or writes captures. You can also switch modes from the window when no jobs or session-owned monitor interfaces are active.

## Requirements

- Linux, a Rust toolchain supporting edition 2024 and iced 0.14, and a Wayland or X11 desktop.
- `airmon-ng`, `airodump-ng`, `aireplay-ng`, and `aircrack-ng` on `PATH`.
- Optional: `hcxpcapngtool` (hcxtools) for conversion and `hashcat` plus its GPU compute runtime for GPU recovery.
- `pkexec` (polkit) and a running desktop polkit authentication agent for privileged radio operations.
- An adapter/driver supporting monitor mode; injection support for deauthentication.

The **Adapter & tool details** section in Monitoring reports detected executables. On NixOS, `nix-shell` provides build and GUI runtime libraries, plus the external tools. Desktop polkit configuration still comes from your system.

## Workflow

The four steps at the top are **Monitoring → Discover → Capture → Recover**. Select any step to revisit it; changing steps does not run a command. Each screen contains the controls for that step, with additional options tucked into expandable sections.

1. Select the research adapter, check interfering processes, and enable monitor mode. The application rediscovers interfaces by PHY after `airmon-ng` runs, rather than assuming a `mon` suffix.
2. Start discovery and select a network. The network list shows signal, channel, authentication, and associated clients; it updates from airodump's CSV files.
3. Start target capture. Any discovery job stops first; the capture is fixed to the selected BSSID and channel.
4. If needed, choose a client and send a finite deauthentication burst during capture. An empty client field addresses all clients of that AP. The burst count is aireplay's count, not an exact packet count.
5. Stop capture, then inspect its handshake information. Check the actual Aircrack-ng output: a capture file's existence does not establish that it contains a usable handshake.
6. Choose a wordlist and run Aircrack-ng, or convert to `.hc22000` and run Hashcat. You can enter existing capture/hash paths, too. Aircrack-ng inspection and recovery use the network selected in Discover; Hashcat processes every record in its input file.
7. Restore managed mode when finished. Normal window closure stops jobs and attempts to restore the monitor interface this session enabled. Interfaces already in monitor mode before launch are left under your control.

Open **Activity** at the bottom to inspect command output. Every command retains its own full-width panel, independent scrollbar, and top-right **Copy** button for its command and output. Active jobs can be stopped from their screen or Activity; **Stop all** remains available while any job is running. Failed commands and capture inspection open Activity automatically.

Capture and recovery files are retained under `captures/session-…/`, with private session directories. Capture files created by elevated tools may be root-owned. Recovered secrets may appear in CLI output and result files. Nothing is uploaded. Hashcat uses a session-local potfile; the GUI reports its exit status and result path rather than interpreting all progress/result formats.

Monitor mode may disconnect the chosen adapter. The app reports conflicting processes but does not run `airmon-ng check kill` or restart system networking services. A separate research adapter is useful when you need to keep a network connection.

## Implementation

- `src/app.rs`: workflow state, adapter ownership, and job coordination.
- `src/app/view.rs` and `src/app/appearance.rs`: step-based screens and shared visual styles.
- `src/model.rs`: sysfs adapter discovery, airodump CSV parsing, and input validation.
- `src/command.rs`: typed operations and argument builders. Commands use `Command::args`, never a shell; absolute paths keep file names from becoming options.
- `src/runner.rs`: worker IPC, concurrent output readers, snapshot delivery, and process-group cancellation.

The desktop UI runs as your user. Radio operations start this executable's `--worker` mode through `pkexec`; offline operations start an unprivileged worker. The authenticated helper receives a single typed request over stdin and executes the tool with a fresh process group. Closing the pipe requests cancellation inside the worker, so it can stop root-owned descendants. It sends SIGINT first to let capture files flush, then SIGKILL after a grace period. A GUI crash closes its pipes and stops tool processes, but cannot guarantee restoration of adapter mode; use `airmon-ng stop <monitor-interface>` if needed.

This is a local development helper, not an installed privileged service. Keep polkit's interactive authorization: do not make the executable setuid or create a passwordless policy for a user-writable build. The helper executes the tool paths discovered in your environment, including Nix wrappers.

## Scope and validation

The initial workflow covers 2.4/5 GHz discovery and WPA/WPA2 PSK dictionary recovery. It does not implement WPA3 SAE recovery, enterprise authentication, 6 GHz survey controls, Hashcat masks/rules, persistent job resume, automatic handshake verification, or a file picker. File paths are editable text fields. Tool status and errors remain visible in the activity panel; handshake quality and recovery success depend on the input and tools.

```sh
cargo fmt --all -- --check
cargo test
cargo clippy --all-targets -- -D warnings
```

Tests use synthetic scan data and fake local subprocesses, including cancellation; they do not scan, transmit packets, change interfaces, or attempt password recovery against real captures. Hardware behavior needs validation on a research adapter and test AP.

Command formats follow the [Aircrack-ng documentation](https://www.aircrack-ng.org/documentation.html) and [Hashcat WPA recovery documentation](https://hashcat.net/wiki/doku.php?id=cracking_wpawpa2). UI APIs follow [iced 0.14](https://docs.rs/iced/0.14.0/iced/).
