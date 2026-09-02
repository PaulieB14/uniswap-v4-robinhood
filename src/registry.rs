//! The Robinhood Stock Token registry, and the equity filter built on it.
//!
//! # Why this module is the product
//!
//! Uniswap Labs already publishes the raw v2/v3/v4 tape for this chain. A V4
//! port with no registry filter would mostly index memecoins, because that is
//! where most DEX volume on Robinhood Chain is. The thing nobody else ships is
//! *the equity subset, correctly identified and correctly scaled*.
//!
//! # Identity is the address, never the ticker
//!
//! Searching the explorer for `GME` returns the official token and several
//! impersonators, some of them `is_verified_via_admin_panel: true`. Robinhood's
//! own docs say it plainly: "a token with a matching name/ticker but a different
//! contract address is not a Robinhood Stock Token." So nothing here reads
//! `symbol()`. Membership is an address set, and the ticker is derived from it.
//!
//! # ERC-8056 scaled amounts
//!
//! A Stock Token's raw ERC-20 balance is not the share count after a corporate
//! action. Each asset carries a `currentMultiplier` and the UI amount is
//! `raw * multiplier / 1e18`. CRWD's is exactly 4.0 — the explorer shows 17.006
//! where the holder has ~68 shares — so publishing raw amounts alone is a 4x
//! error on that ticker. Raw values are preserved; UI values are additive.

use crate::registry_data::STOCK_TOKENS;
use substreams::scalar::BigDecimal;
use std::str::FromStr;

/// Quote legs allowed opposite a stock token. WETH and USDG are the cash and
/// native sides of the equity pools; address(0) is V4's native sentinel.
pub const WETH: &str = "0x0bd7d308f8e1639fab988df18a8011f41eacad73";
pub const USDG: &str = "0x5fc5360d0400a0fd4f2af552add042d716f1d168";
pub const NATIVE: &str = "0x0000000000000000000000000000000000000000";

/// Is this address an official RHJ Stock Token?
pub fn is_stock(addr: &str) -> bool {
    lookup(addr).is_some()
}

/// Registry entry for an address: `(ticker, multiplier)`.
///
/// Linear scan over 194 entries. A HashSet would need building per module
/// invocation, which for a set this size costs more than the scan it saves.
pub fn lookup(addr: &str) -> Option<(&'static str, &'static str)> {
    let a = addr.trim().to_ascii_lowercase();
    STOCK_TOKENS
        .iter()
        .find(|(addr, _, _)| *addr == a)
        .map(|(_, sym, mult)| (*sym, *mult))
}

/// Is this address acceptable as the *other* leg of an equity pool?
/// Three addresses, chosen because each is independently priceable: native and
/// WETH are the pricing base by definition, and USDG is the configured
/// stablecoin, so a stock paired with any of them yields a USD value.
///
/// # The known omission
///
/// Classifying all 45,827 registry-touching `Initialize` events on this chain
/// with the rule below keeps 6,785 pools (USDG 4,776 / native 1,396 / WETH 517
/// / stock-stock 96) and drops 39,042. The largest single dropped counterparty
/// is `0x0ff7a742…` — `symbol()` "wUSDG", `name()` "Wrapped Global Dollar",
/// `decimals()` 6, an ERC-1967 proxy — appearing in 574 dropped pools.
///
/// It is deliberately NOT admitted here. Adding it would let those 574 pools
/// through, but a quote leg is only useful if it prices, and wUSDG has not been
/// shown to hold 1:1 against USDG. Admitting it as a stablecoin without that
/// proof would mint USD figures from an unverified peg; admitting it as a plain
/// quote leg would pass pools through that then go unpriced. Verify the wrapper
/// first, then decide which of the two it is.
pub fn is_quote_leg(addr: &str) -> bool {
    let a = addr.trim().to_ascii_lowercase();
    a == WETH || a == USDG || a == NATIVE
}

/// Does this pool belong in the equity feed?
///
/// Both legs must be in `registry ∪ {WETH, USDG, native}`, and at least one
/// must be a stock token. Requiring both is deliberate: a NVDA/memecoin pool
/// touches the registry but is not an equity market, and letting it through
/// would put this package back in the business of indexing noise.
pub fn is_stock_pool(token0: &str, token1: &str) -> bool {
    let t0_stock = is_stock(token0);
    let t1_stock = is_stock(token1);
    if !t0_stock && !t1_stock {
        return false;
    }
    (t0_stock || is_quote_leg(token0)) && (t1_stock || is_quote_leg(token1))
}

