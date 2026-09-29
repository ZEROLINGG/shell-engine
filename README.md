# shell-engine

> **Languages:** [English](README.md) | [简体中文](README-zh_CN.md)

A persistent, long-lived shell session manager for async Rust. Spawn a shell once, then interact with it across multiple commands — state, environment variables, and working directory persist.

```rust
use shell_engine::Shell;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut sh = Shell::new("bash")
        .enable_buffer()
        .spawn()
        .await?;

    sh.send_line("export FOO=bar").await?;
    sh.send_line("echo $FOO").await?;
    let out = sh.output(None, None).await;
    assert_eq!(out.stdout.trim(), "bar");
    sh.exit().await?;
    Ok(())
}
```

## Table of Contents

- [Design Philosophy](#design-philosophy)
- [Quick Start](#quick-start)
- [When to use / When not to use](#when-to-use--when-not-to-use)
- [Feature Flags](#feature-flags)
- [Platform & Environment Support](#platform--environment-support)
- [Minimum Supported Rust Version](#minimum-supported-rust-version-msrv)
- [Contributing](#contributing)
- [Changelog](#changelog)
- [License](#license)

## Design Philosophy

1. **Single session entry, no duplicated logic** — Pipe mode and PTY mode share the same `backend::launch()` startup and the same `OutputPump` decode+dispatch state machine (`stream.rs`), avoiding two copies of read logic that tend to drift apart.
2. **Explicit over implicit** — Buffering, callback granularity, and PTY toggle are all controlled explicitly via the `ShellBuilder` chain; no magic defaults.
3. **Minimal dependencies** — Runtime deps are only `anyhow`, `tokio`, `serde`; PTY-related deps (`rust-pty`, `vt100`) are isolated as optional features.

### Trade-offs

| We chose | Instead of | Why |
|---|---|---|
| Bounded output buffer (drop oldest on overflow) | Unbounded accumulation | Long-running sessions have a memory cap, with `truncated_bytes` stats |
| Optional PTY (feature-gated) | Dual-backend always loaded | `vt100`/`rust-pty` are heavy; pure-pipe users shouldn't pay for them |
| PTY mode merges stdout/stderr | Splitting into two streams | Pseudoterminals naturally merge; splitting needs an extra parse layer for little gain |

## Quick Start

Add the dependency to your `Cargo.toml`:

```toml
[dependencies]
shell-engine = "0.2"
```

Basic usage:

```rust
use shell_engine::Shell;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut sh = Shell::new("bash")
        .enable_buffer()
        .spawn()
        .await?;

    sh.send_line("export FOO=bar").await?;
    sh.send_line("echo $FOO").await?;
    let out = sh.output(None, None).await;
    assert_eq!(out.stdout.trim(), "bar");
    sh.exit().await?;
    Ok(())
}
```

## Features

- **Persistent process** — One spawn, many commands; env, cwd, and shell state survive between calls
- **Two backends** — Pipe mode (default, clean I/O separation) and PTY mode (optional, for full terminal apps, job control, colored output)
- **Two callback modes** — Raw (lowest latency, per-chunk) or Line (buffered per-line with 80 ms idle flush for interactive prompts)
- **Bounded buffering** — Configurable-capacity `OutputBuffer` with overflow truncation tracking
- **Async callbacks** — Hooks for stdout, stderr, exit, close, and pre-send filtering
- **Lifecycle control** — `send`, `send_line`, `send_control_char`, `send_keys`, `send_eof`, `reset`, `exit`, `close`, `join_close`, `join_exit`, and auto-close on drop
- **Control characters** — `send("^C")` auto-detects `^@`–`^_` and `^?` syntax; PTY mode supports full `^A`–`^_` and `^?`, pipe mode supports `^R` (reset) and `^D` (EOF)
- **Screen snapshot** — PTY mode captures rendered terminal content via `vt100` parser (`output_snapshot`)
- **Cross-platform & shell-agnostic** — Built-in defaults for Unix (bash, zsh, sh, fish, python, node) and Windows (cmd, powershell, pwsh, python, node); any other shell or console program works too
- **Encoding support** — Stateful incremental decoder; auto-detects Windows code page
- **One-shot execution** — `exec()` for fire-and-forget commands with optional timeout

## Supported Shells

Any shell or console program that communicates via standard input/output is supported. The following programs have some built-in defaults (startup parameters, initialization input, exit input): sh, bash, zsh, fish, cmd, PowerShell, pwsh, python, node.

Anything else — nushell, elvish, xonsh, irb, sqlite3, custom REPLs, etc. — can also be spawned: unknown executables are launched with zero arguments and no built-in init/exit input (`exit()` then relies on closing stdin / EOF). Use `args`, `init_input`, `exit_input`, `work_dir`, and `env` on the builder to configure them as needed.

## When to use / When not to use

**Use it when:**
- Automation scripts need session state (env vars, working dir, internal shell state) across multiple commands
- Driving interactive REPLs / terminal programs (editors, `htop`) that need a real pseudoterminal (`enable_pty()`)
- You need real-time callbacks, line-level parsing, or bounded buffering on subprocess output

**Do NOT use it when:**
- A few fire-and-forget commands suffice — use `exec()` instead (new process each call, no session)
- You need strict stdout/stderr separation in PTY mode (PTY merges the two; `on_error` won't fire)
- You need process clusters / distributed task orchestration — this crate manages a single local session

## Installation

```toml
[dependencies]
shell-engine = "0.2"
```

PTY support (via `rust-pty` + `vt100`) is enabled by default. To disable it:

```toml
[dependencies]
shell-engine = { version = "0.2", default-features = false }
```

## Feature Flags

This crate supports the following [Cargo features](https://doc.rust-lang.org/cargo/reference/features.html):

- `default`: enabled by default; equivalent to `pty` (PTY backend + `vt100` screen snapshots).
- `pty`: enables the pseudoterminal backend and the `rust-pty` / `vt100` optional deps. When disabled, only pipe mode remains; PTY-only APIs (`enable_pty`, `output_snapshot`, `resize`, etc.) are excluded from compilation.

Pipe-only setup:

```toml
[dependencies]
shell-engine = { version = "0.2", default-features = false }
```

## Usage

### Persistent Shell

```rust
use shell_engine::Shell;
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut shell = Shell::new("bash")
        .enable_buffer()              // Buffer stdout/stderr (default 1 MiB)
        .line_callback()              // Complete lines; partial flushed after 80 ms idle
        .on_output(|line| async move {
            println!("[stdout] {line}");
        })

        .on_error(|line| async move {
            eprintln!("[stderr] {line}");
        })
        .on_exit(|code| async move {
            println!("Shell exited with code: {code:?}");
        })
        .on_close(|| async move {
            println!("Shell closed");
        })
        .on_send(|cmd| async move {
            // Filter or transform commands before they reach the shell
            if cmd.contains("rm -rf") {
                return None; // Block dangerous commands
            }
            Some(cmd)
        })
        .spawn()
        .await?;

    shell.send_line("ls -la").await?;
    let output = shell.output(None, None).await;
    println!("stdout: {}", output.stdout);

    shell.send_line("echo done").await?;
    let output = shell.output(Some(Duration::from_millis(500)), None).await;
    println!("stdout: {}", output.stdout);

    // Reset: kill the process and spawn a fresh one (buffers + callbacks preserved)
    shell.reset().await?;

    shell.exit().await?;
    Ok(())
}
```

### One-Shot Execution

For commands where persistent state isn't needed:

```rust
use shell_engine::exec::{exec, ExecResult};
use std::time::Duration;

let result: ExecResult = exec("echo hello", "bash", Some(Duration::from_secs(5))).await?;
assert_eq!(result.stdout.trim(), "hello");
assert!(result.success());

// Unwrap stdout on success, or get stderr on failure
let output: anyhow::Result<String> = result.ok();
```

### Process Configuration (work dir / env / args / init & exit input)

These builder methods override the per-shell built-in defaults; overrides survive `reset()`:

```rust
use shell_engine::Shell;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut shell = Shell::new("bash")
        .work_dir("/tmp")                     // Child process working directory
        .env("MY_VAR", "42")                  // Extra env vars (layered over parent env)
        .envs([("A", "1"), ("B", "2")])       // ...or in bulk
        // .args(["--login", "-i"])           // Override launch args entirely
        .spawn()
        .await?;

    shell.send_line("pwd; echo $MY_VAR").await?;
    let out = shell.output(None, None).await;
    println!("{}", out.stdout.trim());        // "/tmp" and "42"

    shell.exit().await?;
    Ok(())
}
```

Notes:
- `init_input(...)` replaces the built-in session-init input (e.g. cmd's `chcp 65001`); it is written to stdin right after spawn and must include its own line terminator (e.g. `"cd /var\n"`).
- `exit_input(...)` replaces the input sent by `exit()` (e.g. `"quit()\n"` for REPLs); it must also include a line terminator.
- Unknown shells spawn with zero args and no default init/exit input — `exit()` then relies on closing stdin (EOF).

### PTY Mode

For full terminal applications (editors, `htop`, `screen`, etc.), enable the pseudo-terminal backend.
**Note:** In PTY mode stdout and stderr are merged into a single stream — `on_error` callbacks will never fire and `error_truncated_bytes` is always 0.

```rust
use shell_engine::Shell;
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut shell = Shell::new("bash")
        .enable_pty()                     // Use PTY instead of pipes
        .enable_buffer()
        .spawn()
        .await?;

    shell.send_line("vim").await?;

    // Wait for output and capture a rendered screen snapshot
    let snap = shell.output_snapshot(Some(Duration::from_millis(500)), None).await?;
    println!("Screen: {snap}");

    // Send control characters (fully supported in PTY mode)
    shell.send_control_char('C').await?;  // ^C interrupt

    // Adjust terminal window size
    shell.resize(120, 40).await?;

    shell.exit().await?;
    Ok(())
}
```

### Global Singletons

```rust
// Unix: shared global bash instance
#[cfg(unix)]
{
    let bash = shell_engine::bash().await?;
    let mut sh = bash.lock().await;
    sh.send_line("echo hello").await?;
}

// Windows: shared global powershell instance
#[cfg(windows)]
{
    let ps = shell_engine::powershell().await?;
    let mut sh = ps.lock().await;
    sh.send_line("Write-Output hello").await?;
}
```

### Output Buffer

```rust
use shell_engine::OutputBuffer;

let buf = OutputBuffer::new(1024 * 1024); // 1 MiB
buf.push("line 1\n".into()).await;

// Wait for new data (note: ensure notified() is called before push to avoid race)
buf.notify.notified().await;
let content = buf.take().await;

// Track overflow
let lost = buf.truncated_bytes.load(std::sync::atomic::Ordering::Relaxed);
```

## API Overview

All key types are re-exported at the crate root: `use shell_engine::{Shell, ShellBuilder, ShellOutput, OutputBuffer, CallbackMode};`

### `shell` module

| Type | Description |
|------|-------------|
| `Shell` | Live handle to a persistent shell process |
| `ShellBuilder` | Fluent builder for configuring and spawning a `Shell` |
| `ShellOutput` | Result struct containing `stdout` and `stderr` fields; provides `is_empty()` |
| `OutputBuffer` | Bounded, async-concurrent output accumulator |
| `CallbackMode` | `Raw` (per-chunk) or `Line` (per-line) callback mode |

**`Shell` lifecycle methods:** `send` (auto-detects `"^C"` syntax; routes to `send_control_char`), `send_line`, `send_keys` (send `Key::SpecialKey`/`Key::Char` sequences, e.g. direction keys, F1–F12), `send_control_char` (PTY: full `^A`–`^_` + `^?`; pipe: `^R` reset, `^D` EOF), `send_eof`, `reset`, `exit`, `close`, `join_close`, `join_exit`

**`Shell` output methods:** `output(idle, max_wait)`, `output_until(substring, timeout)`

**`Shell` stats:** `output_truncated_bytes`, `error_truncated_bytes` (always 0 in PTY mode)

**`Shell` PTY methods (requires `pty` feature):** `output_snapshot`, `screen_clone`, `resize`, `is_pty`, `pty_window_size`, `send_signal`, `cursor_position`, `move_cursor_to`

**`ShellBuilder` config:** `enable_buffer`, `enable_buffer_with_capacity`, `line_callback`, `raw_callback`

**`ShellBuilder` process config:** `work_dir`, `env`, `envs`, `args`, `init_input`, `exit_input` (override the per-shell built-in defaults; survive `reset()`; unknown shells spawn with zero args)

**`ShellBuilder` PTY config (requires `pty` feature):** `enable_pty`, `pty_size`, `scrollback`, `disable_snapshot` (saves CPU/memory; `output_snapshot`/`screen_clone`/`cursor_position` will error)

**`ShellBuilder` hooks:** `on_output`, `on_error`, `on_exit`, `on_close`, `on_send`

**`OutputBuffer` methods:** `new`, `push`, `take`, `is_empty`

**`OutputBuffer` public fields:** `notify` (waker for new data), `truncated_bytes` (overflow counter)

### Crate root re-exports

| Re-export | Feature gate | Description |
|-----------|-------------|-------------|
| `shell_engine::vt100` | `pty` | Re-exported so you can use `shell_engine::vt100::Screen` without adding your own dep |
| `shell_engine::PtySignal` | `pty` | Signal type for `send_signal()` |
| `shell_engine::WindowSize` | `pty` | PTY window size struct |
| `shell_engine::bash()` | unix | Global singleton bash `Arc<Mutex<Shell>>` |
| `shell_engine::powershell()` | windows | Global singleton powershell `Arc<Mutex<Shell>>` |

### `tool` module

Low-level helpers: `StreamDecoder` (incremental text decoder), `decode_bytes`, `detect_encoding`, `normalize_shell_name`, `strip_ansi_codes`.

### `exec` module

| Type | Description |
|------|-------------|
| `exec()` | Run a one-shot command with optional timeout |
| `ExecResult` | Result with `stdout`, `stderr`, `exit_code`, `ok()`, `success()`, `failed()` |

## Platform & Environment Support

- Supported OSes: Linux, macOS, Windows (Windows provides built-in `powershell` defaults and automatic code-page detection)
- `no_std` support: no (depends on the `tokio` async runtime)
- Unsafe code: only `tool::detect_encoding` on Windows calls `GetConsoleOutputCP`; the `rust-pty` dependency contains FFI internally

## Minimum Supported Rust Version (MSRV)

`Cargo.toml` uses `edition = "2024"`, which requires **Rust 1.85 or newer**; the project does not declare `rust-version` explicitly.

MSRV policy: evolves with Rust releases; raised only on minor version bumps and documented in the CHANGELOG.

## Contributing

Issues and Pull Requests are welcome!

- Local dev setup: `cargo build && cargo test` (PTY tests require a Unix environment).
- Please read the [Design Philosophy](#design-philosophy) before opening a PR; feature requests that conflict with the core principles may not be accepted (feel free to discuss in an Issue first).
- Follow the Conventional Commits format for commit messages (`feat` / `fix` / `docs` / `refactor`, etc.).

## Changelog

See [CHANGELOG.md](CHANGELOG.md) for version history.

## License

MIT © 2026
