# Test the macOS DMG from a fresh account

This checklist exercises ESeq's R1 packaging and first-run user-content flow.
Use a new **Standard** (non-administrator) macOS account so the test does not
inherit the developer account's ESeq data, shell environment, or access to the
installed application bundle.

## Test artifact

Download
[`ESeq-0.1.0-e8f4142d.dmg`](https://github.com/universalsequences/eseq/releases/download/r1-test-e8f4142d/ESeq-0.1.0-e8f4142d.dmg).
It was built from commit `e8f4142d` for Apple Silicon macOS 11 or newer.

SHA-256:

```text
00b1be9381c3c1e56a4a491de9a87219bfe4df71bf42227c49ff222271dffc34
```

Optionally verify the download before opening it:

```sh
shasum -a 256 ~/Downloads/ESeq-0.1.0-e8f4142d.dmg
```

## Scope of this test

A fresh account tests clean first-run initialization, ordinary-user filesystem
permissions, bundled factory content and persistence under a new home
directory. It does **not** prove operation on a Mac without Xcode Command Line
Tools: those tools are installed system-wide and remain visible to every
account. That proof requires a clean macOS installation, VM, or second Mac.
ESeq's packaged runtime is nevertheless expected to use only its bundled
DGenLisp compiler and hermetic clang/lld stage.

## Install

1. Create a **Standard** account in **System Settings > Users & Groups**, then
   sign in to it.
2. Download the DMG from the link above in the new account. Opening a browser
   download also exercises macOS quarantine behavior that a locally copied DMG
   may not reproduce.
3. Open the DMG and drag **ESeq** onto the **Applications** shortcut.
4. This R1 build is ad-hoc signed rather than notarized. In Terminal, remove
   quarantine from the installed test build:

   ```sh
   xattr -dr com.apple.quarantine /Applications/ESeq.app
   ```

5. Eject the DMG and open `/Applications/ESeq.app`. Do not run the copy on the
   mounted image.

Before first launch, neither user-content root should exist:

```sh
test ! -e "$HOME/Library/Application Support/com.universalsequences.eseq"
test ! -e "$HOME/.eseq.d"
```

Each command exits silently with status zero when the account is clean.

## First-run checklist

- [ ] ESeq opens a window from `/Applications/ESeq.app`.
- [ ] Factory instruments and effects are available.
- [ ] A factory instrument can make sound.
- [ ] A new DGen instrument compiles and can be auditioned.
- [ ] The new instrument can be saved.
- [ ] A project can be saved.
- [ ] After quitting and reopening ESeq, the saved instrument and project are
      still available.
- [ ] Moving `ESeq.app` to `~/Applications/` and opening it there still works.

After first launch, inspect the roots ESeq created:

```sh
find "$HOME/Library/Application Support/com.universalsequences.eseq" \
  -maxdepth 3 -print
find "$HOME/.eseq.d" -maxdepth 3 -print
```

Generated compiler artifacts belong under:

```text
~/Library/Caches/com.universalsequences.eseq/
```

They must not be written inside `ESeq.app`. Installing the application into
`/Applications` as an administrator and running it from the Standard account
makes accidental bundle writes fail rather than silently succeeding.

## Report a failure

Record:

- the checklist step that failed;
- the macOS version and Mac model;
- whether the account was Standard or Administrator;
- whether Xcode Command Line Tools were already installed;
- the exact error shown by ESeq or Terminal; and
- relevant entries from **Console.app** at the time of failure.

Do not delete the fresh account's Application Support or Caches directories
until any failure has been investigated; they may contain useful compiler and
crash diagnostics.
