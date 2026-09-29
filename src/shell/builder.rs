//! `ShellBuilder`：链式配置 + `spawn()`。

use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Result, ensure};
use tokio::sync::Notify;

use crate::shell::Shell;
use crate::shell::backend::LaunchConfig;
#[cfg(feature = "pty")]
use crate::shell::backend::PtyOptions;
use crate::shell::buffer::OutputBuffer;
use crate::shell::callbacks::{
    AsyncCloseCallback, AsyncErrorCallback, AsyncExitCallback, AsyncOutputCallback,
    AsyncPreSendCallback, CallbackHub, CallbackMode, Callbacks, PreSendHook,
};
use crate::shell::profile::ShellProfile;

const DEFAULT_BUFFER_CAPACITY: usize = 1024 * 1024;

/// 链式配置持久 shell 会话的构建器。
///
/// 通过 [`Shell::new`](crate::shell::Shell::new) 或 [`ShellBuilder::new`]
/// 创建，随后链式调用各配置项，最后以 [`spawn`](ShellBuilder::spawn) 启动。
pub struct ShellBuilder {
    shell_path: String,
    pre_send: Option<AsyncPreSendCallback>,
    callbacks: Callbacks,
    close_notify: Arc<Notify>,
    buffer_capacity: Option<usize>,

    // 进程级覆盖项：设置后优先于 ShellProfile 的内置默认值
    args: Option<Vec<String>>,
    init_input: Option<String>,
    exit_input: Option<String>,
    work_dir: Option<PathBuf>,
    envs: Option<Vec<(String, String)>>,

    #[cfg(feature = "pty")]
    pty_opts: Option<PtyOptions>,
}

impl ShellBuilder {
    /// 新建一个构建器，指定要启动的 shell（可传路径，如 `/bin/zsh`）。
    pub fn new(shell: impl Into<String>) -> Self {
        Self {
            shell_path: shell.into(),
            pre_send: None,
            callbacks: Callbacks::default(), // mode = Raw
            close_notify: Arc::new(Notify::new()),
            buffer_capacity: None,

            args: None,
            init_input: None,
            exit_input: None,
            work_dir: None,
            envs: None,

            #[cfg(feature = "pty")]
            pty_opts: None,
        }
    }

    // ── 进程配置（profile 覆盖项）──────────────────────────────────────────

    /// 覆盖启动参数。设置后不再使用各 shell 的内置默认参数
    /// （注意：内置默认会根据 pipe/pty 模式自动切换，覆盖后两种模式使用同一组参数）。
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        S: Into<String>,
        I: IntoIterator<Item = S>,
    {
        self.args = Some(args.into_iter().map(Into::into).collect());
        self
    }

    /// 覆盖会话建立后立即写入 stdin 的初始化输入。
    /// 需自带行结束符（如 `"cd /tmp\n"`）。
    pub fn init_input(mut self, input: impl Into<String>) -> Self {
        self.init_input = Some(input.into());
        self
    }

    /// 覆盖 `exit()` 时发送的退出输入。需自带行结束符；
    /// 未设置时未知 shell 将只通过关闭 stdin（EOF）退出。
    pub fn exit_input(mut self, input: impl Into<String>) -> Self {
        self.exit_input = Some(input.into());
        self
    }

