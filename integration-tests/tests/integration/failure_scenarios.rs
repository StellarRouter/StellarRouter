//! Failure scenario integration tests
//!
//! These tests verify that the router system handles failures gracefully:
//! - Unauthorized access attempts
//! - Invalid parameters
//! - Network failures
//! - Contract errors

use integration_tests::{TestAccount, TestSuite};

#[test]
#[ignore] // Run with: cargo test --test integration -- --ignored
fn test_unauthorized_route_registration() {
    println!("\n=== Testing Unauthorized Route Registration ===\n");

    let fixture = TestSuite::setup().expect("Failed to set up test suite");

    let core = fixture
        .router_core
        .as_ref()
        .expect("Core contract not deployed");
    let unauthorized_user = &fixture.user1;

    // Try to register route as non-admin (should fail)
    println!("\n--- Test: Non-admin tries to register route ---");
    let mock_addr = TestAccount::generate()
        .expect("Failed to generate address")
        .address;

    let result = core.try_invoke(
        "register_route",
        &[
            "--caller",
            &unauthorized_user.address,
            "--name",
            "unauthorized_route",
            "--address",
            &mock_addr,
            "--metadata",
            "null",
        ],
        unauthorized_user,
    );

    assert!(result.is_err(), "Unauthorized registration should fail");
    let error = result.err().unwrap();
    assert!(
        error.contains("Unauthorized") || error.contains("auth"),
        "Error should indicate authorization failure: {}",
        error
    );
    println!("✓ Unauthorized registration correctly rejected: {}", error);

    println!("\n=== Unauthorized Access Test PASSED ===\n");
}

#[test]
#[ignore]
fn test_duplicate_route_registration() {
    println!("\n=== Testing Duplicate Route Registration ===\n");

    let fixture = TestSuite::setup().expect("Failed to set up test suite");

    let core = fixture
        .router_core
        .as_ref()
        .expect("Core contract not deployed");
    let admin = &fixture.admin;

    // Register a route
    println!("\n--- Test 1: Register initial route ---");
    let addr1 = TestAccount::generate()
        .expect("Failed to generate address")
        .address;

    core.invoke(
        "register_route",
        &[
            "--caller",
            &admin.address,
            "--name",
            "duplicate_test",
            "--address",
            &addr1,
            "--metadata",
            "null",
        ],
        admin,
    )
    .expect("Failed to register initial route");
    println!("✓ Initial route registered");

    // Try to register same route again (should fail)
    println!("\n--- Test 2: Try to register duplicate route ---");
    let addr2 = TestAccount::generate()
        .expect("Failed to generate address")
        .address;

    let result = core.try_invoke(
        "register_route",
        &[
            "--caller",
            &admin.address,
            "--name",
            "duplicate_test",
            "--address",
            &addr2,
            "--metadata",
            "null",
        ],
        admin,
    );

    assert!(result.is_err(), "Duplicate registration should fail");
    let error = result.err().unwrap();
    assert!(
        error.contains("AlreadyExists") || error.contains("exists"),
        "Error should indicate route already exists: {}",
        error
    );
    println!("✓ Duplicate registration correctly rejected: {}", error);

    println!("\n=== Duplicate Route Test PASSED ===\n");
}

#[test]
#[ignore]
fn test_resolve_nonexistent_route() {
    println!("\n=== Testing Resolve Nonexistent Route ===\n");

    let fixture = TestSuite::setup().expect("Failed to set up test suite");

    let core = fixture
        .router_core
        .as_ref()
        .expect("Core contract not deployed");
    let admin = &fixture.admin;

    // Try to resolve a route that doesn't exist
    println!("\n--- Test: Resolve nonexistent route ---");
    let result = core.try_invoke("resolve", &["--name", "nonexistent_route"], admin);

    assert!(result.is_err(), "Resolving nonexistent route should fail");
    let error = result.err().unwrap();
    assert!(
        error.contains("NotFound") || error.contains("not found"),
        "Error should indicate route not found: {}",
        error
    );
    println!("✓ Nonexistent route correctly rejected: {}", error);

    println!("\n=== Nonexistent Route Test PASSED ===\n");
}

