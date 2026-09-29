//src/shell/mod.rs

mod backend;
pub(crate) mod buffer;
mod builder;
pub(crate) mod callbacks;
mod pipe;
pub(crate) mod profile;
#[cfg(feature = "pty")]
mod pty;
pub(crate) mod stream;

pub use buffer::OutputBuffer;
pub use builder::ShellBuilder;
pub use callbacks::CallbackMode;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::{Result, anyhow, ensure};
use tokio::sync::{Mutex, Notify, OnceCell, mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::shell::backend::LaunchConfig;
#[cfg(feature = "pty")]
use crate::shell::backend::PtyState;
use crate::shell::callbacks::{BoxFuture, CallbackHub, PreSendHook};
use crate::shell::profile::ShellProfile;
use crate::shell::stream::StdinMsg;
#[cfg(feature = "pty")]
use rust_pty::PtySignal;
#[cfg(feature = "pty")]
use tokio::time::sleep;

// ─── 全局单例 ────────────────────────────────────────────────────────────────

#[cfg(unix)]
static BASH: OnceCell<Result<Arc<Mutex<Shell>>>> = OnceCell::const_new();
#[cfg(windows)]
static POWERSHELL: OnceCell<Result<Arc<Mutex<Shell>>>> = OnceCell::const_new();

/// 获取全局共享的 bash 会话单例（首次调用时自动创建并启用缓冲）。
///
/// # Errors
///
/// 当 bash 不存在或首次启动失败时返回 `Err`。
#[cfg(unix)]
pub async fn bash() -> Result<Arc<Mutex<Shell>>> {
    let result = BASH
        .get_or_init(|| async {
            Shell::new("bash")
                .enable_buffer()
                .spawn()
                .await
                .map(|s| Arc::new(Mutex::new(s)))
        })
        .await;
    match result {
        Ok(s) => Ok(s.clone()),
        Err(e) => Err(anyhow!("{e}")),
    }
}

/// 获取全局共享的 powershell 会话单例（首次调用时自动创建并启用缓冲）。
///
/// # Errors
///
/// 当 powershell 不存在或首次启动失败时返回 `Err`。
#[cfg(windows)]
pub async fn powershell() -> Result<Arc<Mutex<Shell>>> {
    let result = POWERSHELL
        .get_or_init(|| async {
            Shell::new("powershell")
                .enable_buffer()
                .spawn()
                .await
                .map(|s| Arc::new(Mutex::new(s)))
        })
        .await;
    match result {
        Ok(s) => Ok(s.clone()),
        Err(e) => Err(anyhow!("{e}")),
    }
}

/// 单个按键描述：可直接指定特殊键，或单个字符；也可用 `"[Up]"` / `"[f1]"` 等
/// 字符串标签，发送时会尝试解析为 [`SpecialKey`]。
pub enum Key {
    SpecialKey(SpecialKey),
    Char(char),
    StringChar(String), // 处理时尝试解析[Up],[Down]……为SpecialKey
}

/// 特殊按键（方向键、功能键、编辑键等）。
pub enum SpecialKey {
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    Tab,
    BackTab,
    Enter,
    Escape,
    Backspace,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
}
impl SpecialKey {
    /// 按常见 xterm 编码（不考虑 DECCKM application mode，覆盖绝大多数场景）
    fn to_bytes(&self) -> &'static str {
        match self {
            SpecialKey::Up => "\x1b[A",
            SpecialKey::Down => "\x1b[B",
            SpecialKey::Right => "\x1b[C",
            SpecialKey::Left => "\x1b[D",
            SpecialKey::Home => "\x1b[H",
            SpecialKey::End => "\x1b[F",
            SpecialKey::PageUp => "\x1b[5~",
            SpecialKey::PageDown => "\x1b[6~",
            SpecialKey::Insert => "\x1b[2~",
            SpecialKey::Delete => "\x1b[3~",
            SpecialKey::Tab => "\t",
            SpecialKey::BackTab => "\x1b[Z",
            SpecialKey::Enter => "\r",
            SpecialKey::Escape => "\x1b",
            SpecialKey::Backspace => "\x7f",
            SpecialKey::F1 => "\x1bOP",
            SpecialKey::F2 => "\x1bOQ",
            SpecialKey::F3 => "\x1bOR",
            SpecialKey::F4 => "\x1bOS",
            SpecialKey::F5 => "\x1b[15~",
            SpecialKey::F6 => "\x1b[17~",
            SpecialKey::F7 => "\x1b[18~",
            SpecialKey::F8 => "\x1b[19~",
            SpecialKey::F9 => "\x1b[20~",
            SpecialKey::F10 => "\x1b[21~",
            SpecialKey::F11 => "\x1b[23~",
            SpecialKey::F12 => "\x1b[24~",
        }
    }
    /// 将 `"[Up]"`、`"[f1]"` 等字符串标签解析为对应的特殊键；无法识别时返回 `None`。
    ///
    /// # Examples
    ///
    /// ```rust
    /// use shell_engine::shell::SpecialKey;
    ///
    /// assert!(matches!(
    ///     SpecialKey::from_str_tag("[enter]"),
    ///     Some(SpecialKey::Enter)
    /// ));
    /// assert!(SpecialKey::from_str_tag("[unknown]").is_none());
    /// ```
    pub fn from_str_tag(tag: &str) -> Option<Self> {
        match tag.to_lowercase().as_str() {
            "[up]" => Some(SpecialKey::Up),
            "[down]" => Some(SpecialKey::Down),
            "[left]" => Some(SpecialKey::Left),
            "[right]" => Some(SpecialKey::Right),
            "[home]" => Some(SpecialKey::Home),
            "[end]" => Some(SpecialKey::End),
            "[pageup]" => Some(SpecialKey::PageUp),
            "[pagedown]" => Some(SpecialKey::PageDown),
            "[insert]" => Some(SpecialKey::Insert),
            "[delete]" => Some(SpecialKey::Delete),
            "[tab]" => Some(SpecialKey::Tab),
            "[backtab]" => Some(SpecialKey::BackTab),
            "[enter]" | "[return]" => Some(SpecialKey::Enter),
            "[escape]" | "[esc]" => Some(SpecialKey::Escape),
            "[backspace]" => Some(SpecialKey::Backspace),
            "[f1]" => Some(SpecialKey::F1),
            "[f2]" => Some(SpecialKey::F2),
            "[f3]" => Some(SpecialKey::F3),
            "[f4]" => Some(SpecialKey::F4),
            "[f5]" => Some(SpecialKey::F5),
            "[f6]" => Some(SpecialKey::F6),
            "[f7]" => Some(SpecialKey::F7),
            "[f8]" => Some(SpecialKey::F8),
            "[f9]" => Some(SpecialKey::F9),
            "[f10]" => Some(SpecialKey::F10),
            "[f11]" => Some(SpecialKey::F11),
            "[f12]" => Some(SpecialKey::F12),
            _ => None,
        }
    }
}

