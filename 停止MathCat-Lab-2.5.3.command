#!/bin/sh
cd "$(dirname "$0")" || exit 1
exec sh ./scripts/stop-version.sh
