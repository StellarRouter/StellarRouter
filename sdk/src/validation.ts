/**
 * Input validation that mirrors the on-chain rules, so SDK callers get the
 * same error names as the contracts, before paying for a round-trip.
 */

import { RouterSdkError } from "./errors.js";

const CONTRACT_ID_RE = /^C[0-9A-Z]{55}$/;
const ROUTE_NAME_RE = /^[A-Za-z0-9/-]{1,64}$/;

/** Assert `id` looks like a Soroban contract id (`C...`). */
export function assertContractId(id: string): void {
  if (!CONTRACT_ID_RE.test(id)) {
    throw new RouterSdkError(
      "InvalidContractId",
      `"${id}" is not a valid Soroban contract id (expected a "C{55}" string).`,
      { contract: "core" },
    );
  }
}

/** Assert `name` matches the router's route-name rules (1-64 chars, [A-Za-z0-9/-]). */
export function assertRouteName(name: string): void {
  if (!ROUTE_NAME_RE.test(name)) {
    throw new RouterSdkError(
      "InvalidRouteName",
      `Route name "${name}" is invalid: use 1-64 characters from [A-Za-z0-9/-].`,
      { contract: "core" },
    );
  }
}