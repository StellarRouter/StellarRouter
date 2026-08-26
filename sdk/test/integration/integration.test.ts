import { StrKey } from "@stellar/stellar-sdk";

import { RouterClient } from "../../src/index.js";
import { RouterSdkError } from "../../src/errors.js";

/**
 * Live-network integration tests, modeled on the Rust `--ignored` jobs.
 *
 * Skipped by default. To run against a deployment provide:
 *   STELLAR_RPC_URL            (required)
 *   ROUTER_CORE_CONTRACT_ID    (required)
 *   ROUTER_TEST_ROUTE          (required — a route registered on that network)
 *   ROUTER_QUOTE_CONTRACT_ID   (optional — enables the quote smoke checks)
 *   STELLAR_NETWORK_PASSPHRASE (optional — defaults to testnet)
 *
 * Example:
 *   $env:STELLAR_RPC_URL="https://soroban-testnet.stellar.org"
 *   $env:ROUTER_CORE_CONTRACT_ID="C..."
 *   $env:ROUTER_TEST_ROUTE="oracle"
 *   npm run test:integration
 */

const rpcUrl = process.env.STELLAR_RPC_URL;
const coreContractId = process.env.ROUTER_CORE_CONTRACT_ID;
const quoteContractId = process.env.ROUTER_QUOTE_CONTRACT_ID;
const testRoute = process.env.ROUTER_TEST_ROUTE;
const networkPassphrase = process.env.STELLAR_NETWORK_PASSPHRASE;

const configured = Boolean(rpcUrl && coreContractId && testRoute);
const describeSuite = configured ? describe : describe.skip;

/** Build the client inside the run phase so a skipped suite never constructs it. */
function buildClient(): RouterClient {
  if (!rpcUrl || !coreContractId || !testRoute) {
    throw new Error("integration environment is not configured");
  }
  return new RouterClient({
    rpcUrl,
    network: networkPassphrase ? { rpcUrl, networkPassphrase } : "testnet",
    coreContractId,
    quoteContractId: quoteContractId || undefined,
  });
}

function assertContractId(value: unknown): void {
  expect(typeof value).toBe("string");
  expect(value).toMatch(/^C[0-9A-Z]{55}$/);
}

describeSuite("live network (integration)", () => {
  it("reports a routed counter and route count", async () => {
    const client = buildClient();
    const totalRouted = await client.totalRouted();
    expect(typeof totalRouted).toBe("bigint");
    const routeCount = await client.core.routeCount();
    expect(typeof routeCount).toBe("number");
  });

  it("reads the admin address", async () => {
    const client = buildClient();
    const admin = await client.core.admin();
    expect(StrKey.isValidEd25519PublicKey(admin)).toBe(true);
  });

  it("resolves a registered route to a contract address", async () => {
    const client = buildClient();
    const address = await client.resolve(testRoute!);
    assertContractId(address);
  });

  it("returns null for an unknown route", async () => {
    const client = buildClient();
    await expect(client.getRoute("definitely-not-a-route")).resolves.toBeNull();
  });

  it("maps an on-chain RouteNotFound error to a RouterSdkError", async () => {
    const client = buildClient();
    const err = await client.resolve("definitely-not-a-route").catch((e: unknown) => e);
    expect(err).toBeInstanceOf(RouterSdkError);
    expect((err as RouterSdkError).code).toBe("RouteNotFound");
  });

  describeSuite("router-quote smoke checks", () => {
    const quoteSuite = quoteContractId ? describe : describe.skip;
    quoteSuite("quote contract configured", () => {
      it("reads the default fee and the route fee", async () => {
        const client = buildClient();
        const defaultFee = await client.quote!.getDefaultFee();
        expect(typeof defaultFee).toBe("number");
        const routeFee = await client.quote!.getRouteFee(testRoute!);
        expect(typeof routeFee).toBe("number");
      });
    });
  });
});