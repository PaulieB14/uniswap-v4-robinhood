# uniswap-v4-robinhood

Enriched Uniswap **V4** on Robinhood Chain (Arbitrum Orbit L2, chain id **4663**),
filtered to the official Robinhood Stock Token registry.

Ported from [`uniswap-v4-base@v0.1.4`](https://github.com/PaulieB14/uniswap-v4-substreams).

## What this is

The equity subset of Uniswap V4 on Robinhood Chain, correctly identified and
correctly scaled.

## What this is not

**Not a raw-events clone.** Uniswap Labs already publishes
`uniswap-database-changes-pools-robinhood`, which has the undecorated v2/v3/v4
tape. If you want raw logs, use that.

**Not the Base package pointed at a new RPC.** Most DEX volume on this chain is
memecoins. A V4 port with no registry filter indexes noise.

What this adds on top of the raw tape: hook permission decoding from the low 14
bits of the hook address, the per-swap effective fee, the `ModifyLiquidity`
salt, and an explicit pool store — a V4 `Swap` log carries only the `poolId`,
because the tokens, fee, tickSpacing and hook live in the `PoolKey` that was
hashed away at `Initialize`. `store_pools` remembers every `Initialize` and
`map_enriched` joins it back onto every swap and liquidity row.

`map_totals` then adds decimal-adjusted amounts and USD values, and
`map_stock_events` keeps only the pools that are actually equity markets —
carrying that pricing through and attaching UI share counts.

## Modules

| Module | What it gives you |
|---|---|
| `map_events` | The complete, unfiltered V4 tape. Debug against this. |
| `map_enriched` | Same rows with tokens, symbols, fee tier, hook permissions joined on. |
| `map_totals` | `map_enriched` plus decimal-adjusted amounts, USD values and running totals. |
| **`map_stock_events`** | **The equity subset of `map_totals`, with ERC-8056 UI amounts. This is the point.** |
| `db_out` | Postgres sink over the enriched stream. |

## Running it

```bash
substreams run ./uniswap-v4-robinhood-v0.1.0.spkg map_stock_events \
  -e robinhood -s 9070 -t +50000 -o jsonl
```

`-e robinhood` (or `robinhood.substreams.pinax.network:443`). **Every module
starts at block 9070** — the block the PoolManager was deployed. A relative
stop (`-t +N`) needs an absolute start; `--start-block -1` with `+N` fails.

Do not run `db_out` or `map_enriched` from 9070 to head on a small quota.
`store_pools` must see every `Initialize` or the pools it missed are
permanently un-enriched, so a production backfill is long by construction. A
*local* test manifest may raise `initialBlock`; never ship one that does.

## Addresses

All verified against chain 4663 via `eth_getCode` / `symbol()` / `decimals()` on
2026-09-02, not copied from documentation.

| What | Address | |
|---|---|---|
| Uniswap V4 PoolManager | `0x8366a39CC670B4001A1121B8F6A443A643e40951` | 24,009 bytes of code; deployed block 9070 |
| WETH | `0x0Bd7D308f8E1639FAb988df18A8011f41EAcAD73` | `symbol() = WETH`, 18 decimals |
| USDG | `0x5fc5360D0400a0Fd4f2af552ADD042D716F1d168` | `symbol() = USDG`, **6 decimals** |
| Native sentinel | `0x0000000000000000000000000000000000000000` | V4 native currency |

**PositionManager and the Arrakis hook factory are not wired.** Uniswap has
published no PositionManager for this chain. The Base addresses are still in
`src/position_manager.rs` and `src/arrakis.rs`, marked BASE-ONLY and not called
from `map_events` — pointing them at a chain they were not deployed on would
either no-op or, worse, silently decode whatever unrelated contract occupies
those addresses.

## USD pricing

**Anchored.** `STABLECOIN_NATIVE_POOL_ID` is
`0xfcfae8fa0bd6da961bcf5d990f27690932deac4f093e99bf3e871691c6586593` — the
WETH/USDG pool at fee 500 / tickSpacing 10 / no hook, initialized at block
8,793,983.

That pool was not guessed. There are **134** WETH/USDG pools on this
PoolManager; over the 200k blocks to 52,625,973 this one took **917 swaps**
against 128 for the next busiest and zero for the rest. Its latest
`sqrtPriceX96` prices 1 WETH at **2,400.21 USDG**, which is the check that
matters — a bad anchor poisons every other price in the package, silently.

`STABLECOIN_IS_TOKEN0` is `false`: all 134 pools have `currency0 = WETH`.

**Filter on the `priced` flags, never on `amount_usd > 0`.** A genuine zero-value
swap and an unpriced leg are different things.

## Stock Tokens

Identity is the **contract address**. Never the ticker.

Searching the explorer for `GME` returns the official token *and* impersonators,
some flagged `is_verified_via_admin_panel: true`. Robinhood's own docs are
explicit: *"a token with a matching name/ticker but a different contract address
is not a Robinhood Stock Token."* Nothing in this package reads `symbol()`.

`registry/registry.json` holds 194 tokens, all `chainId: 4663`, snapshotted
2026-09-02. Regenerate:

```bash
./scripts/gen-registry.sh          # -> registry/registry.json
python3 scripts/gen-registry-rs.py # -> src/registry_data.rs
```

The API returns an **object with an `assets` key**, not a bare array — a `jq`
of `.[] | .deployments[]?` silently yields nothing.

### ERC-8056 UI amounts

A raw ERC-20 balance is not the share count after a corporate action. Each
asset carries a `currentMultiplier`, and:

```
ui_amount = amount_adjusted * multiplier          # amount_adjusted = -raw / 10^decimals
```

Two traps here, and each produces a plausible-looking wrong number.

**It is not `raw * multiplier`.** `amount0`/`amount1` are the raw int128s the
PoolManager emits; the decimals have not been divided out. Multiplying those
gives a raw-scaled integer, not a share count — a real 0.3185 CRWD swap comes
out as `1274147532818248372`. The input must be `amount0_adjusted`, which is
written by `attach_usd()` inside `map_totals`. That is why `map_stock_events`
reads `map_totals` and not `map_enriched`: on `map_enriched` the adjusted fields
are still empty, and the module would silently emit no UI amounts at all.

**It is not `… / 1e18`.** The registry publishes the multiplier as a plain
decimal written to 18 places — CRWD is `"4.000000000000000000"`, meaning
**4.0**, not 4e18. Reading those eighteen zeros as a fixed-point scale turns
17.006 CRWD into 6.8e-17 instead of 68.024 — wrong by exactly the factor you
were correcting for. AAPL's `"1.000566080061092436"` settles it: a small split
adjustment as a decimal, meaningless as a 1e18-scaled integer.

CRWD is the case to remember. The explorer shows 17.006 where the holder has
~68 shares. Raw amounts are preserved unchanged in `amount0` / `amount1`; UI
amounts are additive fields, and empty means *not a registry token*, never 1.0.

**Multipliers are current, not historical.** A swap dated before the last
multiplier change is scaled by today's multiplier, so this is **not
corporate-action-correct for historical dates**. The token contracts emit
`UIMultiplierUpdated`; decoding that per-token is the fix and is not done here.

**And "current" means as of the snapshot, which ages fast.** The multipliers are
compiled in from `registry/registry.json` (`SNAPSHOT_DATE`, currently
2026-09-02) because a substreams module is deterministic and cannot fetch at
runtime. They are not only corporate-action figures: dividend-accruing names
move continuously. `F` drifted from `1.000000000000000000` to
`1.000145502866134027` (+0.0146%) within two hours of this snapshot being taken.

So the UI amount is **exact for splits** — the failure that actually matters,
where a raw amount is wrong by hundreds of percent — and **approximate for
accruals**, with an error that grows with the age of the snapshot. Regenerate
via `scripts/gen-registry.sh` and re-publish to reset it; read `ui_multiplier`
on each row to see exactly what was applied.

## Tests

```bash
cargo test --lib     # 111 tests
substreams build
substreams pack substreams.yaml
```

The price maths tests are pinned to real numbers. Where Base fixtures were
chain-specific they were retargeted, not deleted — `ALT_TOKEN` replaces Base's
ZORA as "a whitelist token that is neither native nor a stablecoin", which on
this chain is a registry stock token.

The fee_tier vs `swap.fee` regression tests are kept. That bug is protocol
level, not Base specific.

## Known gaps

- **No module in this package has ever been executed against live Robinhood
  blocks.** Everything below the wasm boundary is verified — 111 unit tests, and
  every on-chain PoolManager `topic0` matched against the module's decoders to
  prove the v4-core ABI is unchanged from Base — but an end-to-end
  `substreams run` needs a streaming JWT that was not available while this was
  built. `map_stock_events` in particular has never seen a real block. Treat
  v0.1.0 as compile- and logic-verified, not runtime-verified.
- V4 event density here is **comparable to Base, not sparser**: 14 of 15 blocks
  sampled across ~1,450 recent blocks carried PoolManager logs (93%), averaging
  9.67 logs per block, against Base's 148/148. That is why this port keeps Base's
  decision not to ship a filtered-events index — at this density it would skip
  almost nothing. (Small sample: single-block `eth_getLogs` calls, because the
  public RPC caps range queries at 10,000 matches and rejects any window wide
  enough to be representative.)
- Historical multipliers, and snapshot drift on accruing names (both above).
- No PositionManager or Arrakis (above).
- `map_stock_events` emits no block-level aggregates. `pool_stats` / `hook_stats`
  are computed across the whole chain upstream, and carrying them into the
  filtered view would label whole-chain totals as equity totals.
