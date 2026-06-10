# code-abyss v0.2 工程设计：代码关系图 + 时间维度 + 影响面分析

## 0. 设计原则

- **无 embedding 依赖**：v0.2 全部能力基于 tree-sitter AST + git log + SQLite 图查询，零模型推理
- **增量计算**：引用图和时间数据都支持增量更新，不需要全量重建
- **秒级查询**：所有 MCP tool 响应 < 500ms（SQLite 索引 + 预计算）
- **语言可扩展**：每种语言的引用提取规则是独立模块，新增语言不改核心

---

## 1. 数据模型扩展

### 1.1 新增 SQLite 表

```sql
-- ═══ 代码关系图 ═══

-- 跨文件引用边（调用图的边）
CREATE TABLE refs (
    id              INTEGER PRIMARY KEY,
    source_file_id  INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    source_line     INTEGER NOT NULL,
    source_symbol   TEXT,              -- 调用方符号名（可选，用于 scope 感知）
    target_name     TEXT NOT NULL,     -- 被引用的符号名
    target_file_id  INTEGER,           -- 解析后的目标文件（NULL = 未解析）
    target_symbol_id INTEGER,          -- 解析后的目标符号（NULL = 未解析）
    kind            TEXT NOT NULL,     -- call | type_ref | import | inherit | implement | field_access
    confidence      REAL DEFAULT 1.0   -- 解析置信度 (0.0-1.0)
);
CREATE INDEX idx_refs_source ON refs(source_file_id);
CREATE INDEX idx_refs_target ON refs(target_file_id);
CREATE INDEX idx_refs_target_name ON refs(target_name);

-- ═══ 时间维度 ═══

-- Git commits
CREATE TABLE commits (
    id      INTEGER PRIMARY KEY,
    hash    TEXT NOT NULL UNIQUE,
    author  TEXT NOT NULL,
    ts      INTEGER NOT NULL,          -- unix timestamp
    message TEXT
);

-- Commit ↔ File 关联（含 diff stats）
CREATE TABLE commit_files (
    commit_id INTEGER NOT NULL REFERENCES commits(id) ON DELETE CASCADE,
    file_path TEXT NOT NULL,
    added     INTEGER DEFAULT 0,
    deleted   INTEGER DEFAULT 0,
    PRIMARY KEY (commit_id, file_path)
);
CREATE INDEX idx_cf_path ON commit_files(file_path);

-- ═══ 预计算指标 ═══

-- 文件级指标（定期刷新）
CREATE TABLE file_metrics (
    file_id          INTEGER PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
    cyclomatic        REAL DEFAULT 0,     -- 圈复杂度（最大函数）
    max_func_lines    INTEGER DEFAULT 0,  -- 最长函数行数
    change_count_30d  INTEGER DEFAULT 0,  -- 近 30 天变更次数
    change_count_90d  INTEGER DEFAULT 0,
    last_changed_ts   INTEGER,
    unique_authors    INTEGER DEFAULT 0,  -- 贡献者数
    hotspot_score     REAL DEFAULT 0,     -- change_count × cyclomatic
    has_tests         INTEGER DEFAULT 0   -- 是否有对应测试文件
);

-- 变更耦合矩阵（top N 对）
CREATE TABLE change_coupling (
    file_a         TEXT NOT NULL,
    file_b         TEXT NOT NULL,
    co_changes     INTEGER NOT NULL,     -- 同一 commit 出现次数
    total_changes  INTEGER NOT NULL,     -- file_a 总变更数
    coupling_score REAL NOT NULL,        -- co_changes / total_changes
    PRIMARY KEY (file_a, file_b)
);
```

### 1.2 表关系总览

```
files ──1:N──→ chunks ──1:N──→ symbols
  │                               ↑
  │              refs ────────────┘
  │           (source → target)
  │
  ├──→ file_metrics (1:1 预计算)
  │
  └──→ commit_files ──→ commits
              ↓
        change_coupling (预计算)
```

---

## 2. 引用提取引擎 (Reference Extractor)

