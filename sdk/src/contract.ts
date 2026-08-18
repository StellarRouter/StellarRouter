import {
  Account,
  BASE_FEE,
  Contract,
  Keypair,
  Transaction,
  TransactionBuilder,
  scValToNative,
} from "@stellar/stellar-sdk";
import type { xdr } from "@stellar/stellar-sdk";

import { RouterSdkError, parseContractErrorCode } from "./errors.js";
import type { RpcLike } from "./rpc.js";
import type { Signer } from "./signer.js";
import type { ScVal } from "./types.js";
import { assertContractId } from "./validation.js";

/** Options for {@link ContractInvoker}. */
export interface ContractInvokerOptions {
  /** The Soroban contract id (`C...`) to invoke. */
  contractId: string;
  /** RPC client the invoker talks to. */
  rpc: RpcLike;
  /** Network passphrase used when building transactions. */
  networkPassphrase: string;
  /** Optional signer used to authorise state-changing calls. */
  signer?: Signer;
  /** Base fee for the transaction (defaults to {@link BASE_FEE}). */
  fee?: string | number;
  /** How long to poll for a submitted transaction before giving up (seconds). */
  timeoutSeconds?: number;
  /** Delay between transaction-status polls (milliseconds). */
  pollIntervalMs?: number;
  /** Short label used in error messages (e.g. `"core"`, `"quote"`). */
  label?: string;
}

/** Options for read-only {@link ContractInvoker.simulate}. */
export interface SimulateOptions {
  /** Override the source account public key used to build the simulation. */
  source?: string;
}

/** Options for state-changing {@link ContractInvoker.invoke}. */
export type InvokeOptions = SimulateOptions;

const DEFAULT_TIMEOUT_SECONDS = 60;
export { DEFAULT_TIMEOUT_SECONDS };

const sleep = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));

/**
 * Low-level contract invocation helper.
 *
 * This is the primitive the higher-level router clients are built on. It
 * exposes two flavours of call against a Soroban RPC endpoint:
 *
 * - {@link simulate} — build + simulate a transaction and return its result
 *   value. Suitable for read-only methods (`resolve`, `get_quote`, `admin`,
 *   ...).
 * - {@link invoke} — prepare, sign, submit and confirm a transaction. Used for
 *   state-changing, authenticated methods (`register_route`, `get_quote` is
 *   not a write but `set_*` methods are, ...).
 *
 * Both surface failures as {@link RouterSdkError}, including mapping on-chain
 * `contracterror` codes to their names (e.g. `RouteNotFound`).
 */
export class ContractInvoker {
  /** The contract id this invoker targets. */
  readonly contractId: string;
  /** Network passphrase in use. */
  readonly networkPassphrase: string;
  /** The signer used to authenticate writes, if one was provided. */
  readonly signer?: Signer;

  private readonly rpc: RpcLike;
  private readonly contract: Contract;
  private readonly fee: string;
  private readonly timeoutMs: number;
  private readonly pollIntervalMs: number;
  private readonly label: string;

  constructor(options: ContractInvokerOptions) {
    assertContractId(options.contractId);
    if (!options.rpc) {
      throw new RouterSdkError(
        "InvalidConfiguration",
        "A `rpc` client is required when constructing a ContractInvoker.",
      );
    }
    if (!options.networkPassphrase) {
      throw new RouterSdkError(
        "InvalidConfiguration",
        "A `networkPassphrase` is required when constructing a ContractInvoker.",
      );
    }
    this.contractId = options.contractId;
    this.rpc = options.rpc;
    this.signer = options.signer;
    this.networkPassphrase = options.networkPassphrase;
    this.fee = options.fee === undefined ? BASE_FEE : String(options.fee);
    this.timeoutMs = (options.timeoutSeconds ?? 60) * 1000;
    this.pollIntervalMs = options.pollIntervalMs ?? 1000;
    this.label = options.label ?? this.contractId;
    this.contract = new Contract(this.contractId);
  }

  /**
   * Simulate a read-only call and return the decoded result value.
   *
   * @param method name of the contract function.
   * @param args encoded ScVal arguments.
   * @param options optional source-account override.
   * @throws {@link RouterSdkError} when the simulation fails.
   */
  async simulate<T = unknown>(method: string, args: ScVal[], options: SimulateOptions = {}): Promise<T> {
    const account = await this.sourceAccount(options.source);
    const tx = this.buildTransaction(account, method, args);
    let sim;
    try {
      sim = await this.rpc.simulateTransaction(tx);
    } catch (cause) {
      throw RouterSdkError.fromCause(this.label, cause);
    }
    this.throwIfSimulationFailed(sim);
    const retval = sim.result?.retval;
    return retval === undefined ? (undefined as T) : (scValToNative(retval) as T);
  }

