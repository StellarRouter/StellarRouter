import { Keypair, Networks, Transaction, TransactionBuilder } from "@stellar/stellar-sdk";

import { RouterSdkError } from "./errors.js";

/**
 * Signing abstraction (see `docs/signing-abstraction.md`).
 *
 * Implementations may return the public key synchronously (an in-memory
 * keypair) or asynchronously (a wallet extension).
 */
export interface Signer {
  /** The Stellar public key (`G...`) that authenticates transactions. */
  publicKey(): string | Promise<string>;
  /** Return `transaction` signed by this signer. */
  sign(transaction: Transaction): Promise<Transaction>;
}

/**
 * Signs with an in-memory Stellar {@link Keypair}.
 */
export class LocalSigner implements Signer {
  readonly keypair: Keypair;

  constructor(keypair: Keypair) {
    this.keypair = keypair;
  }

  publicKey(): string {
    return this.keypair.publicKey();
  }

  async sign(transaction: Transaction): Promise<Transaction> {
    try {
      transaction.sign(this.keypair);
      return transaction;
    } catch (cause) {
      throw new RouterSdkError(
        "SigningFailed",
        "Failed to sign the transaction with the provided keypair.",
        { contract: "core", cause },
      );
    }
  }
}

/** Options for {@link FreighterSigner}. */
export interface FreighterSignerOptions {
  /** Network passphrase used to parse the signed transaction XDR. */
  networkPassphrase?: string;
}

/**
 * Delegates signing to the Freighter browser extension.
 *
 * The module is loaded lazily via dynamic `import`, so the package can be
 * used in Node without installing `@stellar/freighter-api`.
 */
export class FreighterSigner implements Signer {
  private readonly networkPassphrase: string;

  constructor(options: FreighterSignerOptions = {}) {
    this.networkPassphrase = options.networkPassphrase ?? Networks.TESTNET;
  }

  async publicKey(): Promise<string> {
    try {
      const { getPublicKey } = await import("@stellar/freighter-api");
      return await getPublicKey();
    } catch (cause) {
      throw new RouterSdkError(
        "SigningFailed",
        "Failed to read the public key from Freighter. Is the extension unlocked?",
        { cause },
      );
    }
  }

  async sign(transaction: Transaction): Promise<Transaction> {
    try {
      const { signTransaction } = await import("@stellar/freighter-api");
      const signedXdr = await signTransaction(transaction.toXDR(), {
        networkPassphrase: this.networkPassphrase,
      });
      return TransactionBuilder.fromXDR(signedXdr, this.networkPassphrase) as Transaction;
    } catch (cause) {
      throw new RouterSdkError(
        "SigningFailed",
        "Failed to sign the transaction with Freighter.",
        { cause },
      );
    }
  }
}