//! An async shell runtime for persistent sessions, PTY I/O, output buffering,
//! real-time callbacks, and screen snapshots.
//!
//! Spawn a long-lived shell process once (`Shell::new(...).spawn()`), then interact
//! with it across multiple commands: `send_line`, `output`, `reset`, `exit`. Two
//! backends are available:
//!
//! - **pipe mode** (default): clean stdout/stderr separation;
//! - **PTY mode** (`pty` feature, enabled by default): real pseudo-terminal for
//!   full-screen programs, job control, and rendered screen snapshots.
//!
//! # Examples
//!
//! ```rust
//! use shell_engine::Shell;
//!
//! #[tokio::main(flavor = "current_thread")]
//! async fn main() -> anyhow::Result<()> {
//!     let mut sh = Shell::new("bash")
//!         .enable_buffer()
//!         .spawn()
//!         .await?;
//!
//!     sh.send_line("export FOO=bar").await?;
//!     sh.send_line("echo $FOO").await?;
//!     let out = sh.output(None, None).await;
//!     assert_eq!(out.stdout.trim(), "bar");
//!     sh.exit().await?;
//!     Ok(())
//! }
//! ```
#![cfg_attr(docsrs, feature(doc_cfg))]
/// 一次性命令执行：`exec()` 与 `ExecResult`。
pub mod exec;
/// 持久化 shell 会话管理：`Shell`、`ShellBuilder`、`OutputBuffer`、`CallbackMode`。
pub mod shell;
/// 低层工具：流式解码、ANSI 剥离、shell 名称规范化。
pub mod tool;

pub use shell::{CallbackMode, OutputBuffer, Shell, ShellBuilder, ShellOutput};

#[cfg(unix)]
pub use shell::bash;
#[cfg(windows)]
pub use shell::powershell;

#[cfg(feature = "pty")]
#[cfg_attr(docsrs, doc(cfg(feature = "pty")))]
pub use rust_pty::{PtySignal, WindowSize};
#[cfg(feature = "pty")]
#[cfg_attr(docsrs, doc(cfg(feature = "pty")))]
pub use vt100;
