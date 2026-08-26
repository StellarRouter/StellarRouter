import { Keypair, Networks, xdr } from "@stellar/stellar-sdk";

import { ContractInvoker } from "../src/contract.js";
import { RouterSdkError } from "../src/errors.js";
import { toStringVal, toU32, toU64 } from "../src/scval.js";import { LocalSigner } from "../src/signer.js";
import { FakeRpc } from "./helpers/fake-rpc.js";

const CONTRACT_ID = "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4";
const keypair = Keypair.random();

function makeInvoker(overrides: Partial<ConstructorParameters<typeof ContractInvoker>[0]> = {}) {
  const fake = new FakeRpc();
  const invoker = new ContractInvoker({
    contractId: CONTRACT_ID,
    rpc: fake,
    networkPassphrase: Networks.TESTNET,
    signer: new LocalSigner(keypair),
    label: "core",
    timeoutSeconds: 1,
    ...overrides,
  });
  return { fake, invoker };
}

describe("ContractInvoker", () => {
  describe("simulate", () => {
    it("returns the decoded value on success", async () => {
      const { fake, invoker } = makeInvoker();
      fake.simulate.set("total_routed", { retval: toU64(42n) });
      await expect(invoker.simulate("total_routed", [])).resolves.toBe(42n);
    });

    it("returns undefined when there is no result value", async () => {
      const { fake, invoker } = makeInvoker();
      fake.simulate.set("noop", {});
      await expect(invoker.simulate("noop", [])).resolves.toBeUndefined();
    });

    it("maps an on-chain contract error to its SDK error code", async () => {
      const { fake, invoker } = makeInvoker();
      fake.simulate.set("resolve", { error: "HostError: Error(Contract, #4)" });
      await expect(invoker.simulate("resolve", [toStringVal("x")])).rejects.toMatchObject({
        code: "RouteNotFound",
      });
    });

    it("wraps non-contract simulation errors", async () => {
      const { fake, invoker } = makeInvoker();
      fake.simulate.set("resolve", { error: "host error: ValueNotFound" });
      await expect(invoker.simulate("resolve", [toStringVal("x")])).rejects.toMatchObject({
        code: "SimulationFailed",
      });
    });

    it("wraps network-level RPC failures", async () => {
      const { fake, invoker } = makeInvoker();
      fake.simulateError = new Error("connection refused");
      await expect(invoker.simulate("resolve", [toStringVal("x")])).rejects.toMatchObject({
        code: "RpcError",
      });
    });
  });

  describe("invoke", () => {
    it("prepares, signs, submits, and confirms a call", async () => {
      const { fake, invoker } = makeInvoker();
      fake.getTransaction = async () => ({ status: "SUCCESS" });
      await expect(invoker.invoke("register_route", [toU32(1), toStringVal("oracle")])).resolves.toBeUndefined();
      expect(fake.prepareCalls).toHaveLength(1);
      expect(fake.sendCalls).toHaveLength(1);
      expect(fake.lastInvocation?.method).toBe("register_route");
      expect(fake.lastInvocation?.args).toHaveLength(2);
    });

    it("returns the confirmed return value when present", async () => {
      const { fake, invoker } = makeInvoker();
      fake.getTransaction = async () => ({ status: "SUCCESS", returnValue: toU32(7) });
      await expect(invoker.invoke("do_something", [])).resolves.toBe(7);
    });

    it("throws NoSigner when no signer is configured", async () => {
      const { invoker } = makeInvoker({ signer: undefined });
      await expect(invoker.invoke("register_route", [])).rejects.toMatchObject({ code: "NoSigner" });
    });

    it("throws TransactionRejected when the network rejects submission", async () => {
      const { fake, invoker } = makeInvoker();
      fake.sendStatus = "ERROR";
      await expect(invoker.invoke("do_something", [])).rejects.toMatchObject({
        code: "TransactionRejected",
      });
    });

    it("throws TryAgainLater when the network asks for a retry", async () => {
      const { fake, invoker } = makeInvoker();
      fake.sendStatus = "TRY_AGAIN_LATER";
      await expect(invoker.invoke("do_something", [])).rejects.toMatchObject({
        code: "TryAgainLater",
      });
    });

    it("throws TransactionFailed when the transaction fails on-chain", async () => {
      const { fake, invoker } = makeInvoker();
      fake.getTransactionStatuses = [{ status: "FAILED" }];
      await expect(invoker.invoke("do_something", [])).rejects.toMatchObject({
        code: "TransactionFailed",
      });
    });

    it("throws TransactionTimeout when confirmation never arrives", async () => {
      const { fake, invoker } = makeInvoker();
      fake.getTransactionStatuses = [{ status: "NOT_FOUND" }, { status: "NOT_FOUND" }];
      const fast = new ContractInvoker({
        contractId: CONTRACT_ID,
        rpc: fake,
        networkPassphrase: Networks.TESTNET,
        signer: new LocalSigner(keypair),
        timeoutSeconds: 0.1,
        pollIntervalMs: 10,
      });
      await expect(fast.invoke("do_something", [])).rejects.toMatchObject({ code: "TransactionTimeout" });
    });
  });

  describe("configuration", () => {
    it("rejects an invalid contract id", () => {
      expect(
        () =>
          new ContractInvoker({
            contractId: "not-a-contract",
            rpc: new FakeRpc(),
            networkPassphrase: Networks.TESTNET,
          }),
      ).toThrow(RouterSdkError);
    });

    it("rejects missing rpc/network config", () => {
      expect(
        () =>
          new ContractInvoker({
            contractId: CONTRACT_ID,
            rpc: undefined as never,
            networkPassphrase: Networks.TESTNET,
          }),
      ).toThrow(/rpc/);
    });
  });
});