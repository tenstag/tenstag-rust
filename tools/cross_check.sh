#!/bin/sh
# Builds and lints tenstag for Windows and macOS from Linux (`make cross`), so that code written for those systems
# at least compiles and passes clippy. Nothing is linked or run: this is not a test on those systems.
#
# The build has no C code (since 0.6.0, when HTTPS and its `ring` crate went), so no C compiler or SDK for the
# targets is needed. Needs: rustup (the targets are added when missing).
#
# Windows is checked as x86_64-pc-windows-gnu; tenstag's own code is the same for the msvc target (it tests
# cfg(windows), never the environment).
set -eu
for rust in x86_64-pc-windows-gnu aarch64-apple-darwin x86_64-apple-darwin; do
    rustup target add "$rust" >/dev/null 2>&1 || rustup target add "$rust"
    echo "== $rust"
    cargo clippy --all-targets --target "$rust" -- -D warnings
done
