import { Keypair, Networks, StrKey, scValToNative, xdr } from "@stellar/stellar-sdk";

import { RouterCoreClient } from "../src/core.js";
import { RouterSdkError } from "../src/errors.js";
import { toAddress, toSymbol, toU32, toU64 } from "../src/scval.js";
import { LocalSigner } from "../src/signer.js";
import { FakeRpc } from "./helpers/fake-rpc.js";

const CORE_ID = "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4";
const keypair = Keypair.random();
const caller = keypair.publicKey();
const TARGET = StrKey.encodeContract(Buffer.from(Array(32).fill(7)));

function makeClient(overrides: { signer?: boolean } = {}) {
  const fake = new FakeRpc();
  const client = new RouterCoreClient({
    contractId: CORE_ID,
    rpc: fake,
    networkPassphrase: Networks.TESTNET,
    signer: overrides.signer === false ? undefined : new LocalSigner(keypair),
  });
  return { fake, client };
}

function routeEntryScVal(address: string, name: string, paused: boolean): xdr.ScVal {
  return xdr.ScVal.scvMap([
    new xdr.ScMapEntry({ key: toSymbol("address"), val: toAddress(address) }),
    new xdr.ScMapEntry({ key: toSymbol("name"), val: xdr.ScVal.scvString(name) }),
    new xdr.ScMapEntry({ key: toSymbol("paused"), val: xdr.ScVal.scvBool(paused) }),
    new xdr.ScMapEntry({ key: toSymbol("updated_by"), val: toAddress(caller) }),
  ]);
}

describe("RouterCoreClient (read)", () => {
  it("resolves a route name to its address", async () => {
    const { fake, client } = makeClient();
    fake.simulate.set("resolve", { retval: toAddress(TARGET) });
    await expect(client.resolve("oracle")).resolves.toBe(TARGET);
    expect(fake.lastInvocation?.method).toBe("resolve");
  });

  it("parses a RouteEntry from get_route", async () => {
    const { fake, client } = makeClient();
    fake.simulate.set("get_route", { retval: routeEntryScVal(TARGET, "oracle", false) });
    await expect(client.getRoute("oracle")).resolves.toEqual({
      address: TARGET,
      name: "oracle",
      paused: false,
      updatedBy: caller,
    });
  });

  it("returns null from get_route when the entry is None", async () => {
    const { fake, client } = makeClient();
    fake.simulate.set("get_route", { retval: xdr.ScVal.scvVoid() });
    await expect(client.getRoute("missing")).resolves.toBeNull();
  });

  it("maps a missing route to RouterSdkError RouteNotFound", async () => {
    const { fake, client } = makeClient();
    fake.simulate.set("get_route", { error: "host error: Error(Contract, #4)" });
    await expect(client.getRoute("nope")).rejects.toBeInstanceOf(RouterSdkError);
    await expect(client.getRoute("nope")).rejects.toMatchObject({ code: "RouteNotFound" });
  });

  it("returns the total routed count as a bigint", async () => {
    const { fake, client } = makeClient();
    fake.simulate.set("total_routed", { retval: toU64(99n) });
    await expect(client.totalRouted()).resolves.toBe(99n);
  });

  it("decodes batch_resolve Ok/Err unions", async () => {
    const { fake, client } = makeClient();
    fake.simulate.set("batch_resolve", {
      retval: xdr.ScVal.scvVec([
        xdr.ScVal.scvMap([new xdr.ScMapEntry({ key: toSymbol("Ok"), val: toAddress(TARGET) })]),
        xdr.ScVal.scvMap([new xdr.ScMapEntry({ key: toSymbol("Err"), val: toSymbol("RouterPaused") })]),
      ]),
    });
    await expect(client.batchResolve(["oracle", "paused"])).resolves.toEqual([
      { ok: true, route: "oracle", address: TARGET },
      { ok: false, route: "paused", error: "RouterPaused" },
    ]);
  });
});

describe("RouterCoreClient (write)", () => {
  it("registers a route with metadata and the admin caller", async () => {
    const { fake, client } = makeClient();
    fake.getTransaction = async () => ({ status: "SUCCESS" });
    await client.registerRoute("oracle", TARGET, { description: "Price feed", tags: ["defi"] });
    const inv = fake.lastInvocation;
    expect(inv?.method).toBe("register_route");
    expect(inv?.args).toHaveLength(4);
    const native = inv ? inv.args.map((a) => scValToNative(a)) : [];
    expect(native[0]).toBe(caller); // caller
    expect(native[1]).toBe("oracle"); // name
    expect(native[2]).toBe(TARGET); // address
    expect(native[3]).toEqual({ description: "Price feed", tags: ["defi"], owner: caller });
  });

  it("omits metadata (None) when not provided", async () => {
    const { fake, client } = makeClient();
    fake.getTransaction = async () => ({ status: "SUCCESS" });
    await client.registerRoute("oracle", TARGET);
    const inv = fake.lastInvocation;
    expect(scValToNative(inv!.args[3]!)).toBeNull();
  });

  it("throws NoSigner for admin-only writes without a signer", async () => {
    const { client } = makeClient({ signer: false });
    await expect(client.registerRoute("oracle", TARGET)).rejects.toMatchObject({ code: "NoSigner" });
  });

  it("encodes paused flag and caller for set_route_paused", async () => {
    const { fake, client } = makeClient();
    fake.getTransaction = async () => ({ status: "SUCCESS" });
    await client.setRoutePaused("oracle", true);
    const inv = fake.lastInvocation;
    expect(inv?.method).toBe("set_route_paused");
    const native = inv ? inv.args.map((a) => scValToNative(a)) : [];
    expect(native[0]).toBe(caller);
    expect(native[1]).toBe("oracle");
    expect(native[2]).toBe(true);
  });

  it("validates route names and contract ids before invoking", async () => {
    const { fake, client } = makeClient();
    fake.getTransaction = async () => ({ status: "SUCCESS" });
    await expect(client.registerRoute("bad name!", TARGET)).rejects.toMatchObject({
      code: "InvalidRouteName",
    });
    await expect(client.registerRoute("oracle", "not-an-address")).rejects.toMatchObject({
      code: "InvalidContractId",
    });
    expect(fake.prepareCalls).toHaveLength(0);
  });

  it("decodes a route_score payload", async () => {
    const { fake, client } = makeClient();
    fake.simulate.set("get_route_score", {
      retval: xdr.ScVal.scvMap([
        new xdr.ScMapEntry({ key: toSymbol("liquidity_score"), val: toU32(80) }),
        new xdr.ScMapEntry({ key: toSymbol("fee_bps"), val: toU32(30) }),
        new xdr.ScMapEntry({ key: toSymbol("reliability_score"), val: toU32(90) }),
      ]),
    });
    await expect(client.getRouteScore("oracle")).resolves.toEqual({
      liquidityScore: 80,
      feeBps: 30,
      reliabilityScore: 90,
    });
  });
});