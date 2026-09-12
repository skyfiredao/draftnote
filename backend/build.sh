#!/usr/bin/env bash
# Build the draftnote backend binary and Docker image.
# Run from the backend module directory: bash build.sh

set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"

cd "$script_dir"

CGO_ENABLED=0 GOOS=linux GOARCH=amd64 \
  go build -trimpath -ldflags "-s -w" \
  -o draftnote-backend .

docker build -t draftnote-backend:latest -f Dockerfile .

echo "built: $script_dir/draftnote-backend"
echo "built: draftnote-backend:latest"
