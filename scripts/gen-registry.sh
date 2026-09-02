#!/usr/bin/env bash
# Regenerate registry/registry.json from the official Robinhood asset API.
#
# The payload is an OBJECT with an `assets` key, not a bare array — a jq of
# `.[] | .deployments[]?` silently yields nothing. Inspect before you filter.
set -euo pipefail
OUT="$(dirname "$0")/../registry/registry.json"
curl -sS --max-time 60 https://api.robinhood.com/rhj/assets \
  | jq '{
      generated_from: "https://api.robinhood.com/rhj/assets",
      chain_id: 4663,
      count: ([.assets[] | .deployments[]? | select(.chainId == 4663)] | length),
      tokens: [
        .assets[]
        | . as $a
        | .deployments[]?
        | select(.chainId == 4663)
        | {
            address: (.contractAddress | ascii_downcase),
            symbol: $a.tokenSymbol,
            name: $a.tokenName,
            decimals: $a.tokenDecimals,
            current_multiplier: $a.currentMultiplier,
            isin: $a.isin,
            uid: $a.id,
            status: $a.status
          }
      ] | sort_by(.symbol)
    }' > "$OUT"
echo "wrote $OUT ($(jq -r '.count' "$OUT") tokens on chain 4663)"
