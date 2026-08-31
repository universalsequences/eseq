# ESeq for macOS

ESeq is an Apple Silicon application for macOS 11 or newer. It does not
require a developer checkout after installation. Release DMGs are Developer ID
signed, notarized, and stapled; the build script can also produce an
ad-hoc-signed test build (the R1 path below).

For a clean-account acceptance pass, use the
[fresh-account DMG test checklist](../../docs/macos-dmg-fresh-account-test.md).

## Install the DMG

1. Open `ESeq-<version>-<hash>.dmg`.
2. Drag **ESeq** onto the **Applications** shortcut.
3. Open **ESeq** from `/Applications`.

If — and only if — you were handed an unsigned (ad-hoc R1) test build,
Gatekeeper will refuse to open it. Remove the download quarantine first:

```sh
xattr -dr com.apple.quarantine /Applications/ESeq.app
```

A signed release never needs this; if Gatekeeper complains about a DMG that
was supposed to be signed, stop and report it instead of working around it.

The command-line program is bundled at
`/Applications/ESeq.app/Contents/MacOS/eseq`; R1 does not add it to `PATH`.
User content is stored under
`~/Library/Application Support/com.universalsequences.eseq/` and generated
compiler artifacts under `~/Library/Caches/com.universalsequences.eseq/`.
The application bundles its exact JetBrains Mono 2.304 layout face under the
SIL Open Font License 1.1; it does not depend on fonts installed in the user's
account.

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

## Signed and notarized release (R3)

The same script produces a Developer ID signed, notarized, and stapled
artifact when two environment variables are set:

```sh
ESEQ_SIGNING_IDENTITY="Developer ID Application: Your Name (TEAMID)" \
ESEQ_NOTARY_PROFILE=eseq-notary \
./dist/macos/build.sh
```

In this mode the script signs the nested helper executables individually
(`eseq`, `DGenLisp-macos-arm64`, `dgen-clang`, `ld64.lld`) with the hardened
runtime, signs the outer app with the reviewed entitlements in
`dist/macos/entitlements.plist`, notarizes and staples the app before it goes
into the DMG, then signs, notarizes, and staples the DMG itself. `metal_seq`
and `eseq` carry `com.apple.security.cs.disable-library-validation` because
ESeq compiles and loads DGen dylibs at runtime and those dylibs can only carry
the ad-hoc signature the embedded linker produces — see the toolchain spec's
"Code Signing And Hardened Runtime" section.

A notarization pass alone is NOT release acceptance: the stapled app must
still compile, load, and run a new DGen instrument on a clean Mac with no
Xcode or Command Line Tools (toolchain spec, Release acceptance;
bead `eseq-toolchain.3`).

### One-time Apple setup

1. Enroll in the Apple Developer Program (developer.apple.com, USD 99/year).
2. Create a **Developer ID Application** certificate. Easiest path: Xcode →
   Settings → Accounts → your team → Manage Certificates → **+** →
   Developer ID Application. (Without Xcode: Keychain Access → Certificate
   Assistant → Request a Certificate From a Certificate Authority, then
   upload the CSR at developer.apple.com → Certificates.) Confirm with
   `security find-identity -v -p codesigning`; the quoted name of that
   identity is the `ESEQ_SIGNING_IDENTITY` value.
3. Create an app-specific password at account.apple.com → Sign-In and
   Security → App-Specific Passwords, then store notarization credentials
   once:

   ```sh
   xcrun notarytool store-credentials eseq-notary \
     --apple-id you@example.com --team-id TEAMID
   ```

   `notarytool` ships with Xcode or the Command Line Tools; it is needed on
   the build machine only, never on a tester's machine.
