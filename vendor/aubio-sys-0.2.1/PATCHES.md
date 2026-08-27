# Local patches

This directory contains `aubio-sys` 0.2.1 from crates.io (published checksum
`99ef2dfeaceccd0b8a6d72203409acc927d9eebc8180c5756099549c9f8f20a8`). It is
patched locally because the upstream crate has not released a Linux build fix.

The build script now probes for `<strings.h>` and defines `HAVE_STRINGS_H` when
it is available. `aubio_priv.h` includes that header under the detected feature.
This provides the POSIX declaration of `strncasecmp` without changing the C
language mode or enabling unrelated libc extensions globally.
