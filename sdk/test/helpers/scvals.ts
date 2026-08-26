import { XdrLargeInt, xdr } from "@stellar/stellar-sdk";

import { toAddress, toSymbol, toU32 } from "../../src/scval.js";

/** An i128 ScVal arm (large values only fit when passed as an array). */
export function i128(n: bigint): xdr.ScVal {
  return new XdrLargeInt("i128", [n]).toI128();
}

export interface QuoteResponseMock {
  route: string;
  tokenIn: string;
  tokenOut: string;
  amountIn: bigint;
  amountOut: bigint;
  feeAmount: bigint;
  feeBps: number;
  priceImpactBps: bigint;
}

export const DEFAULT_TOKEN = "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4";

export function quoteResponseScVal(q: Partial<QuoteResponseMock> = {}): xdr.ScVal {
  const data: QuoteResponseMock = {
    route: "uniswap",
    tokenIn: DEFAULT_TOKEN,
    tokenOut: DEFAULT_TOKEN,
    amountIn: 1000n,
    amountOut: 1050n,
    feeAmount: 2n,
    feeBps: 30,
    priceImpactBps: 20n,
    ...q,
  };
  return xdr.ScVal.scvMap([
    new xdr.ScMapEntry({ key: toSymbol("route"), val: xdr.ScVal.scvString(data.route) }),
    new xdr.ScMapEntry({ key: toSymbol("token_in"), val: toAddress(data.tokenIn) }),
    new xdr.ScMapEntry({ key: toSymbol("token_out"), val: toAddress(data.tokenOut) }),
    new xdr.ScMapEntry({ key: toSymbol("amount_in"), val: i128(data.amountIn) }),
    new xdr.ScMapEntry({ key: toSymbol("amount_out"), val: i128(data.amountOut) }),
    new xdr.ScMapEntry({ key: toSymbol("fee_amount"), val: i128(data.feeAmount) }),
    new xdr.ScMapEntry({ key: toSymbol("fee_bps"), val: toU32(data.feeBps) }),
    new xdr.ScMapEntry({ key: toSymbol("price_impact_bps"), val: i128(data.priceImpactBps) }),
  ]);
}