  /**
   * Invoke a state-changing call: prepare, sign, submit, and wait for the
   * transaction to be confirmed.
   *
   * A {@link Signer} (or `source`) must be available.
   *
   * @param method name of the contract function.
   * @param args encoded ScVal arguments.
   * @param options optional source-account override.
   * @returns the decoded result value, or `undefined` for void methods.
   * @throws {@link RouterSdkError} if simulation/preparation, submission, or
   * confirmation fails.
   */
  async invoke<T = unknown>(method: string, args: ScVal[], options: InvokeOptions = {}): Promise<T> {
    const publicKey = await this.writePublicKey(method, options.source);
    const account = await this.sourceAccount(publicKey);
    const tx = this.buildTransaction(account, method, args);

    let prepared: Transaction;
    try {
      prepared = await this.rpc.prepareTransaction(tx);
    } catch (cause) {
      throw RouterSdkError.fromCause(this.label, cause);
    }

    let signed: Transaction;
    try {
      signed = await this.signTransaction(prepared);
    } catch (cause) {
      if (cause instanceof RouterSdkError) throw cause;
      throw RouterSdkError.fromCause(this.label, cause);
    }

    let response;
    try {
      response = await this.rpc.sendTransaction(signed);
    } catch (cause) {
      throw RouterSdkError.fromCause(this.label, cause);
    }

    if (response.status === "ERROR") {
      const code = this.transactionResultCode(response.errorResult);
      throw new RouterSdkError("TransactionRejected", "The network rejected the transaction.", {
        contract: this.label,
        cause: new Error(`result code: ${code ?? "unknown"}`),
        rawError: code,
      });
    }
    if (response.status === "TRY_AGAIN_LATER") {
      throw new RouterSdkError(
        "TryAgainLater",
        "The network asked the client to retry later. Please try again.",
        { contract: this.label },
      );
    }

    const hash = response.hash;
    if (!hash) {
      throw new RouterSdkError(
        "TransactionRejected",
        `The network did not return a transaction hash (status: ${response.status}).`,
        { contract: this.label },
      );
    }

    const result = await this.waitForTerminalStatus(hash);

    if (result.status === "FAILED") {
      const code = this.transactionResultCode(result.resultXdr);
      throw new RouterSdkError("TransactionFailed", `Transaction ${hash} failed on-chain.`, {
        contract: this.label,
        rawError: code,
      });
    }

    const retval = result.returnValue;
    return retval === undefined ? (undefined as T) : (scValToNative(retval) as T);
  }

  private async writePublicKey(method: string, source?: string): Promise<string> {
    if (source) return source;
    if (this.signer) return this.signer.publicKey();
    throw new RouterSdkError(
      "NoSigner",
      `Calling "${method}" on ${this.label} requires an authenticated account. ` +
        "Provide a signer (or a keypair) when constructing the client.",
      { contract: this.label },
    );
  }

  private async sourceAccount(publicKey?: string): Promise<Account> {
    const key =
      publicKey ?? (this.signer ? await this.signer.publicKey() : undefined) ?? Keypair.random().publicKey();
    try {
      return await this.rpc.getAccount(key);
    } catch {
      // Simulation-only fallback: an on-chain missing source account still
      // builds a valid Soroban transaction for read calls. Authenticated
      // writes will surface a network error instead of this fallback.
      return new Account(key, "0");
    }
  }

  private buildTransaction(account: Account, method: string, args: ScVal[]): Transaction {
    return new TransactionBuilder(account, {
      fee: this.fee,
      networkPassphrase: this.networkPassphrase,
    })
      .setTimeout(0)
      .addOperation(this.contract.call(method, ...args))
      .build();
  }

  private async signTransaction(tx: Transaction): Promise<Transaction> {
    if (!this.signer) {
      throw new RouterSdkError(
        "NoSigner",
        `A signer is required to submit transactions for ${this.label}.`,
        { contract: this.label },
      );
    }
    return this.signer.sign(tx);
  }

  private throwIfSimulationFailed(sim: { error?: string }): void {
    if (!sim.error) return;
    const code = parseContractErrorCode(sim.error);
    if (code === undefined) {
      throw new RouterSdkError("SimulationFailed", "The contract simulation failed: " + sim.error, {
        contract: this.label,
        rawError: sim.error,
      });
    }
    throw RouterSdkError.fromContractCode(code, { contract: this.label, rawError: sim.error });
  }

  private async waitForTerminalStatus(hash: string): Promise<{
    status: string;
    returnValue?: xdr.ScVal;
    resultXdr?: xdr.TransactionResult;
  }> {
    const deadline = Date.now() + this.timeoutMs;
    for (;;) {
      let response;
      try {
        response = await this.rpc.getTransaction(hash);
      } catch (cause) {
        throw RouterSdkError.fromCause(this.label, cause);
      }
      if (response.status === "SUCCESS" || response.status === "FAILED") {
        return response;
      }
      if (Date.now() >= deadline) {
        throw new RouterSdkError(
          "TransactionTimeout",
          `Timed out waiting for transaction ${hash} after ${this.timeoutMs / 1000}s.`,
          { contract: this.label },
        );
      }
      await sleep(this.pollIntervalMs);
    }
  }

  private transactionResultCode(resultXdr?: xdr.TransactionResult): string | undefined {
    if (!resultXdr) return undefined;
    try {
      return resultXdr.result().switch().name;
    } catch {
      return undefined;
    }
  }
}