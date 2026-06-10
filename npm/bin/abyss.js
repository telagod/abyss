#!/usr/bin/env node
// Shim: forwards to the platform binary downloaded by install.js.
"use strict";

const path = require("path");
const { spawnSync, execFileSync } = require("child_process");
const fs = require("fs");

const bin = path.join(
  __dirname,
  process.platform === "win32" ? "abyss-bin.exe" : "abyss-bin"
);

if (!fs.existsSync(bin)) {
  // postinstall may have been skipped (--ignore-scripts); fetch lazily.
  try {
    execFileSync(process.execPath, [path.join(__dirname, "..", "install.js")], {
      stdio: ["ignore", "inherit", "inherit"],
    });
  } catch {
    process.exit(1);
  }
}

const r = spawnSync(bin, process.argv.slice(2), { stdio: "inherit" });
process.exit(r.status === null ? 1 : r.status);
