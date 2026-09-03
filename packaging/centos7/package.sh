#!/usr/bin/env bash
# Build two independently installable customer archives from verified binaries.
# Oracle and database.toml are explicit inputs because they can contain deployment-sensitive data.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
BIN_ROOT="$HERE/out/bin"
OUTPUT_DIR="$HERE/out/packages"
PLATFORM="linux/amd64"
ORACLE_ZIP="${ORACLE_CLIENT_ZIP:-}"
DATABASE_CONFIG="${QBS_DATABASE_CONFIG:-}"
RPM_DIR="${QBS_RPM_DIR:-}"
PACKAGE_VERSION="${QBS_PACKAGE_VERSION:-$(git -C "$ROOT" rev-parse --short HEAD)}"

usage() {
  cat <<'USAGE'
Usage:
  packaging/centos7/package.sh \
    --platform linux/amd64 \
    --oracle-client-zip /path/to/instantclient-basic-linux.x64-19*.zip \
    --database-config /path/to/database.toml

Required inputs:
  --platform              linux/amd64 or linux/arm64; defaults to linux/amd64
  --oracle-client-zip     Oracle Instant Client 19c Basic zip for the source archive
  --database-config       deployment database.toml; it is copied only to source

Optional inputs:
  --rpm-dir DIR           include supplied CentOS 7 RPM files in both archives
  --output-dir DIR        output directory; defaults to packaging/centos7/out/packages
  --version VALUE         archive version; defaults to the current git short SHA
  --force                 replace archives with the same names
USAGE
}

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

