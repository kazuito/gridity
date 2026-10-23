# Releasing

Releases go to crates.io (`cargo install gridity`), npm (`npx gridity`) and GitHub Releases
(`cargo binstall gridity`). The npm package holds prebuilt binaries for macOS (arm64, x64),
Linux (arm64, x64; static musl) and Windows (x64), and a small Node launcher (TypeScript in
`packaging/npm/src/`, bundled with tsdown) that runs the one for the current platform. The
GitHub release holds the same binaries as `gridity-<target>-v<version>.tgz` (`.zip` for
Windows) plus `SHA256SUMS`; cargo-binstall finds them through `[package.metadata.binstall]`
in `Cargo.toml`.

## Every release

1. Update `version` in `Cargo.toml`, run `cargo build` so `Cargo.lock` follows, and commit.
2. Tag and push: `git tag v0.2.0 && git push origin main v0.2.0`.
3. `.github/workflows/release.yml` checks that the tag matches `Cargo.toml`, builds the five
   binaries with `--profile dist`, publishes with `cargo publish` and
   `npm publish --provenance`, and creates the GitHub release with the archives.

## One-time setup

Both registries use trusted publishing (no stored tokens), which can only be configured for
a package that already exists, so the first version is published by hand:

1. In the GitHub repository settings, create the `release` environment (optionally with
   required reviewers).
2. Push the first tag (`v0.1.0`). Its `build` job produces the five `bin-*` artifacts; its
   `crates` and `npm` jobs fail because no trusted publisher exists yet. That is expected.
3. crates.io: `cargo login`, then `cargo publish` from the tagged commit. In the crate's
   settings on crates.io, add a trusted publisher: repository `kazuito/gridity`, workflow
   `release.yml`, environment `release`.
4. npm: download all `bin-*` artifacts of that run into one directory (`gh run download
   <run-id> -p 'bin-*' -D bins`), then `npm ci` in `packaging/npm/`,
   `node packaging/npm/build.ts --all bins npm-package` and `npm publish --access public`
   from `npm-package/`. Always use `--all` for anything
   you publish: a version published without every platform cannot be replaced. In the
   package's settings on npmjs.com, add a trusted publisher for GitHub Actions with the
   same repository, workflow and environment.

From the next tag on, the workflow publishes both by itself.

## Checking a package locally

For `npm pack` only; never publish a package built this way (it holds one platform).

```sh
cargo publish --dry-run
cargo build --release
mkdir -p target/manual/bins/aarch64-apple-darwin && cp target/release/gridity target/manual/bins/aarch64-apple-darwin/
(cd packaging/npm && npm ci)
node packaging/npm/build.ts target/manual/bins target/manual/npm
(cd target/manual/npm && npm pack --pack-destination ..)
npx --package ./target/manual/gridity-<version>.tgz gridity --version
```
