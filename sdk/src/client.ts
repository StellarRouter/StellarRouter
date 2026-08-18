import type { Keypair } from "@stellar/stellar-sdk";

import { RouterCoreClient } from "./core.js";
import { RouterSdkError } from "./errors.js";
import { resolveNetwork, withOverrides } from "./networks.js";
import { RouterQuoteClient } from "./quote.js";
import { createRpcServer } from "./rpc.js";
import type { RpcLike } from "./rpc.js";
import { LocalSigner } from "./signer.js";
import type { Signer } from "./signer.js";
import type {
  NetworkConfig,
  NetworkInput,
  QuoteRequest,
  QuoteResponse,
  RegisterRouteInput,
  RouteEntry,
  RouteMetadataInput,
} from "./types.js";
import { assertContractId } from "./validation.js";

/** Options for {@link RouterClient}. */
export interface RouterClientOptions {
  /** Network preset (`"testnet"`, `"futurenet"`, `"mainnet"`, `"local"`) or a full config. */
  network?: NetworkInput;
  /** Override the RPC endpoint URL for the chosen network. */
  rpcUrl?: string;
  /** Override the network passphrase for the chosen network. */
  networkPassphrase?: string;
  /** The deployed `router-core` contract id (`C...`). */
  coreContractId: string;
  /** The deployed `router-quote` contract id (`C...`). Enables quote methods. */
  quoteContractId?: string;
  /** Signer used to authenticate state-changing calls. */
  signer?: Signer;
  /** Convenience for `signer: new LocalSigner(keypair)`. */
  keypair?: Keypair;
  /** Inject a custom RPC client (used for tests/mocking). */
  rpc?: RpcLike;
  /** Base fee for transactions (defaults to the Soroban base fee). */
  fee?: string | number;
  /** How long to poll for transaction confirmation (seconds). */
  timeoutSeconds?: number;
  /** Delay between transaction-status polls (milliseconds). */
  pollIntervalMs?: number;
}

/**
 * High-level client for the stellar-router contract suite.
 *
 * Aggregates a {@link RouterCoreClient} (route resolution/registration) and a
 * {@link RouterQuoteClient} (quote fetching, when `quoteContractId` is set).
 *
 * @example
 * import { RouterClient } from "stellar-router-sdk";
 * import { Keypair } from "@stellar/stellar-sdk";
 *
 * const client = new RouterClient({
 *   network: "testnet",
 *   coreContractId: "C...",
 *   keypair: Keypair.fromSecret("S..."),
 * });
 *
 * const address = await client.resolve("oracle");
 * await client.registerRoute("oracle", address);
 */
export class RouterClient {
  /** The resolved network configuration in use. */
  readonly network: NetworkConfig;
  /** The router-core client. */
  readonly core: RouterCoreClient;
  /** The router-quote client, when `quoteContractId` was provided. */
  readonly quote: RouterQuoteClient | null;

  constructor(options: RouterClientOptions) {
    const preset = resolveNetwork(options.network ?? "testnet");
    this.network = withOverrides(preset, {
      rpcUrl: options.rpcUrl,
      networkPassphrase: options.networkPassphrase,
    });

    assertContractId(options.coreContractId);
    if (options.quoteContractId) assertContractId(options.quoteContractId);

    const signer: Signer | undefined =
      options.signer ?? (options.keypair ? new LocalSigner(options.keypair) : undefined);
    const rpc: RpcLike = options.rpc ?? createRpcServer(this.network.rpcUrl);
    const shared = {
      rpc,
      networkPassphrase: this.network.networkPassphrase,
      signer,
      fee: options.fee,
      timeoutSeconds: options.timeoutSeconds,
      pollIntervalMs: options.pollIntervalMs,
    };

    this.core = new RouterCoreClient({ ...shared, contractId: options.coreContractId });
    this.quote = options.quoteContractId
      ? new RouterQuoteClient({ ...shared, contractId: options.quoteContractId })
      : null;
  }

  // ── router-core (docs/sdk.md surface) ─────────────────────────────────────

  /** Resolve a route name (or alias) to its contract address. */
  resolve(name: string): Promise<string> {
    return this.core.resolve(name);
  }

  /** Register a new route (admin). */
  registerRoute(name: string, address: string, metadata?: RouteMetadataInput): Promise<void> {
    return this.core.registerRoute(name, address, metadata);
  }

  /** Register multiple routes atomically (admin). */
  registerRoutesBatch(routes: RegisterRouteInput[]): Promise<void> {
    return this.core.registerRoutesBatch(routes);
  }

  /** Point an existing route at a new address (admin). */
  updateRoute(name: string, newAddress: string): Promise<void> {
    return this.core.updateRoute(name, newAddress);
  }

  /** Remove a route (admin). */
  removeRoute(name: string): Promise<void> {
    return this.core.removeRoute(name);
  }

  /** Pause/unpause a specific route (admin). */
  setRoutePaused(name: string, paused: boolean): Promise<void> {
    return this.core.setRoutePaused(name, paused);
  }

  /** Pause/unpause the whole router (admin). */
  setPaused(paused: boolean): Promise<void> {
    return this.core.setPaused(paused);
  }

  /** Fetch a route entry, or `null` when not found. */
  getRoute(name: string): Promise<RouteEntry | null> {
    return this.core.getRoute(name);
  }

  /** Total number of successful `resolve` invocations. */
  totalRouted(): Promise<bigint> {
    return this.core.totalRouted();
  }

  // ── router-quote ──────────────────────────────────────────────────────────

  /** Fetch a single quote (requires `quoteContractId`). */
  async getQuote(request: QuoteRequest): Promise<QuoteResponse> {
    return this.quoteClient().getQuote(request);
  }

  /** Fetch quotes for several requests (requires `quoteContractId`). */
  async getQuotes(requests: QuoteRequest[]): Promise<QuoteResponse[]> {
    return this.quoteClient().getQuotes(requests);
  }

  /** Fetch the quote with the highest `amount_out` (requires `quoteContractId`). */
  async getBestQuote(requests: QuoteRequest[]): Promise<QuoteResponse> {
    return this.quoteClient().getBestQuote(requests);
  }

  /** Compare quotes under a price-impact threshold (requires `quoteContractId`). */
  async compareQuotes(requests: QuoteRequest[], maxPriceImpactBps: number | bigint): Promise<QuoteResponse[]> {
    return this.quoteClient().compareQuotes(requests, maxPriceImpactBps);
  }

  /** Fee (bps) for a route (requires `quoteContractId`). */
  async getRouteFee(route: string): Promise<number> {
    return this.quoteClient().getRouteFee(route);
  }

  private quoteClient(): RouterQuoteClient {
    if (!this.quote) {
      throw new RouterSdkError(
        "QuoteContractNotConfigured",
        "Quote methods require a `quoteContractId` in the RouterClient options.",
      );
    }
    return this.quote;
  }
}