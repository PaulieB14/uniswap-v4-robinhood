//! `map_stock_events` — the equity subset of the enriched V4 tape.
//!
//! # Why this is a separate module rather than a filter on `map_events`
//!
//! `map_events` stays a complete, unfiltered V4 tape. That is deliberate: it is
//! what you debug against, and silently dropping rows upstream would make every
//! "why is this pool missing" question unanswerable. The filtering happens here,
//! downstream of `map_totals`, so the two views coexist.
//!
//! # What survives the filter
//!
//! A pool whose two legs are both in `registry ∪ {WETH, USDG, native}`, with at
//! least one being a registry stock token. See [`crate::registry::is_stock_pool`]
//! for why both legs are checked rather than either.
//!
//! # Ordering dependencies
//!
//! Two, and both are silent when violated:
//!
//! 1. This runs after `store_pools` has joined `token0` / `token1` onto each
//!    row. A V4 `Swap` log carries only the poolId — the currencies live in the
//!    PoolKey that was hashed away at `Initialize` — so filtering before that
//!    join would see empty token fields and drop *everything*.
//! 2. The input must be `map_totals`, not `map_enriched`. `attach_usd()` runs
//!    inside `map_totals` and nowhere else, and it is what writes both
//!    `amountN_adjusted` (which the UI amounts are derived from) and the USD
//!    fields `amountN_usd` / `amount_usd` / `priced`. Reading `map_enriched`
//!    instead yields an equity tape with neither — silently, and with no error.

use crate::pb::uniswap::v4::v1 as pb;
use crate::registry;

/// Keep only equity pools, and attach ERC-8056 UI amounts to their swaps.
#[substreams::handlers::map]
pub fn map_stock_events(events: pb::Events) -> Result<pb::Events, substreams::errors::Error> {
    Ok(filter_stock_events(events))
}

/// The filter itself, separate from the handler.
///
/// `#[substreams::handlers::map]` rewrites the signature it decorates, so a
/// test cannot call the handler directly. Keeping the logic in a plain function
/// means the behaviour that matters is testable without a substreams runtime.
pub fn filter_stock_events(events: pb::Events) -> pb::Events {
    let mut out = pb::Events::default();

    out.pools = events
        .pools
        .into_iter()
        .filter(|p| registry::is_stock_pool(&p.token0, &p.token1))
        .collect();

    out.swaps = events
        .swaps
        .into_iter()
        .filter(|s| registry::is_stock_pool(&s.token0, &s.token1))
        .map(annotate_swap)
        .collect();

    out.modify_liquidity = events
        .modify_liquidity
        .into_iter()
        .filter(|m| registry::is_stock_pool(&m.token0, &m.token1))
        .collect();

    // Per-pool and per-block, with token0/token1 denormalised on, so these
    // filter exactly like the rows above and stay correct for the equity subset.
    // PoolStats is built from THIS block's own events (see enrich.rs), so a
    // filtered set is a true equity-only block aggregate, not a slice of a
    // whole-chain number.
    out.pool_stats = events
        .pool_stats
        .into_iter()
        .filter(|p| registry::is_stock_pool(&p.token0, &p.token1))
        .collect();

    out.donates = events
        .donates
        .into_iter()
        .filter(|d| registry::is_stock_pool(&d.token0, &d.token1))
        .collect();

    // Everything else is deliberately left empty, for three different reasons.
    //
    // `hook_stats` aggregates per HOOK across every pool that hook serves, and
    // those pools are not all equity. There is no honest way to filter it: the
    // hook's swap_count is not decomposable from here, so a filtered row would
    // be a whole-chain number wearing an equity label.
    //
    // `pool_totals` / `hook_totals` are LIFETIME totals read back out of the
    // add-policy stores, which were accumulated over the unfiltered tape. Same
    // problem, permanently.
    //
    // `protocol_fee_events` and `claim_token_events` carry no token pair —
    // ProtocolFeeEvent has a pool_id but no currencies, and ClaimTokenEvent is
    // ERC-6909 accounting keyed by currency_id with no pool at all — so equity
    // membership is not decidable here without a second join this module does
    // not have. `position_events` and `hook_deployments` are always empty on
    // this chain (neither contract is deployed).
    //
    // Absent rather than wrong: a missing number invites a lookup, a wrong one
    // does not.
    out
}

