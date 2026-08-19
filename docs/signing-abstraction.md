# Transaction Signing Abstraction

> **Status: planned, not yet implemented.** This describes the signing
> interface for the [stellar-router-sdk](sdk.md), which does not exist in
> this repository yet. `Signer`, `LocalSigner`, and `FreighterSigner` below
> are a design proposal, not shipped code.

A flexible signing interface that supports local keypairs, hardware wallets,
and external signers (e.g. Freighter, WalletConnect).

## Signer Interface (TypeScript)

```typescript
interface Signer {
  publicKey(): string;
  sign(transaction: Transaction): Promise<Transaction>;
}
```

## Built-in Implementations

### LocalSigner — signs with a Stellar keypair in memory

```typescript
class LocalSigner implements Signer {
  constructor(private keypair: Keypair) {}
  publicKey() { return this.keypair.publicKey(); }
  async sign(tx: Transaction) {
    tx.sign(this.keypair);
    return tx;
  }
}
```

### FreighterSigner — delegates to the Freighter browser extension

```typescript
class FreighterSigner implements Signer {
  async publicKey() { return await getPublicKey(); }
  async sign(tx: Transaction) {
    const network = process.env.STELLAR_NETWORK || "TESTNET";
    const signed = await signTransaction(tx.toXDR(), { network });
    return TransactionBuilder.fromXDR(signed, network === "PUBLIC" ? Networks.PUBLIC : Networks.TESTNET);
  }
}
```

## Usage with RouterClient

```typescript
import { RouterClient, LocalSigner } from "stellar-router-sdk";

const signer = new LocalSigner(Keypair.fromSecret("S..."));
const network = process.env.STELLAR_NETWORK?.toLowerCase() || "testnet";
const client = new RouterClient({ network, coreContractId: "C...", signer });

await client.registerRoute("oracle", "C...");
```

## Adding a Custom Signer

Implement the Signer interface and pass it to RouterClient.
Any signing method is supported as long as it returns a signed Transaction.
