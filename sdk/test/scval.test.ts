import { Keypair, StrKey, scValToNative, xdr } from "@stellar/stellar-sdk";

import {
  quoteRequestToScVal,
  routeMetadataToScVal,
  toAddress,
  toBool,
  toI128,
  toMap,
  toNone,
  toSymbol,
  toU32,
  toU64,
  toVec,
} from "../src/scval.js";

const G = Keypair.random().publicKey();
const CONTRACT = StrKey.encodeContract(Buffer.alloc(32));

describe("scval encoding", () => {
  it("encodes u32 with the correct ScVal arm", () => {
    const scv = toU32(100);
    expect(scv.switch().name).toBe("scvU32");
    expect(scValToNative(scv)).toBe(100);
  });

  it("rejects out-of-range or non-integer u32 values", () => {
    expect(() => toU32(-1)).toThrow(RangeError);
    expect(() => toU32(2 ** 32)).toThrow(RangeError);
    expect(() => toU32(1.5)).toThrow(RangeError);
  });

  it("encodes u64/i128 as bigint-backed arms that round-trip", () => {
    expect(scValToNative(toU64(123n))).toBe(123n);
    const i128 = toI128(10_000n);
    expect(i128.switch().name).toBe("scvI128");
    expect(scValToNative(i128)).toBe(10_000n);
  });

  it("encodes addresses as scvAddress", () => {
    const scv = toAddress(G);
    expect(scv.switch().name).toBe("scvAddress");
    expect(scValToNative(scv)).toBe(G);
  });

  it("encodes strings, symbols, booleans, and None", () => {
    expect(scValToNative(toSymbol("oracle"))).toBe("oracle");
    expect(scValToNative(toBool(true))).toBe(true);
    expect(scValToNative(toNone())).toBe(null);
  });

  it("encodes vectors", () => {
    const scv = toVec([toSymbol("a"), toSymbol("b")]);
    expect(scValToNative(scv)).toEqual(["a", "b"]);
  });

  it("encodes maps keyed by symbols", () => {
    const scv = toMap({ name: toSymbol("oracle"), enabled: toBool(true) });
    expect(scValToNative(scv)).toEqual({ name: "oracle", enabled: true });
  });

  it("encodes a QuoteRequest struct", () => {
    const scv = quoteRequestToScVal({ route: "uniswap", tokenIn: G, tokenOut: CONTRACT, amountIn: 1000n });
    const native = scValToNative(scv);
    expect(native).toEqual({
      route: "uniswap",
      token_in: G,
      token_out: CONTRACT,
      amount_in: 1000n,
    });
    expect(scv.switch().name).toBe("scvMap");
  });

  it("encodes RouteMetadata with tags as symbols and owner as address", () => {
    const scv = routeMetadataToScVal({ description: "Price feed", tags: ["defi", "oracle"], owner: G });
    const native = scValToNative(scv);
    expect(native).toEqual({
      description: "Price feed",
      tags: ["defi", "oracle"],
      owner: G,
    });
  });
});

describe("xdr map construction", () => {
  it("encodes with object-form ScMapEntry", () => {
    const entry = new xdr.ScMapEntry({ key: toSymbol("k"), val: toU32(1) });
    expect(scValToNative(entry.key())).toBe("k");
    expect(scValToNative(entry.val())).toBe(1);
  });
});