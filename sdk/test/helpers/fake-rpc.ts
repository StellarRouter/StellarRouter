import { Account, Keypair, Transaction, xdr } from "@stellar/stellar-sdk";

import type { RpcLike } from "../../src/rpc.js";
import { extractInvocation } from "./invoke.js";

export interface SimulateAnswer {
  retval?: xdr.ScVal;
  error?: string;
}

/**
 * An in-memory {@link RpcLike} for unit tests.
 *
 * Simulations are dispatched by the invoked method name; each method can be
 * scripted with a return ScVal or a contract error string. Submission and
 * status polling are scripted via {@link getTransactionStatuses}.
 */
export class FakeRpc implements RpcLike {
  /** Per-method simulation answers, keyed by the contract method name. */
  readonly simulate: Map<string, SimulateAnswer> = new Map();
  /** Queued `getTransaction` statuses; the last one is reused for polls. */
  getTransactionStatuses: Array<{ status: "SUCCESS" | "FAILED" | "NOT_FOUND"; returnValue?: xdr.ScVal }> = [];
  /** Override for `sendTransaction` response status. */
  sendStatus = "PENDING";
  /** Throw this from `simulateTransaction` when set (network-level failure). */
  simulateError?: Error;

  readonly account = new Account(Keypair.random().publicKey(), "0");
  prepareCalls: Transaction[] = [];
  sendCalls: Transaction[] = [];
  getTransactionCalls = 0;

  /** The invocation captured from the most recent `prepareTransaction` call. */
  lastInvocation: ReturnType<typeof extractInvocation> = null;

  async getAccount(): Promise<Account> {
    return this.account;
  }

  async simulateTransaction(tx: Transaction): Promise<{ error?: string; result?: { retval?: xdr.ScVal } }> {
    this.lastInvocation = extractInvocation(tx);
    if (this.simulateError) throw this.simulateError;
    const invocation = extractInvocation(tx);
    const answer = invocation ? this.simulate.get(invocation.method) : undefined;
    if (!answer) return { result: { retval: xdr.ScVal.scvVoid() } };
    return { error: answer.error, result: answer.retval ? { retval: answer.retval } : undefined };
  }

  async prepareTransaction(tx: Transaction): Promise<Transaction> {
    this.lastInvocation = extractInvocation(tx);
    this.prepareCalls.push(tx);
    return tx;
  }

  async sendTransaction(tx: Transaction): Promise<{ status: string; hash?: string }> {
    this.sendCalls.push(tx);
    return { status: this.sendStatus, hash: "ff00112233445566778899aabbccddeeff00112233445566778899aabbccddee" };
  }

  async getTransaction(): Promise<{ status: string; returnValue?: xdr.ScVal }> {
    this.getTransactionCalls += 1;
    if (this.getTransactionStatuses.length === 0) {
      return { status: "SUCCESS" };
    }
    const next = this.getTransactionStatuses[0] ?? { status: "NOT_FOUND" };
    if (this.getTransactionStatuses.length > 1) {
      this.getTransactionStatuses.shift();
    }
    return next;
  }
}