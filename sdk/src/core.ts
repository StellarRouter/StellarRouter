import type { xdr } from "@stellar/stellar-sdk";

import { ContractInvoker } from "./contract.js";
import type { ContractInvokerOptions } from "./contract.js";
import { RouterSdkError } from "./errors.js";
import {
  parseBatchResolveResult,
  parseRouteEntry,
  parseRouteMetadata,
  parseRouteScore,
} from "./parse.js";
import {
  routeMetadataToScVal,
  toAddress,
  toBool,
  toI64,
  toMap,
  toNone,
  toStringVal,
  toU32,
  toVec,
} from "./scval.js";
import type {
  BatchResolveResult,
  RegisterRouteInput,
  RouteEntry,
  RouteMetadata,
  RouteMetadataInput,
  RouteScore,
} from "./types.js";
import { assertContractId, assertRouteName } from "./validation.js";

/** Options for {@link RouterCoreClient}. */
export interface RouterCoreClientOptions
  extends Omit<ContractInvokerOptions, "contractId" | "label"> {
  /** The deployed `router-core` contract id. */
  contractId: string;
}

/**
 * Client for the `router-core` contract: route registration, resolution,
 * pause controls, aliases, scoring, and admin management.
 *
 * Read-only methods need only a network connection; state-changing methods
 * require a {@link Signer} (the admin).
 */
export class RouterCoreClient {
  /** The underlying low-level invocation helper. */
  readonly invoker: ContractInvoker;

  constructor(options: RouterCoreClientOptions) {
    assertContractId(options.contractId);
    this.invoker = new ContractInvoker({ ...options, label: "core" });
  }

  // ── Read-only ────────────────────────────────────────────────────────────

  /** Resolve a route name (or alias) to its contract address. */
  async resolve(name: string): Promise<string> {
    assertRouteName(name);
    return this.invoker.simulate("resolve", [toStringVal(name)]);
  }

  /** Resolve several route names in a single call. */
  async batchResolve(names: string[]): Promise<BatchResolveResult[]> {
    if (names.length === 0) return [];
    const raw = await this.invoker.simulate("batch_resolve", [toVec(names.map(toStringVal))]);
    return toArray(raw).map((entry, i) => parseBatchResolveResult(entry, names[i] ?? ""));
  }

  /** Fetch a route entry, or `null` when no route with `name` exists. */
  async getRoute(name: string): Promise<RouteEntry | null> {
    assertRouteName(name);
    const raw = await this.invoker.simulate("get_route", [toStringVal(name)]);
    return parseRouteEntry(raw);
  }

  /** Fetch route metadata, or `null` when no route/metadata exists. */
  async getMetadata(name: string): Promise<RouteMetadata | null> {
    assertRouteName(name);
    const raw = await this.invoker.simulate("get_metadata", [toStringVal(name)]);
    return parseRouteMetadata(raw);
  }

  /** Total number of successful `resolve` invocations since initialization. */
  async totalRouted(): Promise<bigint> {
    const value = await this.invoker.simulate("total_routed", []);
    return asBigint(value, "total_routed");
  }

  /** Number of registered routes. */
  async routeCount(): Promise<number> {
    const value = await this.invoker.simulate("route_count", []);
    return asNumber(value, "route_count");
  }

  /** The current admin address. */
  async admin(): Promise<string> {
    const value = await this.invoker.simulate("admin", []);
    return String(value);
  }

  /** Best route from `candidates` by composite score, or `fallbackName`. */
  async getBestRoute(
    candidates: string[],
    minScore: number | bigint,
    fallbackName?: string,
  ): Promise<string | null> {
    if (candidates.length === 0) return null;
    candidates.forEach(assertRouteName);
    const args: xdr.ScVal[] = [
      toVec(candidates.map(toStringVal)),
      toI64(minScore),
      fallbackName === undefined ? toNone() : toStringVal(fallbackName),
    ];
    const raw = await this.invoker.simulate("get_best_route", args);
    return raw === null || raw === undefined ? null : String(raw);
  }

  /** The route score for `name`, or `null`. */
  async getRouteScore(name: string): Promise<RouteScore | null> {
    assertRouteName(name);
    const raw = await this.invoker.simulate("get_route_score", [toStringVal(name)]);
    return parseRouteScore(raw);
  }

  /** The canonical route name `aliasName` points to (or `null`). */
  async getAliasTarget(aliasName: string): Promise<string | null> {
    assertRouteName(aliasName);
    const raw = await this.invoker.simulate("get_alias_target", [toStringVal(aliasName)]);
    return raw === null || raw === undefined ? null : String(raw);
  }

  // ── State-changing (admin) ───────────────────────────────────────────────

  /** Register a new route. */
  async registerRoute(
    name: string,
    address: string,
    metadata?: RouteMetadataInput,
  ): Promise<void> {
    assertRouteName(name);
    assertContractId(address);
    const caller = await this.requireAdmin();
    const meta = metadata ? normalizeMetadata(metadata, caller) : undefined;
    await this.invoker.invoke("register_route", [
      toAddress(caller),
      toStringVal(name),
      toAddress(address),
      meta ? routeMetadataToScVal(meta) : toNone(),
    ]);
  }

