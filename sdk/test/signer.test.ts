import { Account, Contract, Keypair, Networks, TransactionBuilder, xdr } from "@stellar/stellar-sdk";

import { RouterSdkError } from "../src/errors.js";
import { FreighterSigner, LocalSigner, Signer } from "../src/signer.js";

const CONTRACT_ID = "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4";

function buildTx(kp: Keypair): import("@stellar/stellar-sdk").Transaction {
  return new TransactionBuilder(new Account(kp.publicKey(), "0"), {
    fee: "100",
    networkPassphrase: Networks.TESTNET,
  })
    .setTimeout(30)
    .addOperation(new Contract(CONTRACT_ID).call("noop", xdr.ScVal.scvVoid()))
    .build();
}

describe("LocalSigner", () => {
  it("exposes the source account public key", () => {
    const kp = Keypair.random();
    const signer = new LocalSigner(kp);
    expect(signer.publicKey()).toBe(kp.publicKey());
  });

  it("signs a transaction with its keypair", async () => {
    const kp = Keypair.random();
    const signer = new LocalSigner(kp);
    const tx = buildTx(kp);
    await signer.sign(tx);
    expect(tx.signatures).toHaveLength(1);
    expect(kp.verify(tx.hash(), tx.signatures[0]!.signature())).toBe(true);
  });

  it("implements the Signer interface contract", () => {
    const signer: Signer = new LocalSigner(Keypair.random());
    expect(typeof signer.publicKey).toBe("function");
    expect(typeof signer.sign).toBe("function");
  });
});

describe("FreighterSigner", () => {
  it("surfaces failures while reading the public key as a RouterSdkError", async () => {
    const signer = new FreighterSigner();
    const err = await signer.publicKey().catch((e: unknown) => e);
    expect(err).toBeInstanceOf(RouterSdkError);
    expect((err as RouterSdkError).code).toBe("SigningFailed");
  });

  it("wraps import failures as a RouterSdkError when signing", async () => {
    const signer = new FreighterSigner();
    const tx = buildTx(Keypair.random());
    const err = await signer.sign(tx).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(RouterSdkError);
    // The @stellar/freighter-api peer is not installed in CI, so the lazy
    // dynamic import fails for real here — reachable in a browser context
    // where the package is bundled.
  });
});