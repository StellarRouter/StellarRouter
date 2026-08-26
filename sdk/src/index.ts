/**
 * Types shared across the SDK.
 */
export type { NetworkConfig, NetworkInput, NetworkName, QuoteRequest, QuoteResponse, RouteEntry, RouteMetadata, RouteMetadataInput } from "./types.js";

/** Error types and code tables. */
export { RouterSdkError } from "./errors.js";
export type { RouterSdkErrorOptions } from "./errors.js";
export { ROUTER_CORE_ERRORS, ROUTER_QUOTE_ERRORS, parseContractErrorCode } from "./errors.js";

/** Network presets and helpers. */
export { NETWORKS, resolveNetwork } from "./networks.js";

/** Signer abstraction (signing-abstraction.md). */
export { FreighterSigner, LocalSigner } from "./signer.js";
export type { Signer } from "./signer.js";

/** Low-level ScVal encoding helpers. */
export {
  toAddress,
  toBool,
  toI64,
  toI128,
  toMap,
  toNone,
  toSymbol,
  toStringVal,
  toU32,
  toU64,
  toVec,
  quoteRequestToScVal,
  routeMetadataToScVal,
  scValToNative,
} from "./scval.js";

/** Low-level contract invocation helpers. */
export { ContractInvoker } from "./contract.js";
export type { ContractInvokerOptions, InvokeOptions, SimulateOptions } from "./contract.js";
export type { RpcLike } from "./rpc.js";

/** router-core client. */
export { RouterCoreClient } from "./core.js";

/** router-quote client. */
export { RouterQuoteClient } from "./quote.js";

/** The high-level facade. */
export { RouterClient } from "./client.js";
export type { RouterClientOptions } from "./client.js";