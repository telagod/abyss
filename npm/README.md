# @code-abyss/cli

**abyss** — the code graph your agent checks before it edits.

npm wrapper that downloads the prebuilt `abyss` binary from
[GitHub Releases](https://github.com/telagod/abyss/releases) on install
(Node 18+ fetch + system tar, zero npm dependencies).

```sh
npm install -g @code-abyss/cli
abyss index          # build the code graph (~150ms on a 100-file repo)
abyss context <file> # callers / dependencies / hotspot before you edit
abyss mcp            # MCP server (8 tools) over stdio
```

- Call-graph resolution: 97.2% gated precision vs SCIP ground truth (gin corpus)
- Languages: Rust, Go, TypeScript, JavaScript, Python, Java, and more
- Temporal intelligence: hotspot scores and co-change coupling from git history
- Agent hooks: `abyss hook pre-edit` warns about external callers before edits

Full docs, eval methodology, and source: [telagod/abyss](https://github.com/telagod/abyss).

Prefer cargo? `cargo binstall code-abyss` or `cargo install code-abyss`.

License: Apache-2.0
