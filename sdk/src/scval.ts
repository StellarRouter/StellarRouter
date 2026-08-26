import { Address, ScInt, scValToNative, xdr } from "@stellar/stellar-sdk";

/**
 * ScVal <-> JS helpers.
 *
 * The contract suite uses a fixed set of scalar and struct types; these
 * helpers encode those exactly (i128/i64/u64/u32/Address/Symbol), so an SDK
 * consumer never has to hand-roll `xdr.ScVal` for the supported API surface.
 */

const U32_UPPER = 0xffffffff;

/** Encode a non-negative 32-bit integer as `scvU32`. */
export function toU32(n: number): xdr.ScVal {
  if (!Number.isInteger(n) || n < 0 || n > U32_UPPER) {
    throw new RangeError(`toU32 expects an integer in [0, ${U32_UPPER}], got ${n}`);
  }
  return xdr.ScVal.scvU32(n);
}

/** Encode a value as `scvU64`. */
export function toU64(n: number | bigint): xdr.ScVal {
  return new ScInt(BigInt(n)).toU64();
}

/** Encode a value as `scvI64`. */
export function toI64(n: number | bigint): xdr.ScVal {
  return new ScInt(BigInt(n)).toI64();
}

/** Encode a value as `scvI128`. */
export function toI128(n: number | bigint): xdr.ScVal {
  return new ScInt(BigInt(n)).toI128();
}

/** Encode a JS string as `scvString`. */
export function toStringVal(s: string): xdr.ScVal {
  return xdr.ScVal.scvString(s);
}

/** Encode a symbol as `scvSymbol`. */
export function toSymbol(s: string): xdr.ScVal {
  return xdr.ScVal.scvSymbol(s);
}

/** Encode a boolean as `scvBool`. */
export function toBool(b: boolean): xdr.ScVal {
  return xdr.ScVal.scvBool(b);
}

/** Encode a Stellar/contract address as `scvAddress`. */
export function toAddress(address: string): xdr.ScVal {
  return new Address(address).toScVal();
}

/** Encode the absence of a value as `scvVoid` (Soroban `None`). */
export function toNone(): xdr.ScVal {
  return xdr.ScVal.scvVoid();
}

/** Encode an array of ScVals as `scvVec`. */
export function toVec(items: xdr.ScVal[]): xdr.ScVal {
  return xdr.ScVal.scvVec(items);
}

/** Encode an object of ScVals keyed by symbols as `scvMap` (a struct). */
export function toMap(entries: Record<string, xdr.ScVal>): xdr.ScVal {
  return xdr.ScVal.scvMap(
    Object.entries(entries).map(
      ([key, value]) => new xdr.ScMapEntry({ key: toSymbol(key), val: value }),
    ),
  );
}

/**
 * Encode a {@link QuoteRequest} into the `QuoteRequest` struct consumed by
 * `router-quote`.
 */
export function quoteRequestToScVal(req: {
  route: string;
  tokenIn: string;
  tokenOut: string;
  amountIn: bigint;
}): xdr.ScVal {
  return toMap({
    route: toStringVal(req.route),
    token_in: toAddress(req.tokenIn),
    token_out: toAddress(req.tokenOut),
    amount_in: toI128(req.amountIn),
  });
}

/**
 * Encode a fully-populated route metadata record into the `RouteMetadata`
 * struct consumed by `router-core`.
 */
export function routeMetadataToScVal(meta: {
  description: string;
  tags: string[];
  owner: string;
}): xdr.ScVal {
  return toMap({
    description: toStringVal(meta.description),
    tags: toVec(meta.tags.map((tag) => toSymbol(tag))),
    owner: toAddress(meta.owner),
  });
}

/**
 * Decode a return-value ScVal into native JS values.
 *
 * Map structs decode to objects keyed by their on-chain (snake_case) field
 * names, addresses decode to `G...`/`C...` strings, `u64`/`i128` decode to
 * `bigint`, and `None` decodes to `null`.
 */
export { scValToNative };