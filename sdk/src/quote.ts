import { ContractInvoker } from "./contract.js";
import type { ContractInvokerOptions } from "./contract.js";
import { RouterSdkError } from "./errors.js";
import { parseQuoteResponse } from "./parse.js";
import { quoteRequestToScVal, toAddress, toI128, toStringVal, toU32, toVec } from "./scval.js";
import type { QuoteRequest, QuoteResponse } from "./types.js";
import { assertContractId, assertRouteName } from "./validation.js";

/** Options for {@link RouterQuoteClient}. */
export interface RouterQuoteClientOptions
  extends Omit<ContractInvokerOptions, "contractId" | "label"> {
  /** The deployed `router-quote` contract id. */
  contractId: string;
}

/**
 * Client for the `router-quote` contract: fee-based quote calculation,
 * best-route selection, and price-impact filtering.
 *
 * Quote retrieval is read-only; fee management requires the admin signer.
 */
export class RouterQuoteClient {
  /** The underlying low-level invocation helper. */
  readonly invoker: ContractInvoker;

  constructor(options: RouterQuoteClientOptions) {
    assertContractId(options.contractId);
    this.invoker = new ContractInvoker({ ...options, label: "quote" });
  }

  // ── Read-only ────────────────────────────────────────────────────────────

  /** Fetch a single quote for `request`. */
  async getQuote(request: QuoteRequest): Promise<QuoteResponse> {
    const raw = await this.invoker.simulate("get_quote", [quoteRequestToScVal(request)]);
    return parseQuoteResponse(raw);
  }

  /** Fetch quotes for several requests at once. */
  async getQuotes(requests: QuoteRequest[]): Promise<QuoteResponse[]> {
    if (requests.length === 0) return [];
    const raw = await this.invoker.simulate("get_quotes", [toVec(requests.map(quoteRequestToScVal))]);
    return toArray(raw).map(parseQuoteResponse);
  }

  /** Fetch the single quote with the highest `amount_out`. */
  async getBestQuote(requests: QuoteRequest[]): Promise<QuoteResponse> {
    if (requests.length === 0) {
      throw new RouterSdkError("NoQuotesProvided", "getBestQuote requires at least one request.", {
        contract: "quote",
      });
    }
    const raw = await this.invoker.simulate("get_best_quote", [
      toVec(requests.map(quoteRequestToScVal)),
    ]);
    return parseQuoteResponse(raw);
  }

  /**
   * Evaluate quotes on `max_price_impact_bps`, returning survivors sorted by
   * `amount_out` descending (best first). May return an empty array.
   */
  async compareQuotes(
    requests: QuoteRequest[],
    maxPriceImpactBps: number | bigint,
  ): Promise<QuoteResponse[]> {
    if (requests.length === 0) return [];
    const raw = await this.invoker.simulate("compare_quotes", [
      toVec(requests.map(quoteRequestToScVal)),
      toI128(maxPriceImpactBps),
    ]);
    return toArray(raw).map(parseQuoteResponse);
  }

  /** The fee (bps) for a specific route, falling back to the default fee. */
  async getRouteFee(route: string): Promise<number> {
    assertRouteName(route);
    const value = await this.invoker.simulate("get_route_fee", [toStringVal(route)]);
    return asNumber(value, "get_route_fee");
  }

  /** The default fee (bps) used for routes without a specific fee. */
  async getDefaultFee(): Promise<number> {
    const value = await this.invoker.simulate("get_default_fee", []);
    return asNumber(value, "get_default_fee");
  }

  /** The current admin address. */
  async admin(): Promise<string> {
    const value = await this.invoker.simulate("admin", []);
    return String(value);
  }

  // ── State-changing (admin) ───────────────────────────────────────────────

  /** Set the fee (bps) for a specific route. */
  async setRouteFee(route: string, feeBps: number): Promise<void> {
    assertRouteName(route);
    const caller = await this.requireAdmin();
    await this.invoker.invoke("set_route_fee", [
      toAddress(caller),
      toStringVal(route),
      toU32(feeBps),
    ]);
  }

  /** Set the default fee (bps). */
  async setDefaultFee(feeBps: number): Promise<void> {
    const caller = await this.requireAdmin();
    await this.invoker.invoke("set_default_fee", [toAddress(caller), toU32(feeBps)]);
  }

  private async requireAdmin(): Promise<string> {
    if (this.invoker.signer) {
      return this.invoker.signer.publicKey();
    }
    throw new RouterSdkError(
      "NoSigner",
      "This operation requires the quote-contract admin. Provide a signer (or keypair) when constructing the client.",
      { contract: "quote" },
    );
  }
}

function toArray(value: unknown): unknown[] {
  return Array.isArray(value) ? value : [];
}

function asNumber(value: unknown, method: string): number {
  if (typeof value === "number") return value;
  if (typeof value === "bigint") return Number(value);
  throw new RouterSdkError(
    "UnexpectedResult",
    `Expected a number result from ${method}, got ${JSON.stringify(value)}.`,
    { contract: "quote" },
  );
}