// ─── ShellOutput ───────────────────────────────────────────────────────────

/// 包含标准输出和标准错误的结果结构体。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShellOutput {
    pub stdout: String,
    pub stderr: String,
}

impl ShellOutput {
    /// stdout 与 stderr 是否均为空。
    pub fn is_empty(&self) -> bool {
        self.stdout.is_empty() && self.stderr.is_empty()
    }
}

// ─── Shell ────────────────────────────────────────────────────────────────

/// 一个持久 shell 会话：管道模式或 PTY 模式下的交互句柄。
///
/// 通过 [`Shell::new`] 或 [`ShellBuilder`] 创建。
pub struct Shell {
    pub shell_path: String,
    /// 会话配置（含启动参数/环境变量/工作目录等覆盖项），
    /// 由 Builder 在 spawn 时构造，`reset()` / `exit()` 复用。
    profile: ShellProfile,
    tx_stdin: mpsc::Sender<StdinMsg>,
    drop_tx: Option<oneshot::Sender<()>>,
    pre_send: PreSendHook,
    callbacks: CallbackHub,
    join: Option<JoinHandle<()>>,
    droped: bool,
    close_notify: Arc<Notify>,
    output_buffer: Option<Arc<OutputBuffer>>,
    error_buffer: Option<Arc<OutputBuffer>>,

    /// `None` = 管道模式；`Some` = PTY 模式及其专属状态。
    #[cfg(feature = "pty")]
    pty: Option<PtyState>,
}

impl Shell {
    /// 创建一个新的会话构建器（等价于 [`ShellBuilder::new`]）。构建完成后调用
    /// `spawn()` 才会真正启动子进程。
    #[allow(clippy::new_ret_no_self)]
    pub fn new(shell: impl Into<String>) -> ShellBuilder {
        ShellBuilder::new(shell)
    }