### 2.1 架构

```
src/graph/
├── mod.rs           // pub use
├── extractor.rs     // ReferenceExtractor trait + 调度
├── resolver.rs      // 符号解析（name → definition）
├── query.rs         // 图遍历（impact、depends_on）
└── languages/
    ├── mod.rs       // 语言注册表
    ├── go.rs        // Go 引用提取
    ├── rust_lang.rs // Rust 引用提取
    ├── typescript.rs
    └── python.rs
```

### 2.2 核心 trait

```rust
/// 从 AST 中提取的原始引用（未解析）
pub struct RawReference {
    pub line: u32,
    pub source_symbol: Option<String>,  // 引用发生在哪个函数/方法内
    pub target_name: String,            // 被引用的名字
    pub target_qualifier: Option<String>, // 包名/模块名前缀
    pub kind: RefKind,
}

pub enum RefKind {
    Call,        // 函数/方法调用
    TypeRef,     // 类型引用
    Import,      // 导入语句
    Inherit,     // 继承/实现
    FieldAccess, // 字段访问
}

pub trait LanguageRefExtractor: Send + Sync {
    /// 从 AST 提取原始引用
    fn extract(&self, tree: &Tree, source: &str) -> Vec<RawReference>;

    /// 判断文件是否为测试文件
    fn is_test_file(&self, path: &str) -> bool;

    /// 从导入语句推断目标文件路径
    fn resolve_import(&self, import_path: &str, workspace: &Path) -> Option<PathBuf>;
}
```

### 2.3 Go 引用提取（最复杂的示例）

```rust
// tree-sitter 查询 — Go 语言
const GO_REF_QUERY: &str = r#"
;; 直接函数调用: foo()
(call_expression
  function: (identifier) @callee) @call

;; 限定调用: pkg.Foo()
(call_expression
  function: (selector_expression
    operand: (identifier) @receiver
    field: (field_identifier) @method)) @qualified_call

;; 方法调用: obj.Method()
(call_expression
  function: (selector_expression
    operand: (_) @obj
    field: (field_identifier) @method_name)) @method_call

;; 类型引用: var x SomeType
(type_identifier) @type_ref

;; 限定类型: pkg.SomeType
(qualified_type
  package: (package_identifier) @pkg
  name: (type_identifier) @qtype)

;; 接口嵌入/结构体嵌入
(field_declaration
  type: (type_identifier) @embedded_type
  !name) @embedding
"#;

impl LanguageRefExtractor for GoExtractor {
    fn extract(&self, tree: &Tree, source: &str) -> Vec<RawReference> {
        let query = Query::new(&tree_sitter_go::LANGUAGE.into(), GO_REF_QUERY).unwrap();
        let mut cursor = QueryCursor::new();
        let mut refs = Vec::new();

        // 先建立 scope map：每行属于哪个函数
        let scope_map = build_scope_map(tree, source);

        for m in cursor.matches(&query, tree.root_node(), source.as_bytes()) {
            // 根据 pattern index 分发处理
            match m.pattern_index {
                0 => { /* 直接调用 */ }
                1 => { /* 限定调用 */ }
                // ...
            }
        }
        refs
    }

    fn is_test_file(&self, path: &str) -> bool {
        path.ends_with("_test.go")
    }

    fn resolve_import(&self, import_path: &str, workspace: &Path) -> Option<PathBuf> {
        // Go 的 import path → 本地目录映射
        // 内部包: "github.com/user/repo/internal/service" → workspace/internal/service/
        // 标准库: "fmt" → None (不索引)
        // ...
    }
}
```

### 2.4 符号解析算法（核心难点）

