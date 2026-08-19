/**
 * Decoders for the structs the contracts return, mapping snake_case on-chain
 * field names to idiomatic camelCase JS properties.
 */
import type {
  BatchResolveResult,
  QuoteResponse,
  ResolveErrorCode,
  RouteEntry,
  RouteMetadata,
  RouteScore,
} from "./types.js";

function snakeToCamel(s: string): string {
  return s.replace(/_([a-z0-9])/g, (_, c: string) => c.toUpperCase());
}

function requireRecord(raw: unknown): Record<string, unknown> {
  if (raw === null || raw === undefined || typeof raw !== "object") {
    return {};
  }
  return raw as Record<string, unknown>;
}

function recordToCamel(raw: unknown): Record<string, unknown> {
  const record = requireRecord(raw);
  return Object.fromEntries(Object.entries(record).map(([k, v]) => [snakeToCamel(k), v]));
}

/** Decode a `RouteEntry` payload (`null` for `None`). */
export function parseRouteEntry(raw: unknown): RouteEntry | null {
  if (!raw || typeof raw !== "object") return null;
  const o = requireRecord(raw);
  return {
    address: String(o.address ?? ""),
    name: String(o.name ?? ""),
    paused: Boolean(o.paused),
    updatedBy: String(o.updated_by ?? ""),
  };
}

/** Decode a `RouteMetadata` payload (`null` for `None`). */
export function parseRouteMetadata(raw: unknown): RouteMetadata | null {
  if (!raw || typeof raw !== "object") return null;
  const o = requireRecord(raw);
  const tags = Array.isArray(o.tags) ? (o.tags as unknown[]).map(String) : [];
  return {
    description: String(o.description ?? ""),
    tags,
    owner: String(o.owner ?? ""),
  };
}

/** Decode a `RouteScore` payload (`null` for `None`). */
export function parseRouteScore(raw: unknown): RouteScore | null {
  if (!raw || typeof raw !== "object") return null;
  const o = recordToCamel(raw);
  return {
    liquidityScore: Number(o.liquidityScore ?? 0),
    feeBps: Number(o.feeBps ?? 0),
    reliabilityScore: Number(o.reliabilityScore ?? 0),
  };
}

/** Decode a single `BatchResolveResult` union payload. */
export function parseBatchResolveResult(raw: unknown, route: string): BatchResolveResult {
  const o = requireRecord(raw);
  if (o["Ok"] !== undefined) {
    return { ok: true, route, address: String(o.Ok) };
  }
  const err = String(o.Err ?? "RouteNotFound") as ResolveErrorCode;
  return { ok: false, route, error: err };
}

/** Decode a `QuoteResponse` payload. */
export function parseQuoteResponse(raw: unknown): QuoteResponse {
  const o = recordToCamel(raw);
  return {
    route: String(o.route ?? ""),
    tokenIn: String(o.tokenIn ?? ""),
    tokenOut: String(o.tokenOut ?? ""),
    amountIn: toBigInt(o.amountIn),
    amountOut: toBigInt(o.amountOut),
    feeAmount: toBigInt(o.feeAmount),
    feeBps: Number(o.feeBps ?? 0),
    priceImpactBps: toBigInt(o.priceImpactBps),
  };
}

function toBigInt(v: unknown): bigint {
  if (typeof v === "bigint") return v;
  if (typeof v === "number") return BigInt(v);
  return 0n;
}