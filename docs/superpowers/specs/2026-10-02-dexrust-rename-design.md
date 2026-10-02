# dexrust Rename Design

## Purpose

Rename the Rust reimplementation of Dex from the inconsistent public names
`dexrs` and `dex-cli` to one searchable name: `dexrust`.

After the migration, users should see the same name on GitHub, crates.io,
Homebrew, release artifacts, docs.rs, and the primary executable. Existing
tools and scripts that invoke `dex` must continue to work.

## Current State

The implementation currently has three public identities:

- GitHub repository: `DanielCarmingham/dexrs`
- crates.io package and Homebrew formula: `dex-cli`
- installed executables: `dexrs` and `dex`

This makes the project difficult to find and explain. Searches for `dex-cli`
and `dexrs` also surface unrelated Android DEX tools and an unrelated Dexcom
crate. The name `dexrust` was checked on 2026-10-02 and was unallocated in both
the crates.io sparse index and the `DanielCarmingham` GitHub namespace.

## Target Identity

The project will use `dexrust` consistently:

| Surface | Target |
| --- | --- |
| GitHub repository | `DanielCarmingham/dexrust` |
| Cargo package | `dexrust` |
| Rust library crate | `dexrust` |
| Homebrew formula | `dexrust` |
| Primary executable | `dexrust` |
| Compatibility executable | `dex` |
| Release archive prefix | `dexrust-` |
| Shell installer | `dexrust-installer.sh` |

The old `dexrs` executable will not be shipped by the new package. The `dex`
executable remains supported throughout the 0.x lifecycle so dextui, agents,
and existing scripts do not need to change.

## Migration Strategy

### 1. Prepare the source repository

Make a release-ready rename commit in the existing local `dexrs` checkout
before changing any remote state:

- Change the Cargo package from `dex-cli` to `dexrust`.
- Change the library target from `dexrs` to `dexrust` and update Rust imports.
- Replace the `dexrs` binary target with `dexrust`.
- Keep the `dex` binary target calling the same library entry point.
- Change package metadata, URLs, release configuration, documentation,
  changelog text, tests, and examples to the new name.
- Add cargo-binstall metadata matching cargo-dist's versionless archive names,
  so `cargo binstall dexrust` downloads a release instead of compiling.
- Update repository instructions and release commands to refer to `dexrust`.

The first release under the new name will be version `0.2.0`. The version bump
signals a public packaging and executable rename even though the task-store and
CLI compatibility contracts remain unchanged.

### 2. Retire the old crates.io package

crates.io package names and published source cannot be renamed or deleted. Do
not publish another `dex-cli` version. Once `dexrust` `0.2.0` is verified on
crates.io, yank every published `dex-cli` version, currently `0.1.1` and
`0.1.2`, and stop publishing that package.

Yanking is the closest crates.io-supported equivalent to removing the package.
It prevents ordinary new dependency resolution and installation while leaving
the historical package page and source archive intact. The `dex-cli` name will
remain permanently allocated; it must not be reused for an unrelated project.

### 3. Rename the GitHub repository

Rename `DanielCarmingham/dexrs` to `DanielCarmingham/dexrust` after the source
rename commit is pushed. GitHub redirects ordinary repository URLs and Git
operations from the old name, but the local checkout's `origin` URL will be
updated explicitly to avoid relying on that redirect.

Do not create a new repository later at the old `dexrs` path, because doing so
would break GitHub's redirect. Audit workflow references and raw GitHub URLs;
raw content and GitHub Action references do not receive all repository rename
redirect guarantees.

The local checkout directory may be renamed from `dexrs` to `dexrust` only
after confirming no active worktree, shell, or automation depends on the old
filesystem path. The directory rename is local housekeeping and is not required
for the release.

### 4. Publish dexrust 0.2.0

Run the full release verification against the renamed package, then publish
`dexrust` `0.2.0` to crates.io. Tag the exact published commit as `v0.2.0` and
push it to the renamed GitHub repository so cargo-dist creates:

- four supported platform archives named `dexrust-<target>.tar.xz`;
- `dexrust-installer.sh`;
- the `dexrust` Homebrew formula; and
- checksums and the dist manifest.

Verify the crates.io package and both installed executable names before
announcing the migration:

```bash
cargo install dexrust
dexrust version
dex version
```