```rust
/// 将 RawReference.target_name 解析到具体的 Symbol
pub struct SymbolResolver<'a> {
    repo: &'a Repository,
}

impl<'a> SymbolResolver<'a> {
    /// 解析策略（按置信度从高到低）
    pub fn resolve(&self, raw: &RawReference, source_file_id: i64) -> ResolvedRef {
        // 1. 同文件解析 (confidence = 1.0)
        //    在同一文件的 symbols 中查找
        if let Some(sym) = self.find_in_file(source_file_id, &raw.target_name) {
            return ResolvedRef { target_file_id: Some(source_file_id), confidence: 1.0, .. };
        }

        // 2. 同包解析 (confidence = 0.95)
        //    Go: 同目录; Rust: 同 mod; TS: 同目录 + index.ts
        if let Some((fid, sid)) = self.find_in_package(source_file_id, &raw.target_name) {
            return ResolvedRef { target_file_id: Some(fid), confidence: 0.95, .. };
        }

        // 3. 导入路径解析 (confidence = 0.9)
        //    通过 qualifier (包名前缀) 匹配 import 语句，定位目标文件
        if let Some(qualifier) = &raw.target_qualifier {
            if let Some(fid) = self.resolve_via_import(source_file_id, qualifier, &raw.target_name) {
                return ResolvedRef { target_file_id: Some(fid), confidence: 0.9, .. };
            }
        }

        // 4. 全局唯一匹配 (confidence = 0.8)
        //    符号名在整个 codebase 中唯一
        let candidates = self.find_global(&raw.target_name);
        if candidates.len() == 1 {
            return ResolvedRef { target_file_id: Some(candidates[0].file_id), confidence: 0.8, .. };
        }

        // 5. 未解析 (confidence = 0.0)
        //    保留 target_name，后续可补充解析
        ResolvedRef { target_file_id: None, confidence: 0.0, .. }
    }
}
```

**同包判定逻辑：**

```rust
fn same_package(path_a: &str, path_b: &str, language: &str) -> bool {
    match language {
        "go" => {
            // Go: 同目录 = 同 package
            parent(path_a) == parent(path_b)
        }
        "rust" => {
            // Rust: 同 mod (需要 mod.rs / lib.rs 分析)
            parent(path_a) == parent(path_b)
                || is_mod_child(path_a, path_b)
        }
        "typescript" | "javascript" => {
            // TS: 同目录，或 index.ts re-export
            parent(path_a) == parent(path_b)
        }
        "python" => {
            // Python: 同目录 (__init__.py)
            parent(path_a) == parent(path_b)
        }
        _ => parent(path_a) == parent(path_b),
    }
}
```

---

## 3. 图查询引擎 (Graph Query)

### 3.1 影响面分析（核心杀手功能）

