import { Account, StrKey, Transaction, xdr } from "@stellar/stellar-sdk";

export interface InvocationInfo {
  contractId: string;
  method: string;
  args: xdr.ScVal[];
}

/**
 * Extract the contract invocation from a (prepared) Soroban transaction:
 * contract id, method symbol, and argument ScVals.
 */
export function extractInvocation(tx: Transaction): InvocationInfo | null {
  try {
    const envelope = tx.toEnvelope();
    const operation = envelope.v1().tx().operations()[0];
    if (!operation) return null;
    const body = operation.body();
    if (body.switch().name !== "invokeHostFunction") return null;
    const hostFunction = body.invokeHostFunctionOp().hostFunction();
    if (hostFunction.switch().name !== "hostFunctionTypeInvokeContract") return null;
    const invokeContract = hostFunction.invokeContract();
    const contractId = StrKey.encodeContract(Buffer.from(invokeContract.contractAddress().contractId() as unknown as Uint8Array));
    const method = invokeContract.functionName().toString();
    return { contractId, method, args: invokeContract.args() };
  } catch {
    return null;
  }
}