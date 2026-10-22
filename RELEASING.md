# Releasing

Releases go to crates.io (`cargo install gridity`) and npm (`npx gridity`). The npm package
holds prebuilt binaries for macOS (arm64, x64), Linux (arm64, x64; static musl) and
Windows (x64), and a small Node launcher that runs the one for the current platform.

## Every release

1. Update `version` in `Cargo.toml`, run `cargo build` so `Cargo.lock` follows, and commit.
2. Tag and push: `git tag v0.2.0 && git push origin main v0.2.0`.
3. `.github/workflows/release.yml` checks that the tag matches `Cargo.toml`, builds the five
   binaries with `--profile dist`, and publishes with `cargo publish` and
   `npm publish --provenance`.

## One-time setup

Both registries use trusted publishing (no stored tokens), which can only be configured for
a package that already exists, so the first version is published by hand:

1. crates.io: `cargo login`, then `cargo publish`. On crates.io, open the crate's settings,
   add a trusted publisher for repository `kazuito/gridity`, workflow `release.yml`,
   environment `release`.
2. npm: build the package locally with the binaries you have
   (`node packaging/npm/build.mjs <bins> npm-package`, see the script header), or wait for
   a release run's `bin-*` artifacts, then `npm publish --access public` from
   `npm-package/`. On npmjs.com, open the package's settings and add a trusted publisher
   for GitHub Actions: repository `kazuito/gridity`, workflow `release.yml`, environment
   `release`.
3. In the GitHub repository settings, create the `release` environment (optionally with
   required reviewers).

## Checking a package locally

```sh
cargo publish --dry-run
cargo build --release
mkdir -p target/manual/bins/aarch64-apple-darwin && cp target/release/gridity target/manual/bins/aarch64-apple-darwin/
node packaging/npm/build.mjs target/manual/bins target/manual/npm
(cd target/manual/npm && npm pack --pack-destination ..)
npx --package ./target/manual/gridity-<version>.tgz gridity --version
```
