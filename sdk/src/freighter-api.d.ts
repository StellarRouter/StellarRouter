/**
 * Minimal ambient types for the optional `@stellar/freighter-api` peer
 * dependency. The module is only loaded lazily (in browsers) when a
 * {@link FreighterSigner} is used, so it is never required in Node.
 */
declare module "@stellar/freighter-api" {
  export function getPublicKey(opts?: { network?: "PUBLIC" | "TESTNET" | "FUTURENET" }): Promise<string>;
  export function signTransaction(
    transactionXdr: string,
    opts?: {
      network?: "PUBLIC" | "TESTNET" | "FUTURENET";
      networkPassphrase?: string;
      accountToSign?: string;
    },
  ): Promise<string>;
}