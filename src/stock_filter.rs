//! `map_stock_events` — the equity subset of the enriched V4 tape.
//!
//! # Why this is a separate module rather than a filter on `map_events`
//!
//! `map_events` stays a complete, unfiltered V4 tape. That is deliberate: it is
//! what you debug against, and silently dropping rows upstream would make every
//! "why is this pool missing" question unanswerable. The filtering happens here,
//! downstream of `map_enriched`, so the two views coexist.
//!
//! # What survives the filter
//!
//! A pool whose two legs are both in `registry ∪ {WETH, USDG, native}`, with at
//! least one being a registry stock token. See [`crate::registry::is_stock_pool`]
//! for why both legs are checked rather than either.
//!
//! # Ordering dependency
//!
//! This runs after `store_pools` has joined `token0` / `token1` onto each row.
//! A V4 `Swap` log carries only the poolId — the currencies live in the PoolKey
//! that was hashed away at `Initialize` — so filtering before that join would
//! see empty token fields and drop everything.

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

    // Block-level aggregates are computed over the unfiltered tape upstream, so
    // carrying them here would attribute whole-chain totals to the equity
    // subset. Left empty rather than wrong: an absent number invites a lookup,
    // a wrong one does not.
    out
}

/// Attach registry identity and UI amounts to one swap.
fn annotate_swap(mut s: pb::Swap) -> pb::Swap {
    if let Some((sym, mult)) = registry::lookup(&s.token0) {
        s.token0_is_stock = true;
        s.registry_symbol = sym.to_string();
        s.ui_multiplier = mult.to_string();
        if let Some(ui) = registry::ui_amount(&s.amount0, mult) {
            s.amount0_ui = ui;
        }
    }
    if let Some((sym, mult)) = registry::lookup(&s.token1) {
        s.token1_is_stock = true;
        // A stock/stock pool has two tickers and one field. token0 wins, and
        // the flags remain the complete answer — registry_symbol is a
        // convenience, not the identity.
        if s.registry_symbol.is_empty() {
            s.registry_symbol = sym.to_string();
            s.ui_multiplier = mult.to_string();
        }
        if let Some(ui) = registry::ui_amount(&s.amount1, mult) {
            s.amount1_ui = ui;
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

    fn swap(t0: &str, t1: &str, a0: &str, a1: &str) -> pb::Swap {
        pb::Swap {
            token0: t0.to_string(),
            token1: t1.to_string(),
            amount0: a0.to_string(),
            amount1: a1.to_string(),
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
        let ev = pb::Events {
            swaps: vec![swap(CRWD, USDG, "17.006", "-4000")],
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
        assert_eq!(s.amount0, "17.006");
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
