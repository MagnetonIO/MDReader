#!/usr/bin/env bash
set -euo pipefail

: "${MDREADER_ANDROID_UNSIGNED_AAB:?missing unsigned Android App Bundle}"
: "${MDREADER_ANDROID_OUTPUT_DIRECTORY:?missing Android output directory}"
: "${MDREADER_ANDROID_UPLOAD_KEYSTORE_B64:?missing Android upload keystore}"
: "${MDREADER_ANDROID_UPLOAD_STORE_PASSWORD:?missing Android keystore password}"
: "${MDREADER_ANDROID_UPLOAD_KEY_PASSWORD:?missing Android key password}"
: "${MDREADER_ANDROID_UPLOAD_KEY_ALIAS:?missing reviewed Android key alias}"
: "${MDREADER_ANDROID_UPLOAD_CERT_SHA256:?missing reviewed Android upload certificate}"
: "${MDREADER_VERSION:?missing release version}"
: "${MDREADER_BUILD_NUMBER:?missing build number}"

[[ "$MDREADER_ANDROID_UPLOAD_KEY_ALIAS" =~ ^[A-Za-z0-9._-]{1,80}$ ]] || {
  echo "reviewed Android upload-key alias is invalid" >&2
  exit 1
}
expected_certificate="$(
  printf '%s' "$MDREADER_ANDROID_UPLOAD_CERT_SHA256" \
    | tr '[:upper:]' '[:lower:]' \
    | tr -d ':'
)"
[[ "$expected_certificate" =~ ^[0-9a-f]{64}$ ]] || {
  echo "reviewed Android upload certificate SHA-256 is invalid" >&2
  exit 1
}
[[ "$MDREADER_VERSION" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]] || {
  echo "mobile beta version is invalid" >&2
  exit 1
}
[[ "$MDREADER_BUILD_NUMBER" =~ ^[1-9][0-9]*$ ]] || {
  echo "mobile beta build number is invalid" >&2
  exit 1
}

beta_root="$(mktemp -d "${RUNNER_TEMP:?}/mdreader-android-beta-sign.XXXXXX")"
cleanup() {
  status=$?
  /bin/rm -rf -- "$beta_root"
  exit "$status"
}
trap cleanup EXIT INT TERM

keystore="$beta_root/upload.jks"
printf '%s' "$MDREADER_ANDROID_UPLOAD_KEYSTORE_B64" \
  | /usr/bin/openssl base64 -d -A -out "$keystore"
chmod 600 "$keystore"
export MDREADER_ANDROID_KEYSTORE_PASSWORD_ENV="$MDREADER_ANDROID_UPLOAD_STORE_PASSWORD"
export MDREADER_ANDROID_KEY_PASSWORD_ENV="$MDREADER_ANDROID_UPLOAD_KEY_PASSWORD"

certificate_output="$(
  keytool \
    -list \
    -v \
    -keystore "$keystore" \
    -storepass:env MDREADER_ANDROID_KEYSTORE_PASSWORD_ENV \
    -alias "$MDREADER_ANDROID_UPLOAD_KEY_ALIAS"
)"
actual_certificate="$(
  printf '%s\n' "$certificate_output" \
    | sed -n 's/^[[:space:]]*SHA256:[[:space:]]*//p' \
    | head -1 \
    | tr '[:upper:]' '[:lower:]' \
    | tr -d ':'
)"
[[ "$actual_certificate" == "$expected_certificate" ]] || {
  echo "Android upload certificate differs from reviewed authority" >&2
  exit 1
}

mkdir -p "$MDREADER_ANDROID_OUTPUT_DIRECTORY"
output="$MDREADER_ANDROID_OUTPUT_DIRECTORY/MDReader-${MDREADER_VERSION}-${MDREADER_BUILD_NUMBER}.aab"
[[ ! -e "$output" ]] || {
  echo "refusing to replace an existing signed Android App Bundle" >&2
  exit 1
}
/bin/cp "$MDREADER_ANDROID_UNSIGNED_AAB" "$output"
chmod 600 "$output"
jarsigner \
  -keystore "$keystore" \
  -storepass:env MDREADER_ANDROID_KEYSTORE_PASSWORD_ENV \
  -keypass:env MDREADER_ANDROID_KEY_PASSWORD_ENV \
  -digestalg SHA-256 \
  "$output" \
  "$MDREADER_ANDROID_UPLOAD_KEY_ALIAS"
jarsigner -verify -verbose -certs "$output" >/dev/null
printf '%s\n' "$output"