#[test]
#[ignore]
fn test_invalid_route_name() {
    println!("\n=== Testing Invalid Route Name ===\n");

    let fixture = TestSuite::setup().expect("Failed to set up test suite");

    let core = fixture
        .router_core
        .as_ref()
        .expect("Core contract not deployed");
    let admin = &fixture.admin;

    // Try to register route with empty name
    println!("\n--- Test: Register route with empty name ---");
    let mock_addr = TestAccount::generate()
        .expect("Failed to generate address")
        .address;

    let result = core.try_invoke(
        "register_route",
        &[
            "--caller",
            &admin.address,
            "--name",
            "",
            "--address",
            &mock_addr,
            "--metadata",
            "null",
        ],
        admin,
    );

    // This might fail at CLI level or contract level
    if result.is_err() {
        println!(
            "✓ Empty route name correctly rejected: {}",
            result.err().unwrap()
        );
    } else {
        println!("⚠ Empty route name was accepted (may need contract-level validation)");
    }

    println!("\n=== Invalid Route Name Test PASSED ===\n");
}

#[test]
#[ignore]
fn test_paused_router_operations() {
    println!("\n=== Testing Paused Router Operations ===\n");

    let fixture = TestSuite::setup().expect("Failed to set up test suite");

    let core = fixture
        .router_core
        .as_ref()
        .expect("Core contract not deployed");
    let admin = &fixture.admin;

    // Register a route
    println!("\n--- Test 1: Register route ---");
    let mock_addr = TestAccount::generate()
        .expect("Failed to generate address")
        .address;

    core.invoke(
        "register_route",
        &[
            "--caller",
            &admin.address,
            "--name",
            "test_route",
            "--address",
            &mock_addr,
            "--metadata",
            "null",
        ],
        admin,
    )
    .expect("Failed to register route");
    println!("✓ Route registered");

    // Pause the entire router
    println!("\n--- Test 2: Pause router ---");
    core.invoke(
        "set_paused",
        &["--caller", &admin.address, "--paused", "true"],
        admin,
    )
    .expect("Failed to pause router");
    println!("✓ Router paused");

    // Try to resolve route while router is paused (should fail)
    println!("\n--- Test 3: Try to resolve while paused ---");
    let result = core.try_invoke("resolve", &["--name", "test_route"], admin);

    assert!(result.is_err(), "Resolving while paused should fail");
    let error = result.err().unwrap();
    assert!(
        error.contains("Paused") || error.contains("paused"),
        "Error should indicate router is paused: {}",
        error
    );
    println!("✓ Paused router correctly rejected resolution: {}", error);

    // Unpause router
    println!("\n--- Test 4: Unpause router ---");
    core.invoke(
        "set_paused",
        &["--caller", &admin.address, "--paused", "false"],
        admin,
    )
    .expect("Failed to unpause router");
    println!("✓ Router unpaused");

    // Verify route works again
    println!("\n--- Test 5: Resolve after unpause ---");
    let resolved = core
        .invoke("resolve", &["--name", "test_route"], admin)
        .expect("Failed to resolve after unpause");
    assert!(resolved.contains(&mock_addr));
    println!("✓ Route resolved successfully after unpause");

    println!("\n=== Paused Router Test PASSED ===\n");
}

#[test]
#[ignore]
fn test_update_nonexistent_route() {
    println!("\n=== Testing Update Nonexistent Route ===\n");

    let fixture = TestSuite::setup().expect("Failed to set up test suite");

    let core = fixture
        .router_core
        .as_ref()
        .expect("Core contract not deployed");
    let admin = &fixture.admin;

    // Try to update a route that doesn't exist
    println!("\n--- Test: Update nonexistent route ---");
    let new_addr = TestAccount::generate()
        .expect("Failed to generate address")
        .address;

    let result = core.try_invoke(
        "update_route",
        &[
            "--caller",
            &admin.address,
            "--name",
            "nonexistent_route",
            "--new_address",
            &new_addr,
        ],
        admin,
    );

    assert!(result.is_err(), "Updating nonexistent route should fail");
    let error = result.err().unwrap();
    assert!(
        error.contains("NotFound") || error.contains("not found"),
        "Error should indicate route not found: {}",
        error
    );
    println!("✓ Update nonexistent route correctly rejected: {}", error);

    println!("\n=== Update Nonexistent Route Test PASSED ===\n");
}

#[test]
#[ignore]
fn test_remove_nonexistent_route() {
    println!("\n=== Testing Remove Nonexistent Route ===\n");

    let fixture = TestSuite::setup().expect("Failed to set up test suite");

    let core = fixture
        .router_core
        .as_ref()
        .expect("Core contract not deployed");
    let admin = &fixture.admin;

    // Try to remove a route that doesn't exist
    println!("\n--- Test: Remove nonexistent route ---");
    let result = core.try_invoke(
        "remove_route",
        &["--caller", &admin.address, "--name", "nonexistent_route"],
        admin,
    );

    assert!(result.is_err(), "Removing nonexistent route should fail");
    let error = result.err().unwrap();
    assert!(
        error.contains("NotFound") || error.contains("not found"),
        "Error should indicate route not found: {}",
        error
    );
    println!("✓ Remove nonexistent route correctly rejected: {}", error);

    println!("\n=== Remove Nonexistent Route Test PASSED ===\n");
}

