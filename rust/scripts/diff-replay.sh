#!/usr/bin/env bash
# Differential replay: the C# tool and the Rust port read the same capture; their CaptureReport JSON must match.
#
#   rust/scripts/diff-replay.sh <capture.pcap>...   compare these captures
#   rust/scripts/diff-replay.sh --synthetic          generate the synthetic captures (examples/synth.rs) and compare them
#
# Normalisation: object keys sorted, numbers compared as doubles (C# writes 100, serde 100.0), and every random
# 32-hex id (Guid.NewGuid "N" ids of records and parties) replaced by its order of first appearance.
# Exit status: 0 when every capture matches, 1 otherwise. Set KEEP=1 to keep the JSON next to each capture.
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
export PATH="$HOME/.cargo/bin:$HOME/.dotnet:$PATH"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$root/rust/target/rust-core}"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

captures=("$@")
if [[ ${#captures[@]} -eq 0 || ${captures[0]} == --synthetic ]]; then
  (cd "$root/rust" && cargo run -q -p meter-core --example synth -- "$work/synthetic")
  captures=("$work"/synthetic/*.pcap)
fi

(cd "$root/rust" && cargo build -q -p meter-core --example replay)
dotnet build -v q --nologo "$root/tools/AlbionMeter.Capture" >/dev/null

normalise() {
  python3 -I - "$1" <<'PY'
import json, re, sys
ids = {}
pattern = re.compile(r"^[0-9a-f]{32}$")
def walk(value):
    if isinstance(value, dict):
        return {key: walk(value[key]) for key in sorted(value)}
    if isinstance(value, list):
        return [walk(item) for item in value]
    if isinstance(value, bool) or value is None:
        return value
    if isinstance(value, (int, float)):
        return repr(float(value))
    if isinstance(value, str) and pattern.match(value):
        return ids.setdefault(value, f"<id {len(ids)}>")
    return value
print(json.dumps(walk(json.load(open(sys.argv[1]))), indent=1, ensure_ascii=False))
PY
}

status=0
for capture in "${captures[@]}"; do
  name="$(basename "$capture")"
  dotnet run --no-build --project "$root/tools/AlbionMeter.Capture" -- replay "$capture" >"$work/$name.cs.json"
  "$CARGO_TARGET_DIR/debug/examples/replay" replay "$capture" >"$work/$name.rs.json"
  normalise "$work/$name.cs.json" >"$work/$name.cs.norm"
  normalise "$work/$name.rs.json" >"$work/$name.rs.norm"
  if diff -u "$work/$name.cs.norm" "$work/$name.rs.norm" >"$work/$name.diff"; then
    echo "MATCH  $name"
  else
    echo "DIFFER $name ($(grep -c '^[-+][^-+]' "$work/$name.diff") lines)"
    head -n 80 "$work/$name.diff"
    status=1
  fi
  if [[ -n ${KEEP:-} ]]; then cp "$work/$name".{cs,rs}.json "$(dirname "$capture")/"; fi
done
exit $status