  /** Register multiple routes atomically. */
  async registerRoutesBatch(routes: RegisterRouteInput[]): Promise<void> {
    if (routes.length === 0) return;
    const caller = await this.requireAdmin();
    const entries = routes.map((route) => {
      assertRouteName(route.name);
      assertContractId(route.address);
      const metadata = route.metadata ? normalizeMetadata(route.metadata, caller) : undefined;
      return toMap({
        name: toStringVal(route.name),
        address: toAddress(route.address),
        metadata: metadata ? routeMetadataToScVal(metadata) : toNone(),
      });
    });
    await this.invoker.invoke("register_routes_batch", [toAddress(caller), toVec(entries)]);
  }

  /** Point an existing route at a new address. */
  async updateRoute(name: string, newAddress: string): Promise<void> {
    assertRouteName(name);
    assertContractId(newAddress);
    const caller = await this.requireAdmin();
    await this.invoker.invoke("update_route", [
      toAddress(caller),
      toStringVal(name),
      toAddress(newAddress),
    ]);
  }

  /** Remove a route. */
  async removeRoute(name: string): Promise<void> {
    assertRouteName(name);
    const caller = await this.requireAdmin();
    await this.invoker.invoke("remove_route", [toAddress(caller), toStringVal(name)]);
  }

  /** Remove multiple routes atomically. */
  async removeRoutesBatch(names: string[]): Promise<void> {
    if (names.length === 0) return;
    names.forEach(assertRouteName);
    const caller = await this.requireAdmin();
    await this.invoker.invoke("remove_routes_batch", [
      toAddress(caller),
      toVec(names.map(toStringVal)),
    ]);
  }

  /** Replace the metadata for an existing route. */
  async updateMetadata(name: string, metadata?: RouteMetadataInput): Promise<void> {
    assertRouteName(name);
    const caller = await this.requireAdmin();
    const meta = metadata ? normalizeMetadata(metadata, caller) : undefined;
    await this.invoker.invoke("update_metadata", [
      toAddress(caller),
      toStringVal(name),
      meta ? routeMetadataToScVal(meta) : toNone(),
    ]);
  }

  /** Pause/unpause a specific route. */
  async setRoutePaused(name: string, paused: boolean): Promise<void> {
    assertRouteName(name);
    const caller = await this.requireAdmin();
    await this.invoker.invoke("set_route_paused", [
      toAddress(caller),
      toStringVal(name),
      toBool(paused),
    ]);
  }

  /** Pause/unpause the whole router. */
  async setPaused(paused: boolean): Promise<void> {
    const caller = await this.requireAdmin();
    await this.invoker.invoke("set_paused", [toAddress(caller), toBool(paused)]);
  }

  /** Set the scoring attributes for a route. */
  async setRouteScore(name: string, score: RouteScore): Promise<void> {
    assertRouteName(name);
    const caller = await this.requireAdmin();
    await this.invoker.invoke("set_route_score", [
      toAddress(caller),
      toStringVal(name),
      toMap({
        liquidity_score: toU32(score.liquidityScore),
        fee_bps: toU32(score.feeBps),
        reliability_score: toU32(score.reliabilityScore),
      }),
    ]);
  }

  /** Create an alias for an existing route. */
  async addAlias(existingName: string, aliasName: string): Promise<void> {
    assertRouteName(existingName);
    assertRouteName(aliasName);
    const caller = await this.requireAdmin();
    await this.invoker.invoke("add_alias", [
      toAddress(caller),
      toStringVal(existingName),
      toStringVal(aliasName),
    ]);
  }

  /** Remove an alias. */
  async removeAlias(aliasName: string): Promise<void> {
    assertRouteName(aliasName);
    const caller = await this.requireAdmin();
    await this.invoker.invoke("remove_alias", [toAddress(caller), toStringVal(aliasName)]);
  }

  /** Transfer admin to a new address. */
  async transferAdmin(newAdmin: string): Promise<void> {
    assertContractId(newAdmin);
    const caller = await this.requireAdmin();
    await this.invoker.invoke("transfer_admin", [toAddress(caller), toAddress(newAdmin)]);
  }

  private async requireAdmin(): Promise<string> {
    if (this.invoker.signer) {
      return this.invoker.signer.publicKey();
    }
    throw new RouterSdkError(
      "NoSigner",
      "This operation requires the router admin. Provide a signer (or keypair) when constructing the client.",
      { contract: "core" },
    );
  }
}

function normalizeMetadata(input: RouteMetadataInput, defaultOwner: string): RouteMetadata {
  return {
    description: input.description ?? "",
    tags: input.tags ?? [],
    owner: input.owner ?? defaultOwner,
  };
}

function toArray(value: unknown): unknown[] {
  return Array.isArray(value) ? value : [];
}

function asBigint(value: unknown, method: string): bigint {
  if (typeof value === "bigint") return value;
  if (typeof value === "number") return BigInt(value);
  throw new RouterSdkError(
    "UnexpectedResult",
    `Expected a bigint result from ${method}, got ${JSON.stringify(value)}.`,
    { contract: "core" },
  );
}

function asNumber(value: unknown, method: string): number {
  if (typeof value === "number") return value;
  if (typeof value === "bigint") return Number(value);
  throw new RouterSdkError(
    "UnexpectedResult",
    `Expected a number result from ${method}, got ${JSON.stringify(value)}.`,
    { contract: "core" },
  );
}