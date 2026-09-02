#!/usr/bin/env python3
"""Generate src/registry_data.rs from registry/registry.json. Do not hand-edit
the output. Run scripts/gen-registry.sh first."""
import json, pathlib
root = pathlib.Path(__file__).resolve().parent.parent
d = json.loads((root / "registry" / "registry.json").read_text())
toks = d["tokens"]
out = [
    "//! Official Robinhood Stock Token registry for chain 4663 — GENERATED.",
    "//!",
    "//! Source: https://api.robinhood.com/rhj/assets (an object with an `assets`",
    "//! key, not a bare array). Regenerate with scripts/gen-registry.sh then",
    "//! scripts/gen-registry-rs.py. Do not hand-edit.",
    "//!",
    f"//! Snapshot: {d['count']} tokens, all chainId 4663, all 18 decimals.",
    "//!",
    "//! Identity is the CONTRACT ADDRESS. Ticker search on the explorer returns",
    "//! impersonators — some flagged verified — so a symbol match is never used.",
    "",
    "/// (address, ticker, currentMultiplier as a plain decimal string)",
    "pub const STOCK_TOKENS: &[(&str, &str, &str)] = &[",
]
for t in toks:
    out.append(f'    ("{t["address"]}", "{t["symbol"]}", "{t["current_multiplier"]}"),')
out += ["];", ""]
(root / "src" / "registry_data.rs").write_text("\n".join(out))
print(f"wrote src/registry_data.rs ({len(toks)} tokens)")
