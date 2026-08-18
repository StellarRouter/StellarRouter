import type { NetworkConfig, NetworkName } from "./types.js";

/**
 * Default connection parameters for supported Stellar networks.
 *
 * Consumers with a private or stand-alone RPC endpoint should pass a full
 * {@link NetworkConfig} instead of relying on these presets.
 */
export const NETWORKS: Record<NetworkName, NetworkConfig> = {
  testnet: {
    rpcUrl: "https://soroban-testnet.stellar.org",
    networkPassphrase: "Test SDF Network ; September 2015",
  },
  futurenet: {
    rpcUrl: "https://rpc-futurenet.stellar.org",
    networkPassphrase: "Futurenet Network ; September 2024",
  },
  mainnet: {
    rpcUrl: "https://soroban-mainnet.stellar.org",
    networkPassphrase: "Public Global Stellar Network ; September 2015",
  },
  local: {
    rpcUrl: "http://localhost:8000",
    networkPassphrase: "Standalone Network ; February 2017",
  },
};

/** Applies any overrides to a network preset, producing a concrete config. */
export function withOverrides(
  network: NetworkConfig,
  overrides: Partial<Pick<NetworkConfig, "rpcUrl" | "networkPassphrase">>,
): NetworkConfig {
  return {
    rpcUrl: overrides.rpcUrl ?? network.rpcUrl,
    networkPassphrase: overrides.networkPassphrase ?? network.networkPassphrase,
  };
}

/**
 * Resolve a named preset (or an explicit config object) into concrete
 * connection parameters.
 *
 * @throws if `network` names a preset that is not known to the SDK.
 */
export function resolveNetwork(network: string | NetworkConfig): NetworkConfig {
  if (typeof network !== "string") {
    return network;
  }
  const key = network.toLowerCase() as NetworkName;
  const preset = NETWORKS[key];
  if (!preset) {
    throw new Error(
      `Unknown network "${network}". Use one of ${Object.keys(NETWORKS).join(", ")} ` +
        "or pass an explicit { rpcUrl, networkPassphrase } config.",
    );
  }
  return preset;
}