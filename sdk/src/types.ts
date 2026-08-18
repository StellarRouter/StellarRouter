import type { xdr } from "@stellar/stellar-sdk";

/** A recognized Stellar network used by the SDK defaults. */
export type NetworkName = "testnet" | "futurenet" | "mainnet" | "local";

/** Full connection parameters for a network. */
export interface NetworkConfig {
  /** Soroban RPC endpoint URL. */
  rpcUrl: string;
  /** Network passphrase, used when building/parsing transactions. */
  networkPassphrase: string;
}

/** How the SDK talks to the network. Either a named preset or an explicit config. */
export type NetworkInput = NetworkName | NetworkConfig;

/** A route entry as stored in `router-core`. */
export interface RouteEntry {
  /** The contract address this route resolves to. */
  address: string;
  /** Human-readable route name. */
  name: string;
  /** Whether this specific route is paused. */
  paused: boolean;
  /** The address that last updated this route. */
  updatedBy: string;
}

/** Optional metadata attached to a registered route (router-core). */
export interface RouteMetadata {
  /** Human-readable description (max 256 chars). */
  description: string;
  /** Categorization tags (max 5). */
  tags: string[];
  /** Owner address (use the zero/contract address as sentinel for "no owner"). */
  owner: string;
}

/** Accepts a partial metadata payload; missing fields are filled with defaults. */
export type RouteMetadataInput = Partial<RouteMetadata>;

/** A quote request passed to `router-quote`. */
export interface QuoteRequest {
  /** Route name to get a quote for. */
  route: string;
  /** Input token contract address. */
  tokenIn: string;
  /** Output token contract address. */
  tokenOut: string;
  /** Amount of the input token (in base units). */
  amountIn: bigint;
}

/** A quote returned by `router-quote`. */
export interface QuoteResponse {
  /** Route name. */
  route: string;
  /** Input token contract address. */
  tokenIn: string;
  /** Output token contract address. */
  tokenOut: string;
  /** Amount of the input token (in base units). */
  amountIn: bigint;
  /** Expected output amount after fees (in base units). */
  amountOut: bigint;
  /** Fee amount deducted (in input token units). */
  feeAmount: bigint;
  /** Fee in basis points used for this quote. */
  feeBps: number;
  /** Price impact in basis points (`fee_amount * 10_000 / amount_in`). */
  priceImpactBps: bigint;
}

/** Per-entry outcome of `batch_resolve` on `router-core`. */
export type BatchResolveResult =
  | { ok: true; route: string; address: string }
  | { ok: false; route: string; error: ResolveErrorCode };

/** Resolution errors reported by `router-core`. */
export type ResolveErrorCode = "RouterPaused" | "RouteNotFound" | "RoutePaused";

/** Scoring attributes for a route used in path selection (router-core). */
export interface RouteScore {
  /** Liquidity depth score (0-100). Higher = more liquid. */
  liquidityScore: number;
  /** Fee rate in basis points (e.g. 30 = 0.30%). */
  feeBps: number;
  /** Historical reliability score (0-100). Higher = more reliable. */
  reliabilityScore: number;
}

/** A single route to register in a batch (router-core `register_routes_batch`). */
export interface RegisterRouteInput {
  /** Unique route name. */
  name: string;
  /** Contract address to resolve this route to. */
  address: string;
  /** Optional metadata attached to the route. */
  metadata?: RouteMetadataInput;
}

/** Status of a submitted transaction, from the RPC layer. */
export type TransactionStatus = "SUCCESS" | "FAILED" | "NOT_FOUND";

/** A Soroban `ScVal`, re-exported so consumers building raw invocation args
 * don't have to reach into `@stellar/stellar-sdk` themselves. */
export type ScVal = xdr.ScVal;