import { Keypair, Networks, scValToNative, xdr } from "@stellar/stellar-sdk";

import { RouterSdkError, parseContractErrorCode } from "../src/errors.js";
import { RouterQuoteClient } from "../src/quote.js";
import { toSymbol, toU32 } from "../src/scval.js";
import { LocalSigner } from "../src/signer.js";
import { FakeRpc } from "./helpers/fake-rpc.js";
import { DEFAULT_TOKEN, i128, quoteResponseScVal } from "./helpers/scvals.js";

const QUOTE_ID = "CADQOBYHA4DQOBYHA4DQOBYHA4DQOBYHA4DQOBYHA4DQOBYHA4DQP5KR";
const TOKEN_A = DEFAULT_TOKEN;
const TOKEN_B = QUOTE_ID;
const keypair = Keypair.random();

function makeClient(overrides: { signer?: boolean } = {}) {
  const fake = new FakeRpc();
  const client = new RouterQuoteClient({
    contractId: QUOTE_ID,
    rpc: fake,
    networkPassphrase: Networks.TESTNET,
    signer: overrides.signer === false ? undefined : new LocalSigner(keypair),
  });
  return { fake, client };
}

const quoteReq = { route: "uniswap", tokenIn: TOKEN_A, tokenOut: TOKEN_B, amountIn: 1000n };

describe("RouterQuoteClient (read)", () => {
  it("parses a QuoteResponse into camelCase with bigints", async () => {
    const { fake, client } = makeClient();
    fake.simulate.set("get_quote", { retval: quoteResponseScVal() });
    const quote = await client.getQuote(quoteReq);
    expect(quote).toEqual({
      route: "uniswap",
      tokenIn: TOKEN_A,
      tokenOut: TOKEN_A,
      amountIn: 1000n,
      amountOut: 1050n,
      feeAmount: 2n,
      feeBps: 30,
      priceImpactBps: 20n,
    });
    expect(fake.lastInvocation?.method).toBe("get_quote");
  });

  it("maps on-chain QuoteError codes", async () => {
    const { fake, client } = makeClient();
    fake.simulate.set("get_quote", { error: "host error: Error(Contract, #4)" });
    await expect(client.getQuote(quoteReq)).rejects.toMatchObject({ code: "InvalidAmount" });
  });

  it("decodes a batch of quotes from get_quotes", async () => {
    const { fake, client } = makeClient();
    fake.simulate.set("get_quotes", {
      retval: xdr.ScVal.scvVec([quoteResponseScVal({ route: "a" }), quoteResponseScVal({ route: "b" })]),
    });
    const quotes = await client.getQuotes([quoteReq, quoteReq]);
    expect(quotes).toHaveLength(2);
    expect(quotes[0]!.route).toBe("a");
    expect(quotes[1]!.route).toBe("b");
  });

  it("returns a single best quote from get_best_quote", async () => {
    const { fake, client } = makeClient();
    fake.simulate.set("get_best_quote", { retval: quoteResponseScVal({ amountOut: 2000n }) });
    const best = await client.getBestQuote([quoteReq, { ...quoteReq, route: "sushiswap" }]);
    expect(best.amountOut).toBe(2000n);
    expect(fake.lastInvocation?.method).toBe("get_best_quote");
  });

  it("throws NoQuotesProvided for empty getBestQuote calls", async () => {
    const { client } = makeClient();
    await expect(client.getBestQuote([])).rejects.toMatchObject({ code: "NoQuotesProvided" });
  });

  it("returns survivors from compare_quotes sorted by amount_out", async () => {
    const { fake, client } = makeClient();
    fake.simulate.set("compare_quotes", {
      retval: xdr.ScVal.scvVec([quoteResponseScVal({ route: "best", amountOut: 9000n }), quoteResponseScVal({ route: "second", amountOut: 8000n })]),
    });
    const quotes = await client.compareQuotes([quoteReq, { ...quoteReq, route: "second" }], 50);
    expect(quotes.map((q) => q.route)).toEqual(["best", "second"]);
    const inv = fake.lastInvocation;
    const native = inv ? inv.args.map((a) => scValToNative(a)) : [];
    expect(native[1]).toBe(50n);
  });

  it("returns fee/duration values as numbers", async () => {
    const { fake, client } = makeClient();
    fake.simulate.set("get_route_fee", { retval: toU32(5) });
    fake.simulate.set("get_default_fee", { retval: toU32(10) });
    await expect(client.getRouteFee("uniswap")).resolves.toBe(5);
    await expect(client.getDefaultFee()).resolves.toBe(10);
  });
});

describe("RouterQuoteClient (write)", () => {
  it("sets a route fee with caller + name + bps args", async () => {
    const { fake, client } = makeClient();
    fake.getTransaction = async () => ({ status: "SUCCESS" });
    await client.setRouteFee("uniswap", 15);
    const inv = fake.lastInvocation;
    expect(inv?.method).toBe("set_route_fee");
    const native = inv ? inv.args.map((a) => scValToNative(a)) : [];
    expect(native[0]).toBe(keypair.publicKey());
    expect(native[1]).toBe("uniswap");
    expect(native[2]).toBe(15);
  });

  it("throws NoSigner for admin-only writes without a signer", async () => {
    const { client } = makeClient({ signer: false });
    await expect(client.setDefaultFee(10)).rejects.toMatchObject({ code: "NoSigner" });
  });

  it("validates route names before invoking", async () => {
    const { fake, client } = makeClient();
    fake.getTransaction = async () => ({ status: "SUCCESS" });
    await expect(client.setRouteFee("not valid!", 15)).rejects.toMatchObject({
      code: "InvalidRouteName",
    });
    expect(fake.prepareCalls).toHaveLength(0);
  });
});

describe("error mapping", () => {
  it("reads quote error codes from the shared parser", () => {
    expect(parseContractErrorCode("Error(Contract, #9)")).toBe(9);
    expect(RouterSdkError.fromContractCode(9, { contract: "quote" }).code).toBe("AmountOverflow");
    expect(RouterSdkError.fromContractCode(7, { contract: "quote" }).code).toBe("RouteNotFound");
  });
});

describe("scval round-trips for quote amounts", () => {
  it("encodes i128 amounts that decode back to bigint", () => {
    const scv = i128(10_000_000n);
    expect(scv.switch().name).toBe("scvI128");
    expect(scValToNative(scv)).toBe(10_000_000n);
  });
});