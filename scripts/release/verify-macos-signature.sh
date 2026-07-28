#!/usr/bin/env bash
set -euo pipefail

BUNDLE_ROOT="${1:?bundle root is required}"
EXPECTED_TEAM_ID="${2:?expected Apple team identifier is required}"
[[ "$EXPECTED_TEAM_ID" =~ ^[A-Z0-9]{10}$ ]] || {
  echo 'expected Apple team identifier is invalid' >&2
  exit 1
}
APP_PATH="$(find "$BUNDLE_ROOT" -type d -name 'MDReader.app' -print -quit)"
[[ -n "$APP_PATH" ]] || { echo 'MDReader.app missing' >&2; exit 1; }
APP_BINARY="$APP_PATH/Contents/MacOS/mdreader-app"
RUNTIME_PATH="$APP_PATH/Contents/Frameworks/libonnxruntime.1.23.2.dylib"
[[ -f "$APP_BINARY" && ! -L "$APP_BINARY" ]] || {
  echo 'MDReader executable missing from signed application' >&2
  exit 1
}
[[ -f "$RUNTIME_PATH" && ! -L "$RUNTIME_PATH" ]] || {
  echo 'reviewed ONNX Runtime missing from signed application' >&2
  exit 1
}

ARCHS="$(lipo -archs "$RUNTIME_PATH")"
ARCH_COUNT=0
HAS_ARM64=0
HAS_X86_64=0
for arch in $ARCHS; do
  ARCH_COUNT=$((ARCH_COUNT + 1))
  if [[ "$arch" == "arm64" ]]; then
    HAS_ARM64=1
  elif [[ "$arch" == "x86_64" ]]; then
    HAS_X86_64=1
  else
    echo "signed ONNX Runtime contains unsupported architecture: $arch" >&2
    exit 1
  fi
done
[[ "$ARCH_COUNT" -eq 2 && "$HAS_ARM64" -eq 1 && "$HAS_X86_64" -eq 1 ]] || {
  echo 'signed ONNX Runtime is not universal arm64 and x86_64' >&2
  exit 1
}

INSTALL_NAMES="$(otool -D "$RUNTIME_PATH" | awk '/^[[:space:]]*@/ { print $1 }')"
INSTALL_NAME_COUNT="$(printf '%s\n' "$INSTALL_NAMES" | awk 'NF { count += 1 } END { print count + 0 }')"
[[ "$INSTALL_NAME_COUNT" -eq 2 ]] || {
  echo 'signed ONNX Runtime does not expose one install name per architecture' >&2
  exit 1
}
while IFS= read -r install_name; do
  [[ -z "$install_name" || "$install_name" == '@rpath/libonnxruntime.1.23.2.dylib' ]] || {
    echo 'signed ONNX Runtime install name is not the reviewed @rpath identity' >&2
    exit 1
  }
done <<< "$INSTALL_NAMES"

LINK_COUNT="$(otool -L "$APP_BINARY" | awk '$1 == "@rpath/libonnxruntime.1.23.2.dylib" { count += 1 } END { print count + 0 }')"
[[ "$LINK_COUNT" -eq 1 ]] || {
  echo 'MDReader executable is not linked to the packaged ONNX Runtime exactly once' >&2
  exit 1
}

# `@rpath` is only resolvable if the executable carries the matching LC_RPATH and
# that rpath lands on the bundle's own Frameworks directory: assert the launch
# closure per architecture, not just the recorded dependency.
"$(dirname "${BASH_SOURCE[0]}")/verify-macos-runtime-linkage.sh" bundled "$APP_PATH"

codesign --verify --deep --strict --verbose=2 "$APP_PATH"
codesign --verify --strict --verbose=2 "$RUNTIME_PATH"
APP_TEAM_ID="$(codesign -d --verbose=4 "$APP_PATH" 2>&1 | sed -n 's/^TeamIdentifier=//p')"
RUNTIME_TEAM_ID="$(codesign -d --verbose=4 "$RUNTIME_PATH" 2>&1 | sed -n 's/^TeamIdentifier=//p')"
[[ "$APP_TEAM_ID" == "$EXPECTED_TEAM_ID" && "$RUNTIME_TEAM_ID" == "$EXPECTED_TEAM_ID" ]] || {
  echo 'MDReader and ONNX Runtime signatures do not match the reviewed TeamIdentifier' >&2
  exit 1
}
spctl --assess --type execute --verbose=2 "$APP_PATH"
