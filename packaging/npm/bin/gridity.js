#!/usr/bin/env node
// Runs the prebuilt gridity binary for this platform with the same arguments, stdio and
// exit code, so `npx gridity mcp` works as an MCP server.
"use strict";

const { spawnSync } = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");

const platform = `${process.platform}-${process.arch}`;
const name = process.platform === "win32" ? "gridity.exe" : "gridity";
const binary = path.join(__dirname, "..", "vendor", platform, name);

if (!fs.existsSync(binary)) {
  console.error(
    `error: gridity has no prebuilt binary for ${platform}\n` +
      "hint: install it with `cargo install gridity`",
  );
  process.exit(4);
}

const result = spawnSync(binary, process.argv.slice(2), { stdio: "inherit" });
if (result.error) {
  console.error(`error: cannot run ${binary}: ${result.error.message}`);
  process.exit(1);
}
if (result.signal) {
  process.kill(process.pid, result.signal);
}
process.exit(result.status ?? 1);