/// `raw * multiplier`, as a decimal string.
///
/// The registry publishes `currentMultiplier` as a PLAIN DECIMAL written to 18
/// places — CRWD is `"4.000000000000000000"`, meaning 4.0, not 4e18. It is easy
/// to read those eighteen zeros as a fixed-point scale and divide by 1e18; that
/// turns 17.006 CRWD into 6.8e-17 instead of 68.024, which is wrong by the
/// exact factor you were trying to correct for. AAPL's
/// `"1.000566080061092436"` settles it: as a plain decimal that is a small
/// split adjustment, and as a 1e18-scaled value it is meaningless.
///
/// BigDecimal throughout. An f64 would lose precision on an 18-decimal token
/// amount before the multiplier was even applied, and this number is a share
/// count someone may reconcile against a brokerage statement.
pub fn ui_amount(raw: &str, multiplier: &str) -> Option<String> {
    let raw = BigDecimal::from_str(raw.trim()).ok()?;
    let mult = BigDecimal::from_str(multiplier.trim()).ok()?;
    Some((raw * mult).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Addresses checked against the live registry API on 2026-09-02, and
    // against eth_getCode / symbol() / decimals() on chain 4663.
    const NVDA: &str = "0xd0601ce157db5bdc3162bbac2a2c8af5320d9eec";
    const CRWD: &str = "0xea72ecca2d0f6bfa1394dbbcff85b52cd4233931";
    const GME: &str = "0x1b0e319c6a659f002271b69db8a7df2f911c153e";
    const MEME: &str = "0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef";

    #[test]
    fn registry_is_populated() {
        assert_eq!(STOCK_TOKENS.len(), 194);
    }

    #[test]
    fn every_registry_address_is_lowercase_and_well_formed() {
        // The lookup lowercases its input but not the table; a mixed-case entry
        // would be permanently unmatchable.
        for (addr, sym, _) in STOCK_TOKENS {
            assert_eq!(*addr, addr.to_ascii_lowercase(), "{sym} is not lowercased");
            assert_eq!(addr.len(), 42, "{sym} is not a 20-byte address");
            assert!(addr.starts_with("0x"), "{sym} missing 0x");
        }
    }

    #[test]
    fn known_tickers_resolve_by_address() {
        assert_eq!(lookup(NVDA).map(|(s, _)| s), Some("NVDA"));
        assert_eq!(lookup(GME).map(|(s, _)| s), Some("GME"));
        assert_eq!(lookup(MEME), None);
    }

    #[test]
    fn lookup_is_case_insensitive() {
        assert!(is_stock(&NVDA.to_ascii_uppercase().replace("0X", "0x")));
    }

    #[test]
    fn stock_pool_requires_a_clean_counterparty() {
        assert!(is_stock_pool(NVDA, USDG), "NVDA/USDG is the core case");
        assert!(is_stock_pool(WETH, GME), "order must not matter");
        assert!(is_stock_pool(NVDA, CRWD), "stock/stock is still equity");
        assert!(is_stock_pool(NATIVE, NVDA), "native is a valid quote leg");
        // The case this filter exists for.
        assert!(!is_stock_pool(NVDA, MEME), "NVDA/memecoin is not an equity market");
        assert!(!is_stock_pool(WETH, USDG), "the anchor pool holds no equity");
        assert!(!is_stock_pool(MEME, MEME), "pure noise");
    }

    #[test]
    fn crwd_multiplier_is_the_four_times_case() {
        // The whole reason UI amounts exist. If this ever reads 1.0, the
        // registry snapshot is stale and CRWD amounts are understated 4x.
        let (_, mult) = lookup(CRWD).expect("CRWD in registry");
        assert_eq!(mult, "4.000000000000000000");
        // 17.006 raw is ~68 shares, which is the discrepancy the explorer shows.
        // 17.006 raw * 4.0 = 68.024 — the discrepancy the explorer shows.
        let ui = ui_amount("17.006", mult).unwrap();
        assert!(ui.starts_with("68.02"), "expected ~68.024, got {ui}");
    }

    #[test]
    fn a_unit_multiplier_leaves_the_amount_alone() {
        let (_, mult) = lookup(NVDA).expect("NVDA in registry");
        assert_eq!(mult, "1.000000000000000000");
        assert_eq!(ui_amount("5", mult).unwrap().trim_end_matches(['0', '.']), "5");
    }

    #[test]
    fn ui_amount_handles_a_signed_amount() {
        // V4 swap amounts are signed; a sell leg must scale too.
        let out = ui_amount("-17.006", "4.000000000000000000").unwrap();
        assert!(out.starts_with("-68.02"), "got {out}");
    }

    #[test]
    fn ui_amount_rejects_garbage_rather_than_guessing() {
        assert_eq!(ui_amount("not-a-number", "1.0"), None);
        assert_eq!(ui_amount("1.0", ""), None);
    }
}
