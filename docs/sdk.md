# JavaScript / TypeScript SDK

> **Status: planned, not yet implemented.** The `stellar-router-sdk` npm package,
> the `RouterClient` class, and the API described below do not exist in this
> repository yet. This document describes the intended design so contributors
> can build toward it. Do not `npm install stellar-router-sdk` — it isn't
> published.

A client library for interacting with the stellar-router contracts from JS/TS.

## Installation
npm install stellar-router-sdk

## Quick Start
import { RouterClient } from "stellar-router-sdk";

const client = new RouterClient({
  network: "testnet",
  coreContractId: "C...",
  keypair: Keypair.fromSecret("S..."),
});

const address = await client.resolve("oracle");
await client.registerRoute("oracle", "C...", { description: "Price feed" });

## API
- resolve(name) → Promise<string>
- registerRoute(name, address, metadata?) → Promise<void>
- updateRoute(name, newAddress) → Promise<void>
- removeRoute(name) → Promise<void>
- setRoutePaused(name, paused) → Promise<void>
- setPaused(paused) → Promise<void>
- getRoute(name) → Promise<RouteEntry | null>
- totalRouted() → Promise<bigint>

## Error Handling
All methods throw RouterSdkError with a .code (e.g. "RouteNotFound") on failure.

## Publishing

> This section describes the intended release process once the package
> exists; there is nothing to publish yet.

npm version patch && npm publish
