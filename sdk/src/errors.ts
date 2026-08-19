/**
 * Error model for `stellar-router-sdk`.
 *
 * All failures surface as {@link RouterSdkError} instances carrying a stable,
 * machine-readable `code` (e.g. `"RouteNotFound"`), mirroring the error names
 * returned by the on-chain contracts.
 */

/** @internal contract id label used in error messages. */
const CONTRACT_LABELS: Record<string, string> = {
  core: "router-core",
  quote: "router-quote",
};

/** Error codes exposed by `router-core` (`RouterError`). */
export const ROUTER_CORE_ERRORS: Readonly<Record<number, string>> = {
  1: "AlreadyInitialized",
  2: "NotInitialized",
  3: "Unauthorized",
  4: "RouteNotFound",
  5: "RoutePaused",
  6: "RouterPaused",
  7: "RouteAlreadyExists",
  8: "InvalidRouteName",
  9: "InvalidMetadata",
};

/** Error codes exposed by `router-quote` (`QuoteError`). */
export const ROUTER_QUOTE_ERRORS: Readonly<Record<number, string>> = {
  1: "AlreadyInitialized",
  2: "NotInitialized",
  3: "Unauthorized",
  4: "InvalidAmount",
  5: "InvalidFeeBps",
  6: "NoQuotesProvided",
  7: "RouteNotFound",
  8: "InvalidPriceImpactBps",
  9: "AmountOverflow",
};

/** Options accepted by {@link RouterSdkError}. */
export interface RouterSdkErrorOptions {
  /** The contract the failure originated from (e.g. `"core"`, `"quote"`). */
  contract?: "core" | "quote" | string;
  /** The on-chain numeric error code, when the failure is a contract error. */
  contractErrorCode?: number;
  /** The underlying cause, when the failure wrapped a thrown value. */
  cause?: unknown;
  /** The raw error text from the RPC layer, when available. */
  rawError?: string;
}

/**
 * The error type thrown by every SDK method.
 *
 * @example
 * try {
 *   await client.resolve("does-not-exist");
 * } catch (err) {
 *   if (err instanceof RouterSdkError) {
 *     console.log(err.code); // "RouteNotFound"
 *   }
 * }
 */
export class RouterSdkError extends Error {
  readonly code: string;
  readonly contract?: string;
  readonly contractErrorCode?: number;
  override readonly cause?: unknown;
  readonly rawError?: string;

  constructor(code: string, message: string, options: RouterSdkErrorOptions = {}) {
    super(message);
    this.name = "RouterSdkError";
    this.code = code;
    this.contract = options.contract;
    this.contractErrorCode = options.contractErrorCode;
    this.cause = options.cause;
    this.rawError = options.rawError;
  }

  /** Build an error from a numeric on-chain contract error code. */
  static fromContractCode(code: number, options: RouterSdkErrorOptions): RouterSdkError {
    const table = codeTableFor(options.contract);
    const name = table?.[code];
    const contractLabel = CONTRACT_LABELS[options.contract ?? ""] ?? options.contract ?? "contract";
    const message = name
      ? `${contractLabel} rejected the call with ${name} (#${code}).`
      : `${contractLabel} rejected the call with an unknown error code #${code}.`;
    return new RouterSdkError(name ?? `ContractError${code}`, message, {
      ...options,
      contractErrorCode: code,
    });
  }

  /** Wrap an unexpected thrown value (RPC/network failure) into a RouterSdkError. */
  static fromCause(contract: string, cause: unknown): RouterSdkError {
    const message = cause instanceof Error ? cause.message : String(cause);
    return new RouterSdkError("RpcError", `RPC call failed: ${message}`, {
      contract,
      cause,
    });
  }
}

function codeTableFor(contract?: string): Readonly<Record<number, string>> | undefined {
  if (contract === "core") return ROUTER_CORE_ERRORS;
  if (contract === "quote") return ROUTER_QUOTE_ERRORS;
  return undefined;
}

/**
 * Extract the on-chain contract error number from a raw RPC simulation error
 * string. Soroban renders failed contract errors like
 * `Error(Contract, #4)`, where `4` is the numeric `contracterror` discriminant.
 *
 * @returns the numeric code, or `undefined` when `raw` is not a contract error.
 */
export function parseContractErrorCode(raw: string | null | undefined): number | undefined {
  if (!raw) return undefined;
  const anchored = raw.match(/\(Contract,\s*#(\d+)\)/);
  if (anchored) {
    const n = Number(anchored[1]);
    if (Number.isInteger(n) && n > 0) return n;
  }
  const loose = raw.match(/Contract\D*#?(\d+)/);
  if (loose) {
    const n = Number(loose[1]);
    if (Number.isInteger(n) && n > 0) return n;
  }
  return undefined;
}