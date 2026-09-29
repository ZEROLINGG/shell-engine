# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

## [Unreleased]

### Added
- crate 级 rustdoc：补充简介与快速开始 doctest，与 `Cargo.toml` description 保持一致
- 公共 API 文档标准化：为返回 `Result` 的方法补 `# Errors`，为内部 `unwrap` 补 `# Panics` 前置说明
- README.md（英文）/ README-zh_CN.md（中文）双语言版：新增目录、设计哲学、适用场景 vs 不适用场景、特性标志、平台与环境支持、MSRV、贡献、变更日志小节，顶部相互引用；修正 `util` → `tool` 模块命名

### Changed
- `SpecialKey::from_str_tag` 增加 `# Examples` doctest（使用 `matches!`，不依赖 `PartialEq`）
- Cargo.toml 补充 `readme = "README.md"` 元数据
- Cargo.toml 补充 `rust-version = "1.85"`、`keywords`、`categories`、`[package.metadata.docs.rs]`（all-features + `--cfg docsrs`）
- feature-gated 重导出（`rust_pty::{PtySignal, WindowSize}`、`vt100`）补 `#[cfg_attr(docsrs, doc(cfg(feature = "pty")))]`
- `pub mod exec/shell/tool` 补模块级文档摘要

### Deprecated
-

### Removed
-

### Fixed
- 修复 `cargo clippy --all-targets --all-features -- -D warnings` 的 8 处告警：`needless_borrow`（exec.rs）、`collapsible_if`（pty.rs，改用 let-chains）、`manual_range_contains`（mod.rs）
- `Shell::new` 标注 `#[allow(clippy::new_ret_no_self)]`（保留 builder 习惯 API）
- 修复 `cargo test --doc --no-default-features` 失败：crate 级 doctest 的 `#[tokio::main]` 改为 `flavor = "current_thread"`
- `tokio::time::sleep` import 加 `#[cfg(feature = "pty")]`，消除 pipe-only 配置下的 `unused_imports` 告警

### Security
-

---

## [0.2.3] - 2026-08-25

### Added
- `ShellProfile` 支持自定义启动参数、初始化/退出输入、工作目录、环境变量；覆盖项跨 `reset()` 保留

### Changed
- `pipe` / `pty` 后端移入 `shell` 模块统一组织

---

## [0.2.0] - 2026-07-29

### Fixed
- 修复 edition 2024 下的 `unsafe extern` 语法
- 移除未使用的 import

---

## [0.1.0] - 2026-07-28

### Added
- 首个发布版本：持久化 shell 会话、管道模式、PTY 模式、一次性 `exec()`、输出缓冲与回调

---

<!-- 版本对比链接，便于快速查看每次发布的完整 diff -->
[Unreleased]: https://github.com/ZEROLINGG/shell-engine/compare/v0.2.3...HEAD
[0.2.3]: https://github.com/ZEROLINGG/shell-engine/compare/v0.2.0...v0.2.3
[0.2.0]: https://github.com/ZEROLINGG/shell-engine/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/ZEROLINGG/shell-engine/releases/tag/v0.1.0
