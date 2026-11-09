# Releasing

One `vX.Y.Z` tag on `main` releases everything
([`release.yml`](.github/workflows/release.yml)): `@discogs-rs/node` and its
three platform packages on npm, the `discogs` CLI for three targets, and a
GitHub release with the notes, the archives and `SHA256SUMS`.

1. Set the version in `Cargo.toml` (`[workspace.package]` and the pinned
   `discogs-core`), `crates/discogs-node/package.json` and `Cargo.lock`,
   and open a `## X.Y.Z` section in `CHANGELOG.md`.
2. `node scripts/check-version.mjs vX.Y.Z`, commit, push.
3. When `ci` and `node` pass on `main`: `git tag vX.Y.Z && git push origin vX.Y.Z`.

Run **release** by hand from the Actions tab with `dry-run` on (the
default) to build, pack and install everything without publishing.
Secrets: `NPM_TOKEN` (or npm trusted publishing); environment
`npm-publish` gates the publish job.