/// Attach registry identity and UI amounts to one swap.
///
/// The UI amount is built from `amountN_adjusted`, NOT from `amountN`.
/// `amountN` is the raw int128 the PoolManager emitted; multiplying that by the
/// multiplier yields a raw-scaled integer, not a share count — for CRWD at 18
/// decimals it overstates the answer by 1e18 (0.3185 CRWD came out as
/// 1274147532818248372). `amountN_adjusted` has already had `10^decimals`
/// divided out, so it is the only correct input here.
///
/// That also means the UI amount inherits `amounts_adjusted`: when decimals are
/// unknown the adjusted value is absent, and an absent UI amount is correct.
/// It likewise inherits the POOL-CENTRIC sign of the adjusted fields, which is
/// the opposite of the raw swapper-centric one.
fn annotate_swap(mut s: pb::Swap) -> pb::Swap {
    let adjusted = s.amounts_adjusted;
    if let Some((sym, mult)) = registry::lookup(&s.token0) {
        s.token0_is_stock = true;
        s.registry_symbol = sym.to_string();
        s.ui_multiplier = mult.to_string();
        if adjusted {
            if let Some(ui) = registry::ui_amount(&s.amount0_adjusted, mult) {
                s.amount0_ui = ui;
            }
        }
    }
    if let Some((sym, mult)) = registry::lookup(&s.token1) {
        s.token1_is_stock = true;
        // A stock/stock pool has two tickers and one field. token0 wins, and
        // the flags remain the complete answer — registry_symbol is a
        // convenience, not the identity.
        //
        // Same for ui_multiplier: amount1_ui below is scaled by token1's own
        // multiplier, so on a stock/stock pool the reported ui_multiplier
        // describes amount0_ui only. Documented on the proto field.
        if s.registry_symbol.is_empty() {
            s.registry_symbol = sym.to_string();
            s.ui_multiplier = mult.to_string();
        }
        if adjusted {
            if let Some(ui) = registry::ui_amount(&s.amount1_adjusted, mult) {
                s.amount1_ui = ui;
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const NVDA: &str = "0xd0601ce157db5bdc3162bbac2a2c8af5320d9eec";
    const CRWD: &str = "0xea72ecca2d0f6bfa1394dbbcff85b52cd4233931";
    const USDG: &str = "0x5fc5360d0400a0fd4f2af552add042d716f1d168";
    const MEME: &str = "0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef";

    /// A swap as `map_totals` hands it over: raw int128s AND the decimal-adjusted
    /// values `attach_usd()` derives from them.
    ///
    /// Both spellings are set because the module reads the adjusted pair and the
    /// raw pair must survive untouched. A fixture that set only `amount0` is what
    /// let the raw-vs-adjusted bug pass its own test — the code read `amount0`,
    /// the fixture filled `amount0`, and the assertion matched a number that was
    /// 1e18 too large on real data.
    fn swap(t0: &str, t1: &str, a0: &str, a1: &str) -> pb::Swap {
        swap_adjusted(t0, t1, a0, a0, a1, a1)
    }

    /// The same, with the adjusted values stated independently of the raw ones —
    /// which is how real rows look, since adjusting divides by `10^decimals` and
    /// flips the sign.
    fn swap_adjusted(
        t0: &str,
        t1: &str,
        a0: &str,
        a0_adj: &str,
        a1: &str,
        a1_adj: &str,
    ) -> pb::Swap {
        pb::Swap {
            token0: t0.to_string(),
            token1: t1.to_string(),
            amount0: a0.to_string(),
            amount1: a1.to_string(),
            amount0_adjusted: a0_adj.to_string(),
            amount1_adjusted: a1_adj.to_string(),
            amounts_adjusted: true,
            ..Default::default()
        }
    }

    #[test]
    fn keeps_equity_pools_and_drops_noise() {
        let ev = pb::Events {
            swaps: vec![
                swap(NVDA, USDG, "1", "-100"),
                swap(MEME, USDG, "1", "-1"),
                swap(NVDA, MEME, "1", "-1"),
            ],
            ..Default::default()
        };
        let out = filter_stock_events(ev);
        assert_eq!(out.swaps.len(), 1, "only NVDA/USDG is an equity market");
        assert_eq!(out.swaps[0].token0, NVDA);
    }

    #[test]
    fn crwd_gets_its_four_times_ui_amount() {
        // The case that makes this package worth more than the raw tape.
        // Raw is the int128 the PoolManager emitted; adjusted is what
        // attach_usd() derives (-raw / 10^18, hence the sign flip). The UI
        // amount must come from the adjusted one.
        let ev = pb::Events {
            swaps: vec![swap_adjusted(
                CRWD,
                USDG,
                "-17006000000000000000",
                "17.006",
                "4000000000",
                "-4000",
            )],
            ..Default::default()
        };
        let out = filter_stock_events(ev);
        let s = &out.swaps[0];
        assert!(s.token0_is_stock);
        assert!(!s.token1_is_stock, "USDG is a quote leg, not a stock");
        assert_eq!(s.registry_symbol, "CRWD");
        assert_eq!(s.ui_multiplier, "4.000000000000000000");
        assert!(s.amount0_ui.starts_with("68.02"), "got {}", s.amount0_ui);
        // Raw amounts survive untouched — UI is additive, not a replacement.
        assert_eq!(s.amount0, "-17006000000000000000");
    }

    #[test]
    fn per_block_pool_stats_and_donates_are_filtered_not_dropped() {
        // Both carry token0/token1 and are computed from this block's own
        // events, so the equity subset of them is a true number. Dropping them
        // (as an earlier version did, on the rationale that applies to the
        // LIFETIME totals) threw away correct data.
        let out = filter_stock_events(pb::Events {
            pool_stats: vec![
                pb::PoolStats { pool_id: "a".into(), token0: NVDA.into(), token1: USDG.into(),
                                swap_count: 3, ..Default::default() },
                pb::PoolStats { pool_id: "b".into(), token0: MEME.into(), token1: USDG.into(),
                                swap_count: 9, ..Default::default() },
            ],
            donates: vec![
                pb::Donate { id: "d1".into(), token0: NVDA.into(), token1: USDG.into(),
                             ..Default::default() },
                pb::Donate { id: "d2".into(), token0: MEME.into(), token1: USDG.into(),
                             ..Default::default() },
            ],
            ..Default::default()
        });
        assert_eq!(out.pool_stats.len(), 1, "the MEME pool's stats must not survive");
        assert_eq!(out.pool_stats[0].swap_count, 3);
        assert_eq!(out.donates.len(), 1);
        assert_eq!(out.donates[0].id, "d1");
    }

    #[test]
    fn whole_chain_aggregates_are_still_dropped() {
        // hook_stats spans every pool a hook serves, and the *_totals are
        // lifetime figures accumulated over the unfiltered tape. Neither is
        // decomposable to the equity subset, so both must stay absent.
        let out = filter_stock_events(pb::Events {
            swaps: vec![swap(NVDA, USDG, "1", "-100")],
            hook_stats: vec![pb::HookStats { hook_address: "0xhook".into(),
                                             swap_count: 999, ..Default::default() }],
            ..Default::default()
        });
        assert_eq!(out.swaps.len(), 1, "the equity swap still comes through");
        assert!(out.hook_stats.is_empty(), "a hook's whole-chain count is not an equity count");
    }

    #[test]
    fn a_stock_stock_pool_scales_each_leg_by_its_own_multiplier() {
        // CRWD (4.0) against NVDA (1.0). Each leg must use its own multiplier;
        // reusing token0's for both would quadruple the NVDA side.
        let out = filter_stock_events(pb::Events {
            swaps: vec![swap_adjusted(CRWD, NVDA, "-1", "17.006", "1", "-2.5")],
            ..Default::default()
        });
        let s = &out.swaps[0];
        assert!(s.token0_is_stock && s.token1_is_stock);
        assert!(s.amount0_ui.starts_with("68.02"), "CRWD x4: {}", s.amount0_ui);
        assert!(s.amount1_ui.starts_with("-2.5"), "NVDA x1: {}", s.amount1_ui);
        // token0 wins the single reporting field, so it describes amount0_ui
        // and NOT amount1_ui. This is the documented limitation, asserted so a
        // future change to the tie-break cannot pass silently.
        assert_eq!(s.registry_symbol, "CRWD");
        assert_eq!(s.ui_multiplier, "4.000000000000000000");
    }

    #[test]
    fn usd_pricing_survives_the_filter() {
        // The other half of the map_totals dependency: equity rows must keep
        // the USD figures attach_usd() computed. A filter that rebuilt Swap
        // field-by-field, or that read map_enriched, would drop these and leave
        // every equity row unpriced.
        let mut sw = swap_adjusted(NVDA, USDG, "-1000", "1.0", "2000", "-2.0");
        sw.amount_usd = "180.42".to_string();
        sw.priced = true;
        sw.amount0_usd = "180.42".to_string();
        sw.amount0_priced = true;
        sw.native_price_usd = "2400.21".to_string();

        let out = filter_stock_events(pb::Events { swaps: vec![sw], ..Default::default() });
        let s = &out.swaps[0];
        assert!(s.priced, "equity rows must stay priced");
        assert_eq!(s.amount_usd, "180.42");
        assert_eq!(s.amount0_usd, "180.42");
        assert_eq!(s.native_price_usd, "2400.21");
    }

    #[test]
    fn no_ui_amount_when_the_upstream_left_amounts_unadjusted() {
        // The map_enriched-vs-map_totals wiring trap. If this module is fed a
        // stage that has not run attach_usd(), amountN_adjusted is empty and
        // amounts_adjusted is false. The right behaviour is to emit NO ui
        // amount — not to fall back to the raw int128, which is the same number
        // multiplied by 10^decimals and looks entirely plausible.
        let ev = pb::Events {
            swaps: vec![pb::Swap {
                token0: CRWD.to_string(),
                token1: USDG.to_string(),
                amount0: "-17006000000000000000".to_string(),
                amount1: "4000000000".to_string(),
                amounts_adjusted: false,
                ..Default::default()
            }],
            ..Default::default()
        };
        let s = &filter_stock_events(ev).swaps[0];
        assert!(s.token0_is_stock, "identity still resolves");
        assert_eq!(s.registry_symbol, "CRWD");
        assert!(
            s.amount0_ui.is_empty(),
            "must not derive a UI amount from the raw int128, got {}",
            s.amount0_ui
        );
    }

    #[test]
    fn a_quote_leg_gets_no_ui_amount() {
        // Empty is not 1.0. A consumer must be able to tell "not a stock token"
        // from "multiplier of one".
        let ev = pb::Events {
            swaps: vec![swap(NVDA, USDG, "1", "-100")],
            ..Default::default()
        };
        let out = filter_stock_events(ev);
        assert!(out.swaps[0].amount1_ui.is_empty());
    }

    #[test]
    fn block_aggregates_are_not_carried_over() {
        // They are computed across the whole chain upstream; copying them here
        // would label whole-chain totals as equity totals.
        let ev = pb::Events {
            swaps: vec![swap(NVDA, USDG, "1", "-1")],
            pool_stats: vec![Default::default()],
            hook_stats: vec![Default::default()],
            ..Default::default()
        };
        let out = filter_stock_events(ev);
        assert!(out.pool_stats.is_empty());
        assert!(out.hook_stats.is_empty());
    }
}
