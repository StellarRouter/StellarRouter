# stellar-router-sdk

TypeScript/JavaScript client for the [StellarRouter](https://github.com/StellarRouter/StellarRouter)
contract suite. Talks to the `router-core` and `router-quote` Soroban
contracts over a Stellar RPC endpoint — resolving routes, managing
registrations, and fetching quotes — with typed results and consistent
error codes.

## Install

```bash
npm install stellar-router-sdk
```

Requires `@stellar/stellar-sdk` (v16+) as a peer/dependency. When using
`FreighterSigner`, install `@stellar/freighter-api` in your app.

## Quick start

```ts
import { RouterClient } from "stellar-router-sdk";
import { Keypair } from "@stellar/stellar-sdk";

const client = new RouterClient({
  network: "testnet", // or "futurenet" | "mainnet" | "local" | { rpcUrl, networkPassphrase }
  coreContractId: "C...",
  quoteContractId: "C...", // optional — enables quote methods
  keypair: Keypair.fromSecret("S..."), // optional — enables admin writes
});

// Resolve a route name to its contract address.
const oracle = await client.resolve("oracle");

// Register a route (admin).
await client.registerRoute("oracle", oracle, {
  description: "On-chain price feed",
  tags: ["defi", "oracle"],
});

// Inspect routes.
const route = await client.getRoute("oracle"); // RouteEntry | null
const total = await client.totalRouted(); // bigint

// Fetch a quote (requires quoteContractId).
const quote = await client.getQuote({
  route: "oracle",
  tokenIn: "C...",
  tokenOut: "C...",
  amountIn: 1000n,
});
console.log(quote.amountOut, quote.priceImpactBps);
```

## Errors

Every method rejects with a `RouterSdkError`. The `.code` property names the
failure (`RouteNotFound`, `RouterPaused`, `NoSigner`, `InvalidRouteName`,
`QuoteContractNotConfigured`, ...). On-chain contract errors are decoded from
the RPC `Error(Contract, #N)` payload — see `src/errors.ts` for the full
table.

## API

- `RouterClient` — facade over the two sub-clients (`client.core`,
  `client.quote`). Full surface: see `docs/sdk.md` in the repo root.
- `RouterCoreClient` — route resolution, registration, pause controls,
  aliases, scoring, admin management.
- `RouterQuoteClient` — quotes, best-quote selection, price-impact filtering,
  fee management.
- `Signer` / `LocalSigner` / `FreighterSigner` — signing abstraction for
  admin operations.
- ScVal helpers (`toU32`, `toI128`, `toAddress`, `quoteRequestToScVal`, ...)
  for building raw invocation args.

## Development

```bash
npm install
npm run typecheck
npm run lint
npm test
npm run build
```

Tests run against an in-memory RPC harness (`test/helpers/fake-rpc.ts`); no
network access needed.
