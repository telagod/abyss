# Reproducing the eval

`eval/run.sh` measures abyss's call-graph resolver against SCIP (compiler-grade)
ground truth across six corpora — gin (Go), hono (TS), click (Python),
ripgrep + abyss (Rust), cmark (C). To do that it needs every corpus's SCIP
indexer on PATH; any indexer it can't find is silently skipped.

The fastest path on a fresh Linux box:

```sh
bash eval/setup-indexers.sh   # idempotent: installs whatever's missing
bash eval/run.sh              # clones corpora, builds ground truth, compares
```

## What setup-indexers.sh installs

| Indexer | Method | Lands at |
|---------|--------|---------------------------------|
| `scip` | GitHub release tarball (`v0.8.1`) | `~/.local/bin/scip` |
| `scip-go` | `go install …@latest` (needs `go`) | `$(go env GOPATH)/bin/scip-go` |
| `scip-typescript` | `npm install -g` (needs node ≥18) | npm prefix |
| `scip-python` | `npm install -g` | npm prefix |
| `rust-analyzer` | `rustup component add` (needs `rustup`) | `~/.cargo/bin/rust-analyzer` |
| `scip-clang` | GitHub release binary (`v0.3.2`) | `~/.local/bin/scip-clang` |

The script never sudos. It checks each indexer first and only installs what's
missing — re-running on a fully set-up box prints `all 5 already installed`
and exits 0.

Sanity check on the `scip` CLI: there's an unrelated tool also called `scip`
(ZIB's constraint solver, packaged as `/usr/bin/scip` on some Debian-derived
distros). The script verifies the Sourcegraph CLI by parsing `scip --version`
and re-installs if it finds the wrong tool.

## PATH expectations

`eval/run.sh` reads PATH from the calling shell. Whatever you put on PATH for
the setup script needs to stay on PATH when you run the eval. The script
discovers existing installs across the common layouts:

- `~/.local/bin` — preferred for the two release-binary indexers
- `~/.cargo/bin` — rustup-installed rust-analyzer
- `~/go/bin`, `~/.gvm/pkgsets/*/global/bin` — go-installed scip-go
- npm prefix bin — `npm prefix -g`/bin, or `~/.npm-global/bin` if the script
  had to redirect installs
- fnm: `~/.local/share/fnm/node-versions/*/installation/bin`
- nvm: `~/.nvm/versions/node/*/bin`

If anything was installed into a directory that's not on your login shell's
PATH, the script prints a `add $dir to PATH` warning at the end. Append those
to your `~/.bashrc` / `~/.zshrc` before running `eval/run.sh`.

### npm under fnm / nvm

Both managers point `npm prefix -g` at their own per-version bin directory
(e.g. `~/.local/share/fnm/node-versions/v24.9.0/installation/bin`), not at the
documented `~/.npm-global/bin`. That's correct and the script handles it —
the user-visible quirk is that switching node versions hides the binaries.
Re-run setup-indexers.sh after switching node to reinstall there.

### npm under the system node

If `npm prefix -g` resolves to `/usr` or `/usr/local`, `npm install -g`
needs sudo. The script avoids that by setting `NPM_CONFIG_PREFIX=~/.npm-global`
for the duration of the install and adding `~/.npm-global/bin` to PATH.

## Manual install (skip the script)

```sh
# scip CLI
curl -fsSL https://github.com/scip-code/scip/releases/download/v0.8.1/scip-linux-amd64.tar.gz \
  | tar -xz -C ~/.local/bin scip

# scip-go (requires go ≥1.21)
go install github.com/sourcegraph/scip-go/cmd/scip-go@latest

# scip-typescript + scip-python (requires node ≥18)
npm install -g @sourcegraph/scip-typescript @sourcegraph/scip-python

# rust-analyzer (requires rustup)
rustup component add rust-analyzer

# scip-clang
curl -fsSL https://github.com/sourcegraph/scip-clang/releases/download/v0.3.2/scip-clang-x86_64-linux \
  -o ~/.local/bin/scip-clang
chmod +x ~/.local/bin/scip-clang
```

## Reproducibility

The eval baselines published in `RESULTS.md` are reproducible **only** against
the SCIP indexer versions pinned in `setup-indexers.sh`. SCIP indexers move
silently: between v0.3.6 and v0.4.0 the click corpus drifted 98.7/94.6 →
97.9/93.0 gated precision/recall with zero abyss code change — `scip-python`
v0.6.6 had started emitting 16 extra truth pairs.

### Pinned versions

| Indexer | Pinned to | Pin mechanism |
|---------|-----------|---------------|
| `scip` (CLI) | `v0.8.1` | GitHub release tag (`SCIP_VERSION`) |
| `scip-go` | `v0.2.7` | `go install …@v0.2.7` (`SCIP_GO_VERSION`) |
| `scip-typescript` | `0.4.0` | `npm install -g @sourcegraph/scip-typescript@0.4.0` (`SCIP_TS_VERSION`) |
| `scip-python` | `0.6.6` | `npm install -g @sourcegraph/scip-python@0.6.6` (`SCIP_PYTHON_VERSION`) |
| `scip-clang` | `v0.3.2` | GitHub release binary (`SCIP_CLANG_VERSION`) |
| `rust-analyzer` | tracked with rustup toolchain | `rustup component add rust-analyzer` |

`rust-analyzer` is intentionally not version-pinned in the script — the
project's `rust-toolchain` (or the user's default `rustup` toolchain) decides
which build of the analyzer ships. Capture the actual `rust-analyzer --version`
from the run log if you need to compare across machines.

### Policy

Bumping any pinned SCIP indexer in `setup-indexers.sh` requires re-running
`eval/run.sh` and updating the affected rows in `RESULTS.md` **in the same
commit**. The `--- indexer versions` line that `run.sh` prints to stderr at
the start of every run is the audit trail; copy it into the relevant
"captured-against" note in `RESULTS.md` whenever you publish new numbers.

If a bump moves a baseline in a way that looks like a regression, check the
SCIP indexer's release notes first — the new truth pairs may have changed
without abyss's resolver changing. The 2026-06-17 click correction is the
canonical example.

## What runs

`eval/run.sh` clones each corpus at a pinned ref, builds the ground-truth
SCIP index, runs `abyss index`, and pipes both into `compare.py`. Results land
in `eval/RESULTS.md`. Per-corpus runtime ranges from seconds (gin) to a few
minutes (hono `npm install`).

Partial runs are fine: any corpus whose indexer isn't on PATH is skipped with
a `--- skip` line. The numbers you publish should list which corpora actually
ran.

## Platform notes

The auto-install script targets Linux x86_64. On macOS or arm64:

- `scip` and `scip-clang`: download the matching release asset by hand and
  drop it in `~/.local/bin`
- `scip-go` / npm / rustup steps work identically across platforms
