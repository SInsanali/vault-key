#!/usr/bin/env bash
# One-time setup of the Windows Rust toolchain that builds Vault Key, run from WSL.
#
# Why this exists: the app window uses the `windows` crates, which need a `dlltool` to make
# import libraries. Neither of Rust's GNU-flavoured Windows toolchains ships a usable one, and
# this project avoids needing Visual Studio. The LLVM-flavoured toolchain (gnullvm) links with
# its bundled `rust-lld`, and LLVM's `llvm-ar` acts as `llvm-dlltool` when run under that name.
#
# What it does (all per-user, nothing system-wide):
#   1. installs rustup if missing, and the stable gnu and gnullvm toolchains;
#   2. copies llvm-ar.exe (from the gnu toolchain, where it is statically linked) to
#      %LOCALAPPDATA%\vault-key\tools\llvm-dlltool.exe with the two DLLs it needs.
# `scripts/cargo` then points cargo at those.
set -euo pipefail
profile="$(wslpath "$(cmd.exe /c 'echo %USERPROFILE%' 2>/dev/null | tr -d '\r')")"
rustup="$profile/.cargo/bin/rustup.exe"
if [ ! -x "$rustup" ]; then
  tmp="$(mktemp -d)"
  curl -sSfL -o "$tmp/rustup-init.exe" https://win.rustup.rs/x86_64
  chmod +x "$tmp/rustup-init.exe"
  "$tmp/rustup-init.exe" -y --default-host x86_64-pc-windows-gnu --default-toolchain stable --profile minimal
fi
"$rustup" toolchain install stable-x86_64-pc-windows-gnu --profile minimal -c clippy -c rustfmt -c llvm-tools
"$rustup" toolchain install stable-x86_64-pc-windows-gnullvm --profile minimal -c clippy -c rustfmt
src="$profile/.rustup/toolchains/stable-x86_64-pc-windows-gnu/lib/rustlib/x86_64-pc-windows-gnu/bin"
tools="$profile/AppData/Local/vault-key/tools"
mkdir -p "$tools"
cp "$src/llvm-ar.exe" "$tools/llvm-dlltool.exe"
cp "$src/libgcc_s_seh-1.dll" "$src/libwinpthread-1.dll" "$tools/"
echo "ready: $tools/llvm-dlltool.exe"
