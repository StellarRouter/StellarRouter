# Transaction Signing Abstraction

A flexible signing interface that supports local keypairs, hardware wallets,
and external signers (e.g. Freighter, WalletConnect).

## Signer Interface (TypeScript)
interface Signer {
  publicKey(): string;
  sign(transaction: Transaction): Promise<Transaction>;
}

## Built-in Implementations

### LocalSigner — signs with a Stellar keypair in memory
class LocalSigner implements Signer {
  constructor(private keypair: Keypair) {}
  publicKey() { return this.keypair.publicKey(); }
  async sign(tx: Transaction) {
    tx.sign(this.keypair);
    return tx;
  }
}

### FreighterSigner — delegates to the Freighter browser extension
class FreighterSigner implements Signer {
  async publicKey() { return await getPublicKey(); }
  async sign(tx: Transaction) {
    const network = process.env.STELLAR_NETWORK || "TESTNET";
    const signed = await signTransaction(tx.toXDR(), { network });
    return TransactionBuilder.fromXDR(signed, network === "PUBLIC" ? Networks.PUBLIC : Networks.TESTNET);
  }
}

## Usage with RouterClient
import { RouterClient, LocalSigner } from "stellar-router-sdk";

const signer = new LocalSigner(Keypair.fromSecret("S..."));
const network = process.env.STELLAR_NETWORK?.toLowerCase() || "testnet";
const client = new RouterClient({ network, coreContractId: "C...", signer });

await client.registerRoute("oracle", "C...");

## Adding a Custom Signer
Implement the Signer interface and pass it to RouterClient.
Any signing method is supported as long as it returns a signed Transaction.
