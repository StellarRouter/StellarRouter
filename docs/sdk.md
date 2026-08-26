# JavaScript / TypeScript SDK

> **Status: implemented.** The `stellar-router-sdk` npm package ships the
> `RouterClient` class and the API described below. It is not yet published to
> the npm registry; to use it from this repository, `npm install` inside
> [`sdk/`](../sdk/README.md) or link the package into your own project.

A client library for interacting with the stellar-router contracts from JS/TS.

## Installation
```bash
npm install stellar-router-sdk
```

## Quick Start
```ts
import { RouterClient } from "stellar-router-sdk";
import { Keypair } from "@stellar/stellar-sdk";

const client = new RouterClient({
  network: "testnet",
  coreContractId: "C...",
  quoteContractId: "C...", // optional — enables quote methods
  keypair: Keypair.fromSecret("S..."), // optional — enables admin writes
});

const address = await client.resolve("oracle");
await client.registerRoute("oracle", "C...", { description: "Price feed" });
```

## API

### Router client (router-core)
- `resolve(name)` → `Promise<string>`
- `batchResolve(names)` → `Promise<BatchResolveResult[]>`
- `registerRoute(name, address, metadata?)` → `Promise<void>`
- `registerRoutesBatch(routes)` → `Promise<void>`
- `updateRoute(name, newAddress)` → `Promise<void>`
- `removeRoute(name)` → `Promise<void>`
- `removeRoutesBatch(names)` → `Promise<void>`
- `setRoutePaused(name, paused)` → `Promise<void>`
- `setPaused(paused)` → `Promise<void>`
- `getRoute(name)` → `Promise<RouteEntry | null>`
- `getMetadata(name)` → `Promise<RouteMetadata | null>`
- `totalRouted()` → `Promise<bigint>`
- `routeCount()` → `Promise<number>`
- `getBestRoute(candidates, minScore, fallbackName?)` → `Promise<string | null>`
- `getRouteScore(name)` → `Promise<RouteScore | null>`
- `getAliasTarget(aliasName)` → `Promise<string | null>`

### Quote client (router-quote, requires `quoteContractId`)
- `getQuote(request)` → `Promise<QuoteResponse>`
- `getQuotes(requests)` → `Promise<QuoteResponse[]>`
- `getBestQuote(requests)` → `Promise<QuoteResponse>`
- `compareQuotes(requests, maxPriceImpactBps)` → `Promise<QuoteResponse[]>`
- `getRouteFee(route)` → `Promise<number>`
- `getDefaultFee()` → `Promise<number>`

The underlying sub-clients are exposed as `client.core` and `client.quote`
for lower-level access, including admin fee management (`setRouteFee`,
`setDefaultFee`).

## Error Handling
All methods throw `RouterSdkError` with a `.code` on failure, e.g.
`"RouteNotFound"`, `"RoutePaused"`, `"RouterPaused"`, `"NoSigner"`,
`"InvalidRouteName"`, `"InvalidContractId"`, or
`"QuoteContractNotConfigured"`. On-chain contract errors are mapped to their
SDK names; a raw `Error(Contract, #N)` is parsed and surfaced with
`err.contractErrorCode` and `err.code` set accordingly.

## Signing
State-changing calls require an authenticated account. Pass either a
`keypair` (in-memory signing) or a `signer` implementing the `Signer`
interface (`publicKey()` and `sign(tx)`). `LocalSigner` and `FreighterSigner`
are provided — see [`signing-abstraction.md`](./signing-abstraction.md).

## Publishing

> The release process for the package. Not run automatically.

```bash
cd sdk
npm version patch
npm publish
```