    /// 由 `ShellBuilder::spawn()` 调用的唯一构造入口。
    pub(crate) async fn spawn_new(
        cfg: LaunchConfig,
        pre_send: PreSendHook,
        close_notify: Arc<Notify>,
    ) -> Result<Shell> {
        // launch() 会消耗掉 cfg 里的字段，这里先拷贝出后续要长期持有的部分。
        let shell_path = cfg.profile.get_path().to_string();
        let profile = cfg.profile.clone();
        let callbacks = cfg.callbacks.clone();
        let output_buffer = cfg.output_buffer.clone();
        let error_buffer = cfg.error_buffer.clone();

        let session = backend::launch(cfg).await?;

        Ok(Shell {
            shell_path,
            profile,
            tx_stdin: session.tx_stdin,
            drop_tx: Some(session.drop_tx),
            pre_send,
            callbacks,
            join: Some(session.join),
            droped: false,
            close_notify,
            output_buffer,
            error_buffer,
            #[cfg(feature = "pty")]
            pty: session.pty,
        })
    }

    /// 当前会话是否运行在 PTY 模式下。
    #[cfg(feature = "pty")]
    pub fn is_pty(&self) -> bool {
        self.pty.is_some()
    }

    // ── 输出读取 ──────────────────────────────────────────────────────────