FORCE=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --platform) [[ $# -ge 2 ]] || die "--platform needs a value"; PLATFORM="$2"; shift 2 ;;
    --oracle-client-zip) [[ $# -ge 2 ]] || die "--oracle-client-zip needs a path"; ORACLE_ZIP="$2"; shift 2 ;;
    --database-config) [[ $# -ge 2 ]] || die "--database-config needs a path"; DATABASE_CONFIG="$2"; shift 2 ;;
    --rpm-dir) [[ $# -ge 2 ]] || die "--rpm-dir needs a directory"; RPM_DIR="$2"; shift 2 ;;
    --output-dir) [[ $# -ge 2 ]] || die "--output-dir needs a directory"; OUTPUT_DIR="$2"; shift 2 ;;
    --version) [[ $# -ge 2 ]] || die "--version needs a value"; PACKAGE_VERSION="$2"; shift 2 ;;
    --force) FORCE=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown argument: $1" ;;
  esac
done

case "$PLATFORM" in
  linux/amd64) SLUG=linux-amd64; ORACLE_PATTERN='instantclient-basic-linux.x64-19*.zip' ;;
  linux/arm64) SLUG=linux-arm64; ORACLE_PATTERN='instantclient-basic-linux.arm64-19*.zip' ;;
  *) die "unsupported platform $PLATFORM (use linux/amd64 or linux/arm64)" ;;
esac

[[ -n "$ORACLE_ZIP" ]] || die "--oracle-client-zip is required for a complete source archive"
[[ -f "$ORACLE_ZIP" ]] || die "Oracle client archive not found: $ORACLE_ZIP"
[[ "$(basename "$ORACLE_ZIP")" == $ORACLE_PATTERN ]] || die "Oracle archive must match $ORACLE_PATTERN"
[[ -n "$DATABASE_CONFIG" ]] || die "--database-config is required for a complete source archive"
[[ -f "$DATABASE_CONFIG" ]] || die "database config not found: $DATABASE_CONFIG"
[[ -s "$DATABASE_CONFIG" ]] || die "database config is empty: $DATABASE_CONFIG"

BIN_DIR="$BIN_ROOT/$SLUG"
for binary in db-qbs-source db-qbs-source-run db-qbs-sink; do
  [[ -x "$BIN_DIR/$binary" ]] || die "missing executable $BIN_DIR/$binary; run packaging/centos7/build.sh first"
done

install -d -m 0755 "$OUTPUT_DIR"
SOURCE_NAME="db-qbs-source-${SLUG}-${PACKAGE_VERSION}"
SINK_NAME="db-qbs-sink-${SLUG}-${PACKAGE_VERSION}"
SOURCE_ARCHIVE="$OUTPUT_DIR/${SOURCE_NAME}.tar.gz"
SINK_ARCHIVE="$OUTPUT_DIR/${SINK_NAME}.tar.gz"
CHECKSUMS="$OUTPUT_DIR/SHA256SUMS-${PACKAGE_VERSION}"

for artifact in "$SOURCE_ARCHIVE" "$SINK_ARCHIVE" "$CHECKSUMS"; do
  if [[ -e "$artifact" ]]; then
    (( FORCE )) || die "output exists: $artifact (pass --force to replace it)"
    [[ -f "$artifact" ]] || die "output exists and is not a file: $artifact"
    unlink "$artifact"
  fi
done

STAGE=""
cleanup() {
  if [[ -n "$STAGE" && -d "$STAGE" ]]; then
    find "$STAGE" -depth -delete
  fi
}
trap cleanup EXIT
STAGE="$(mktemp -d "$OUTPUT_DIR/.package-stage.XXXXXX")"

copy_role_tree() {
  local role=$1
  local package_dir=$2
  install -d -m 0755 "$package_dir/bin" "$package_dir/conf" "$package_dir/oracle" "$package_dir/scripts" "$package_dir/systemd" "$package_dir/stunnel"

  if [[ "$role" == source ]]; then
    install -m 0755 "$BIN_DIR/db-qbs-source" "$BIN_DIR/db-qbs-source-run" "$package_dir/bin/"
    install -m 0644 "$ROOT/packaging/customer/source/conf/source.toml.example" "$package_dir/conf/"
    install -m 0600 "$DATABASE_CONFIG" "$package_dir/conf/database.toml"
    install -m 0755 "$ROOT/packaging/customer/source/scripts/"*.sh "$package_dir/scripts/"
    install -m 0644 "$ROOT/packaging/customer/source/systemd/db-qbs-source.service" "$package_dir/systemd/"
    install -m 0755 "$ROOT/packaging/preflight/preflight-source.sh" "$package_dir/preflight-source.sh"
    install -m 0644 "$ROOT/docs/install/source-centos7.md" "$package_dir/INSTALL.md"
    install -m 0644 "$ROOT/packaging/stunnel/README.md" "$package_dir/stunnel/"
    install -m 0644 "$ROOT/packaging/stunnel/source-side/"* "$package_dir/stunnel/"
    install -m 0644 "$ORACLE_ZIP" "$package_dir/oracle/"
  else
    install -m 0755 "$BIN_DIR/db-qbs-sink" "$package_dir/bin/"
    install -m 0644 "$ROOT/packaging/customer/sink/conf/sink.toml.example" "$package_dir/conf/"
    install -m 0755 "$ROOT/packaging/customer/sink/scripts/"*.sh "$package_dir/scripts/"
    install -m 0644 "$ROOT/packaging/customer/sink/systemd/db-qbs-sink.service" "$package_dir/systemd/"
    install -m 0755 "$ROOT/packaging/preflight/preflight-target.sh" "$package_dir/preflight-target.sh"
    install -m 0644 "$ROOT/docs/install/target-centos7.md" "$package_dir/INSTALL.md"
    install -m 0644 "$ROOT/packaging/stunnel/README.md" "$package_dir/stunnel/"
    install -m 0644 "$ROOT/packaging/stunnel/target-side/"* "$package_dir/stunnel/"
  fi

  if [[ -n "$RPM_DIR" ]]; then
    [[ -d "$RPM_DIR" ]] || die "RPM directory not found: $RPM_DIR"
    install -d -m 0755 "$package_dir/rpm"
    rpm_count=0
    for rpm in "$RPM_DIR"/*.rpm; do
      [[ -f "$rpm" ]] || continue
      install -m 0644 "$rpm" "$package_dir/rpm/"
      rpm_count=$((rpm_count + 1))
    done
    (( rpm_count > 0 )) || die "no .rpm files found in $RPM_DIR"
  fi

  printf '%s\n' "$PACKAGE_VERSION" > "$package_dir/VERSION"
  (
    cd "$package_dir"
    find . -type f ! -name SHA256SUMS -print0 | sort -z | xargs -0 shasum -a 256 > SHA256SUMS
  )
}

SOURCE_DIR="$STAGE/$SOURCE_NAME"
SINK_DIR="$STAGE/$SINK_NAME"
copy_role_tree source "$SOURCE_DIR"
copy_role_tree sink "$SINK_DIR"

COPYFILE_DISABLE=1 tar -czf "$SOURCE_ARCHIVE" -C "$STAGE" "$SOURCE_NAME"
COPYFILE_DISABLE=1 tar -czf "$SINK_ARCHIVE" -C "$STAGE" "$SINK_NAME"
chmod 0600 "$SOURCE_ARCHIVE" "$SINK_ARCHIVE"
(
  cd "$OUTPUT_DIR"
  shasum -a 256 "$(basename "$SOURCE_ARCHIVE")" "$(basename "$SINK_ARCHIVE")" > "$(basename "$CHECKSUMS")"
)
chmod 0644 "$CHECKSUMS"

printf 'source=%s\n' "$SOURCE_ARCHIVE"
printf 'sink=%s\n' "$SINK_ARCHIVE"
printf 'checksums=%s\n' "$CHECKSUMS"
shasum -a 256 "$SOURCE_ARCHIVE" "$SINK_ARCHIVE"
