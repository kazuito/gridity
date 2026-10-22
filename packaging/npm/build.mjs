// Assembles the npm package from prebuilt binaries.
// Usage: node packaging/npm/build.mjs [--all] <binaries-dir> <out-dir>
// <binaries-dir>/<rust target>/gridity[.exe] for each target in TARGETS. Missing ones are
// skipped with a warning, so a local build can package just the host binary; --all (used
// for releases) makes them an error.
import { chmodSync, copyFileSync, cpSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const TARGETS = {
  "aarch64-apple-darwin": "darwin-arm64",
  "x86_64-apple-darwin": "darwin-x64",
  "aarch64-unknown-linux-musl": "linux-arm64",
  "x86_64-unknown-linux-musl": "linux-x64",
  "x86_64-pc-windows-msvc": "win32-x64",
};

const args = process.argv.slice(2);
const requireAll = args[0] === "--all";
const [binaries, out] = args.slice(requireAll ? 1 : 0).map((p) => resolve(p));
if (!binaries || !out) {
  console.error("usage: node packaging/npm/build.mjs [--all] <binaries-dir> <out-dir>");
  process.exit(2);
}
const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "..", "..");
const version = readFileSync(join(root, "Cargo.toml"), "utf8").match(/^version = "([^"]+)"/m)[1];

rmSync(out, { recursive: true, force: true });
mkdirSync(out, { recursive: true });
cpSync(join(here, "bin"), join(out, "bin"), { recursive: true });
copyFileSync(join(root, "README.md"), join(out, "README.md"));
copyFileSync(join(root, "LICENSE-MIT"), join(out, "LICENSE-MIT"));
copyFileSync(join(root, "LICENSE-APACHE"), join(out, "LICENSE-APACHE"));
copyFileSync(join(root, "assets/fonts/OFL.txt"), join(out, "OFL.txt"));

let packaged = 0;
for (const [target, platform] of Object.entries(TARGETS)) {
  const name = target.includes("windows") ? "gridity.exe" : "gridity";
  const source = join(binaries, target, name);
  if (!existsSync(source)) {
    if (requireAll) {
      console.error(`error: no binary for ${target} at ${source}`);
      process.exit(1);
    }
    console.warn(`warning: no binary for ${target} at ${source}`);
    continue;
  }
  const dir = join(out, "vendor", platform);
  mkdirSync(dir, { recursive: true });
  copyFileSync(source, join(dir, name));
  chmodSync(join(dir, name), 0o755);
  packaged += 1;
}
if (packaged === 0) {
  console.error("error: no binaries found");
  process.exit(1);
}

const manifest = JSON.parse(readFileSync(join(here, "package.json"), "utf8"));
manifest.version = version;
writeFileSync(join(out, "package.json"), JSON.stringify(manifest, null, 2) + "\n");
console.log(`gridity ${version}: ${packaged} of ${Object.keys(TARGETS).length} platforms in ${out}`);
