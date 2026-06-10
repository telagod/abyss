# abyss v0.3 重写规划：从原型到可信工具

> 定位一句话：**"The code graph your agent checks before it edits."**
>
> 本文档是整体重写的单一事实源，覆盖两个仓库：
> - `telagod/abyss`（本仓库，现 code-abyss-dev）— Rust CLI，主角
> - `telagod/code-abyss`（npm 包）— agent 集成层，发行渠道之一

## 0. 决策记录

| 决策 | 结论 | 理由 |
|------|------|------|
| 仓库形态 | 独立仓库 `telagod/abyss` | CLI 触达不装 persona 包的用户群（Cursor/CI/裸 agent）；叙事干净 |
| crate 名 | `code-abyss`（bin 名仍为 `abyss`） | crates.io 上 `abyss` 已被占用，`code-abyss` 空缺（2026-06 验证） |
| npm 命名 | `@code-abyss/cli` + 平台包 | scope 空缺（2026-06 验证）；学 esbuild/biome 的 optionalDependencies 模式 |
| 语义搜索 | feature-gate `semantic`，dist 默认关闭 | fastembed→ONNX runtime 使二进制达 43M；核心卖点是图+时间智能，不是 embedding |
| 语言覆盖 | 诚实化：宣称且只宣称 go/rust/ts/js/py | Java/C/C++ extractor 未实现，hook 的 case 列表当前虚胖 |
| License | Apache-2.0（沿用 Cargo.toml） | — |

## 1. 现状与病灶（2026-06-10 盘点）

- 代码：~4.6k 行 Rust，零提交（git 未初始化）、零测试、零 CI、无 README
- resolver 五级 confidence 启发式无 ground truth 验证；歧义匹配取 `filtered[0]` 标 0.5，但出口（CLI 输出 / hook 警告）不区分置信度
- 分发断链：npm 包的 hook `command -v abyss || exit 0` —— 没有二进制分发，hook 对绝大多数用户静默失效
- hook 链路依赖 python3 解析 JSON，两次进程往返，Windows 不可用
- 索引新鲜度无闭环：编辑后无增量更新触发，二次 pre-edit 读旧图
- Cargo.toml 拉 14 个 tree-sitter grammar，ref extractor 只有 4 个语言模块

## 2. Phase 0 — 仓库奠基（半天）

1. `git init` + 初始 commit + 建 `telagod/abyss` 远端
2. Cargo.toml：`name = "code-abyss"`，`[[bin]] name = "abyss"`，加 `repository`/`keywords`/`categories`
3. `[features]`：`semantic = ["fastembed"]`，`embed`/`index-all`/`search --semantic` 路径全部 cfg 门控
4. README 骨架：一句话定位 + 60 秒 quickstart + 能力矩阵（语言 × 命令）+ 诚实的「当前支持」
5. CI：`fmt --check` + `clippy -D warnings` + `cargo test`，linux/macos/windows 三平台
6. 迁移 docs/DESIGN-v0.2.md、本文档入库

**验收**：默认（slim）构建通过且二进制 ≤ 20M；CI 三平台绿。

> 实测（2026-06-10）：slim 43M → 18M（fat LTO + codegen-units=1 + tokio 特性裁剪）。
> 剩余大头是 14 个 tree-sitter grammar 静态表（功能本体，不可裁）。
> 后续优化路径：Phase 4 将 html/css/yaml/toml/json/bash 等「文档型 grammar」拆为
> 默认开启的 `extra-langs` feature——chunker 对未知语言有整文件 Module 降级，移除可平滑。

## 3. Phase 1 — 可信度地基：测试 + eval（2~3 天）

### 3.1 单元 / 黄金测试

- `tests/fixtures/<lang>/`：每语言一个微型项目（10~20 文件），覆盖同文件 / 同包 / 跨包 qualifier / 全局歧义四种解析场景
- extractor 黄金测试：源文件 → 期望 `RawReference` 列表（snapshot 风格）
- resolver 测试：fixture 上逐 confidence 档位断言（1.0 / 0.95 / 0.9 / 0.8 / 0.5 各至少 3 例）
- temporal 测试：脚本生成合成 git 历史 → 断言 hotspot / coupling 数值
- 端到端：`index → context/callers/impact` 在 fixture 上的 JSON snapshot

### 3.2 confidence 出口治理（修第②刀）

- `refs` 查询默认 `confidence >= 0.7`，加 `--min-confidence <f>` 旗标
- JSON 输出每个 caller 带 `confidence` 字段；人类可读输出低置信标 `(?)`
- hook 警告只统计 `confidence >= 0.8` 的 production callers，0.5 档归入 `possible_callers` 单列

### 3.3 Eval harness（惊艳的地基）

- `eval/` 目录：3~5 个 pin 死 commit 的真实仓库（建议：gin、zod、flask、ripgrep）
- ground truth：scip-go / scip-typescript / rust-analyzer 产出的 SCIP 索引
- 指标：caller 解析 precision / recall / F1，按语言分桶
- 产出 `eval/RESULTS.md` 表格，写进 README——**敢公布这张表本身就是护城河**
- CI nightly job 跑 eval，回归即红

