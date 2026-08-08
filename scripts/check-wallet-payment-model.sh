#!/usr/bin/env bash
set -euo pipefail

readonly TLC_VERSION="1.7.4"
readonly TLC_SHA256="936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88"
readonly REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

scratch_dir=""
if [[ -n "${TLA2TOOLS_JAR:-}" ]]; then
  tla_jar="${TLA2TOOLS_JAR}"
else
  scratch_dir="$(mktemp -d)"
  trap 'rm -rf -- "$scratch_dir"' EXIT
  tla_jar="${scratch_dir}/tla2tools.jar"
  curl --fail --location --retry 3 --silent --show-error \
    "https://github.com/tlaplus/tlaplus/releases/download/v${TLC_VERSION}/tla2tools.jar" \
    --output "${tla_jar}"
fi

if command -v sha256sum >/dev/null 2>&1; then
  actual_sha256="$(sha256sum "${tla_jar}" | awk '{print $1}')"
else
  actual_sha256="$(shasum -a 256 "${tla_jar}" | awk '{print $1}')"
fi
if [[ "${actual_sha256}" != "${TLC_SHA256}" ]]; then
  printf 'SHA-256 mismatch for %s\n' "${tla_jar}" >&2
  exit 1
fi

cd "${REPO_ROOT}/docs/spec"
java -XX:+UseParallelGC -cp "${tla_jar}" tlc2.TLC \
  -workers 1 \
  -config WalletPaymentAttempts.cfg \
  WalletPaymentAttempts.tla
