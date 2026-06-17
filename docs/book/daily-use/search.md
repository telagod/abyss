# `abyss search`

Symbol + fulltext fusion search.

```sh
abyss search "validate token"
abyss search "validate token" --json
```

The search engine fuses three signals (see `src/search/`):

1. **Symbol-name matching** (`symbol.rs`) — exact/prefix/substring
   match against the indexed symbol names.
2. **FTS5 fulltext** (`fulltext.rs`) — SQLite FTS5 over chunk
   content. Great for finding code by phrase.
3. **Vector similarity** (`semantic.rs`, behind `--features semantic`) —
   embedding cosine similarity, fastembed / ONNX-backed.

Results are merged and deduplicated in `fusion.rs`. The slim build
gives you symbol + FTS5; semantic adds embedding rerank when present.

abyss is not trying to replace your fuzzy finder or your code search
engine. Use it when you need *structural* context, not just text
matches — the search command is here so the same binary can answer
both questions when you don't want to switch tools.
