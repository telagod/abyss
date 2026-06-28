<div align="center">

# abyss

**你的 Agent 浪费了 85% 的 tokens。吾来修。**

AI 编程 Agent 的代码图谱 + Token 压缩层。<br>
单二进制。零配置。14 种语言。

[![CI](https://github.com/telagod/abyss/actions/workflows/ci.yml/badge.svg)](https://github.com/telagod/abyss/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/code-abyss.svg)](https://crates.io/crates/code-abyss)
[![npm](https://img.shields.io/npm/v/@code-abyss/cli.svg)](https://www.npmjs.com/package/@code-abyss/cli)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

[官网](https://telagod.github.io/abyss/) · [文档](https://telagod.github.io/abyss/docs/) · [更新日志](CHANGELOG.md)

**[English](README.md)** · **中文**

</div>

---

## 问题

AI 编程 Agent 改一行代码要读整个文件。跑一次 `cargo test` 倒出 5 万 token，有用的就 3 行。改函数之前不知道谁在调用它。

**abyss** 用两层互相增强的架构解决这个问题：

| 层 | 做什么 | 结果 |
|---|-------|------|
| **代码图谱** | 调用图、爆炸半径、热点评分 — 基于 tree-sitter + git 历史 | Agent 编辑前就知道*谁调了它*、*改了会炸什么* |
| **代理压缩** | 拦截 Agent 命令，结构化压缩输出 | 节省 85% token，零信息损失 |

## 快速演示

```
$ abyss setup
✓ 210 files, 1545 symbols, 12216 refs in 165ms
✓ hooks installed: pre-edit + post-edit + proxy

$ abyss callers batch_resolve_refs
callers of 'batch_resolve_refs' (1 prod):
  1. src/indexer/pipeline.rs:255 → run_structural()  (100%, call)

$ abyss gain
╭────────────────────────────────────────────╮
│  abyss proxy —   96K tokens saved (85%)   │
╰────────────────────────────────────────────╯
  148 commands proxied
  113K raw → 17K delivered
```

## 安装

```sh
# 预编译二进制 (linux / macOS / Windows, x64 / arm64)
curl -fsSL https://raw.githubusercontent.com/telagod/abyss/main/install.sh | bash

# 包管理器
npm install -g @code-abyss/cli
cargo binstall code-abyss       # 或: cargo install code-abyss
```

<details>
<summary>更多安装方式</summary>

```sh
# 国内镜像
curl -fsSL https://cdn.jsdelivr.net/gh/telagod/abyss@main/install.sh | bash

# Windows
npm install -g @code-abyss/cli
# 或: GitHub Releases 下载 .zip

# 从源码编译
git clone https://github.com/telagod/abyss && cd abyss
cargo install --path .

# Shell 补全 (bash / zsh / fish / powershell)
abyss completion zsh > ~/.zfunc/_abyss
```

</details>

## 60 秒上手

```sh
cd your-project
abyss setup       # 索引 + 钩子 + 代理压缩，一条命令搞定
```

完事。你的 Agent 现在拥有：
- **编辑前安全卡** — 调用者、爆炸半径、风险评分，每次编辑前自动展示
- **编辑后增量重索引** — 基于哈希，毫秒级
- **代理压缩** — 所有命令输出自动压缩

支持 **Claude Code**、**Codex CLI**、**Gemini CLI**、**OpenClaw**。零配置。

## 能做什么？

### 代码智能

```sh
abyss callers SetError         # 谁调用了这个？
abyss impact  SetError         # 改了它会炸什么？
abyss context src/auth.go      # 完整编辑前卡片：调用者、依赖、风险、耦合
abyss map                      # 热点 + 变更耦合概览
abyss search  "validate"       # 符号 + 全文融合搜索
abyss where   src/auth.go      # 架构坐标（层 / 模块 / 角色）
abyss history src/auth.go      # 文件演变历史
```

### 代理压缩

```sh
abyss proxy cargo test         # 执行 + 压缩 + 记录
abyss proxy --explain          # 显示哪个处理器命中及原因
abyss gain                     # token 节省仪表盘
```

### 集成

```sh
abyss attach claude            # 为 Claude Code 安装钩子
abyss attach all               # claude + codex + gemini 一键安装
abyss mcp                      # MCP 服务器（9 个工具，stdio）
abyss daemon start --detach    # 后台文件保存时自动重索引
```

所有命令支持 `--json` 供机器消费。

## 真实压缩数据

来自真实编程会话，非合成测试。

| 场景 | 无 abyss | 有 abyss | 压缩比 |
|------|:-------:|:-------:|:-----:|
| "谁调用了这个函数？" | 221 KB（读 6 个文件） | 6 KB（调用图） | **36×** |
| "查找 `run_structural` 所有用法" | 328 KB（grep + 读文件） | 1.7 KB（callers） | **195×** |
| 代码库全景 | 291 KB（读全部 .rs） | 2.1 KB（map） | **138×** |
| `cargo test`（237 个测试通过） | 8,861 tokens | 11 tokens | **99.9%** |
| `cat` 大文件（862 行） | 8,151 tokens | 782 tokens | **90.4%** |
| `git diff` | 4,493 tokens | 862 tokens | **80.8%** |

> 小结果 0% 是正确行为 — never-worse 守卫会放过无法无损压缩的输出。

## 解析精度

不是编译器，也不是猜测。基于 [SCIP](https://sourcegraph.com/docs/code-intelligence/scip)（编译器级）真值测量：

| 语料库 | 语言 | 精度 | 召回率 |
|--------|------|:----:|:-----:|
| gin v1.10 | Go | **99.4%** | 83.0% |
| hono v4.6 | TypeScript | **98.9%** | 64.6% |
| click 8.1 | Python | **99.3%** | 94.6% |
| ripgrep 14.1 | Rust | **98.5%** | 75.5% |
| abyss (dogfood) | Rust | **100%** | 76.0% |
| cmark 0.31 | C | **99.1%** | 74.8% |

所有语料库 ≥ 98.5% 门控精度 — 精度回归是发版阻断项。复现：`cd eval && ./run.sh`

<details>
<summary>解析器工作原理</summary>

分层启发式解析，每层标记置信度分数：

| 层级 | 策略 | 置信度 |
|------|------|:-----:|
| L0 | 接收者类型匹配（`x.M()` 中 `x` 的类型已知） | 0.95 |
| L0b | 命名导入绑定（`import { x } from './m'`） | 0.95 |
| L1 | 同文件，裸调用 / self 调用 | 1.0 |
| L2 | 同包，唯一候选 | 0.95 |
| L3 | 导入限定符匹配，唯一 | 0.9 |
| L4 | 全局唯一符号 | 0.8 |
| L5 | 同包，多候选（降级） | 0.6 |

Agent 接口默认 `min_confidence=0.7` 过滤噪音。

</details>

## 语言支持

**完整调用图**（调用 + 类型引用 + 导入）：Go、Rust、TypeScript/TSX、JavaScript、Python、Java、C、C++

**符号索引 + 搜索**：以上全部 + JSON、TOML、YAML、Bash、HTML、CSS

## 实战验证

在真实代码库上跑 abyss，公开每一个评分 — 包括短板。

| 项目 | 语言 | 文件数 | 索引耗时 | 评分 |
|------|------|------:|--------:|-----:|
| Django 5.1 | Python | 3,292 | 6.9s | **8 / 10** |
| SQLAlchemy 2.0 | Python | 687 | 8.4s | **8 / 10** |
| hono v4.6 | TypeScript | 388 | 0.8s | **8 / 10** |
| helix-editor | Rust | 545 | 1.6s | **7.5 / 10** |
| vite v5.4 | TS/JS 单仓 | 1,793 | 0.9s | **7 / 10** |
| FastAPI 0.115 | Python | 2,164 | 1.1s | **6.5 / 10** |

完整报告：[docs/DOGFOOD.md](docs/DOGFOOD.md)

## vs. 纯压缩工具

| | 纯压缩工具 | **abyss** |
|---|---|---|
| 输出过滤 | ✅ 模式匹配 | ✅ 结构化 + 语义 |
| 代码理解 | ❌ | ✅ 调用图 + 影响分析 |
| 爆炸半径标注 | ❌ | ✅ 风险评分嵌入输出 |
| 智能文件读取 | 花括号计数 | ✅ Tree-sitter AST（14 种语言） |
| 编辑前安全检查 | ❌ | ✅ 调用者、覆盖率缺口 |
| 安装 | 需单独安装 | ✅ 内建于单一二进制 |

## 架构

单 Rust 二进制（~18 MB）。SQLite 索引存储于 `.code-abyss/index.db`。

```
CLI (clap)
 ├── 索引器: 遍历 → tree-sitter 解析 → 分层 SQL 解析器 → git 时序分析
 ├── 代理:   28 个 Rust 处理器 + TOML 规则引擎 → never-worse 守卫
 ├── MCP:    9 个工具，stdio 传输 (rmcp)
 ├── 守护进程: pidfile + Unix socket，文件保存时哈希增量重索引
 └── 钩子:   编辑前卡片 / 编辑后刷新 / 代理重写（2ms 预算）
```

<details>
<summary>构建变体</summary>

| 构建 | 内容 | 大小 |
|------|------|------|
| 默认（精简） | 调用图 + 时序 + 全文 + 代理 + MCP | ~18 MB |
| `--features semantic` | + 嵌入搜索（fastembed / ONNX） | ~43 MB |

</details>

## 开发

```sh
cargo build                    # 精简构建
cargo test                     # 全部测试
cargo clippy -- -D warnings    # 代码检查
cargo fmt --check              # 格式检查

# 冒烟测试
cargo run -- index && cargo run -- stats && cargo run -- map --json
```

## 许可证

Apache-2.0