```rust
pub struct ImpactAnalysis {
    pub direct_callers: Vec<CallerInfo>,
    pub transitive_callers: Vec<CallerInfo>,  // BFS 到 depth N
    pub affected_tests: Vec<TestInfo>,
    pub uncovered_paths: Vec<String>,          // 没有测试覆盖的调用链
    pub risk_score: f64,                       // 综合风险评分
}

pub fn analyze_impact(
    repo: &Repository,
    file_path: &str,
    symbol_name: &str,
    max_depth: u32,
) -> Result<ImpactAnalysis> {
    // 1. 找到目标符号
    let target_sym = repo.find_symbol(file_path, symbol_name)?;

    // 2. 反向 BFS 找所有调用方
    let mut visited: HashSet<i64> = HashSet::new();
    let mut queue: VecDeque<(i64, u32)> = VecDeque::new(); // (symbol_id, depth)
    let mut direct = Vec::new();
    let mut transitive = Vec::new();

    // 种子：所有直接引用目标的 ref
    let direct_refs = repo.query(
        "SELECT r.source_file_id, r.source_line, r.source_symbol, f.path, r.confidence
         FROM refs r JOIN files f ON r.source_file_id = f.id
         WHERE r.target_name = ?1 AND (r.target_file_id = ?2 OR r.target_file_id IS NULL)
         AND r.kind IN ('call', 'field_access')",
        params![symbol_name, target_sym.file_id],
    )?;

    for ref_row in &direct_refs {
        direct.push(CallerInfo { /* ... */ });
        if let Some(src_sym) = &ref_row.source_symbol {
            queue.push_back((src_sym_id, 1));
        }
    }

    // BFS 扩展
    while let Some((sym_id, depth)) = queue.pop_front() {
        if depth > max_depth || visited.contains(&sym_id) { continue; }
        visited.insert(sym_id);

        let callers = repo.find_callers_of(sym_id)?;
        for caller in callers {
            transitive.push(CallerInfo { depth, ..caller });
            queue.push_back((caller.symbol_id, depth + 1));
        }
    }

    // 3. 识别受影响的测试
    let all_affected_files: HashSet<i64> = direct.iter()
        .chain(transitive.iter())
        .map(|c| c.file_id)
        .collect();

    let affected_tests: Vec<TestInfo> = all_affected_files.iter()
        .filter(|fid| repo.is_test_file(**fid).unwrap_or(false))
        .map(|fid| TestInfo { /* ... */ })
        .collect();

    // 4. 发现未覆盖的路径
    let non_test_callers: Vec<_> = direct.iter()
        .chain(transitive.iter())
        .filter(|c| !c.is_test)
        .collect();
    let covered_files: HashSet<i64> = affected_tests.iter().map(|t| t.file_id).collect();
    let uncovered: Vec<String> = non_test_callers.iter()
        .filter(|c| !covered_files.contains(&c.file_id))
        .map(|c| format!("{}:{}", c.file_path, c.symbol_name))
        .collect();

    // 5. 风险评分
    let risk = compute_risk_score(
        direct.len(),
        transitive.len(),
        uncovered.len(),
        repo.get_file_metrics(target_sym.file_id)?,
    );

    Ok(ImpactAnalysis { direct_callers: direct, transitive_callers: transitive,
        affected_tests, uncovered_paths: uncovered, risk_score: risk })
}
```

### 3.2 模块依赖图

```rust
/// 生成目录级依赖图（不是文件级，太细了）
pub fn module_dependency_graph(repo: &Repository) -> Result<ModuleGraph> {
    // 将 refs 表聚合到目录级别
    let edges = repo.query(
        "SELECT
            replace(sf.path, '/' || replace(sf.path, rtrim(sf.path, replace(sf.path, '/', '')), ''), '') as src_module,
            replace(tf.path, '/' || replace(tf.path, rtrim(tf.path, replace(tf.path, '/', '')), ''), '') as dst_module,
            COUNT(*) as weight
         FROM refs r
         JOIN files sf ON r.source_file_id = sf.id
         JOIN files tf ON r.target_file_id = tf.id
         WHERE r.target_file_id IS NOT NULL
           AND sf.path != tf.path
         GROUP BY src_module, dst_module
         HAVING src_module != dst_module
         ORDER BY weight DESC",
        [],
    )?;

    // 构建有向图
    let mut graph = ModuleGraph::new();
    for edge in edges {
        graph.add_edge(edge.src_module, edge.dst_module, edge.weight);
    }

    // 检测循环依赖
    graph.detect_cycles();

    Ok(graph)
}
```

---

## 4. 时间维度引擎 (Temporal)

### 4.1 Git 日志解析

```rust
// src/temporal/git_parser.rs

pub fn parse_git_log(workspace: &Path, repo: &Repository, since_days: u32) -> Result<GitStats> {
    let since = format!("--since={since_days}.days.ago");
    let output = Command::new("git")
        .args(["log", "--numstat", "--format=COMMIT|%H|%an|%at|%s", &since])
        .current_dir(workspace)
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut current_commit: Option<CommitRecord> = None;
    let mut commits = Vec::new();
    let mut file_changes: Vec<(String, CommitFileRecord)> = Vec::new();

    for line in stdout.lines() {
        if line.starts_with("COMMIT|") {
            if let Some(c) = current_commit.take() {
                commits.push(c);
            }
            let parts: Vec<&str> = line.splitn(5, '|').collect();
            current_commit = Some(CommitRecord {
                hash: parts[1].to_string(),
                author: parts[2].to_string(),
                ts: parts[3].parse().unwrap_or(0),
                message: parts.get(4).unwrap_or(&"").to_string(),
            });
        } else if let Some(ref commit) = current_commit {
            // numstat 行: "12\t5\tpath/to/file.go"
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() == 3 {
                file_changes.push((commit.hash.clone(), CommitFileRecord {
                    file_path: parts[2].to_string(),
                    added: parts[0].parse().unwrap_or(0),
                    deleted: parts[1].parse().unwrap_or(0),
                }));
            }
        }
    }

    // 批量写入 SQLite
    repo.begin_transaction()?;
    for commit in &commits {
        repo.insert_commit(commit)?;
    }
    for (hash, cf) in &file_changes {
        repo.insert_commit_file(hash, cf)?;
    }
    repo.commit()?;

    Ok(GitStats { commits: commits.len(), files_touched: file_changes.len() })
}
```

