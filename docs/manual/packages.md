# Packages

A package is a folder of instruments, effects, samples, or Lisp modules that someone else can install. Use packages to share a pack of sounds you made, or to install a pack a friend sent you.

## Install a package

1. **File > Import Package**.
2. Choose the package folder, or a `.eseqpack` or `.zip` archive of one.
3. Read the summary. It lists what the package carries and where it will be installed.
4. Click **Install**. If a package with the same name is already installed, the button reads **Replace**.

The package's instruments and effects appear in the browser under a heading with the package name, beside **Factory** and **Library**. Use the **Packages** chip above the instrument list to show only packages. Samples from the package appear in the sample browser.

Installing a package runs its Lisp inside eseq. Only install packages from people you trust.

Packages live in `~/.eseq.d/packages/`. Deleting a package's folder there removes it on the next launch.

## Export a package

Only instruments and effects in your **Library** can be exported. To share a factory sound, fork it first so a copy lands in your library.

1. **File > Export Package**.
2. Enter a package name in the form `author/name`, such as `alec/acid-tools`, and a version.
3. Turn on each instrument and effect to include.
4. Click **Export** and choose where to save the `.eseqpack` archive.

The archive is a zip file. Send it as is; the recipient imports it with **File > Import Package**.

Presets saved for an exported instrument travel with it. Library macros used by an exported instrument are copied into it, so it compiles on a machine without your macro library. If an instrument refers to a file by an absolute path, the export reports it; that file will not exist on another machine, so move it inside the instrument's folder and refer to it by a relative path before exporting.

## Package ids

Once installed, a package's instruments are identified as `pkg:author.name/instrument` in projects. The same instrument name in your library, in the factory set, and in a package are three different instruments, so installing a package never changes the sound of an existing project.
