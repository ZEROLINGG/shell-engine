//! 管道模式的进程启动逻辑：拆分独立的 stdout/stderr 读取任务、stdin 写入任务、
//! 以及等待子进程退出的监督任务。

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::shell::buffer::OutputBuffer;
use crate::shell::callbacks::CallbackHub;
use crate::shell::profile::ShellProfile;
use crate::shell::stream::{read_stream, StdinMsg};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

/// 启动管道模式子进程，返回驱动交互所需的句柄。
pub(crate) async fn spawn_process(
    profile: &ShellProfile,
    callbacks: CallbackHub,
    output_buffer: Option<Arc<OutputBuffer>>,
    error_buffer: Option<Arc<OutputBuffer>>,
) -> Result<(mpsc::Sender<StdinMsg>, oneshot::Sender<()>, JoinHandle<()>)> {
    let mut cmd = Command::new(profile.get_path());

    // 未知 shell 允许零参数启动（get_args 返回 None）
    if let Some(args) = profile.get_args(false) {
        cmd.args(args);
    }
    if let Some(dir) = profile.get_work_dir() {
        cmd.current_dir(dir);
    }
    if let Some(envs) = profile.get_env() {
        for (k, v) in envs {
            cmd.env(k, v);
        }
    }

    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    let mut child = cmd.spawn()?;

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();

    let (tx_stdin, mut rx_stdin) = mpsc::channel::<StdinMsg>(32);
    let (drop_tx, drop_rx) = oneshot::channel::<()>();

    if let Some(init) = profile.get_init_input() {
        stdin.write_all(init.as_bytes()).await?;
        stdin.flush().await?;
    }

    // 读取 mode：spawn 后 mode 不再变化，直接拷贝出来避免锁开销
    let mode = callbacks.mode().await;

    let cb_stdout = callbacks.clone();
    let ob_stdout = output_buffer.clone();
    let stdout_task = tokio::spawn(read_stream(stdout, ob_stdout, cb_stdout, false, mode));

    let cb_stderr = callbacks.clone();
    let ob_stderr = error_buffer.clone();
    let stderr_task = tokio::spawn(read_stream(stderr, ob_stderr, cb_stderr, true, mode));

    let stdin_task = tokio::spawn(async move {
        let mut stdin_opt = Some(stdin);
        while let Some(msg) = rx_stdin.recv().await {
            match msg {
                StdinMsg::Close => break,
                StdinMsg::Eof => {
                    if let Some(mut s) = stdin_opt.take() {
                        let _ = s.flush().await;
                        drop(s);
                    }
                }
                StdinMsg::Data(data) => {
                    if let Some(mut stdin) = stdin_opt.take() {
                        const STDIN_TIMEOUT: Duration = Duration::from_secs(30);

                        let write_ok = tokio::time::timeout(
                            STDIN_TIMEOUT,
                            stdin.write_all(data.as_bytes()),
                        )
                            .await
                            .ok()
                            .and_then(|r| r.ok())
                            .is_some();
                        if !write_ok {
                            break; // 确实是管道已断，直接放弃
                        }

                        let flush_ok = tokio::time::timeout(STDIN_TIMEOUT, stdin.flush())
                            .await
                            .ok()
                            .and_then(|r| r.ok())
                            .is_some();
                        if !flush_ok {
                            break;
                        }

                        stdin_opt = Some(stdin);
                    }
                }
                // 管道模式不支持终端尺寸调整，直接忽略
                #[cfg(feature = "pty")]
                StdinMsg::Resize(_, _) => {}
            }
        }
        drop(stdin_opt);
    });

    let cb_main = callbacks.clone();
    let join = tokio::spawn(async move {
        tokio::select! {
            status = child.wait() => {
                let code = status.ok().and_then(|s| s.code());
                cb_main.fire_exit(code).await;
            }
            _ = drop_rx => {
                if let Err(e) = child.kill().await {
                    eprintln!("kill failed (process may have already exited): {e}");
                }
                let _ = child.wait().await;
            }
        }

        let _ = tokio::join!(stdout_task, stderr_task, stdin_task);
        cb_main.fire_close().await;
    });

    Ok((tx_stdin, drop_tx, join))
}