Both version commands must identify the implementation as `dexrust v0.2.0`.

### 5. Replace the Homebrew formula

Allow cargo-dist to publish the new `Formula/dexrust.rb`. After the new formula
is installable, delete the old generated `Formula/dex-cli.rb` from the tap. Do
not retain an alias or deprecated compatibility formula because there are no
known users to migrate. Validate the supported user path:

```bash
brew install DanielCarmingham/tap/dexrust
brew upgrade
```

Check the new formula with `brew style` and `brew audit`. After removal,
`brew install DanielCarmingham/tap/dex-cli` is expected to fail rather than
install an obsolete or separately maintained package.

### 6. Update dextui

Once at least one verified `dexrust` installation route is live, update dextui:

- Replace `dexrs` and `dex-cli` marketing links and package commands with
  `dexrust`.
- Continue requiring a compatible executable named `dex` on `PATH`.
- Recommend `brew install DanielCarmingham/tap/dextui
  DanielCarmingham/tap/dexrust` for prebuilt installation.
- Recommend `cargo install dextui dexrust` for source installation.
- Recommend `cargo binstall dextui dexrust` only after the new binstall metadata
  is verified against the published assets.
- Keep the explanation that PATH order decides which `dex` compatibility
  executable runs if the JavaScript implementation is also installed.

No dextui application code changes are expected because it already invokes
`dex`, the compatibility name being preserved.

## Release Ordering

The migration order prevents documentation from pointing at packages that do
not exist and isolates irreversible publication steps:

1. Apply and verify the source/package rename to `dexrust` `0.2.0`.
2. Push the rename commit.
3. Rename the GitHub repository and update the local remote.
4. Publish and verify the crates.io package `dexrust` `0.2.0`.
5. Tag and push `v0.2.0` to produce GitHub and Homebrew artifacts.
6. Verify Cargo, cargo-binstall, shell-installer, and Homebrew installations.
7. Update and release downstream dextui documentation.
8. Yank every published `dex-cli` version.
9. Delete the old `dex-cli` Homebrew formula.

Steps involving crates.io publication, the GitHub repository rename, tag push,
and Homebrew tap mutation require explicit confirmation immediately before they
run. Local source preparation and verification are reversible.

## Verification

Before the source rename commit:

- Run the complete Rust test suite.
- Run Clippy with warnings denied.
- Check formatting.
- Run cargo-dist planning and generated-workflow checks.
- Run `cargo publish --dry-run` for the package identity being published.
- Inspect the packaged file list and Cargo metadata.
- Confirm both `dexrust` and `dex` binaries execute the same CLI and report the
  new implementation name.
- Search tracked files for stale `dexrs`, `dex-cli`, and old repository URLs;
  allow only intentional migration-history references.

After publishing:

- Install into isolated temporary Cargo and Homebrew prefixes.
- Confirm store compatibility with the JavaScript Dex fixtures.
- Run a concurrent-writer smoke test through the `dex` compatibility binary.
- Confirm release archive names match cargo-binstall metadata.
- Confirm the old GitHub URL redirects to the renamed repository.
- Confirm dextui starts and completes a read/write self-test using the newly
  installed `dex` compatibility executable.

## Failure and Rollback Boundaries

Before publication, revert or amend the local rename normally.

After `dexrust` `0.2.0` is published, the crate name and version cannot be
reused. Fix packaging problems in a patch release rather than yanking a usable
release. Yank only if the published package is materially broken or unsafe.

Yanking `dex-cli` can be reversed with `cargo yank --undo` if an unknown user
reports a legitimate migration need. It does not delete the published source or
free the package name.

If GitHub release or Homebrew publication fails after the crates.io release,
leave the crate available, repair the release pipeline, and publish the missing
artifacts from the same tagged source. Do not republish different source under
the same version.

Do not yank `dex-cli` until the new `dexrust` install routes and the downstream
dextui documentation are ready to land. This keeps a usable recovery path
during the migration even though no long-term compatibility release is planned.

## Out of Scope

- Renaming the task-store directory, config directory, environment variables,
  file formats, MCP protocol, or the `dex` compatibility command.
- Removing or uninstalling the JavaScript Dex implementation.
- Maintaining feature parity in two Rust packages after the migration.
- Reusing the old GitHub `dexrs` repository name.