**验收**：`cargo test` ≥ 60 个用例全绿；eval 表首版数字落盘（无论高低，先有锚点）。

## 4. Phase 2 — 分发链路（1 天）

1. cargo-dist 接管 release：tag → GitHub Releases 出 5 平台二进制（linux x64/arm64、macos x64/arm64、windows x64），默认 `--no-default-features`（无 semantic，~10-15M）
2. `install.sh` 改为下载预编译二进制（保留 `--from-source` 回退）
3. npm `@code-abyss/cli`：wrapper 包 + 平台 optionalDependencies 包，由本仓库 release workflow 发布 → `npx @code-abyss/cli index` 可用
4. `cargo publish`（crate `code-abyss`）
5. **code-abyss npm 包侧**：installer 检测无 `abyss` 时引导安装（调 `@code-abyss/cli` 或直接拉 GitHub Release），hook 不再静默失效——失效时输出一次性提示

**验收**：全新 Linux/macOS 机器 `curl ... | sh && abyss index` 60 秒内可用；`npx @code-abyss/cli stats` 可用。

## 5. Phase 3 — hook 链路重铸（1 天）

1. 新子命令 `abyss hook pre-edit --stdin [--platform claude|codex|gemini|pi|hermes|openclaw]`
   - 内置各平台 tool-input JSON schema 的 file_path 抽取
   - 内联 staleness 检查：目标文件 mtime > 索引 mtime → 单文件增量重建（预算 < 100ms）再查询
   - 输出：stderr 简明警告（现 shell 脚本的格式），`--json` 给结构化
2. 新子命令 `abyss hook post-edit --stdin`：编辑后单文件增量更新索引（修第⑥刀，新鲜度闭环）
3. code-abyss 仓库 `skills/indexing-code/hooks/`：各平台脚本收缩为一行二进制调用；删除 python3 依赖；Windows 直接可用
4. hook 的文件后缀 case 列表收缩到已实现语言（修第⑤刀的一半）

**验收**：六平台 hook 配置均为单行调用；在无 python3 的容器里 hook 正常出警告；连续两次编辑同文件，第二次读到新图。

## 6. Phase 4 — 语言诚实化（持续）

- Cargo.toml 审计：未参与符号提取的 grammar 移除（json/toml/yaml/bash/html/css 逐个核对用途）
- extractor trait 化 + 黄金测试框架，使新增语言 = 一个模块 + 一组 fixture
- 路线：Java（需求最大）→ C/C++（最难，留到 trait 稳定后）
- README 能力矩阵随实现同步，**永不提前宣称**

## 7. Phase 5 — 叙事重写（1 天，两仓库同步）

### abyss README
- 首屏：一句话 + 终端 demo GIF（大仓库 `abyss impact` 秒出 blast radius）
- 60 秒 quickstart、命令速查、MCP + 六平台 hook 集成矩阵
- benchmark 表（index 耗时 / db 体积，按 repo 规模）+ eval precision/recall 表

### code-abyss（npm）README / site
- 主轴换位：code graph intelligence 升为头条，"powered by abyss"；persona/skills 重述为 agent runtime 层
- `skills/indexing-code/SKILL.md` 安装段改为二进制安装引导
- site/i18n 同步（注意 MEMORY 里记的 skill 数漂移问题，发版时一并收）

### Dogfooding 即 demo
- abyss 仓库 CI：PR 触发 `abyss context` 对 diff 文件出影响面评论（GitHub Action）

## 8. Phase 6 — 惊艳一击：agent A/B 回归实验（2~3 天）

- 20 个真实修改任务（从 fixture 仓库 + 本项目历史 commit 反推）
- 同一 agent、同一模型：带 abyss pre-edit hook vs 裸跑
- 指标：引入回归数（测试失败 / 调用方破坏）、修复轮次、token 消耗
- 产出 `docs/EVAL-agent-ab.md` + README 一张对比图——**这是别人都没有的卖点**

## 9. 里程碑与版本

| 版本 | 内容 | 发布动作 |
|------|------|---------|
| v0.3.0 | Phase 0-3：测试 + 分发 + hook 重铸 | GitHub Release + crates.io + npm @code-abyss/cli |
| v0.3.x | Phase 4 语言补齐（Java） | 补丁版本 |
| v0.4.0 | Phase 5-6：eval 表 + A/B 实验公布 | 对外推广的起点 |
| code-abyss v4.7 | 集成层适配（installer 引导 + hook 单行化 + 叙事重写） | 与 abyss v0.3.0 同窗发布 |

## 10. 风险

| 风险 | 缓解 |
|------|------|
| eval 首版数字难看（precision < 80%） | 仍然公布，把弱项写进 roadmap——诚实本身是差异化；resolver 改进有测试网兜底 |
| cargo-dist 与 npm 平台包联动复杂 | 先 GitHub Release + curl 安装达成「可用」，npm 包作为第二迭代 |
| 单文件增量重建超 100ms 预算 | 降级策略：超时跳过重建、警告标注 "index stale" |
| 双仓库版本协同 | code-abyss installer 读 abyss `--version`，最低版本写死在 skill 常量里 |