    /// 设置子进程的工作目录。
    pub fn work_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.work_dir = Some(dir.into());
        self
    }

    /// 追加一个子进程环境变量（在父进程环境之上叠加）。
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.envs
            .get_or_insert_with(Vec::new)
            .push((key.into(), value.into()));
        self
    }

    /// 批量追加子进程环境变量（在父进程环境之上叠加）。
    pub fn envs<I, K, V>(mut self, envs: I) -> Self
    where
        K: Into<String>,
        V: Into<String>,
        I: IntoIterator<Item = (K, V)>,
    {
        self.envs
            .get_or_insert_with(Vec::new)
            .extend(envs.into_iter().map(|(k, v)| (k.into(), v.into())));
        self
    }

    // ── PTY 配置 ──────────────────────────────────────────────────────────

    #[cfg(feature = "pty")]
    fn pty_opts_mut(&mut self) -> &mut PtyOptions {
        self.pty_opts.get_or_insert_with(PtyOptions::default)
    }

    /// 启用后使用 PTY（伪终端）模式而不是管道模式。
    ///
    /// PTY 模式下：
    /// - stdout / stderr 会被**合并**为一路输出（`on_error` / `stderr` 永远为空）；
    /// - 子进程会看到自己连接在一个真实终端上（支持全屏程序、prompt 着色、
    ///   job control）；
    /// - 终端默认开启回显（echo），发送的命令本身也会出现在输出里；
    /// - 支持 `resize()`、`output_snapshot()`（渲染后的屏幕快照）。
    #[cfg(feature = "pty")]
    pub fn enable_pty(mut self) -> Self {
        self.pty_opts_mut();
        self
    }

    /// 设置 PTY 初始窗口尺寸（默认 80x24）。仅在 `enable_pty()` 后生效。
    #[cfg(feature = "pty")]
    pub fn pty_size(mut self, cols: u16, rows: u16) -> Self {
        let opts = self.pty_opts_mut();
        opts.cols = cols;
        opts.rows = rows;
        self
    }

    /// 设置 vt100 屏幕快照的回滚缓冲行数（默认 2000）。
    #[cfg(feature = "pty")]
    pub fn scrollback(mut self, lines: usize) -> Self {
        self.pty_opts_mut().scrollback = lines;
        self
    }

    /// 关闭 vt100 屏幕追踪，节省 CPU/内存。关闭后 `output_snapshot()` /
    /// `screen_clone()` 将返回错误。
    #[cfg(feature = "pty")]
    pub fn disable_snapshot(mut self) -> Self {
        self.pty_opts_mut().track_screen = false;
        self
    }

    // ── 缓冲区 ────────────────────────────────────────────────────────────

    /// 启用输出缓冲，使用默认容量（1 MB）。
    pub fn enable_buffer(mut self) -> Self {
        self.buffer_capacity = Some(DEFAULT_BUFFER_CAPACITY);
        self
    }

    /// 启用输出缓冲，指定容量上限（字节）；超出后丢弃最旧数据。
    pub fn enable_buffer_with_capacity(mut self, max_bytes: usize) -> Self {
        self.buffer_capacity = Some(max_bytes);
        self
    }

    // ── 回调模式 ──────────────────────────────────────────────────────────

    /// 切换为行回调模式：`on_output`/`on_error` 以完整行为单位触发。
    pub fn line_callback(mut self) -> Self {
        self.callbacks.mode = CallbackMode::Line;
        self
    }

    /// 切换为原始块回调模式（默认）：读到多少立即回调，延迟最低。
    pub fn raw_callback(mut self) -> Self {
        self.callbacks.mode = CallbackMode::Raw;
        self
    }

    // ── 回调注册 ──────────────────────────────────────────────────────────

    /// 注册发送前拦截钩子：每次 `Shell::send` 发送内容前调用，返回 `Some(新内容)`
    /// 表示放行（可改写），返回 `None` 表示拦截该次发送。
    pub fn on_send<F, Fut>(mut self, mut f: F) -> Self
    where
        F: FnMut(String) -> Fut + Send + 'static,
        Fut: Future<Output = Option<String>> + Send + 'static,
    {
        let cb: AsyncPreSendCallback = Box::new(move |s| Box::pin(f(s)));
        self.pre_send = Some(cb);
        self
    }

    /// 注册标准输出回调：输出到达时触发，回调内容为解码后的文本。
    pub fn on_output<F, Fut>(mut self, mut f: F) -> Self
    where
        F: FnMut(String) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let cb: AsyncOutputCallback = Box::new(move |s| Box::pin(f(s)));
        self.callbacks.on_output = Some(cb);
        self
    }

    /// 注册 stderr 回调。**注意**：PTY 模式下 stdout/stderr 合并，此回调不会被触发。
    pub fn on_error<F, Fut>(mut self, mut f: F) -> Self
    where
        F: FnMut(String) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let cb: AsyncErrorCallback = Box::new(move |s| Box::pin(f(s)));
        self.callbacks.on_error = Some(cb);
        self
    }

    /// 注册退出回调：子进程退出时触发，参数为退出码（`None` 表示无法获取）。
    pub fn on_exit<F, Fut>(mut self, mut f: F) -> Self
    where
        F: FnMut(Option<i32>) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let cb: AsyncExitCallback = Box::new(move |c| Box::pin(f(c)));
        self.callbacks.on_exit = Some(cb);
        self
    }

    /// 注册关闭回调：会话完全关闭（子进程退出、IO 任务收尾完成）后触发。
    pub fn on_close<F, Fut>(mut self, mut f: F) -> Self
    where
        F: FnMut() -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let cb: AsyncCloseCallback = Box::new(move || Box::pin(f()));
        self.callbacks.on_close = Some(cb);
        self
    }

    // ── spawn ─────────────────────────────────────────────────────────────

    /// 按照当前配置启动会话，返回可交互的 [`Shell`] 实例。
    ///
    /// # Errors
    ///
    /// - 当 shell 路径为空或无法解析出名称时返回 `Err`；
    /// - 当底层子进程启动失败时返回 `Err`。
    pub async fn spawn(self) -> Result<Shell> {
        let shell_path = self.shell_path.trim().to_string();
        ensure!(!shell_path.is_empty(), "shell path cannot be empty");

        // 唯一的 profile 构造点：名称归一化失败在此统一报错
        let mut profile = ShellProfile::new(shell_path)?;
        if let Some(args) = self.args {
            profile.set_args(args);
        }
        if let Some(input) = self.init_input {
            profile.set_init_input(input);
        }
        if let Some(input) = self.exit_input {
            profile.set_exit_input(input);
        }
        if let Some(dir) = self.work_dir {
            profile.set_work_dir(dir);
        }
        if let Some(envs) = self.envs {
            profile.set_env(envs);
        }

        let pre_send = PreSendHook::new(self.pre_send);
        let callbacks = CallbackHub::new(self.callbacks);
        let output_buffer = self
            .buffer_capacity
            .map(|cap| Arc::new(OutputBuffer::new(cap)));

        #[cfg(feature = "pty")]
        let is_pty = self.pty_opts.is_some();
        #[cfg(not(feature = "pty"))]
        let is_pty = false;

        // PTY 模式下 stdout/stderr 合并到 output_buffer，不需要独立的 error_buffer。
        let error_buffer = if is_pty {
            None
        } else {
            self.buffer_capacity
                .map(|cap| Arc::new(OutputBuffer::new(cap)))
        };

        let cfg = LaunchConfig {
            profile,
            callbacks,
            output_buffer,
            error_buffer,
            #[cfg(feature = "pty")]
            pty_opts: self.pty_opts,
        };

        Shell::spawn_new(cfg, pre_send, self.close_notify).await
    }
}