### 4.2 热点分析

```rust
// hotspot = change_frequency × complexity
// 高频变更 + 高复杂度 = 维护地狱

pub fn compute_hotspots(repo: &Repository, days: u32) -> Result<Vec<Hotspot>> {
    let hotspots = repo.query(
        "SELECT
            f.path,
            f.id,
            COUNT(DISTINCT cf.commit_id) as change_count,
            fm.cyclomatic,
            fm.max_func_lines,
            COUNT(DISTINCT cf.commit_id) * COALESCE(fm.cyclomatic, 1) as hotspot_score
         FROM files f
         JOIN commit_files cf ON cf.file_path = f.path
         JOIN commits c ON cf.commit_id = c.id
         LEFT JOIN file_metrics fm ON fm.file_id = f.id
         WHERE c.ts > unixepoch() - ?1 * 86400
         GROUP BY f.path
         ORDER BY hotspot_score DESC
         LIMIT 20",
        params![days],
    )?;

    Ok(hotspots)
}
```

### 4.3 变更耦合计算

```rust
/// 找出总是一起改的文件对
pub fn compute_change_coupling(repo: &Repository, min_co_changes: u32) -> Result<()> {
    // 自连接 commit_files：同一 commit 出现的文件对
    repo.execute(
        "INSERT OR REPLACE INTO change_coupling (file_a, file_b, co_changes, total_changes, coupling_score)
         SELECT
            a.file_path as file_a,
            b.file_path as file_b,
            COUNT(DISTINCT a.commit_id) as co_changes,
            (SELECT COUNT(DISTINCT commit_id) FROM commit_files WHERE file_path = a.file_path) as total_a,
            CAST(COUNT(DISTINCT a.commit_id) AS REAL) /
                (SELECT COUNT(DISTINCT commit_id) FROM commit_files WHERE file_path = a.file_path) as coupling_score
         FROM commit_files a
         JOIN commit_files b ON a.commit_id = b.commit_id AND a.file_path < b.file_path
         GROUP BY a.file_path, b.file_path
         HAVING co_changes >= ?1
         ORDER BY coupling_score DESC",
        params![min_co_changes],
    )?;
    Ok(())
}
```

### 4.4 函数级演化追溯

```rust
pub struct EvolutionTrace {
    pub symbol_name: String,
    pub file_path: String,
    pub commits: Vec<EvolutionCommit>,
    pub coupled_files: Vec<(String, f64)>,  // (path, coupling_score)
    pub churn_rate: f64,                     // lines changed / total lines
}

pub fn trace_evolution(
    repo: &Repository,
    workspace: &Path,
    file_path: &str,
    symbol_name: Option<&str>,
) -> Result<EvolutionTrace> {
    // 1. 用 git log -L 追踪函数级历史
    //    git log -L :funcName:path/to/file.go --format=...
    let commits = if let Some(sym) = symbol_name {
        let output = Command::new("git")
            .args(["log", "-L", &format!(":{sym}:{file_path}"),
                   "--format=COMMIT|%H|%an|%at|%s", "--no-patch"])
            .current_dir(workspace)
            .output()?;
        parse_evolution_log(&output.stdout)
    } else {
        // 文件级历史
        let output = Command::new("git")
            .args(["log", "--format=COMMIT|%H|%an|%at|%s", "--", file_path])
            .current_dir(workspace)
            .output()?;
        parse_evolution_log(&output.stdout)
    };

    // 2. 查变更耦合
    let coupled = repo.query(
        "SELECT file_b, coupling_score FROM change_coupling
         WHERE file_a = ?1 ORDER BY coupling_score DESC LIMIT 10",
        params![file_path],
    )?;

    Ok(EvolutionTrace { /* ... */ })
}
```

