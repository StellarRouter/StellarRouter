//! Shared integration test helpers for stellar-router.

use std::env;
use std::process::Command;
use std::time::Duration;

/// Configuration for Stellar testnet integration tests.
#[derive(Debug, Clone)]
pub struct TestnetConfig {
    pub network: String,
    pub rpc_url: String,
    pub network_passphrase: String,
}

impl Default for TestnetConfig {
    fn default() -> Self {
        Self {
            network: env::var("STELLAR_NETWORK").unwrap_or_else(|_| "testnet".to_string()),
            rpc_url: env::var("STELLAR_RPC_URL")
                .unwrap_or_else(|_| "https://soroban-testnet.stellar.org".to_string()),
            network_passphrase: env::var("STELLAR_NETWORK_PASSPHRASE")
                .unwrap_or_else(|_| "Test SDF Network ; September 2015".to_string()),
        }
    }
}

/// Test account with keypair.
#[derive(Debug, Clone)]
pub struct TestAccount {
    pub address: String,
    pub secret: String,
}

impl TestAccount {
    /// Generate a new test account.
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

    /// Fund this account using Friendbot.
    pub fn fund(&self, network: &str) -> Result<(), String> {
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

        std::thread::sleep(Duration::from_secs(2));
        Ok(())
    }
}

/// Deployed contract instance.
#[derive(Debug, Clone)]
pub struct DeployedContract {
    pub contract_id: String,
    pub wasm_path: String,
    pub name: String,
    pub network: String,
}

impl DeployedContract {
    /// Deploy a contract to testnet.
    pub fn deploy(
        wasm_path: &str,
        name: &str,
        source_account: &TestAccount,
        network: &str,
    ) -> Result<Self, String> {
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
        std::thread::sleep(Duration::from_secs(2));

        Ok(Self {
            contract_id,
            wasm_path: wasm_path.to_string(),
            name: name.to_string(),
            network: network.to_string(),
        })
    }

    /// Invoke a contract method.
    pub fn invoke(
        &self,
        method: &str,
        args: &[&str],
        source_account: &TestAccount,
    ) -> Result<String, String> {
        let mut cmd_args = vec![
            "contract",
            "invoke",
            "--id",
            &self.contract_id,
            "--network",
            &self.network,
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

    /// Try to invoke a contract method, expecting it to fail.
    pub fn try_invoke(
        &self,
        method: &str,
        args: &[&str],
        source_account: &TestAccount,
    ) -> Result<String, String> {
        let mut cmd_args = vec![
            "contract",
            "invoke",
            "--id",
            &self.contract_id,
            "--network",
            &self.network,
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

/// Shared integration-test fixture that deploys and initializes all contracts.
pub struct TestSuite {
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

impl TestSuite {
    /// Build and fully initialize the suite.
    pub fn setup() -> Result<Self, String> {
        let mut suite = Self::new()?;
        suite.deploy_all_contracts()?;
        suite.initialize_all_contracts()?;
        Ok(suite)
    }

    /// Optional cleanup hook for local runs.
    pub fn teardown(&self) {
        println!("Test suite teardown complete");
    }

    pub fn core(&self) -> Result<&DeployedContract, String> {
        self.router_core
            .as_ref()
            .ok_or("Core contract not deployed".to_string())
    }

    pub fn registry(&self) -> Result<&DeployedContract, String> {
        self.router_registry
            .as_ref()
            .ok_or("Registry contract not deployed".to_string())
    }

    pub fn access(&self) -> Result<&DeployedContract, String> {
        self.router_access
            .as_ref()
            .ok_or("Access contract not deployed".to_string())
    }

    pub fn middleware(&self) -> Result<&DeployedContract, String> {
        self.router_middleware
            .as_ref()
            .ok_or("Middleware contract not deployed".to_string())
    }

    pub fn timelock(&self) -> Result<&DeployedContract, String> {
        self.router_timelock
            .as_ref()
            .ok_or("Timelock contract not deployed".to_string())
    }

    pub fn multicall(&self) -> Result<&DeployedContract, String> {
        self.router_multicall
            .as_ref()
            .ok_or("Multicall contract not deployed".to_string())
    }

    fn new() -> Result<Self, String> {
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

    fn deploy_all_contracts(&mut self) -> Result<(), String> {
        let network = &self.config.network;

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

        Ok(())
    }

    fn initialize_all_contracts(&self) -> Result<(), String> {
        if let Some(ref core) = self.router_core {
            core.invoke("initialize", &["--admin", &self.admin.address], &self.admin)?;
        }

        if let Some(ref registry) = self.router_registry {
            registry.invoke("initialize", &["--admin", &self.admin.address], &self.admin)?;
        }

        if let Some(ref access) = self.router_access {
            access.invoke(
                "initialize",
                &["--super_admin", &self.admin.address],
                &self.admin,
            )?;
        }

        if let Some(ref middleware) = self.router_middleware {
            middleware.invoke("initialize", &["--admin", &self.admin.address], &self.admin)?;
        }

        if let Some(ref timelock) = self.router_timelock {
            timelock.invoke(
                "initialize",
                &["--admin", &self.admin.address, "--min_delay", "60"],
                &self.admin,
            )?;
        }

        if let Some(ref multicall) = self.router_multicall {
            multicall.invoke(
                "initialize",
                &["--admin", &self.admin.address, "--max_batch_size", "10"],
                &self.admin,
            )?;
        }

        Ok(())
    }
}

impl Drop for TestSuite {
    fn drop(&mut self) {
        self.teardown();
    }
}
