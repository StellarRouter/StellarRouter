//! Testnet setup utilities for integration tests
//!
//! This module provides utilities for setting up and managing Stellar testnet
//! accounts and contracts for integration testing.

use std::env;
use std::process::Command;
use std::time::Duration;

/// Configuration for testnet integration tests
#[derive(Debug, Clone)]
pub struct TestnetConfig {
    pub network: String,
    pub rpc_url: String,
    pub network_passphrase: String,
}

impl Default for TestnetConfig {
    fn default() -> Self {
        Self {
            network: env::var("STELLAR_NETWORK")
                .unwrap_or_else(|_| "testnet".to_string()),
            rpc_url: env::var("STELLAR_RPC_URL")
                .unwrap_or_else(|_| "https://soroban-testnet.stellar.org".to_string()),
            network_passphrase: env::var("STELLAR_NETWORK_PASSPHRASE")
                .unwrap_or_else(|_| "Test SDF Network ; September 2015".to_string()),
        }
    }
}

/// Test account with keypair
#[derive(Debug, Clone)]
pub struct TestAccount {
    pub address: String,
    pub secret: String,
}

impl TestAccount {
    /// Generate a new test account
    pub fn generate() -> Result<Self, String> {
        let output = Command::new("stellar")
            .args(["keys", "generate", "--no-fund"])
            .output()
            .map_err(|e| format!("Failed to generate keypair: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "stellar keys generate failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = stdout.lines().collect();

        // Parse output to extract address and secret
        let address = lines
            .iter()
            .find(|l| l.contains("Public key:"))
            .and_then(|l| l.split(':').nth(1))
            .map(|s| s.trim().to_string())
            .ok_or("Failed to parse public key")?;

        let secret = lines
            .iter()
            .find(|l| l.contains("Secret key:"))
            .and_then(|l| l.split(':').nth(1))
            .map(|s| s.trim().to_string())
            .ok_or("Failed to parse secret key")?;

        Ok(Self { address, secret })
    }

    /// Fund this account using Friendbot
    pub fn fund(&self, network: &str) -> Result<(), String> {
        println!("Funding account {} via Friendbot on {}...", self.address, network);

        let output = Command::new("stellar")
            .args(["keys", "fund", &self.address, "--network", network])
            .output()
            .map_err(|e| format!("Failed to fund account: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "Friendbot funding failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        // Wait for funding to be confirmed
        std::thread::sleep(Duration::from_secs(2));
        Ok(())
    }

    /// Get account balance
    pub fn get_balance(&self, network: &str) -> Result<String, String> {
        let output = Command::new("stellar")
            .args([
                "contract",
                "invoke",
                "--network",
                network,
                "--source",
                &self.address,
                "--",
                "balance",
            ])
            .output()
            .map_err(|e| format!("Failed to get balance: {}", e))?;

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }
}

/// Deployed contract instance
#[derive(Debug, Clone)]
pub struct DeployedContract {
    pub contract_id: String,
    pub wasm_path: String,
    pub name: String,
}

impl DeployedContract {
    /// Deploy a contract to testnet
    pub fn deploy(
        wasm_path: &str,
        name: &str,
        source_account: &TestAccount,
        network: &str,
    ) -> Result<Self, String> {
        println!("Deploying {} from {}...", name, wasm_path);

        let output = Command::new("stellar")
            .args([
                "contract",
                "deploy",
                "--wasm",
                wasm_path,
                "--network",
                network,
                "--source",
                &source_account.address,
            ])
            .output()
            .map_err(|e| format!("Failed to deploy contract: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "Contract deployment failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        let contract_id = String::from_utf8_lossy(&output.stdout).trim().to_string();

        // Wait for deployment to be confirmed
        std::thread::sleep(Duration::from_secs(2));

        Ok(Self {
            contract_id,
            wasm_path: wasm_path.to_string(),
            name: name.to_string(),
        })
    }

    /// Invoke a contract method
    pub fn invoke(
        &self,
        method: &str,
        args: &[&str],
        source_account: &TestAccount,
        network: &str,
    ) -> Result<String, String> {
        let mut cmd_args = vec![
            "contract",
            "invoke",
            "--id",
            &self.contract_id,
            "--network",
            network,
            "--source",
            &source_account.address,
            "--",
            method,
        ];
        cmd_args.extend_from_slice(args);

        let output = Command::new("stellar")
            .args(&cmd_args)
            .output()
            .map_err(|e| format!("Failed to invoke contract: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "Contract invocation failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Try to invoke a contract method, expecting it to fail
    pub fn try_invoke(
        &self,
        method: &str,
        args: &[&str],
        source_account: &TestAccount,
        network: &str,
    ) -> Result<String, String> {
        let mut cmd_args = vec![
            "contract",
            "invoke",
            "--id",
            &self.contract_id,
            "--network",
            network,
            "--source",
            &source_account.address,
            "--",
            method,
        ];
        cmd_args.extend_from_slice(args);

        let output = Command::new("stellar")
            .args(&cmd_args)
            .output()
            .map_err(|e| format!("Failed to invoke contract: {}", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

        if !output.status.success() {
            Err(stderr)
        } else {
            Ok(stdout)
        }
    }
}

/// Test fixture managing all deployed contracts and accounts
pub struct TestFixture {
    pub config: TestnetConfig,
    pub admin: TestAccount,
    pub user1: TestAccount,
    pub user2: TestAccount,
    pub router_core: Option<DeployedContract>,
    pub router_registry: Option<DeployedContract>,
    pub router_access: Option<DeployedContract>,
    pub router_middleware: Option<DeployedContract>,
    pub router_timelock: Option<DeployedContract>,
    pub router_multicall: Option<DeployedContract>,
}

impl TestFixture {
    /// Create a new test fixture with funded accounts
    pub fn new() -> Result<Self, String> {
        println!("Setting up test fixture...");

        let config = TestnetConfig::default();

        let admin = TestAccount::generate()?;
        admin.fund(&config.network)?;

        let user1 = TestAccount::generate()?;
        user1.fund(&config.network)?;

        let user2 = TestAccount::generate()?;
        user2.fund(&config.network)?;

        Ok(Self {
            config,
            admin,
            user1,
            user2,
            router_core: None,
            router_registry: None,
            router_access: None,
            router_middleware: None,
            router_timelock: None,
            router_multicall: None,
        })
    }

    /// Deploy all router contracts in dependency order
    pub fn deploy_all_contracts(&mut self) -> Result<(), String> {
        println!("Deploying all router contracts...");

        let network = &self.config.network;

        // Deploy in dependency order
        self.router_registry = Some(DeployedContract::deploy(
            "target/wasm32-unknown-unknown/release/router_registry.wasm",
            "router-registry",
            &self.admin,
            network,
        )?);

        self.router_access = Some(DeployedContract::deploy(
            "target/wasm32-unknown-unknown/release/router_access.wasm",
            "router-access",
            &self.admin,
            network,
        )?);

        self.router_middleware = Some(DeployedContract::deploy(
            "target/wasm32-unknown-unknown/release/router_middleware.wasm",
            "router-middleware",
            &self.admin,
            network,
        )?);

        self.router_timelock = Some(DeployedContract::deploy(
            "target/wasm32-unknown-unknown/release/router_timelock.wasm",
            "router-timelock",
            &self.admin,
            network,
        )?);

        self.router_multicall = Some(DeployedContract::deploy(
            "target/wasm32-unknown-unknown/release/router_multicall.wasm",
            "router-multicall",
            &self.admin,
            network,
        )?);

        self.router_core = Some(DeployedContract::deploy(
            "target/wasm32-unknown-unknown/release/router_core.wasm",
            "router-core",
            &self.admin,
            network,
        )?);

        println!("All contracts deployed successfully!");
        Ok(())
    }

    /// Initialize all contracts with default configuration
    pub fn initialize_all_contracts(&self) -> Result<(), String> {
        println!("Initializing all contracts...");

        let network = &self.config.network;

        // Initialize router-core
        if let Some(ref core) = self.router_core {
            core.invoke("initialize", &["--admin", &self.admin.address], &self.admin, network)?;
            println!("✓ router-core initialized");
        }

        // Initialize router-registry
        if let Some(ref registry) = self.router_registry {
            registry.invoke("initialize", &["--admin", &self.admin.address], &self.admin, network)?;
            println!("✓ router-registry initialized");
        }

        // Initialize router-access
        if let Some(ref access) = self.router_access {
            access.invoke(
                "initialize",
                &["--super_admin", &self.admin.address],
                &self.admin,
                network,
            )?;
            println!("✓ router-access initialized");
        }

        // Initialize router-middleware
        if let Some(ref middleware) = self.router_middleware {
            middleware.invoke("initialize", &["--admin", &self.admin.address], &self.admin, network)?;
            println!("✓ router-middleware initialized");
        }

        // Initialize router-timelock
        if let Some(ref timelock) = self.router_timelock {
            timelock.invoke(
                "initialize",
                &["--admin", &self.admin.address, "--min_delay", "60"],
                &self.admin,
                network,
            )?;
            println!("✓ router-timelock initialized");
        }

        // Initialize router-multicall
        if let Some(ref multicall) = self.router_multicall {
            multicall.invoke(
                "initialize",
                &["--admin", &self.admin.address, "--max_batch_size", "10"],
                &self.admin,
                network,
            )?;
            println!("✓ router-multicall initialized");
        }

        println!("All contracts initialized successfully!");
        Ok(())
    }

    /// Clean up test resources (optional, for local testing)
    pub fn cleanup(&self) {
        println!("Test fixture cleanup complete");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore] // Run with: cargo test --test integration -- --ignored
    fn test_account_generation() {
        let account = TestAccount::generate().expect("Failed to generate account");
        assert!(!account.address.is_empty());
        assert!(!account.secret.is_empty());
        println!("Generated account: {}", account.address);
    }

    #[test]
    #[ignore]
    fn test_account_funding() {
        let account = TestAccount::generate().expect("Failed to generate account");
        account.fund("testnet").expect("Failed to fund account");
        println!("Funded account: {}", account.address);
    }
}
