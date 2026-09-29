# shell-engine
> **一个面向异步 Rust 的持久化、长生命周期 shell 会话管理器。**


<!-- ============ 徽章区 ============ -->
<!-- 第一行：核心发布信息 -->
[![Crates.io](https://img.shields.io/crates/v/shell-engine.svg)](https://crates.io/crates/shell-engine)
[![Downloads](https://img.shields.io/crates/d/shell-engine.svg)](https://crates.io/crates/shell-engine)
[![Documentation](https://docs.rs/shell-engine/badge.svg)](https://docs.rs/shell-engine)
[![License](https://img.shields.io/crates/l/shell-engine.svg)](#开源协议-license)

<!-- 第二行：工程状态信息 -->
[![MSRV](https://img.shields.io/badge/MSRV-1.85-blue.svg)](#最小-rust-版本-msrv)
<!-- CI badge：尚未配置 .github/workflows/ci.yml，配置后取消注释 -->
<!-- [![CI](https://github.com/ZEROLINGG/shell-engine/actions/workflows/ci.yml/badge.svg)](https://github.com/ZEROLINGG/shell-engine/actions) -->

**语言：** [English](README.md) | [简体中文](README-zh_CN.md)


---

## 目录

- [设计哲学](#设计哲学)
- [快速开始](#快速开始-quick-start)
- [API 总览](#api-总览)
- [功能特性](#功能特性)
- [支持的 Shell](#支持的-shell)
- [适用场景 vs 不适用场景](#适用场景-vs-不适用场景)
- [安装](#安装)
- [特性标志](#特性标志-feature-flags)
- [用法](#用法)
- [平台与环境支持](#平台与环境支持)
- [最小 Rust 版本](#最小-rust-版本-msrv)
- [安全性](#安全性)
- [贡献](#贡献-contributing)
- [变更日志](#变更日志)
- [开源协议](#开源协议-license)
- [致谢](#致谢)

## 设计哲学

1. **单一会话入口，杜绝重复逻辑** —— 管道模式与 PTY 模式共用 `backend::launch()` 启动、共用 `OutputPump` 解码+分派状态机（`stream.rs`），避免两份容易行为漂移的读取逻辑。
2. **显式优于隐式** —— 缓冲开关、回调粒度、PTY 是否启用全部由 `ShellBuilder` 链式配置显式控制，不靠魔法默认。
3. **最小依赖** —— 运行时仅依赖 `anyhow`、`tokio`、`serde`；PTY 相关依赖（`rust-pty`、`vt100`）全部作为可选 feature 隔离。

### 权衡取舍

| 我们选择了 | 而不是 | 原因 |
|---|---|---|
| 有界输出缓冲（超出丢最旧） | 无限累积 | 长跑会话内存有上限，同时提供 `truncated_bytes` 统计 |
| 可选 PTY（feature 隔离） | 默认双后端常驻 | `vt100`/`rust-pty` 依赖较重，纯管道场景不应背负 |
| PTY 模式 stdout/stderr 合并 | 拆分为两路 | 伪终端天然合并，拆分需要额外解析层，收益有限 |

### 非目标

- **不做进程集群/编排框架** —— 本库只管理单个本机会话，不涉分布式任务池或远程 SSH 集群
- **不做 PTY 级别的 stdout/stderr 分离** —— 伪终端天然合并两路流，我们不额外加解析层拆分
- **不做 shell 解析器 / AST 库** —— 输出以原始文本交付（可选剥离 ANSI）；结构化解析交给调用方

## 快速开始 (Quick Start)

将依赖加入你的 `Cargo.toml`：

```toml
[dependencies]
shell-engine = "0.2"
```

基本用法：

```rust
use shell_engine::Shell;

#[tokio::main(flavor = "current_thread")]
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

## API 总览

所有核心类型都在 crate 根重导出：`use shell_engine::{Shell, ShellBuilder, ShellOutput, OutputBuffer, CallbackMode};`

### `shell` 模块

| 类型 | 描述 |
|------|-------------|
| `Shell` | 持久化 shell 进程的活动句柄 |
| `ShellBuilder` | 配置并 spawn `Shell` 的链式 builder |
| `ShellOutput` | 包含 `stdout` 与 `stderr` 字段的结果结构体；提供 `is_empty()` |
| `OutputBuffer` | 有界、异步并发的输出累加器 |
| `CallbackMode` | `Raw`（按块）或 `Line`（按行）回调模式 |

**`Shell` 生命周期方法：** `send`（自动识别 `"^C"` 语法，转走 `send_control_char`）、`send_line`、`send_keys`（发送 `Key::SpecialKey`/`Key::Char` 序列，如方向键、F1–F12）、`send_control_char`（PTY：完整 `^A`–`^_` + `^?`；管道：`^R` 重置、`^D` EOF）、`send_eof`、`reset`、`exit`、`close`、`join_close`、`join_exit`

**`Shell` 输出方法：** `output(idle, max_wait)`、`output_until(substring, timeout)`

**`Shell` 统计：** `output_truncated_bytes`、`error_truncated_bytes`（PTY 模式下恒为 0）

**`Shell` PTY 方法（需要 `pty` feature）：** `output_snapshot`、`screen_clone`、`resize`、`is_pty`、`pty_window_size`、`send_signal`、`cursor_position`、`move_cursor_to`

**`ShellBuilder` 配置：** `enable_buffer`、`enable_buffer_with_capacity`、`line_callback`、`raw_callback`

**`ShellBuilder` 进程配置：** `work_dir`、`env`、`envs`、`args`、`init_input`、`exit_input`（覆盖各 shell 内置默认值；`reset()` 后保留；未知 shell 以零参数启动）

**`ShellBuilder` PTY 配置（需要 `pty` feature）：** `enable_pty`、`pty_size`、`scrollback`、`disable_snapshot`（节省 CPU/内存；`output_snapshot`/`screen_clone`/`cursor_position` 将报错）

**`ShellBuilder` 钩子：** `on_output`、`on_error`、`on_exit`、`on_close`、`on_send`

**`OutputBuffer` 方法：** `new`、`push`、`take`、`is_empty`

**`OutputBuffer` 公开字段：** `notify`（新数据唤醒器）、`truncated_bytes`（溢出计数器）

### Crate 根重导出

| 重导出 | Feature 门控 | 描述 |
|-----------|-------------|-------------|
| `shell_engine::vt100` | `pty` | 重导出，方便直接用 `shell_engine::vt100::Screen` 而无需自行加依赖 |
| `shell_engine::PtySignal` | `pty` | `send_signal()` 的信号类型 |
| `shell_engine::WindowSize` | `pty` | PTY 窗口尺寸结构体 |
| `shell_engine::bash()` | unix | 全局单例 bash `Arc<Mutex<Shell>>` |
| `shell_engine::powershell()` | windows | 全局单例 powershell `Arc<Mutex<Shell>>` |

### `tool` 模块

底层辅助：`StreamDecoder`（增量文本解码器）、`decode_bytes`、`detect_encoding`、`normalize_shell_name`、`strip_ansi_codes`。

### `exec` 模块

| 类型 | 描述 |
|------|-------------|
| `exec()` | 运行一次性命令（可选超时） |
| `ExecResult` | 含 `stdout`、`stderr`、`exit_code`、`ok()`、`success()`、`failed()` 的结果 |

## 功能特性

- **持久进程** —— 一次 spawn，多次命令；环境变量、工作目录与 shell 内部状态在调用之间保留
- **双后端** —— 管道模式（默认，干净的 I/O 分离）与 PTY 模式（可选，用于全屏终端应用、job control、彩色输出）
- **双回调模式** —— Raw（延迟最低，按块）或 Line（按行缓冲，交互式 prompt 在 80 ms 空闲后强制 flush）
- **有界缓冲** —— 容量可配的 `OutputBuffer`，提供溢出截断计数
- **异步回调** —— stdout / stderr / exit / close / 发送前过滤 钩子
- **生命周期控制** —— `send`、`send_line`、`send_control_char`、`send_keys`、`send_eof`、`reset`、`exit`、`close`、`join_close`、`join_exit`，以及 drop 时自动关闭
- **控制字符** —— `send("^C")` 自动识别 `^@`–`^_` 与 `^?` 语法；PTY 模式支持完整 `^A`–`^_` + `^?`，管道模式支持 `^R`（重置）与 `^D`（EOF）
- **屏幕快照** —— PTY 模式通过 `vt100` 解析器捕获已渲染的终端内容（`output_snapshot`）
- **跨平台、不绑定具体 shell** —— Unix（bash、zsh、sh、fish、python、node）与 Windows（cmd、powershell、pwsh、python、node）内置默认值；其它 shell 或控制台程序同样可用
- **编码支持** —— 有状态增量解码器；自动检测 Windows 代码页
- **一次性执行** —— `exec()` 支持 fire-and-forget 命令与可选超时

## 支持的 Shell

任何通过标准输入/输出通信的 shell 或控制台程序都受支持。以下程序具有部分内置默认值（启动参数、初始化输入、退出输入）：sh、bash、zsh、fish、cmd、PowerShell、pwsh、python、node。

其它——nushell、elvish、xonsh、irb、sqlite3、自定义 REPL 等——同样可以 spawn：未知可执行文件以零参数、无内置 init/exit 输入启动（`exit()` 此时依赖关闭 stdin / EOF）。可使用 builder 上的 `args`、`init_input`、`exit_input`、`work_dir`、`env` 自行配置。

## 适用场景 vs 不适用场景

**适合：**
- 需要跨多条命令保持会话状态（环境变量、工作目录、shell 内部状态）的自动化脚本
- 交互式 REPL / 终端程序（编辑器、`htop`）驱动，需要真实伪终端（`enable_pty()`）
- 需要对子进程输出做实时回调、行级解析、或有界缓冲的场景

**不适合：**
- 一次性执行几条命令即可的场景——请用 `exec()`（每次启动新进程，无会话）
- 需要严格分离 stdout/stderr 的 PTY 场景（PTY 模式下两者合并，`on_error` 不会触发）
- 需要进程集群/任务编排的分布式场景——本项目只管理单个本机会话

## 安装

```toml
[dependencies]
shell-engine = "0.2"
```

PTY 支持（通过 `rust-pty` + `vt100`）默认启用。如需关闭：

```toml
[dependencies]
shell-engine = { version = "0.2", default-features = false }
```

## 特性标志 (Feature Flags)

本项目支持以下 [Cargo features](https://doc.rust-lang.org/cargo/reference/features.html)：

- `default`：默认启用，等价于 `pty`（PTY 后端 + `vt100` 屏幕快照）。
- `pty`：启用伪终端后端与 `rust-pty`/`vt100` 两个可选依赖；关闭后仅剩管道模式，
  相关 API（`enable_pty`、`output_snapshot`、`resize` 等）将不参与编译。

纯管道模式配置：

```toml
[dependencies]
shell-engine = { version = "0.2", default-features = false }
```

## 用法

### 持久化 Shell

```rust
use shell_engine::Shell;
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut shell = Shell::new("bash")
        .enable_buffer()              // 缓冲 stdout/stderr（默认 1 MiB）
        .line_callback()              // 完整行；80 ms 空闲后 flush 半行
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
            // 在命令到达 shell 前过滤或改写
            if cmd.contains("rm -rf") {
                return None; // 拦截危险命令
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

    // 重置：杀掉进程并重新拉起一个（缓冲与回调保留）
    shell.reset().await?;

    shell.exit().await?;
    Ok(())
}
```

### 一次性执行

不需要持久状态的命令：

```rust
use shell_engine::exec::{exec, ExecResult};
use std::time::Duration;

let result: ExecResult = exec("echo hello", "bash", Some(Duration::from_secs(5))).await?;
assert_eq!(result.stdout.trim(), "hello");
assert!(result.success());

// 成功取出 stdout，失败取出 stderr
let output: anyhow::Result<String> = result.ok();
```

### 进程配置（工作目录 / 环境变量 / 参数 / 初始化与退出输入）

这些 builder 方法会覆盖各 shell 的内置默认值；覆盖项在 `reset()` 后保留：

```rust
use shell_engine::Shell;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut shell = Shell::new("bash")
        .work_dir("/tmp")                     // 子进程工作目录
        .env("MY_VAR", "42")                  // 追加环境变量（叠在父进程之上）
        .envs([("A", "1"), ("B", "2")])       // ...或批量
        // .args(["--login", "-i"])           // 完全覆盖启动参数
        .spawn()
        .await?;

    shell.send_line("pwd; echo $MY_VAR").await?;
    let out = shell.output(None, None).await;
    println!("{}", out.stdout.trim());        // "/tmp" 与 "42"

    shell.exit().await?;
    Ok(())
}
```

注意：
- `init_input(...)` 替换内置的会话初始化输入（如 cmd 的 `chcp 65001`）；它在 spawn 后立即写入 stdin，必须自带行结束符（如 `"cd /var\n"`）。
- `exit_input(...)` 替换 `exit()` 发送的输入（如 REPL 的 `"quit()\n"`）；同样必须自带行结束符。
- 未知 shell 以零参数、无默认 init/exit 输入启动——`exit()` 此时依赖关闭 stdin（EOF）。

### PTY 模式

用于运行全屏终端应用（编辑器、`htop`、`screen` 等），启用伪终端后端。
**注意：** PTY 模式下 stdout 与 stderr 合并为一路——`on_error` 回调永远不会触发，`error_truncated_bytes` 恒为 0。

```rust
use shell_engine::Shell;
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut shell = Shell::new("bash")
        .enable_pty()                     // 使用 PTY 而非管道
        .enable_buffer()
        .spawn()
        .await?;

    shell.send_line("vim").await?;

    // 等待输出并捕获已渲染的屏幕快照
    let snap = shell.output_snapshot(Some(Duration::from_millis(500)), None).await?;
    println!("Screen: {snap}");

    // 发送控制字符（PTY 模式完整支持）
    shell.send_control_char('C').await?;  // ^C 中断

    // 调整终端窗口大小
    shell.resize(120, 40).await?;

    shell.exit().await?;
    Ok(())
}
```

### 全局单例

```rust
// Unix：共享的全局 bash 实例
#[cfg(unix)]
{
    let bash = shell_engine::bash().await?;
    let mut sh = bash.lock().await;
    sh.send_line("echo hello").await?;
}

// Windows：共享的全局 powershell 实例
#[cfg(windows)]
{
    let ps = shell_engine::powershell().await?;
    let mut sh = ps.lock().await;
    sh.send_line("Write-Output hello").await?;
}
```

### 输出缓冲区

```rust
use shell_engine::OutputBuffer;

let buf = OutputBuffer::new(1024 * 1024); // 1 MiB
buf.push("line 1\n".into()).await;

// 等待新数据（注意：为避免竞态，应在 push 前调用 notified()）
buf.notify.notified().await;
let content = buf.take().await;

// 跟踪溢出
let lost = buf.truncated_bytes.load(std::sync::atomic::Ordering::Relaxed);
```

## 平台与环境支持

- 支持操作系统：Linux、macOS、Windows（Windows 下提供 `powershell` 内置默认值与代码页自动检测）
- `no_std` 支持：否（依赖 `tokio` 异步运行时）
- Unsafe 代码：仅 Windows 侧 `tool::detect_encoding` 调用 `GetConsoleOutputCP`；`rust-pty` 依赖内部含 FFI

## 最小 Rust 版本 (MSRV)

`Cargo.toml` 已声明 `edition = "2024"` 与 `rust-version = "1.85"`，因此最低支持 Rust 版本为 **1.85**。

MSRV 变更策略：随 Rust 版本演进，仅会在 minor 版本升级时调整，并在 CHANGELOG 中说明。

## 安全性

如发现安全漏洞，请勿直接提交公开 Issue，而是私下联系维护者上报。如有条件请附上最小复现步骤。

## 贡献 (Contributing)

欢迎提交 Issue 和 Pull Request！

- 本地开发环境搭建：`cargo build && cargo test`（PTY 测试需要 Unix 环境）。
- 提交 PR 前请先阅读[设计哲学](#设计哲学)，与项目核心原则冲突的功能建议可能不会被采纳（欢迎在 Issue 中先讨论）。
- 提交信息请遵循 Conventional Commits 格式（`feat` / `fix` / `docs` / `refactor` 等）。

## 变更日志

版本变更详情请见 [CHANGELOG.md](CHANGELOG.md)。

## 开源协议 (License)

MIT © 2026

## 致谢

- [rust-pty](https://crates.io/crates/rust-pty) —— 跨平台 PTY 接口，驱动 `pty` feature
- [vt100](https://crates.io/crates/vt100) —— 终端状态机，实现屏幕快照
- [tokio](https://crates.io/crates/tokio) —— 本库所基于的异步运行时
