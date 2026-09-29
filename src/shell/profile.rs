//! `ShellProfile`：把各个具体 shell（bash/zsh/cmd/...）的"特性"
//! （启动参数 / 初始化命令 / 退出命令）集中到一处，避免这三件事
//! 分散在三个不同的 `match` 语句里，新增一种 shell 支持时容易漏改。

use anyhow::Result;
use std::path::PathBuf;

use crate::tool::normalize_shell_name;

#[derive(Debug, Clone)]
pub(crate) struct ShellProfile {
    name: String,
    path: String,
    args: Option<Vec<String>>,
    init_input: Option<String>,
    exit_input: Option<String>,
    work_dir: Option<PathBuf>,
    env: Option<Vec<(String, String)>>,
}

impl ShellProfile {
    pub fn new(shell_path: String) -> Result<Self> {
        Ok(Self {
            name: normalize_shell_name(shell_path.as_str())?,
            path: shell_path,
            args: None,
            init_input: None,
            exit_input: None,
            work_dir: None,
            env: None,
        })
    }
    pub fn set_env<S, I>(&mut self, envs: I)
    where
        S: Into<String>,
        I: IntoIterator<Item = (S, S)>,
    {
        self.env = Some(
            envs.into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        );
    }
    pub fn set_args<S, I>(&mut self, args: I)
    where
        S: Into<String>,
        I: IntoIterator<Item = S>,
    {
        self.args = Some(args.into_iter().map(|arg| arg.into()).collect());
    }

    pub fn set_init_input<S: Into<String>>(&mut self, init_input: S) {
        self.init_input = Some(init_input.into());
    }
    pub fn set_exit_input<S: Into<String>>(&mut self, exit_input: S) {
        self.exit_input = Some(exit_input.into());
    }
    pub fn set_work_dir<P: Into<PathBuf>>(&mut self, work_dir: P) {
        self.work_dir = Some(work_dir.into());
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn get_name(&self) -> &str {
        &self.name
    }
    pub fn get_path(&self) -> &str {
        &self.path
    }
    pub fn get_work_dir(&self) -> Option<&std::path::Path> {
        self.work_dir.as_deref()
    }
    pub fn get_env(&self) -> Option<&[(String, String)]> {
        self.env.as_deref()
    }

    /// 获取启动参数；如果用户已经通过 `set_args` 设置，则优先返回用户设置的值。
    /// 否则根据 `self.name` 提供各 shell 的默认参数（会考虑 pty_mode）。
    pub fn get_args(&self, pty_mode: bool) -> Option<Vec<String>> {
        if let Some(args) = self.args.as_ref() {
            return Some(args.clone()); // 用户设置优先
        }
        match self.name.as_str() {
            "bash" => {
                if pty_mode {
                    Some(vec!["--norc".into(), "--noprofile".into(), "-i".into()])
                } else {
                    Some(vec!["--norc".into(), "--noprofile".into(), "-s".into()])
                }
            }
            "zsh" => {
                if pty_mode {
                    Some(vec!["-f".into(), "-i".into()])
                } else {
                    Some(vec!["-f".into(), "-s".into()])
                }
            }
            "sh" => {
                if pty_mode {
                    Some(vec!["-i".into()])
                } else {
                    Some(vec!["-s".into()])
                }
            }
            "fish" => Some(vec!["--no-config".into(), "-i".into()]),
            "cmd" => Some(vec!["/Q".into(), "/K".into(), "prompt $G".into()]),
            "powershell" | "pwsh" => Some(vec![
                "-ExecutionPolicy".into(),
                "Bypass".into(),
                "-NoExit".into(),
                "-NoProfile".into(),
            ]),
            "python" => Some(vec!["-u".into(), "-i".into()]),
            "node" => Some(vec!["-i".into()]),
            _ => None, // 允许未知 shell
        }
    }

    pub fn get_init_input(&self) -> Option<String> {
        if let Some(init_input) = self.init_input.as_ref() {
            return Some(init_input.clone());
        }
        match self.name.as_str() {
            "cmd" => Some("chcp 65001 >nul 2>&1\n".into()),
            "powershell" | "pwsh" => Some(
                "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8;\
            [Console]::InputEncoding = [System.Text.Encoding]::UTF8;\
            $OutputEncoding = [System.Text.Encoding]::UTF8;\n"
                    .into(),
            ),
            _ => None,
        }
    }

    pub fn get_exit_input(&self) -> Option<String> {
        if let Some(exit_input) = self.exit_input.as_ref() {
            return Some(exit_input.clone());
        }
        match self.name.as_str() {
            "python" => Some("quit()\n".into()),
            "node" => Some(".exit\n".into()),
            "cmd" | "powershell" | "pwsh" | "sh" | "bash" | "zsh" | "fish" => Some("exit\n".into()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bash_args_differ_between_pipe_and_pty() {
        let p = ShellProfile::new("/bin/bash".into()).unwrap();
        assert_eq!(
            p.get_args(false),
            Some(vec!["--norc".into(), "--noprofile".into(), "-s".into()])
        );
        assert_eq!(
            p.get_args(true),
            Some(vec!["--norc".into(), "--noprofile".into(), "-i".into()])
        );
    }

    #[test]
    fn user_args_override_defaults() {
        let mut p = ShellProfile::new("bash".into()).unwrap();
        p.set_args(["--login"]);
        assert_eq!(p.get_args(false), Some(vec!["--login".to_string()]));
        // 覆盖后与 pty_mode 无关
        assert_eq!(p.get_args(true), Some(vec!["--login".to_string()]));
    }

    #[test]
    fn unknown_shell_is_allowed_with_no_defaults() {
        let p = ShellProfile::new("/bin/nu".into()).unwrap();
        assert_eq!(p.get_name(), "nu");
        assert_eq!(p.get_args(true), None);
        assert_eq!(p.get_init_input(), None);
        assert_eq!(p.get_exit_input(), None);
    }

    #[test]
    fn name_normalization_truncates_version_suffix() {
        let p = ShellProfile::new("/usr/bin/python3.11".into()).unwrap();
        assert_eq!(p.get_name(), "python");
        assert_eq!(p.get_exit_input(), Some("quit()\n".into()));
    }

    #[test]
    fn init_and_exit_inputs_can_be_overridden() {
        let mut p = ShellProfile::new("bash".into()).unwrap();
        p.set_init_input("export FOO=1\n");
        p.set_exit_input("exit 0\n");
        assert_eq!(p.get_init_input(), Some("export FOO=1\n".into()));
        assert_eq!(p.get_exit_input(), Some("exit 0\n".into()));
    }

    #[test]
    fn env_and_work_dir_roundtrip() {
        let mut p = ShellProfile::new("sh".into()).unwrap();
        assert_eq!(p.get_env(), None);
        assert_eq!(p.get_work_dir(), None);

        p.set_env([("FOO", "bar"), ("A", "B")]);
        p.set_work_dir("/tmp");
        assert_eq!(
            p.get_env(),
            Some(&[("FOO".into(), "bar".into()), ("A".into(), "B".into())][..])
        );
        assert_eq!(p.get_work_dir(), Some(std::path::Path::new("/tmp")));
    }
}
