# Troubleshooting

Common issues and how to resolve them.

---

## RPC Connection Errors

**Symptom:** `stellar contract invoke` or the metrics exporter fails with a connection
error, timeout, or "endpoint not reachable" message.

**Causes and fixes:**

1. Wrong RPC URL — verify `ROUTER_RPC_URL` points to a live Soroban RPC endpoint.
   - Testnet: `https://soroban-testnet.stellar.org`
   - Mainnet: `https://soroban-mainnet.stellar.org`

2. Network is down — check the [Stellar status page](https://status.stellar.org).

3. Firewall or proxy blocking outbound HTTPS — ensure port 443 is open.

4. Metrics exporter shows no data after startup:
   ```bash
   docker compose logs metrics-exporter
   ```
   Look for `scrape error` or `connection refused` lines. Confirm the contract IDs
   are set and the RPC URL is reachable from inside the container.

---

## Contract Not Initialized

**Symptom:** A contract call returns `NotInitialized` or panics with `"not initialized"`.

**Cause:** The contract's `initialize` function has not been called after deployment.

**Fix:** Call `initialize` with the admin address before any other function:

```bash
stellar contract invoke --id <CONTRACT_ID> --network testnet --source admin \
  -- initialize --admin <ADMIN_ADDRESS>
```

Each contract in the suite requires its own `initialize` call. Deploy and initialize
them in dependency order (see [`docs/deployment.md`](deployment.md)).

**Note:** Calling `initialize` a second time returns `AlreadyInitialized`. This is
expected — it is a guard against accidental re-initialization.

---

## Auth Failures

**Symptom:** A contract call returns `Unauthorized` or `HostError: auth failed`.

**Common causes:**

1. Wrong signer — the `--source` account must match the admin address passed to
   `initialize`. Verify with:
   ```bash
   stellar contract invoke --id <CONTRACT_ID> --network testnet --source admin \
     -- admin
   ```

2. Role not granted — for `router-access`-protected routes, the caller must hold
   the required role. Grant it first:
   ```bash
   stellar contract invoke --id <ACCESS_ID> --network testnet --source admin \
     -- grant_role --caller <ADMIN> --role operator --address <CALLER>
   ```

3. Address is blacklisted — check with `has_role` and remove the blacklist entry
   if needed.

4. Timelock not yet ready — operations queued in `router-timelock` cannot be
   executed before their ETA. Check the operation status and wait for the delay
   to pass.

---

## Docker Setup Problems

**Symptom:** `docker compose run tests` or `docker compose up` fails.

**Common causes and fixes:**

1. Docker daemon not running:
   ```bash
   sudo systemctl start docker
   # or on macOS/Windows: start Docker Desktop
   ```

2. Port conflict — Grafana (3000) or Prometheus (9091) already in use:
   ```bash
   # Check what is using the port
   lsof -i :3000
   # Change the port in docker-compose.yml if needed
   ```

3. Missing environment variables for the metrics stack — copy the example file:
   ```bash
   cp metrics/.env.example metrics/.env
   # Edit metrics/.env and fill in contract IDs
   ```

4. WASM build fails inside the container — the `wasm32-unknown-unknown` target
   may not be installed in the image. Rebuild the image:
   ```bash
   docker compose build --no-cache
   ```

5. Tests fail with `AlreadyInitialized` — each test must use a fresh `Env::default()`
   and register a new contract instance. The Soroban test environment is isolated
   per `Env`, so this should not happen unless a test helper is reusing state.

---

## Build Errors

**`wasm32-unknown-unknown` target not found:**
```bash
rustup target add wasm32-unknown-unknown
```

**`stellar` command not found:**
```bash
cargo install --locked stellar-cli
```

**`cargo build` fails with linker errors on Linux:**
Install the required C toolchain:
```bash
sudo apt-get install build-essential
```

---

## Testnet Account Issues

**`stellar contract deploy` fails with "account not found":**

Fund your testnet account using Stellar Friendbot:
```
https://friendbot.stellar.org/?addr=<YOUR_ADDRESS>
```

Or via the CLI:
```bash
stellar keys fund <ACCOUNT_NAME> --network testnet
```

---

## Getting More Help

- [Stellar Developer Docs](https://developers.stellar.org/docs/build/smart-contracts/overview)
- [Soroban Discord](https://discord.gg/stellardev) — `#soroban` channel
- [GitHub Issues](https://github.com/Maki-Zeninn/stellar-router/issues) — open a bug report
