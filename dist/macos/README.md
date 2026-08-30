# ESeq for macOS

ESeq R1 is an ad-hoc-signed Apple Silicon application for macOS 11 or newer.
It does not require a developer checkout after installation.

For a clean-account acceptance pass, use the
[fresh-account DMG test checklist](../../docs/macos-dmg-fresh-account-test.md).

## Install the DMG

1. Open `ESeq-<version>-<hash>.dmg`.
2. Drag **ESeq** onto the **Applications** shortcut.
3. In Terminal, remove the download quarantine from this unsigned test build:

   ```sh
   xattr -dr com.apple.quarantine /Applications/ESeq.app
   ```

4. Open **ESeq** from `/Applications`.

The command-line program is bundled at
`/Applications/ESeq.app/Contents/MacOS/eseq`; R1 does not add it to `PATH`.
User content is stored under
`~/Library/Application Support/com.universalsequences.eseq/` and generated
compiler artifacts under `~/Library/Caches/com.universalsequences.eseq/`.

## Build from a checkout

The build requires Apple Silicon macOS, Rust, the fetched DGenLisp compiler,
and the staged DGen toolchain. Prepare the two untracked dependencies when
needed:

```sh
./scripts/fetch_dgenlisp.sh --target macos-arm64
./rebuild_dgenlisp_tool.sh
```

Then run:

```sh
./dist/macos/build.sh
```

The script builds the release binaries, assembles and ad-hoc-signs
`dist/out/ESeq.app`, and creates `dist/out/ESeq-<version>-<hash>.dmg`.

Release builds are compiled with `--remap-path-prefix` so no path from the
build machine is baked into the shipped binaries, and the script verifies that
before it packages anything. Those flags are part of the build fingerprint, so
packaging uses its own `target/package/` directory rather than fighting
`cargo build --release` for the shared one.
