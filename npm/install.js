#!/usr/bin/env node
// Downloads the prebuilt abyss binary for this platform from GitHub Releases.
// Zero npm dependencies: node fetch + system tar (bsdtar on Windows handles .zip).
"use strict";

const fs = require("fs");
const os = require("os");
const path = require("path");
const { execFileSync } = require("child_process");

const VERSION = require("./package.json").version;
const REPO = "telagod/abyss";

function target() {
  const arch = { x64: "x86_64", arm64: "aarch64" }[process.arch];
  if (!arch) return null;
  switch (process.platform) {
    case "linux":
      return `${arch}-unknown-linux-gnu`;
    case "darwin":
      return `${arch}-apple-darwin`;
    case "win32":
      return arch === "x86_64" ? "x86_64-pc-windows-msvc" : null;
    default:
      return null;
  }
}

function assetName(t) {
  return process.platform === "win32" ? `abyss-${t}.zip` : `abyss-${t}.tar.gz`;
}

function binPath() {
  return path.join(
    __dirname,
    "bin",
    process.platform === "win32" ? "abyss-bin.exe" : "abyss-bin"
  );
}

async function main() {
  const t = target();
  if (!t) {
    console.error(
      `[abyss] unsupported platform: ${process.platform}/${process.arch} — ` +
        `build from source: https://github.com/${REPO}#install`
    );
    process.exit(1);
  }

  const url = `https://github.com/${REPO}/releases/download/v${VERSION}/${assetName(t)}`;
  console.error(`[abyss] downloading ${url}`);

  const res = await fetch(url, { redirect: "follow" });
  if (!res.ok) {
    console.error(
      `[abyss] download failed (HTTP ${res.status}). ` +
        `Try: curl -fsSL https://raw.githubusercontent.com/${REPO}/main/install.sh | bash`
    );
    process.exit(1);
  }

  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "abyss-"));
  const archive = path.join(tmp, assetName(t));
  fs.writeFileSync(archive, Buffer.from(await res.arrayBuffer()));

  // bsdtar (bundled with Windows 10+) extracts both .tar.gz and .zip
  execFileSync("tar", ["-xf", archive, "-C", tmp]);

  const extracted = path.join(
    tmp,
    process.platform === "win32" ? "abyss.exe" : "abyss"
  );
  fs.mkdirSync(path.dirname(binPath()), { recursive: true });
  fs.copyFileSync(extracted, binPath());
  fs.chmodSync(binPath(), 0o755);
  fs.rmSync(tmp, { recursive: true, force: true });

  console.error(`[abyss] installed ${binPath()}`);
}

main().catch((e) => {
  console.error(`[abyss] install failed: ${e.message}`);
  process.exit(1);
});
