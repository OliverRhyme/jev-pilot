#!/usr/bin/env bash
# Builds both APKs and the manifests the Rust crate reads at compile time.
#
# The crate embeds these APKs, so a device can be provisioned with nothing but
# adb — no Android SDK on the machine driving it. Both must therefore be
# rebuilt and committed together with their manifests, or the crate will ship
# one build and claim another.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

export ANDROID_HOME="${ANDROID_HOME:-$HOME/Library/Android/sdk}"
export JAVA_HOME="${JAVA_HOME:-$(/usr/libexec/java_home -v 21)}"

./gradlew --no-daemon :service:assembleRelease :reader:assembleRelease

cp service/build/outputs/apk/release/service-release.apk JevPilotHelper.apk
cp reader/build/outputs/apk/release/reader-release.apk JevPilotReader.apk

python3 - <<'PY'
import datetime, hashlib, json, re

def manifest(gradle, apk, out):
    text = open(gradle).read()
    json.dump({
        "package": re.search(r'applicationId\s*=\s*"([^"]+)"', text).group(1),
        "version_code": int(re.search(r'versionCode\s*=\s*(\d+)', text).group(1)),
        "version_name": re.search(r'versionName\s*=\s*"([^"]+)"', text).group(1),
        "sha256": hashlib.sha256(open(apk, "rb").read()).hexdigest(),
        "built_at": datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds"),
    }, open(out, "w"), indent=2)
    open(out, "a").write("\n")

manifest("service/build.gradle.kts", "JevPilotHelper.apk", "service_manifest.json")
manifest("reader/build.gradle.kts", "JevPilotReader.apk", "reader_manifest.json")
PY

echo
echo "built:"
ls -lh JevPilotHelper.apk JevPilotReader.apk | awk '{print "  " $9 "  " $5}'
