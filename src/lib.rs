//! Uniswap V4 on **Robinhood Chain** (Arbitrum Orbit L2, eip155:4663), filtered
//! to the official Robinhood Stock Token registry.
//!
//! Ported from `uniswap-v4-base`, which was itself a port of the
//! `uniswap-v4-base-3` subgraph (Qmbsc6XQWbiv4DfLVfaNciScqYLyDWUYjWzrFBbzzmRsMB).
//! The v4-core ABI is identical, so every decoder below is unchanged; what
//! differs is the addresses, the USD anchor, and the equity layer on top.
//!
//! # Module graph
//!
//! ```text
//!   sf.ethereum.type.v2.Block
//!            |
//!        map_events        decode PoolManager logs
//!         |      \
//!         |    store_pools      remember every Initialize, keyed pool:<poolId>
//!         |      /
//!       map_enriched       join the two: denormalise the PoolKey onto every
//!            |             swap / modify_liquidity row, emit PoolStats+HookStats
//!            |
//!       store_prices       native USD off the anchor pool, derived native per token
//!            |
//!        map_totals        + decimal-adjusted amounts, per-leg and tracked USD,
//!         |      \         and running PoolTotals / HookTotals
//!         |       \
//!     db_out    map_stock_events    the equity subset, with registry identity
//!                                   and ERC-8056 UI share counts
//! ```
//!
//! `PositionManager` and the Arrakis hook factory are decoded by `map_events` on
//! Base but are NOT wired here — neither is deployed on chain 4663 (`eth_getCode`
//! returns zero bytes at both Base addresses). The modules are kept in the tree
//! so the diff against upstream stays readable; see the bottom of `map_events`.
//!
//! ## Why the store exists at all
//!
//! A V4 `Swap` log carries the **poolId and nothing else about the pool**. The
//! tokens, the configured fee, the tick spacing and the hook are fields of the
//! `PoolKey`, which is keccak-hashed into the id at `initialize` and never
//! re-emitted. A consumer streaming from a recent block therefore cannot say
//! what a swap traded. The subgraph papers over this with `Pool.load(poolId)`
//! against graph-node's implicit entity store; Substreams has no implicit
//! state, so the join has to be an explicit store module. That is the
//! correctness fix this package carries, and it is why nothing downstream
//! consumes `map_events` directly.
//!
//! ## What is exposed to the engine
//!
//! `map_events`, `store_pools`, `map_enriched`, `store_tokens`, `store_prices`,
//! `store_pool_totals`, `store_hook_totals`, `map_totals`, `map_stock_events`
//! and `db_out` are declared in `substreams.yaml`. `map_events` is kept as a
//! public module and NOT folded into `map_enriched`: it is the cacheable,
//! store-free stage, so a consumer that only wants raw decoded logs pays
//! nothing for the join, and re-running the enrichment does not re-decode the
//! chain.
//!
//! Both `db_out` and `map_stock_events` read `map_totals`, not `map_enriched`.
//! `attach_usd()` runs inside `map_totals` and nowhere else, and it writes the
//! adjusted amounts and every USD field; reading `map_enriched` instead yields
//! rows with all of them silently empty.

pub mod registry;
pub mod stock_filter;
pub mod registry_data;
mod abi;
mod arrakis;
mod db_out;
mod enrich;
mod hooks;
mod pb;
mod pool_manager;
mod position_manager;
mod pricing;
mod stats_store;
mod store_pools;
mod tokens;

use substreams::errors::Error;
use substreams_ethereum::pb::eth::v2::Block;

use crate::pb::uniswap::v4::v1 as proto;

// Registers a custom getrandom that always errors. Without it, anything in the
// dependency tree that reaches for entropy fails to LINK on
// wasm32-unknown-unknown rather than failing at runtime.
substreams_ethereum::init!();

/// Single pass over the block, fanned out to the three contract extractors.
///
/// They share one `Events` accumulator instead of returning their own and
/// being merged: each already walks `blk.logs()` and appends to a distinct
/// repeated field, so there is nothing to reconcile and no intermediate
/// allocation.
///
/// Extractor order fixes the order of the repeated fields, but not the order
/// changes are applied downstream — `db_out` ordinals every row by its
/// block-scoped log index, so the sink replays a block in true chain order
/// regardless of how the events are grouped here.
///
/// Output of this module leaves `Swap`/`ModifyLiquidity` pool identity at
/// proto3 defaults; see `enrich` for why and where they are filled.
#[substreams::handlers::map]
pub fn map_events(blk: Block) -> Result<proto::Events, Error> {
    let mut events = proto::Events::default();

    pool_manager::extract(&blk, &mut events);
    // PositionManager and the Arrakis hook factory are NOT wired on Robinhood.
    //
    // Both constants in those modules are Base deployments. Calling them here
    // would be wrong in one of two ways: a no-op if nothing sits at those
    // addresses on this chain, or — worse, and silently — decoding whatever
    // unrelated contract happens to occupy them, since addresses are not
    // reserved across chains. Uniswap has not published a PositionManager for
    // Robinhood Chain; when it does, wire it exactly as Base does.
    //
    // The modules stay in the tree, compiled and tested, so re-enabling is a
    // one-line change rather than a port.
    let _ = &position_manager::extract;
    let _ = &arrakis::extract;

    Ok(events)
}
