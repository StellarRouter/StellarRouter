import { Keypair, Networks, xdr } from "@stellar/stellar-sdk";

import { RouterClient } from "../src/client.js";
import { RouterSdkError } from "../src/errors.js";
import { toAddress, toSymbol, toU64 } from "../src/scval.js";
import { LocalSigner } from "../src/signer.js";
import { FakeRpc } from "./helpers/fake-rpc.js";
import { DEFAULT_TOKEN, quoteResponseScVal } from "./helpers/scvals.js";

const CORE_ID = "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4";
const QUOTE_ID = "CADQOBYHA4DQOBYHA4DQOBYHA4DQOBYHA4DQOBYHA4DQOBYHA4DQP5KR";
const TARGET = QUOTE_ID;
const keypair = Keypair.random();

function makeClient(overrides: { quoteContractId?: string | null; signer?: boolean } = {}) {
  const fake = new FakeRpc();
  const client = new RouterClient({
    rpc: fake,
    networkPassphrase: Networks.TESTNET,
    coreContractId: CORE_ID,
    quoteContractId: overrides.quoteContractId === undefined ? QUOTE_ID : (overrides.quoteContractId ?? undefined),
    signer: overrides.signer === false ? undefined : new LocalSigner(keypair),
  });
  return { fake, client };
}

describe("RouterClient (facade)", () => {
  it("defaults to the testnet network config", () => {
    const { client } = makeClient();
    expect(client.network.rpcUrl).toContain("testnet");
    expect(client.network.networkPassphrase).toContain("Test SDF Network");
  });

  it("exposes the two sub-clients", () => {
    const { client } = makeClient();
    expect(client.core).toBeDefined();
    expect(client.quote).not.toBeNull();
    expect(client.core.invoker.contractId).toBe(CORE_ID);
    expect(client.quote!.invoker.contractId).toBe(QUOTE_ID);
  });

  it("delegates docs/sdk.md read methods to the core client", async () => {
    const { fake, client } = makeClient();
    fake.simulate.set("resolve", { retval: toAddress(TARGET) });
    fake.simulate.set("total_routed", { retval: toU64(3n) });

    await expect(client.resolve("oracle")).resolves.toBe(TARGET);
    await expect(client.totalRouted()).resolves.toBe(3n);

    fake.simulate.set("get_route", { retval: xdr.ScVal.scvVoid() });
    await expect(client.getRoute("oracle")).resolves.toBeNull();
  });

  it("delegates docs/sdk.md write methods with a signer", async () => {
    const { fake, client } = makeClient();
    fake.getTransaction = async () => ({ status: "SUCCESS" });
    await client.registerRoute("oracle", TARGET);
    await client.setRoutePaused("oracle", false);
    await client.setPaused(true);
    expect(fake.lastInvocation?.method).toBe("set_paused");
    expect(fake.prepareCalls).toHaveLength(3);
  });

  it("delegates quote methods when a quote contract is configured", async () => {
    const { fake, client } = makeClient();
    fake.simulate.set("get_quote", { retval: quoteResponseScVal() });
    const quote = await client.getQuote({ route: "uniswap", tokenIn: DEFAULT_TOKEN, tokenOut: TARGET, amountIn: 1000n });
    expect(quote.amountOut).toBe(1050n);
  });

  it("rejects invalid route names via the facade", async () => {
    const { fake, client } = makeClient();
    fake.getTransaction = async () => ({ status: "SUCCESS" });
    await expect(client.registerRoute("bad name!", TARGET)).rejects.toMatchObject({
      code: "InvalidRouteName",
    });
    expect(fake.prepareCalls).toHaveLength(0);
  });
});

describe("RouterClient (configuration)", () => {
  it("rejects an invalid core contract id", () => {
    expect(
      () =>
        new RouterClient({
          rpc: new FakeRpc(),
          networkPassphrase: Networks.TESTNET,
          coreContractId: "nope",
          signer: new LocalSigner(keypair),
        }),
    ).toThrow(RouterSdkError);
  });

  it("throws QuoteContractNotConfigured when quote methods run without a quote contract", async () => {
    const { client } = makeClient({ quoteContractId: null });
    expect(client.quote).toBeNull();
    await expect(
      client.getQuote({ route: "uniswap", tokenIn: DEFAULT_TOKEN, tokenOut: TARGET, amountIn: 1n }),
    ).rejects.toMatchObject({ code: "QuoteContractNotConfigured" });
  });

it("accepts a named network preset", () => {
      const fake = new FakeRpc();
      const client = new RouterClient({
        rpc: fake,
        network: "futurenet",
        coreContractId: CORE_ID,
        signer: new LocalSigner(keypair),
      });
      expect(client.network.rpcUrl).toContain("futurenet");
      expect(client.network.networkPassphrase).toContain("Futurenet");
    });

  it("accepts a custom network config", () => {
    const fake = new FakeRpc();
    const client = new RouterClient({
      rpc: fake,
      network: { rpcUrl: "http://localhost:8000", networkPassphrase: "Standalone Network ; 2024" },
      coreContractId: CORE_ID,
      signer: new LocalSigner(keypair),
    });
    expect(client.network.rpcUrl).toBe("http://localhost:8000");
  });

  it("can be used with a keypair directly (auto signer)", () => {
    const fake = new FakeRpc();
    const client = new RouterClient({
      rpc: fake,
      networkPassphrase: Networks.TESTNET,
      coreContractId: CORE_ID,
      keypair,
    });
    expect(client.core.invoker.signer).toBeDefined();
    expect(client.core.invoker.signer!.publicKey()).toBe(keypair.publicKey());
  });
});