    /// 等待直到 stdout/stderr 均空闲超过 `idle_time`，但不清空缓冲区。
    /// `timeout` 提供了可选的最大整体等待时长，防止在终端持续输出时发生无限等待。
    async fn wait_idle(&self, idle_time: Duration, timeout: Option<Duration>) {
        let ob_out = self.output_buffer.as_ref();
        let ob_err = self.error_buffer.as_ref();

        if ob_out.is_none() && ob_err.is_none() {
            return;
        }

        let deadline = timeout.map(|d| tokio::time::Instant::now() + d);

        loop {
            let notify_out = async {
                if let Some(ob) = ob_out {
                    ob.notify.notified().await;
                } else {
                    std::future::pending::<()>().await;
                }
            };
            let notify_err = async {
                if let Some(ob) = ob_err {
                    ob.notify.notified().await;
                } else {
                    std::future::pending::<()>().await;
                }
            };

            // 创建整体超时的 future，类似于 output_until 的实现
            let sleep_fut: BoxFuture<'_, ()> = match deadline {
                Some(inst) => {
                    if tokio::time::Instant::now() >= inst {
                        break;
                    }
                    Box::pin(tokio::time::sleep_until(inst))
                }
                None => Box::pin(std::future::pending()),
            };

            tokio::select! {
                _ = self.close_notify.notified() => break,
                _ = sleep_fut => break, // 触发整体超时，强制中断等待
                res = tokio::time::timeout(idle_time, async {
                    tokio::select! {
                        _ = notify_out => {}
                        _ = notify_err => {}
                    }
                }) => {
                    match res {
                        Ok(_)  => continue, // 收到了新数据，继续循环等待其空闲
                        Err(_) => break,    // 满足空闲时长 idle_time，退出
                    }
                }
            }
        }
    }

    /// 等待直到 stdout 和 stderr 均空闲超过 `idle_time`（默认 200 ms），然后返回并清空缓冲。
    pub async fn output(
        &mut self,
        idle_time: Option<Duration>,
        max_wait: Option<Duration>,
    ) -> ShellOutput {
        let timeout = idle_time.unwrap_or(Duration::from_millis(200));

        if self.output_buffer.is_none() && self.error_buffer.is_none() {
            return ShellOutput::default();
        }

        self.wait_idle(timeout, Some(max_wait.unwrap_or(Duration::from_secs(60))))
            .await;

        let stdout = if let Some(ob) = &self.output_buffer {
            ob.take().await
        } else {
            String::new()
        };
        let stderr = if let Some(ob) = &self.error_buffer {
            ob.take().await
        } else {
            String::new()
        };

        ShellOutput { stdout, stderr }
    }

    /// 持续等待，直到 stdout 或 stderr 中出现指定的子串 `substring`，
    /// 或等待超过 `timeout`，然后返回期间累积到的全部输出并清空缓冲区。
    pub async fn output_until(
        &mut self,
        substring: String,
        timeout: Option<Duration>,
    ) -> ShellOutput {
        let ob_out = self.output_buffer.as_ref();
        let ob_err = self.error_buffer.as_ref();

        if ob_out.is_none() && ob_err.is_none() {
            return ShellOutput::default();
        }

        let mut stdout_acc = String::new();
        let mut stderr_acc = String::new();
        let deadline = timeout.map(|d| tokio::time::Instant::now() + d);

        loop {
            if let Some(ob) = ob_out {
                let chunk = ob.take().await;
                if !chunk.is_empty() {
                    stdout_acc.push_str(&chunk);
                }
            }
            if let Some(ob) = ob_err {
                let chunk = ob.take().await;
                if !chunk.is_empty() {
                    stderr_acc.push_str(&chunk);
                }
            }

            if stdout_acc.contains(&substring) || stderr_acc.contains(&substring) {
                break;
            }

            let sleep_fut: BoxFuture<'_, ()> = match deadline {
                Some(inst) => {
                    if tokio::time::Instant::now() >= inst {
                        break;
                    }
                    Box::pin(tokio::time::sleep_until(inst))
                }
                None => Box::pin(std::future::pending()),
            };

            let notify_out = async {
                if let Some(ob) = ob_out {
                    ob.notify.notified().await;
                } else {
                    std::future::pending::<()>().await;
                }
            };
            let notify_err = async {
                if let Some(ob) = ob_err {
                    ob.notify.notified().await;
                } else {
                    std::future::pending::<()>().await;
                }
            };

            tokio::select! {
                _ = self.close_notify.notified() => break,
                _ = sleep_fut => break,
                _ = notify_out => {}
                _ = notify_err => {}
            }
        }

        if let Some(ob) = ob_out {
            let chunk = ob.take().await;
            if !chunk.is_empty() {
                stdout_acc.push_str(&chunk);
            }
        }
        if let Some(ob) = ob_err {
            let chunk = ob.take().await;
            if !chunk.is_empty() {
                stderr_acc.push_str(&chunk);
            }
        }

        ShellOutput {
            stdout: stdout_acc,
            stderr: stderr_acc,
        }
    }

    /// 返回渲染后的虚拟终端屏幕快照（仅 PTY 模式，且未 `disable_snapshot()`）。
    ///
    /// # Errors
    ///
    /// - 会话已关闭时返回 `Err`；
    /// - 当前不是 PTY 模式时返回 `Err`；
    /// - 未启用屏幕追踪（`disable_snapshot()`）时返回 `Err`；
    /// - vt100 解析器锁损坏时返回 `Err`。
    #[cfg(feature = "pty")]
    pub async fn output_snapshot(
        &mut self,
        idle_time: Option<Duration>,
        max_wait: Option<Duration>,
    ) -> Result<String> {
        ensure!(!self.droped, "shell is closed");

        let parser = self
            .pty
            .as_ref()
            .ok_or_else(|| anyhow!("output_snapshot() requires enable_pty()"))?
            .vt_parser()?
            .clone();

        if let Some(idle) = idle_time {
            self.wait_idle(idle, Some(max_wait.unwrap_or(Duration::from_secs(60))))
                .await;
        } else {
            sleep(max_wait.unwrap_or(Duration::from_secs(60))).await;
        }

        let guard = parser
            .lock()
            .map_err(|_| anyhow!("vt100 parser lock poisoned"))?;
        Ok(guard.screen().contents())
    }

    /// 克隆一份 `vt100::Screen`，用于自定义渲染（拿光标位置、每格颜色等）。
    ///
    /// # Errors
    ///
    /// - 会话已关闭时返回 `Err`；
    /// - 当前不是 PTY 模式时返回 `Err`；
    /// - 未启用屏幕追踪时返回 `Err`；
    /// - vt100 解析器锁损坏时返回 `Err`。
    #[cfg(feature = "pty")]
    pub fn screen_clone(&self) -> Result<vt100::Screen> {
        ensure!(!self.droped, "shell is closed");

        let parser = self
            .pty
            .as_ref()
            .ok_or_else(|| anyhow!("screen_clone() requires enable_pty()"))?
            .vt_parser()?;

        let guard = parser
            .lock()
            .map_err(|_| anyhow!("vt100 parser lock poisoned"))?;
        Ok(guard.screen().clone())
    }

    /// 调整 PTY 窗口尺寸（仅 PTY 模式）。
    ///
    /// # Errors
    ///
    /// - 会话已关闭时返回 `Err`；
    /// - `cols` / `rows` 为 0 时返回 `Err`；
    /// - 当前不是 PTY 模式时返回 `Err`；
    /// - stdin 通道已关闭时返回 `Err`。
    #[cfg(feature = "pty")]
    pub async fn resize(&mut self, cols: u16, rows: u16) -> Result<()> {
        ensure!(!self.droped, "shell is closed");
        ensure!(cols > 0 && rows > 0, "cols and rows must be >= 1");
        let state = self
            .pty
            .as_mut()
            .ok_or_else(|| anyhow!("resize() requires enable_pty()"))?;
        state.cols = cols;
        state.rows = rows;

        self.tx_stdin
            .send(StdinMsg::Resize(cols, rows))
            .await
            .map_err(|_| anyhow!("resize failed: stdin channel closed"))
    }

    /// 当前记录的 PTY 窗口尺寸（列, 行）；管道模式下返回 `None`。
    #[cfg(feature = "pty")]
    pub fn pty_window_size(&self) -> Option<(u16, u16)> {
        self.pty.as_ref().map(|p| (p.cols, p.rows))
    }

    /// 向 PTY 子进程转发一个信号（如 `PtySignal::Interrupt`）。仅 PTY 模式可用。
    ///
    /// # Errors
    ///
    /// - 会话已关闭时返回 `Err`；
    /// - 当前不是 PTY 模式时返回 `Err`；
    /// - 信号通道已关闭时返回 `Err`。
    #[cfg(feature = "pty")]
    pub async fn send_signal(&mut self, sig: PtySignal) -> Result<()> {
        ensure!(!self.droped, "shell is closed");
        let state = self
            .pty
            .as_ref()
            .ok_or_else(|| anyhow!("send_signal() requires enable_pty()"))?;
        state
            .signal_tx
            .send(sig)
            .await
            .map_err(|_| anyhow!("signal channel closed"))
    }

    /// 返回 vt100 屏幕上的光标位置 `(row, col)`（从 0 开始）。仅 PTY 模式可用。
    ///
    /// # Errors
    ///
    /// - 会话已关闭时返回 `Err`；
    /// - 当前不是 PTY 模式时返回 `Err`；
    /// - 未启用屏幕追踪时返回 `Err`；
    /// - vt100 解析器锁损坏时返回 `Err`。
    #[cfg(feature = "pty")]
    pub fn cursor_position(&self) -> Result<(u16, u16)> {
        ensure!(!self.droped, "shell is closed");

        let parser = self
            .pty
            .as_ref()
            .ok_or_else(|| anyhow!("cursor_position() requires enable_pty()"))?
            .vt_parser()?;

        let guard = parser
            .lock()
            .map_err(|_| anyhow!("vt100 parser lock poisoned"))?;

        // vt100 库返回的光标位置通常是 (row, col)，从 0 开始
        Ok(guard.screen().cursor_position())
    }
    /// 通过 CUP 控制序列把光标移动到 1-based 的 `(row, col)` 位置。仅 PTY 模式可用。
    ///
    /// # Errors
    ///
    /// - 会话已关闭时返回 `Err`；
    /// - 当前不是 PTY 模式时返回 `Err`；
    /// - `row` / `col` 为 0 时返回 `Err`；
    /// - stdin 通道已关闭时返回 `Err`。
    #[cfg(feature = "pty")]
    pub async fn move_cursor_to(&mut self, row: u16, col: u16) -> Result<()> {
        ensure!(!self.droped, "shell is closed");
        ensure!(self.pty.is_some(), "move_cursor_to() requires enable_pty()");
        ensure!(row >= 1 && col >= 1, "row/col are 1-based and must be >= 1");

        // \x1b[{row};{col}H 是标准的 CUP (Cursor Position) 控制序列
        let seq = format!("\x1b[{};{}H", row, col);
        self.tx_stdin
            .send(StdinMsg::Data(seq))
            .await
            .map_err(|_| anyhow!("move_cursor_to failed"))
    }

    /// 返回 stdout 缓冲区因超出容量而丢弃的累计字节数。
    pub fn output_truncated_bytes(&self) -> usize {
        self.output_buffer
            .as_ref()
            .map(|ob| ob.truncated_bytes.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    /// 返回 stderr 缓冲区因超出容量而丢弃的累计字节数。
    /// PTY 模式下 stdout/stderr 合并，恒为 0。
    pub fn error_truncated_bytes(&self) -> usize {
        self.error_buffer
            .as_ref()
            .map(|ob| ob.truncated_bytes.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    // ── 发送 ──────────────────────────────────────────────────────────────

    /// 发送一组按键（支持特殊键、字符与字符串标签）。
    ///
    /// # Errors
    ///
    /// - 会话已关闭时返回 `Err`；
    /// - stdin 通道已关闭时返回 `Err`。
    pub async fn send_keys(&mut self, keys: Vec<Key>) -> Result<()> {
        ensure!(!self.droped, "shell is closed");

        let mut buffer = String::new();

        for key in keys {
            match key {
                Key::SpecialKey(sk) => {
                    buffer.push_str(sk.to_bytes());
                }
                Key::Char(c) => {
                    buffer.push(c);
                }
                Key::StringChar(s) => {
                    if let Some(sk) = SpecialKey::from_str_tag(&s) {
                        buffer.push_str(sk.to_bytes());
                    } else {
                        buffer.push_str(&s);
                    }
                }
            }
        }

        if !buffer.is_empty() {
            self.tx_stdin
                .send(StdinMsg::Data(buffer))
                .await
                .map_err(|_| anyhow!("send_keys failed: stdin channel closed"))?;
        }
        Ok(())
    }

    /// 发送原始命令文本（不追加换行符，不经行缓冲）。若文本是 `^C` / `^?` 等
    /// 控制字符简写则走控制字符路径。
    ///
    /// # Errors
    ///
    /// - 会话已关闭时返回 `Err`；
    /// - stdin 通道已关闭时返回 `Err`。
    pub async fn send(&mut self, cmd: &str) -> Result<()> {
        ensure!(!self.droped, "shell is closed");

        if let Some(ctrl) = parse_control_shortcut(cmd) {
            return self.send_control_char(ctrl).await;
        }

        if let Some(s) = self.pre_send.process(cmd.to_string()).await {
            self.tx_stdin
                .send(StdinMsg::Data(s))
                .await
                .map_err(|_| anyhow!("send failed: stdin channel closed"))?;
        }
        Ok(())
    }

    /// 发送一行命令（自动追加换行符）。
    ///
    /// # Errors
    ///
    /// - 会话已关闭时返回 `Err`；
    /// - stdin 通道已关闭时返回 `Err`。
    pub async fn send_line(&mut self, cmd: &str) -> Result<()> {
        self.send(&format!("{cmd}\n")).await
    }

    /// 发送 EOF（关闭子进程 stdin）。管道模式下子进程通常会因此退出。
    ///
    /// # Errors
    ///
    /// - stdin 通道已关闭时返回 `Err`。
    pub async fn send_eof(&mut self) -> Result<()> {
        self.tx_stdin
            .send(StdinMsg::Eof)
            .await
            .map_err(|_| anyhow!("send EOF failed"))
    }

    /// 发送控制字符（`^C`、`^D` 等）。PTY 模式下按标准 ASCII 控制字符映射发送；
    /// 管道模式下只有 `R`（重置会话）与 `D`（EOF）有意义。
    ///
    /// # Errors
    ///
    /// - stdin 通道已关闭时返回 `Err`。
    pub async fn send_control_char(&mut self, ctrl: char) -> Result<()> {
        let upper = ctrl.to_ascii_uppercase();

        #[cfg(feature = "pty")]
        if self.is_pty() {
            // PTY 模式下：使用公式计算完整的控制字符并直接发送给终端
            // 标准 ASCII 控制字符对应的可打印字符范围是 '@' (0x40) 到 '_' (0x5F)
            if ('@'..='_').contains(&upper) {
                // 公式：字符的 ASCII 码与 0x40 异或，映射到 0x00 - 0x1F
                let ctrl_byte = upper as u8 ^ 0x40;
                let data = String::from_utf8(vec![ctrl_byte]).unwrap_or_default();

                return self
                    .tx_stdin
                    .send(StdinMsg::Data(data))
                    .await
                    .map_err(|_| anyhow!("send control char failed: stdin channel closed"));
            } else if upper == '?' {
                // 特例：终端标准中，`^?` 通常代表 DEL (0x7F / 127)
                return self
                    .tx_stdin
                    .send(StdinMsg::Data("\x7F".to_string()))
                    .await
                    .map_err(|_| anyhow!("send DEL failed: stdin channel closed"));
            }

            // 如果不是有效范围内的字符，忽略
            return Ok(());
        }

        // 管道模式（非 PTY 模式）下：只保留特殊的硬编码控制语义
        match upper {
            'R' => self.reset().await,    // 自定义语义：彻底重置 Shell 会话
            'D' => self.send_eof().await, // EOF：向底层管道发送结束信号 (关闭 stdin)
            _ => Ok(()),                  // 按需求，忽略管道模式下无意义的其他控制字符
        }
    }

    // ── 生命周期 ──────────────────────────────────────────────────────────

    /// 等待子进程与所有 IO 任务完全收尾（会话关闭后立即返回）。
    ///
    /// # Errors
    ///
    /// - 后台任务 panic 时返回 `Err`。
    pub async fn join_close(&mut self) -> Result<()> {
        if !self.droped {
            self.close_notify.notified().await;
        }
        if let Some(handle) = self.join.take() {
            let _ = handle.await;
        }
        Ok(())
    }

    /// 等待子进程退出并回收后台任务（会话关闭后立即返回）。
    ///
    /// # Errors
    ///
    /// - 后台任务 panic 时返回 `Err`。
    pub async fn join_exit(&mut self) -> Result<()> {
        if let Some(handle) = self.join.take() {
            handle.await.map_err(|e| anyhow!("join_exit failed: {e}"))?;
        }
        Ok(())
    }

    /// 重新拉起一个全新的会话（复用相同的 shell_path / 回调 / 缓冲区 / PTY 配置）。
    ///
    /// # Errors
    ///
    /// - 会话已关闭时返回 `Err`；
    /// - 旧会话无法正常退出、或新会话启动失败时返回 `Err`。
    pub async fn reset(&mut self) -> Result<()> {
        ensure!(!self.droped, "shell is closed");
        self.exit().await?;
        if let Some(handle) = self.join.take() {
            let _ = handle.await;
        }

        let cfg = LaunchConfig {
            profile: self.profile.clone(),
            callbacks: self.callbacks.clone(),
            output_buffer: self.output_buffer.clone(),
            error_buffer: self.error_buffer.clone(),
            #[cfg(feature = "pty")]
            pty_opts: self.pty.as_ref().map(PtyState::options),
        };

        let session = backend::launch(cfg).await?;

        self.tx_stdin = session.tx_stdin;
        self.drop_tx = Some(session.drop_tx);
        self.join = Some(session.join);
        #[cfg(feature = "pty")]
        {
            self.pty = session.pty;
        }
        self.droped = false;
        Ok(())
    }

    /// 关闭当前会话（可通过 `reset()` 恢复）。
    ///
    /// # Errors
    ///
    /// - 会话已关闭时返回 `Err`；
    /// - 子进程在 10 秒内未退出时改为 `close()`，不返回 `Err`；
    /// - 后台任务 panic 时返回 `Err`。
    pub async fn exit(&mut self) -> Result<()> {
        ensure!(!self.droped, "shell is closed");

        // 自定义/未知的退出输入由 profile 决定；为 None 时仅靠关闭 stdin（EOF）退出
        if let Some(exit_cmd) = self.profile.get_exit_input() {
            let _ = self.tx_stdin.send(StdinMsg::Data(exit_cmd)).await;
        }
        let _ = self.tx_stdin.send(StdinMsg::Close).await;

        const EXIT_TIMEOUT: Duration = Duration::from_secs(10);
        match tokio::time::timeout(EXIT_TIMEOUT, self.join_exit()).await {
            Ok(result) => {
                if !self.droped {
                    self.droped = true;
                    let _ = self.drop_tx.take();
                    self.close_notify.notify_waiters();
                }
                result
            }
            Err(_) => self.close(),
        }
    }

    /// 立即关闭 Shell 实例（不可恢复，同步）。
    ///
    /// # Errors
    ///
    /// 本方法不依赖子进程状态，始终返回 `Ok`。
    pub fn close(&mut self) -> Result<()> {
        if self.droped {
            return Ok(());
        }
        self.droped = true;
        let _ = self.drop_tx.take();
        let _ = self.tx_stdin.try_send(StdinMsg::Close);
        self.close_notify.notify_waiters();
        Ok(())
    }
}

impl Drop for Shell {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

/// 识别 `^C` / `^A` / `^?` 这类控制字符简写，返回大写的控制字符。
fn parse_control_shortcut(cmd: &str) -> Option<char> {
    if cmd.len() >= 5 {
        return None;
    }
    let trimmed = cmd.trim();
    let mut chars = trimmed.chars();
    match (chars.next(), chars.next(), chars.next()) {
        (Some('^'), Some(c), None) => {
            let upper = c.to_ascii_uppercase();
            // 允许 '@' 到 '_' (包含了 A-Z) 以及 '?'
            if ('@'..='_').contains(&upper) || upper == '?' {
                Some(upper)
            } else {
                None
            }
        }
        _ => None,
    }
}

// ─── 测试 ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    #![allow(unused)]
    use super::*;
    use tokio::sync::mpsc as test_mpsc;
    use tokio::time::{Duration, timeout};

    #[test]
    fn control_shortcut_parsing() {
        assert_eq!(parse_control_shortcut("^C"), Some('C'));
        assert_eq!(parse_control_shortcut(" ^d "), Some('D'));
        assert_eq!(parse_control_shortcut("echo hi"), None);
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn basic_output() {
        let mut shell = Shell::new("sh")
            .enable_buffer()
            .spawn()
            .await
            .expect("failed to spawn sh");

        shell.send_line("echo hello_tokio").await.unwrap();
        let out = shell.output(Some(Duration::from_millis(300)), None).await;
        assert!(out.stdout.contains("hello_tokio"));

        shell.exit().await.unwrap();
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn line_callback_mode() {
        let (tx, mut rx) = test_mpsc::channel::<String>(10);

        let mut shell = Shell::new("sh")
            .line_callback()
            .on_output(move |line| {
                let tx = tx.clone();
                async move {
                    let _ = tx.send(line).await;
                }
            })
            .spawn()
            .await
            .unwrap();

        shell.send_line("echo line1").await.unwrap();
        let line1 = timeout(Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(line1, "line1");

        shell.exit().await.unwrap();
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn pre_send_filter() {
        let mut shell = Shell::new("sh")
            .enable_buffer()
            .on_send(|cmd| async move {
                if cmd.contains("BLOCK") {
                    None
                } else {
                    Some(cmd.replace("FOO", "BAR"))
                }
            })
            .spawn()
            .await
            .unwrap();

        shell.send_line("echo BLOCK_ME").await.unwrap();
        let blocked = shell.output(Some(Duration::from_millis(200)), None).await;
        assert!(!blocked.stdout.contains("BLOCK_ME"));

        shell.send_line("echo FOO_TEST").await.unwrap();
        let modified = shell.output(Some(Duration::from_millis(200)), None).await;
        assert!(modified.stdout.contains("BAR_TEST"));

        shell.exit().await.unwrap();
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn reset_clears_environment() {
        let mut shell = Shell::new("sh").enable_buffer().spawn().await.unwrap();

        shell.send_line("EXPORTED_VAR=12345").await.unwrap();
        shell.send_line("echo $EXPORTED_VAR").await.unwrap();
        let out1 = shell.output(Some(Duration::from_millis(200)), None).await;
        assert!(out1.stdout.contains("12345"));

        shell.reset().await.expect("reset failed");

        shell.send_line("echo $EXPORTED_VAR").await.unwrap();
        let out2 = shell.output(Some(Duration::from_millis(200)), None).await;
        assert!(!out2.stdout.contains("12345"));

        shell.exit().await.unwrap();
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn work_dir_and_env_take_effect() {
        let mut shell = Shell::new("sh")
            .enable_buffer()
            .work_dir("/tmp")
            .env("SHELL_ENGINE_X", "42")
            .spawn()
            .await
            .unwrap();

        shell
            .send_line("pwd; echo X=$SHELL_ENGINE_X")
            .await
            .unwrap();
        let out = shell.output(Some(Duration::from_millis(300)), None).await;
        assert!(out.stdout.contains("/tmp"));
        assert!(out.stdout.contains("X=42"));

        shell.exit().await.unwrap();
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn reset_preserves_profile_overrides() {
        let mut shell = Shell::new("sh")
            .enable_buffer()
            .work_dir("/tmp")
            .env("SHELL_ENGINE_Y", "keep")
            .spawn()
            .await
            .unwrap();

        shell.reset().await.expect("reset failed");

        shell
            .send_line("pwd; echo Y=$SHELL_ENGINE_Y")
            .await
            .unwrap();
        let out = shell.output(Some(Duration::from_millis(300)), None).await;
        assert!(out.stdout.contains("/tmp"));
        assert!(out.stdout.contains("Y=keep"));

        shell.exit().await.unwrap();
    }

    #[tokio::test]
    #[cfg(all(unix, feature = "pty"))]
    async fn pty_work_dir_takes_effect() {
        let mut shell = Shell::new("sh")
            .enable_pty()
            .enable_buffer()
            .work_dir("/tmp")
            .spawn()
            .await
            .unwrap();

        shell.send_line("pwd").await.unwrap();
        let out = shell.output(Some(Duration::from_millis(300)), None).await;
        assert!(out.stdout.contains("/tmp"));

        shell.exit().await.unwrap();
    }

    #[tokio::test]
    #[cfg(all(unix, feature = "pty"))]
    async fn pty_basic_output_and_snapshot() {
        let mut shell = Shell::new("sh")
            .enable_pty()
            .enable_buffer()
            .spawn()
            .await
            .expect("failed to spawn pty shell");

        assert!(shell.is_pty());

        shell.send_line("printf 'SNAP_MARK\\n'").await.unwrap();
        let snap = shell
            .output_snapshot(Some(Duration::from_millis(300)), None)
            .await
            .expect("snapshot failed");
        assert!(snap.contains("SNAP_MARK"));

        shell.exit().await.unwrap();
    }
}
