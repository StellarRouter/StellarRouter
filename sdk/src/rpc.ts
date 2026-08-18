import { rpc } from "@stellar/stellar-sdk";
import type { Account, Transaction, xdr } from "@stellar/stellar-sdk";

/**
 * The subset of the Soroban RPC surface used by the SDK, expressed as an
 * interface so callers can inject a mock/stub for unit tests.
 */
export interface RpcLike {
  /** Load the account with the given public key (for building transactions). */
  getAccount(address: string): Promise<Account>;
  /** Simulate a transaction, returning the (possibly failed) simulation. */
  simulateTransaction(tx: Transaction): Promise<{
    error?: string;
    result?: { retval?: xdr.ScVal };
  }>;
  /** Prepare (simulate + authorise) a transaction for submission. */
  prepareTransaction(tx: Transaction): Promise<Transaction>;
  /** Submit a signed transaction. */
  sendTransaction(tx: Transaction): Promise<{
    status: string;
    hash?: string;
    errorResult?: xdr.TransactionResult;
  }>;
  /** Fetch the status of a previously submitted transaction. */
  getTransaction(hash: string): Promise<{
    status: string;
    returnValue?: xdr.ScVal;
    resultXdr?: xdr.TransactionResult;
  }>;
}

/** Create an {@link RpcLike} backed by a real Soroban RPC server. */
export function createRpcServer(rpcUrl: string): RpcLike {
  return new rpc.Server(rpcUrl) as unknown as RpcLike;
}