---

## 5. 新增 MCP Tools

### 5.1 工具清单

```rust
#[tool_router(server_handler)]
impl McpServer {

    // ═══ 原有 ═══
    #[tool(name = "search_context", ...)]
    #[tool(name = "index_project", ...)]
    #[tool(name = "get_symbols", ...)]

    // ═══ v0.2 新增 ═══

    #[tool(
        name = "impact_analysis",
        description = "Analyze the blast radius of changing a symbol. Returns direct/transitive callers, affected tests, uncovered paths, and risk score."
    )]
    fn impact_analysis(&self, Parameters(input): Parameters<ImpactInput>) -> Json<ImpactOutput>;

    #[tool(
        name = "code_map",
        description = "Generate a high-level map of the codebase: module dependencies, hotspots (high churn × complexity), risk zones (low test coverage), and circular dependencies."
    )]
    fn code_map(&self, Parameters(input): Parameters<CodeMapInput>) -> Json<CodeMapOutput>;

    #[tool(
        name = "evolution",
        description = "Trace the history of a file or symbol: commits, authors, change frequency, coupled files that always change together."
    )]
    fn evolution(&self, Parameters(input): Parameters<EvolutionInput>) -> Json<EvolutionOutput>;

    #[tool(
        name = "find_callers",
        description = "Find all callers of a function/method, with transitive expansion. Answers 'who uses this?'"
    )]
    fn find_callers(&self, Parameters(input): Parameters<FindCallersInput>) -> Json<FindCallersOutput>;

    #[tool(
        name = "find_dependencies",
        description = "Find all dependencies of a file or module. Answers 'what does this depend on?'"
    )]
    fn find_dependencies(&self, Parameters(input): Parameters<FindDepsInput>) -> Json<FindDepsOutput>;
}
```

### 5.2 参数和返回类型

```rust
#[derive(Deserialize, schemars::JsonSchema)]
pub struct ImpactInput {
    /// File path (relative to workspace)
    pub file: String,
    /// Symbol name (function, struct, etc.). If omitted, analyzes the whole file.
    pub symbol: Option<String>,
    /// Max transitive depth (default: 3)
    #[serde(default = "default_depth")]
    pub depth: u32,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct ImpactOutput {
    pub target: String,
    pub direct_callers: Vec<CallerItem>,
    pub transitive_callers: Vec<CallerItem>,
    pub affected_tests: Vec<TestItem>,
    pub uncovered_paths: Vec<String>,
    pub risk_score: f64,
    pub risk_factors: Vec<String>,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct CallerItem {
    pub file_path: String,
    pub symbol: String,
    pub line: u32,
    pub depth: u32,           // 0 = direct
    pub confidence: f64,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct CodeMapInput {
    /// Directory scope (default: entire workspace)
    pub scope: Option<String>,
    /// Number of days for hotspot/coupling analysis (default: 30)
    #[serde(default = "default_30")]
    pub days: u32,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct CodeMapOutput {
    pub modules: Vec<ModuleInfo>,
    pub dependencies: Vec<DependencyEdge>,
    pub hotspots: Vec<HotspotItem>,
    pub risk_zones: Vec<RiskZoneItem>,
    pub circular_deps: Vec<Vec<String>>,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct HotspotItem {
    pub file_path: String,
    pub change_count: u32,
    pub complexity: f64,
    pub hotspot_score: f64,
    pub last_changed: String,     // ISO 8601
    pub top_authors: Vec<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct EvolutionInput {
    pub file: String,
    pub symbol: Option<String>,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct EvolutionOutput {
    pub commits: Vec<EvolutionCommit>,
    pub coupled_files: Vec<CoupledFile>,
    pub churn_rate: f64,
    pub unique_authors: u32,
    pub first_introduced: String, // ISO 8601
}
```

