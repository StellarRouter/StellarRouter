import {
  ROUTER_CORE_ERRORS,
  ROUTER_QUOTE_ERRORS,
  RouterSdkError,
  parseContractErrorCode,
} from "../src/errors.js";

describe("parseContractErrorCode", () => {
  it("extracts the numeric code from a Soroban contract error", () => {
    expect(parseContractErrorCode("HostError: Error(Contract, #4)")).toBe(4);
    expect(parseContractErrorCode("host error: Error(Contract, #9). Invalid")).toBe(9);
  });

  it("falls back to a loose Contract match", () => {
    expect(parseContractErrorCode("Contract: 7")).toBe(7);
    expect(parseContractErrorCode("some Contract, #3 text")).toBe(3);
  });

  it("returns undefined for non-contract errors", () => {
    expect(parseContractErrorCode(undefined)).toBeUndefined();
    expect(parseContractErrorCode("")).toBeUndefined();
    expect(parseContractErrorCode("host error: ValueNotFound")).toBeUndefined();
    expect(parseContractErrorCode("ClientError: bad request")).toBeUndefined();
  });
});

describe("error code tables", () => {
  it("mirrors router-core RouterError discriminants", () => {
    expect(ROUTER_CORE_ERRORS[4]).toBe("RouteNotFound");
    expect(ROUTER_CORE_ERRORS[6]).toBe("RouterPaused");
    expect(ROUTER_CORE_ERRORS[7]).toBe("RouteAlreadyExists");
  });

  it("mirrors router-quote QuoteError discriminants", () => {
    expect(ROUTER_QUOTE_ERRORS[4]).toBe("InvalidAmount");
    expect(ROUTER_QUOTE_ERRORS[8]).toBe("InvalidPriceImpactBps");
    expect(ROUTER_QUOTE_ERRORS[9]).toBe("AmountOverflow");
  });
});

describe("RouterSdkError", () => {
  it("maps a known core error code to its name", () => {
    const err = RouterSdkError.fromContractCode(4, { contract: "core" });
    expect(err).toBeInstanceOf(RouterSdkError);
    expect(err).toBeInstanceOf(Error);
    expect(err.code).toBe("RouteNotFound");
    expect(err.contractErrorCode).toBe(4);
    expect(err.message).toContain("router-core");
  });

  it("falls back to a numeric code for unknown errors", () => {
    const err = RouterSdkError.fromContractCode(99, { contract: "core" });
    expect(err.code).toBe("ContractError99");
  });

  it("wraps thrown causes", () => {
    const cause = new Error("boom");
    const err = RouterSdkError.fromCause("core", cause);
    expect(err.code).toBe("RpcError");
    expect(err.cause).toBe(cause);
  });

  it("preserves custom properties on the name/code fields", () => {
    const err = new RouterSdkError("NoSigner", "need a signer", { contract: "quote" });
    expect(err.name).toBe("RouterSdkError");
    expect(err.code).toBe("NoSigner");
    expect(err.contract).toBe("quote");
  });
});