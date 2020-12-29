# cargo ci-precache

[![CI](https://github.com/Jarcho/cargo-ci-precache/workflows/CI/badge.svg?branch=main&event=push)](https://github.com/Jarcho/cargo-ci-precache/actions?query=workflow%3A%22CI%22)

A tool to clean up the target directory and the crate download cache for the purpose of caching only the required external build dependencies.

## Quick start

The tool can be run in two modes, one to clear the crate download cache, and the other to clear the target directory.

To clear the crate download cache run:

```sh
cargo ci-precache cargo-cache
```

To clear the target directory run:

```sh
cargo ci-precache target
```

These will delete anything not in use by the current project with the default feature enabled, taking into account all targets. For the download cache this will delete from both `~/.cargo/git/db` and `~/.cargo/registry/cache`, but not from `~/.cargo/git/checkouts` and `~/.cargo/registry/src`.

To change which features are enabled, use `--all-features`, `--no-default-features`, or `--features`. To change the target platform use `--filter-platform`

### GitHub Actions Examples

An example for running tests on the stable channel for windows, macos and ubuntu. Uses [actions-rs] for rustup.

```yaml
name: Test

on:
  push:
  pull_request:

# Disable incremental building, it will just be deleted anyways.
env:
  CARGO_INCREMENTAL: 0

jobs:
  test-stable:
    strategy:
      matrix:
        os: [ubuntu, windows, macos]
        include:
          - os: ubuntu
            components: rustfmt, clippy
            platform: x86_64-unknown-linux-gnu
            ci_precache_checksum: a2400ce8ca692496cc2ff1c8b16f912adc548b8f4b44488658e59df09d97d8b6
          - os: macos
            platform: x86_64-apple-darwin
            ci_precache_checksum: d2335a9ddf96d8c9244caad4cc2f535fcf0c4754296b0fb3ce9c1610800c1881
          - os: windows
            platform: x86_64-pc-windows-msvc
            ci_precache_checksum: 5513b98095a37162e5c1bf9c9abaa4805f971fdc89bcc650217bfc5f0cbe533e
    name: ${{ matrix.os }}-stable
    runs-on: ${{ matrix.os }}-latest

    steps:
      # Makes sure line endings on Cargo.lock are the same on all platforms.
      # Needed to share the cache to be shared between platforms
      - run: git config --global core.autocrlf false
      - uses: actions/checkout@v2

      # See: https://github.com/actions/cache/issues/403
      - name: Install GNU tar (macos)
        if: matrix.os == 'macos'
        run: |
          brew install gnu-tar
          echo "PATH=/usr/local/opt/gnu-tar/libexec/gnubin:$PATH" >> $GITHUB_ENV

      - uses: actions-rs/toolchain@v1
        id: rustup
        with:
          toolchain: stable
          profile: minimal
          override: true

      # Cache the registry index and downloaded dependencies. Only needed if a
      # lockfile is checked in.
      - name: Cache dependencies
        uses: actions/cache@v2
        id: cache-deps
        with:
          path: |
            ~/.cargo/registry/index
            ~/.cargo/registry/cache
            ~/.cargo/git/db
          key: deps-${{ hashfiles('./Cargo.lock') }}
          restore-keys: deps

      # Fetch for all targets, allowing the cache to be shared by all platforms.
      - name: Fetch Dependencies
        if: steps.cache-deps.outputs.cache-hit != 'true'
        run: |
          cargo fetch --target x86_64-pc-windows-msvc
          cargo fetch --target x86_64-apple-darwin
          cargo fetch --target x86_64-unknown-linux-gnu

      # Cache the target directory.
      # Cargo.toml included to detect feature changes
      - name: Cache build dependencies
        uses: actions/cache@v2
        id: cache-build-deps
        with:
          path: |
            ./target/debug
          key: build-deps-${{ steps.rustup.outputs.rustc_hash }}-${{ hashfiles('./Cargo.lock', '**/Cargo.toml') }}
          restore-keys: build-deps-${{ steps.rustup.outputs.rustc_hash }}

      # Run tests.
      - name: Test
        run: cargo test --workspace

      # Download cargo-ci-precache
      - name: Download cargo-ci-precache (Windows)
        if: ${{ matrix.os }} == 'windows' && (steps.cache-deps.outputs.cache-hit != 'true' || steps.cache-build-deps.outputs.cache-hit != 'true')
        run: |
          cd ~/.cargo/bin
          Invoke-WebRequest 'https://github.com/Jarcho/cargo-ci-precache/releases/download/v0.1.0/cargo-ci-precache_windows_v0.1.0.7z' -OutFile "ci-precache.7z"
          if($(get-filehash -Algorithm sha256 ci-precache.7z).hash.tolower() != '${{matrix.ci_precache_checksum }}') {
            exit 1
          }
          7z e "ci-precache.7z" -y

      - name: Download cargo-ci-precache
        if: ${{ matrix.os }} != 'windows' && (steps.cache-deps.outputs.cache-hit != 'true' || steps.cache-build-deps.outputs.cache-hit != 'true')
        run: |
          cd ~/.cargo/bin
          curl 'https://github.com/Jarcho/cargo-ci-precache/releases/download/v0.1.0/cargo-ci-precache_${{ matrix.os }}_v0.1.0.tar.gz' -o ci-precache.tar.gz
          if [ "$(shasum -a 256 ci-precache.tar.gz | cut -d ' ' -f 1)" != "${{ matrix.ci_precache_checksum }}" ]; then
            exit 1
          fi
          tar -xzf ci-precache.tar.gz

      # Prep the global crate cache. Only needed if a lockfile is checked in.
      - name: Pre-cache dependencies
        if: steps.cache-deps.outputs.cache-hit != 'true'
        run: cargo ci-precache cargo-cache --temp=./target/.temp

      # Prep the target directory.
      - name: Pre-cache build dependencies
        if: steps.cache-build-deps.outputs.cache-hit != 'true'
        run: cargo ci-precache target --temp=./target/.temp --filter-platform=${{ matrix.platform }}
```

## Note on lockfiles

Keeping a lockfile checked in for building an executable, staticlib or cdylib as the resulting output is not subject to semantic versioning by cargo. For a regular library, however, cargo will automatically build against updated versions of your dependencies. This means you will have to be testing against the latest version of your dependencies. The way currently recommended by the rust documentation<sup>[1]</sup> is to not have a lockfile checked in. This has a few problems, CI performance, frequency of update checks, and non-deterministic testing.

An alternative solution would be to keep a lockfile checked in, but check for updates on a schedule. See an example [here](./.github/workflows/ci.yaml). This provides an improvement on all three fronts. CI performance is improved by not having to update the index (can take over a minute). Updates are tested on a schedule, instead of just whenever you push a change. And tests will no longer fail just because a dependency had an incompatible update (the update check will fail, so you'll still be notified when this happens).

## Details

```plain
cargo-ci-precache 1.0
Jason Newcomb <jsnewcomb@pm.me>

USAGE:
    cargo-ci-precache.exe [FLAGS] [OPTIONS] <mode>

ARGS:
    <mode>    Whether to clear the global cargo cache, or the projects target directory
              [possible values: cargo-cache, target]

FLAGS:
        --all-features           Activate all available features
        --dry-run                Do not make any changes, but show a list of files to be deleted
    -h, --help                   Prints help information
        --no-default-features    Do not activate the `default` feature
    -V, --version                Prints version information

OPTIONS:
        --features <features>                  Comma separated list of features to activate
        --filter-platform <filter-platform>
            Only include dependencies matching the given target-triple

        --manifest-path <manifest-path>        Path to Cargo.toml
        --temp <temp>
            Temporary directory to move directories into, will default to $TEMP
```

The following arguments are passed directly into cargo metadata:

* `--all-features`
* `--no-default-features`
* `--features`
* `--filter-platform`
* `--manifest-path`

Instead of deleting directories they will instead be moved into a temporary directory (see `--temp`). This is done to avoid having to recursively delete files. As this is meant to be run for CI purposes, changes not explicitly cached are discarded. This renders moving directories as a more efficient way of deleting them.

## License

Licensed under either of [Apache License](./LICENSE-APACHE), Version 2.0 or [MIT license](./LICENSE-MIT) at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this crate by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.

[actions-rs]: https://github.com/actions-rs
[1]: https://doc.rust-lang.org/cargo/faq.html#why-do-binaries-have-cargolock-in-version-control-but-not-libraries
