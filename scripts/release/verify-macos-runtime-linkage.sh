#!/usr/bin/env bash
set -euo pipefail

# The reviewed ONNX Runtime dylib carries the install name
# `@rpath/libonnxruntime.1.24.2.dylib`, so the MDReader executable only resolves
# it at launch when the binary carries a matching LC_RPATH and the dylib is
# present where that LC_RPATH points. Nothing in Tauri's macOS bundler adds the
# load command, so this contract asserts both halves — the rpath the linker
# recorded and the file dyld would find through it — before credentials are used
# (staged) and again on the signed application (bundled).
#
# Written for bash 3.2 (the macOS system shell): no mapfile, no associative
# arrays, no empty-array expansion under `set -u`.

MODE="${1:?mode is required (staged|bundled|runtime)}"
EXPECTED_RPATH='@executable_path/../Frameworks'
RUNTIME_DYLIB='libonnxruntime.1.24.2.dylib'
RUNTIME_INSTALL_NAME='@rpath/libonnxruntime.1.24.2.dylib'
SUPPORTED_MACOS_FLOOR='12.0'

fail() {
  echo "$1" >&2
  exit 1
}

directory_identity() {
  stat -f '%d:%i' "$1"
}

architectures() {
  lipo -archs "$1"
}

version_at_most() {
  awk -v actual="$1" -v maximum="$2" 'BEGIN {
    split(actual, left, ".")
    split(maximum, right, ".")
    for (part = 1; part <= 3; part += 1) {
      left[part] += 0
      right[part] += 0
      if (left[part] < right[part]) exit 0
      if (left[part] > right[part]) exit 1
    }
    exit 0
  }'
}

reviewed_runtime_dependency() {
  case "$1" in
    /System/Library/Frameworks/Foundation.framework/Versions/C/Foundation | \
      /System/Library/Frameworks/CoreFoundation.framework/Versions/A/CoreFoundation | \
      /usr/lib/libiconv.2.dylib | \
      /usr/lib/libc++.1.dylib | \
      /usr/lib/libSystem.B.dylib)
      return 0
      ;;
    *)
      return 1
      ;;
  esac
}

verify_bundled_runtime_inventory() {
  frameworks="$1"
  entry_count=0
  while IFS= read -r -d '' entry; do
    entry_count=$((entry_count + 1))
    [[ "$entry" == "$frameworks/$RUNTIME_DYLIB" && -f "$entry" && ! -L "$entry" ]] ||
      fail "packaged runtime directory contains an unreviewed Frameworks entry: ${entry#"$frameworks/"}"
  done < <(find "$frameworks" -mindepth 1 -maxdepth 1 -print0)
  [[ "$entry_count" -eq 1 ]] ||
    fail 'packaged runtime directory must contain exactly the reviewed ONNX Runtime dylib'
}

verify_runtime_compatibility() {
  runtime="$1"
  [[ -f "$runtime" && ! -L "$runtime" ]] ||
    fail "packaged ONNX Runtime is missing from $runtime"

  archs="$(architectures "$runtime")"
  arch_count=0
  has_arm64=0
  has_x86_64=0
  for arch in $archs; do
    arch_count=$((arch_count + 1))
    case "$arch" in
      arm64) has_arm64=1 ;;
      x86_64) has_x86_64=1 ;;
      *) fail "ONNX Runtime contains an unreviewed architecture: $arch" ;;
    esac
  done
  [[ "$arch_count" -eq 2 && "$has_arm64" -eq 1 && "$has_x86_64" -eq 1 ]] ||
    fail 'ONNX Runtime must contain exactly the reviewed arm64 and x86_64 slices'

  for arch in $archs; do
    build_versions="$(otool -arch "$arch" -l "$runtime" | awk '
      $1 == "cmd" { command = $2 }
      command == "LC_BUILD_VERSION" && $1 == "platform" { platform = $2 }
      command == "LC_BUILD_VERSION" && $1 == "minos" {
        print platform, $2
        command = ""
        platform = ""
      }
    ')"
    build_version_count="$(
      printf '%s\n' "$build_versions" |
        awk 'NF { count += 1 } END { print count + 0 }'
    )"
    [[ "$build_version_count" -eq 1 ]] ||
      fail "$arch ONNX Runtime slice does not carry exactly one LC_BUILD_VERSION"
    platform="$(printf '%s\n' "$build_versions" | awk 'NF { print $1 }')"
    minimum="$(printf '%s\n' "$build_versions" | awk 'NF { print $2 }')"
    [[ "$platform" == '1' || "$platform" == 'macos' ]] ||
      fail "$arch ONNX Runtime slice targets an unreviewed platform: $platform"
    version_at_most "$minimum" "$SUPPORTED_MACOS_FLOOR" ||
      fail "$arch ONNX Runtime minimum macOS $minimum exceeds supported floor $SUPPORTED_MACOS_FLOOR"

    install_name_count=0
    while IFS= read -r dependency; do
      [[ -n "$dependency" ]] || continue
      if [[ "$dependency" == "$RUNTIME_INSTALL_NAME" ]]; then
        install_name_count=$((install_name_count + 1))
      elif ! reviewed_runtime_dependency "$dependency"; then
        fail "$arch ONNX Runtime slice has an unreviewed dependency: $dependency"
      fi
    done <<< "$(
      otool -arch "$arch" -L "$runtime" |
        tail -n +2 |
        awk 'NF { print $1 }'
    )"
    [[ "$install_name_count" -eq 1 ]] ||
      fail "$arch ONNX Runtime slice does not expose exactly one reviewed install name"
  done
}