---

## 6. 索引流程扩展

### 6.1 新的 Pipeline 阶段

```
index 命令 (v0.2):

Phase 1: 结构索引 (已有, ~5s)
  文件遍历 → tree-sitter 解析 → chunks + symbols + FTS5

Phase 2: 引用提取 (新增, ~3-8s)
  对每个已解析文件:
    1. 用 LanguageRefExtractor 提取 RawReference
    2. 用 SymbolResolver 解析 target
    3. 写入 refs 表

Phase 3: Git 历史 (新增, ~2-5s)
  git log --numstat → commits + commit_files
  计算 hotspot、coupling

Phase 4: 预计算指标 (新增, ~1s)
  聚合 file_metrics
  计算 change_coupling 矩阵

Phase 5: Embedding (已有, 可选/后台)
```

### 6.2 增量更新策略

```rust
/// 增量引用更新：只重新提取变更文件的引用
pub fn update_refs_incremental(
    repo: &Repository,
    changed_files: &[i64],
    extractor: &dyn LanguageRefExtractor,
    resolver: &SymbolResolver,
) -> Result<()> {
    for file_id in changed_files {
        // 1. 删除该文件作为 source 的所有旧引用
        repo.delete_refs_from_file(*file_id)?;

        // 2. 重新提取
        let file = repo.get_file(*file_id)?;
        let source = std::fs::read_to_string(&file.path)?;
        let tree = parser.parse(&source, &file.language)?;
        let raw_refs = extractor.extract(&tree, &source);

        // 3. 解析 + 写入
        for raw in raw_refs {
            let resolved = resolver.resolve(&raw, *file_id);
            repo.insert_ref(*file_id, &raw, &resolved)?;
        }
    }

    // 4. 也需要更新引用到变更文件的边（target 可能变了）
    //    这步通过 target_name 重新匹配即可
    repo.re_resolve_refs_targeting(changed_files)?;

    Ok(())
}
```

---

## 7. 复杂度计算

### 7.1 圈复杂度（纯 tree-sitter，无需额外工具）

```rust
/// 通过计算控制流分支数估算圈复杂度
pub fn cyclomatic_complexity(tree: &Tree, source: &str, language: &str) -> u32 {
    let branch_nodes = match language {
        "go" => &[
            "if_statement", "for_statement", "switch_statement",
            "case_clause", "select_statement", "comm_clause",
            "binary_expression",  // && 和 || 各算一个分支
        ],
        "rust" => &[
            "if_expression", "match_expression", "match_arm",
            "for_expression", "while_expression", "loop_expression",
            "binary_expression",
        ],
        "typescript" | "javascript" => &[
            "if_statement", "for_statement", "for_in_statement",
            "while_statement", "do_statement", "switch_case",
            "catch_clause", "ternary_expression",
            "binary_expression",
        ],
        _ => &["if_statement", "for_statement", "while_statement"],
    };

    let mut complexity = 1u32; // 基线
    walk_tree(tree.root_node(), &mut |node| {
        let kind = node.kind();
        if branch_nodes.contains(&kind) {
            // && 和 || 只在特定 operator 时计数
            if kind == "binary_expression" {
                if let Some(op) = node.child_by_field_name("operator") {
                    let op_text = &source[op.start_byte()..op.end_byte()];
                    if op_text == "&&" || op_text == "||" {
                        complexity += 1;
                    }
                }
            } else {
                complexity += 1;
            }
        }
    });

    complexity
}
```

---

## 8. 风险评分模型

