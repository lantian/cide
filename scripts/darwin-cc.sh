#!/usr/bin/env sh
# A `cc` a Linux host can point at when type-checking for macOS.
#
# `cargo check` and `cargo clippy` never link, so Apple's linker, frameworks and SDK are
# irrelevant to them. What actually stops a `--target aarch64-apple-darwin` check on Linux is a
# handful of *dependency build scripts* that compile C or Objective-C with flags a GNU `cc`
# rejects: `-arch arm64`, `-isysroot <sdk>`, `-mmacosx-version-min=…`, `-xobjective-c`.
#
# So those flags are dropped and the real source is compiled for the host instead. The objects are
# wrong for Darwin and that does not matter: nothing a check produces is ever linked. Compiling an
# empty file instead would be simpler and does NOT work — `openssl-sys`'s vendored build inspects
# what it compiled, and an empty translation unit fails its configure step.
#
# Used by README.md's *Type-checking for macOS from Linux*, which says what it does not prove.
set -eu

args=""
skip_next=0
for arg in "$@"; do
    if [ "$skip_next" = 1 ]; then skip_next=0; continue; fi
    case "$arg" in
        -arch|-isysroot) skip_next=1; continue ;;
        -mmacosx-version-min=*|--target=*|-fobjc-*|-xobjective-c) continue ;;
    esac
    args="$args $(printf '%s' "$arg" | sed "s/'/'\\\\''/g; s/^/'/; s/\$/'/")"
done

eval "exec cc $args"
