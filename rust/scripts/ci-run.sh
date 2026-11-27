#!/usr/bin/env bash
# Runs a command for CI; when it fails, its compiler and test errors are also written as workflow annotations (readable
# from the check run without downloading the log).
set -o pipefail
log=$(mktemp)
"$@" 2>&1 | tee "$log"
status=${PIPESTATUS[0]}
if [ "$status" -ne 0 ]; then
  python3 - "$log" "$*" <<'PY'
import re, sys
text = open(sys.argv[1], encoding='utf-8', errors='replace').read()
text = re.sub(r'\x1b\[[0-9;]*m', '', text)
blocks = re.findall(r'^(?:error(?:\[E\d+\])?:.*?|thread .* panicked at .*?|---- .* stdout ----.*?)(?=^\s*$)', text, re.M | re.S)
if not blocks:
    blocks = [text[-6000:]]
for block in blocks[:10]:
    message = block.strip()[:4000].replace('%', '%25').replace('\r', '').replace('\n', '%0A')
    print(f'::error title={sys.argv[2][:80]}::{message}')
PY
fi
exit "$status"