```rust
pub fn compute_risk_score(
    direct_callers: usize,
    transitive_callers: usize,
    uncovered_paths: usize,
    metrics: &FileMetrics,
) -> f64 {
    let blast_radius = (direct_callers as f64).ln_1p() * 2.0
        + (transitive_callers as f64).ln_1p();

    let test_risk = if uncovered_paths > 0 {
        (uncovered_paths as f64).ln_1p() * 3.0
    } else {
        0.0
    };

    let complexity_risk = (metrics.cyclomatic / 10.0).min(5.0);

    let churn_risk = (metrics.change_count_30d as f64 / 10.0).min(3.0);

    let raw = blast_radius + test_risk + complexity_risk + churn_risk;

    // 归一化到 0-10
    (raw / 2.0).min(10.0)
}

pub fn risk_factors(
    direct_callers: usize,
    uncovered_paths: usize,
    metrics: &FileMetrics,
) -> Vec<String> {
    let mut factors = Vec::new();
    if direct_callers > 10 {
        factors.push(format!("high blast radius ({direct_callers} direct callers)"));
    }
    if uncovered_paths > 0 {
        factors.push(format!("{uncovered_paths} call paths without test coverage"));
    }
    if metrics.cyclomatic > 20.0 {
        factors.push(format!("high complexity (cyclomatic: {:.0})", metrics.cyclomatic));
    }
    if metrics.change_count_30d > 10 {
        factors.push(format!("high churn ({} changes in 30 days)", metrics.change_count_30d));
    }
    if metrics.unique_authors > 5 {
        factors.push(format!("many contributors ({} authors)", metrics.unique_authors));
    }
    factors
}
```

---

## 9. CLI 扩展

```rust
#[derive(Subcommand)]
enum Commands {
    // 已有
    Index, Embed, IndexAll, Search, Stats, Mcp,

    // v0.2 新增
    /// Analyze impact of changing a symbol
    Impact {
        file: String,
        #[arg(short, long)]
        symbol: Option<String>,
        #[arg(short, long, default_value = "3")]
        depth: u32,
    },
    /// Show codebase map with hotspots and risks
    Map {
        #[arg(short, long)]
        scope: Option<String>,
        #[arg(long, default_value = "30")]
        days: u32,
    },
    /// Trace evolution of a file or symbol
    History {
        file: String,
        #[arg(short, long)]
        symbol: Option<String>,
    },
    /// Find all callers of a symbol
    Callers {
        symbol: String,
        #[arg(short, long, default_value = "10")]
        limit: usize,
    },
}
```

---

## 10. 性能预算

目标：2000 文件 Go 项目

| 阶段 | 预算 | 方式 |
|------|------|------|
| Phase 1: 结构索引 | 5s | 已验证 |
| Phase 2: 引用提取 | 3-8s | tree-sitter query + SQLite batch insert |
| Phase 3: Git 历史 | 2-5s | git log 解析 + batch insert |
| Phase 4: 预计算 | 1s | SQL 聚合 |
| impact 查询 | < 200ms | SQLite 索引 + BFS (通常 < 100 节点) |
| code_map 查询 | < 500ms | 预计算 + SQL 聚合 |
| evolution 查询 | < 1s | git log -L (IO bound) |

**总计索引时间：~15s**，全部能力就绪，不需要任何 ML 模型。

---

## 11. 实现优先级

```
Sprint 1 (核心图):
  [1] refs 表 schema + Repository 方法
  [2] Go 引用提取器 (LanguageRefExtractor for Go)
  [3] SymbolResolver (4 级解析)
  [4] Pipeline Phase 2 集成
  [5] find_callers MCP tool
  
Sprint 2 (影响面):
  [6] 反向 BFS (analyze_impact)
  [7] 测试文件识别 + 覆盖映射
  [8] impact_analysis MCP tool
  [9] impact CLI 命令

Sprint 3 (时间维度):
  [10] git log 解析器
  [11] hotspot 计算
  [12] change_coupling 计算
  [13] code_map MCP tool
  [14] map CLI 命令

Sprint 4 (演化 + 复杂度):
  [15] 圈复杂度计算
  [16] evolution 追溯 (git log -L)
  [17] 风险评分模型
  [18] evolution MCP tool
  [19] TS/Rust/Python 引用提取器
```
