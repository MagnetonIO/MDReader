#!/usr/bin/env bash
set -euo pipefail

: "${MDREADER_IOS_UNSIGNED_ARCHIVE_TAR:?missing unsigned iOS archive}"
: "${MDREADER_IOS_OUTPUT_DIRECTORY:?missing iOS output directory}"
: "${MDREADER_IOS_DISTRIBUTION_CERT_P12_B64:?missing iOS distribution certificate}"
: "${MDREADER_IOS_DISTRIBUTION_CERT_PASSWORD:?missing iOS certificate password}"
: "${MDREADER_IOS_PROVISIONING_PROFILE_B64:?missing iOS provisioning profile}"
: "${MDREADER_APPLE_TEAM_ID:?missing reviewed Apple team identifier}"
: "${MDREADER_VERSION:?missing release version}"
: "${MDREADER_BUILD_NUMBER:?missing build number}"

[[ "$MDREADER_APPLE_TEAM_ID" =~ ^[A-Z0-9]{10}$ ]] || {
  echo "reviewed Apple team identifier is invalid" >&2
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

beta_root="$(mktemp -d "${RUNNER_TEMP:?}/mdreader-ios-beta-sign.XXXXXX")"
keychain="$beta_root/signing.keychain-db"
profile_destination=""

cleanup() {
  status=$?
  if [[ -n "$profile_destination" && -e "$profile_destination" ]]; then
    /bin/rm -f -- "$profile_destination"
  fi
  /usr/bin/security delete-keychain "$keychain" >/dev/null 2>&1 || true
  /bin/rm -rf -- "$beta_root"
  exit "$status"
}
trap cleanup EXIT INT TERM

certificate="$beta_root/distribution.p12"
profile="$beta_root/profile.mobileprovision"
printf '%s' "$MDREADER_IOS_DISTRIBUTION_CERT_P12_B64" \
  | /usr/bin/openssl base64 -d -A -out "$certificate"
printf '%s' "$MDREADER_IOS_PROVISIONING_PROFILE_B64" \
  | /usr/bin/openssl base64 -d -A -out "$profile"
chmod 600 "$certificate" "$profile"

keychain_password="$(/usr/bin/openssl rand -hex 32)"
/usr/bin/security create-keychain -p "$keychain_password" "$keychain"
/usr/bin/security set-keychain-settings -lut 21600 "$keychain"
/usr/bin/security unlock-keychain -p "$keychain_password" "$keychain"
/usr/bin/security import "$certificate" \
  -k "$keychain" \
  -P "$MDREADER_IOS_DISTRIBUTION_CERT_PASSWORD" \
  -T /usr/bin/codesign \
  -T /usr/bin/security
/usr/bin/security set-key-partition-list \
  -S apple-tool:,apple:,codesign: \
  -s \
  -k "$keychain_password" \
  "$keychain" >/dev/null
/usr/bin/security list-keychains -d user -s "$keychain"

profile_plist="$beta_root/profile.plist"
/usr/bin/security cms -D -i "$profile" > "$profile_plist"
profile_uuid="$(/usr/bin/plutil -extract UUID raw -o - "$profile_plist")"
profile_team="$(/usr/bin/plutil -extract TeamIdentifier.0 raw -o - "$profile_plist")"
[[ "$profile_uuid" =~ ^[0-9A-Fa-f-]{36}$ ]] || {
  echo "iOS provisioning profile UUID is invalid" >&2
  exit 1
}
[[ "$profile_team" == "$MDREADER_APPLE_TEAM_ID" ]] || {
  echo "iOS provisioning profile team differs from reviewed authority" >&2
  exit 1
}
profile_directory="$HOME/Library/MobileDevice/Provisioning Profiles"
mkdir -p "$profile_directory"
profile_destination="$profile_directory/$profile_uuid.mobileprovision"
[[ ! -e "$profile_destination" ]] || {
  echo "refusing to replace an existing provisioning profile" >&2
  exit 1
}
/bin/cp "$profile" "$profile_destination"
chmod 600 "$profile_destination"

archive_root="$beta_root/archive"
node scripts/release/safe-package-extract.mjs \
  tar \
  "$MDREADER_IOS_UNSIGNED_ARCHIVE_TAR" \
  "$archive_root"
archive_paths=()
while IFS= read -r -d '' candidate; do
  archive_paths[${#archive_paths[@]}]="$candidate"
done < <(find "$archive_root" -type d -name '*.xcarchive' -print0)
[[ "${#archive_paths[@]}" -eq 1 ]] || {
  echo "expected exactly one unsigned iOS xcarchive; found ${#archive_paths[@]}" >&2
  exit 1
}
archive_path="${archive_paths[0]}"

export_options="$beta_root/ExportOptions.plist"
/usr/libexec/PlistBuddy -c 'Add :method string app-store-connect' "$export_options"
/usr/libexec/PlistBuddy -c 'Add :destination string export' "$export_options"
/usr/libexec/PlistBuddy -c 'Add :signingStyle string manual' "$export_options"
/usr/libexec/PlistBuddy -c "Add :teamID string $MDREADER_APPLE_TEAM_ID" "$export_options"
/usr/libexec/PlistBuddy -c 'Add :signingCertificate string Apple Distribution' "$export_options"
/usr/libexec/PlistBuddy -c 'Add :manageAppVersionAndBuildNumber bool false' "$export_options"
/usr/libexec/PlistBuddy -c 'Add :provisioningProfiles dict' "$export_options"
/usr/libexec/PlistBuddy \
  -c "Add :provisioningProfiles:dev.mdreader.app string $profile_uuid" \
  "$export_options"

export_root="$beta_root/export"
mkdir -p "$export_root" "$MDREADER_IOS_OUTPUT_DIRECTORY"
/usr/bin/xcodebuild \
  -exportArchive \
  -archivePath "$archive_path" \
  -exportPath "$export_root" \
  -exportOptionsPlist "$export_options"
ipa_paths=()
while IFS= read -r -d '' candidate; do
  ipa_paths[${#ipa_paths[@]}]="$candidate"
done < <(find "$export_root" -maxdepth 1 -type f -name '*.ipa' -print0)
[[ "${#ipa_paths[@]}" -eq 1 ]] || {
  echo "expected exactly one signed IPA; found ${#ipa_paths[@]}" >&2
  exit 1
}
ipa_path="${ipa_paths[0]}"
output="$MDREADER_IOS_OUTPUT_DIRECTORY/MDReader-${MDREADER_VERSION}-${MDREADER_BUILD_NUMBER}.ipa"
[[ ! -e "$output" ]] || {
  echo "refusing to replace an existing signed IPA" >&2
  exit 1
}
/bin/cp "$ipa_path" "$output"
chmod 600 "$output"
printf '%s\n' "$output"
