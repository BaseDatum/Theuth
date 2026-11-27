#!/usr/bin/env bash
# Lint and test the cloud-nodes plugin: no network, no cloud credentials, no engine.
#   plugins/cloud-nodes/check.sh
set -euo pipefail
cd "$(dirname "$0")"
python3 -m py_compile main.py cloud_nodes.py test_cloud_nodes.py
if command -v uvx >/dev/null; then
	uvx --quiet ruff check --line-length 120 --exclude theuth_plugin.py .
else
	echo "ruff: skipped (no uvx)"
fi
python3 -m unittest -v test_cloud_nodes 2>&1