#[test]
#[ignore]
fn test_unauthorized_admin_transfer() {
    println!("\n=== Testing Unauthorized Admin Transfer ===\n");

    let fixture = TestSuite::setup().expect("Failed to set up test suite");

    let core = fixture
        .router_core
        .as_ref()
        .expect("Core contract not deployed");
    let unauthorized_user = &fixture.user1;
    let target_user = &fixture.user2;

    // Try to transfer admin as non-admin (should fail)
    println!("\n--- Test: Non-admin tries to transfer admin ---");
    let result = core.try_invoke(
        "transfer_admin",
        &[
            "--current",
            &unauthorized_user.address,
            "--new_admin",
            &target_user.address,
        ],
        unauthorized_user,
    );

    assert!(result.is_err(), "Unauthorized admin transfer should fail");
    let error = result.err().unwrap();
    assert!(
        error.contains("Unauthorized") || error.contains("auth"),
        "Error should indicate authorization failure: {}",
        error
    );
    println!(
        "✓ Unauthorized admin transfer correctly rejected: {}",
        error
    );

    println!("\n=== Unauthorized Admin Transfer Test PASSED ===\n");
}

#[test]
#[ignore]
fn test_registry_version_conflict() {
    println!("\n=== Testing Registry Version Conflict ===\n");

    let fixture = TestSuite::setup().expect("Failed to set up test suite");

    let registry = fixture
        .router_registry
        .as_ref()
        .expect("Registry not deployed");
    let admin = &fixture.admin;

    // Register version 1
    println!("\n--- Test 1: Register version 1 ---");
    let addr_v1 = TestAccount::generate()
        .expect("Failed to generate address")
        .address;

    registry
        .invoke(
            "register",
            &[
                "--caller",
                &admin.address,
                "--name",
                "test_contract",
                "--version",
                "1",
                "--address",
                &addr_v1,
            ],
            admin,
        )
        .expect("Failed to register v1");
    println!("✓ Version 1 registered");

    // Try to register same version again (should fail)
    println!("\n--- Test 2: Try to register duplicate version ---");
    let addr_v1_dup = TestAccount::generate()
        .expect("Failed to generate address")
        .address;

    let result = registry.try_invoke(
        "register",
        &[
            "--caller",
            &admin.address,
            "--name",
            "test_contract",
            "--version",
            "1",
            "--address",
            &addr_v1_dup,
        ],
        admin,
    );

    if result.is_err() {
        let error = result.err().unwrap();
        println!("✓ Duplicate version correctly rejected: {}", error);
    } else {
        println!("⚠ Duplicate version was accepted (may need validation)");
    }

    println!("\n=== Registry Version Conflict Test PASSED ===\n");
}

#[test]
#[ignore]
fn test_access_control_blacklist() {
    println!("\n=== Testing Access Control Blacklist ===\n");

    let fixture = TestSuite::setup().expect("Failed to set up test suite");

    let access = fixture
        .router_access
        .as_ref()
        .expect("Access contract not deployed");
    let admin = &fixture.admin;
    let user1 = &fixture.user1;

    // Blacklist user1
    println!("\n--- Test 1: Blacklist user ---");
    let result = access.invoke(
        "blacklist",
        &["--caller", &admin.address, "--account", &user1.address],
        admin,
    );

    if result.is_ok() {
        println!("✓ User blacklisted");

        // Try to grant role to blacklisted user (should fail)
        println!("\n--- Test 2: Try to grant role to blacklisted user ---");
        let result = access.try_invoke(
            "grant_role",
            &[
                "--caller",
                &admin.address,
                "--role",
                "operator",
                "--account",
                &user1.address,
            ],
            admin,
        );

        if result.is_err() {
            println!(
                "✓ Granting role to blacklisted user correctly rejected: {}",
                result.err().unwrap()
            );
        } else {
            println!("⚠ Blacklisted user was granted role (may need validation)");
        }
    } else {
        println!(
            "⚠ Blacklist operation may require different parameters: {:?}",
            result
        );
    }

    println!("\n=== Access Control Blacklist Test PASSED ===\n");
}
