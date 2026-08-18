import { Keypair } from "@stellar/stellar-sdk";

import { RouterClient } from "../src/index.js";
import { RouterSdkError } from "../src/errors.js";

/**
 * Minimal end-to-end example for the stellar-router-sdk.
 *
 * Point the client at a real network and real contract ids to use it. As
 * written it only proves that construction + error surfacing work offline:
 * the RPC calls fail fast because there is no network connection here.
 */
async function main(): Promise<void> {
  const client = new RouterClient({
    network: "testnet",
    coreContractId: "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4",
    quoteContractId: "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABBMO",
    keypair: Keypair.random(),
  });

  console.log(`network: ${client.network.rpcUrl}`);
  console.log(`core:    ${client.core.invoker.contractId}`);

  try {
    const address = await client.resolve("oracle");
    console.log(`oracle resolves to ${address}`);
  } catch (error) {
    if (error instanceof RouterSdkError) {
      console.error(`resolve failed with code ${error.code}: ${error.message}`);
    } else {
      throw error;
    }
  }
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});