# LC_RPATH paths recorded for one architecture slice.
load_command_rpaths() {
  otool -arch "$2" -l "$1" | awk '
    $1 == "cmd" { command = $2 }
    command == "LC_RPATH" && $1 == "path" { print $2 }
  '
}

# @rpath-relative dependencies recorded for one architecture slice. The first
# `otool -L` line is the file header, never a load command.
load_command_rpath_dependencies() {
  otool -arch "$2" -L "$1" | tail -n +2 | awk '$1 ~ /^@rpath\// { print $1 }'
}

verify_linkage() {
  binary="$1"
  frameworks="$2"

  [[ -f "$binary" && ! -L "$binary" ]] ||
    fail "MDReader executable is missing from $binary"
  [[ -d "$frameworks" && ! -L "$frameworks" ]] ||
    fail "packaged runtime directory is missing from $frameworks"
  if [[ "$MODE" == "bundled" ]]; then
    verify_bundled_runtime_inventory "$frameworks"
  fi
  verify_runtime_compatibility "$frameworks/$RUNTIME_DYLIB"

  archs="$(architectures "$binary")"
  [[ -n "$archs" ]] || fail 'MDReader executable exposes no architecture slice'

  for arch in $archs; do
    rpath_count=0
    while IFS= read -r rpath; do
      [[ -n "$rpath" ]] || continue
      rpath_count=$((rpath_count + 1))
      [[ "$rpath" == "$EXPECTED_RPATH" ]] ||
        fail "MDReader executable ($arch) records an unreviewed LC_RPATH: $rpath"
      resolved="$(dirname "$binary")/${rpath#@executable_path/}"
      if [[ "$MODE" == "bundled" ]]; then
        [[ -d "$resolved" ]] ||
          fail "MDReader executable ($arch) LC_RPATH does not resolve to a directory"
        [[ "$(directory_identity "$resolved")" == "$(directory_identity "$frameworks")" ]] ||
          fail "MDReader executable ($arch) LC_RPATH does not resolve to the packaged runtime directory"
      fi
    done <<< "$(load_command_rpaths "$binary" "$arch")"
    [[ "$rpath_count" -eq 1 ]] ||
      fail "MDReader executable ($arch) does not carry exactly one reviewed LC_RPATH"

    runtime_dependencies=0
    while IFS= read -r dependency; do
      [[ -n "$dependency" ]] || continue
      name="${dependency#@rpath/}"
      [[ "$name" != */* ]] ||
        fail "MDReader executable ($arch) loads a nested @rpath dependency: $dependency"
      [[ -f "$frameworks/$name" && ! -L "$frameworks/$name" ]] ||
        fail "MDReader executable ($arch) loads $dependency, which the packaged runtime does not provide"
      if [[ "$name" == "$RUNTIME_DYLIB" ]]; then
        runtime_dependencies=$((runtime_dependencies + 1))
      fi
    done <<< "$(load_command_rpath_dependencies "$binary" "$arch")"
    [[ "$runtime_dependencies" -eq 1 ]] ||
      fail "MDReader executable ($arch) is not linked to $RUNTIME_DYLIB exactly once"
  done
}

case "$MODE" in
  staged)
    BINARY="${2:?compiled executable is required}"
    STAGED_FRAMEWORKS="${3:?staged ONNX Runtime directory is required}"
    verify_linkage "$BINARY" "$STAGED_FRAMEWORKS"
    ;;
  bundled)
    APP_PATH="${2:?application bundle is required}"
    verify_linkage \
      "$APP_PATH/Contents/MacOS/mdreader-app" \
      "$APP_PATH/Contents/Frameworks"
    ;;
  runtime)
    RUNTIME_PATH="${2:?ONNX Runtime dylib is required}"
    verify_runtime_compatibility "$RUNTIME_PATH"
    ;;
  *)
    fail "unsupported macOS runtime linkage mode: $MODE"
    ;;
esac
