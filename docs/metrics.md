# Transaction Success Metrics

Track and query success/failure rates for routed calls using router-middleware.

## How It Works
router-middleware already emits pre_call and post_call events on every routed
call. An off-chain indexer (e.g. a Node.js service subscribed to Stellar RPC
event streams) can consume these events and store metrics in a database.

## Event Schema
pre_call  → { caller: Address, route: String }
post_call → { caller: Address, route: String, success: bool }

## Suggested Metrics to Track
- success_rate per route (succeeded / total)
- failure_count per route per window
- top callers by volume
- circuit breaker trip frequency

## Integration Example (Node.js)
const server = new StellarRpc.Server("https://soroban-testnet.stellar.org");
const events = await server.getEvents({ filters: [{ type: "contract", contractIds: [MIDDLEWARE_ID] }] });
for (const event of events.events) {
  const topic = event.topic[0].value;
  if (topic === "post_call") {
    const [caller, route, success] = event.value.value;
    db.record({ caller, route, success, timestamp: event.ledger });
  }
}

## Querying Metrics
SELECT route, COUNT(*) as total,
       SUM(CASE WHEN success THEN 1 ELSE 0 END) as succeeded,
       ROUND(100.0 * SUM(CASE WHEN success THEN 1 ELSE 0 END) / COUNT(*), 2) as success_rate
FROM call_events
GROUP BY route
ORDER BY total DESC;

---

## Event Subscription Strategy

For real-time metrics, subscribe to contract events using the Soroban RPC `getEvents` endpoint. This approach avoids on-chain storage costs and provides up-to-date metrics.

### getEvents RPC Usage

The `getEvents` RPC method retrieves events emitted by contracts. Use it to build a real-time event stream for metrics collection.

**Basic Request:**
```javascript
const server = new StellarRpc.Server("https://soroban-testnet.stellar.org");
const events = await server.getEvents({
  filters: [{ type: "contract", contractIds: [MIDDLEWARE_ID] }],
  startLedger: latestLedger,
  limit: 100
});
```

**Parameters:**
- `filters`: Array of filter objects
  - `type`: Must be `"contract"`
  - `contractIds`: Array of contract IDs to monitor
  - `topics`: Optional array of topic filters (see Event Filtering below)
- `startLedger`: Starting ledger number (exclusive)
- `limit`: Maximum events to return (default: 100, max: 1000)
- `pagination`: Optional pagination token for large result sets

### Event Filtering by Topic

Filter events by topic to reduce bandwidth and processing overhead. Topics are the first element of the event topic array.

**Filter by single topic:**
```javascript
const events = await server.getEvents({
  filters: [{
    type: "contract",
    contractIds: [MIDDLEWARE_ID],
    topics: [["post_call"]]  // Only post_call events
  }],
  startLedger: latestLedger
});
```

**Filter by multiple topics:**
```javascript
const events = await server.getEvents({
  filters: [{
    type: "contract",
    contractIds: [MIDDLEWARE_ID],
    topics: [["pre_call"], ["post_call"]]  // Both pre_call and post_call
  }],
  startLedger: latestLedger
});
```

**Filter by contract-specific topics:**
```javascript
// router-quote events
const quoteEvents = await server.getEvents({
  filters: [{
    type: "contract",
    contractIds: [QUOTE_ID],
    topics: [["quote_generated"], ["fee_estimated"]]
  }],
  startLedger: latestLedger
});

// router-core events
const coreEvents = await server.getEvents({
  filters: [{
    type: "contract",
    contractIds: [CORE_ID],
    topics: [["route_registered"], ["route_updated"], ["route_overwritten"]]
  }],
  startLedger: latestLedger
});
```

### Maintaining Running Counters Off-Chain

Since contracts do not persist counters in storage (to avoid gas costs), maintain running counters off-chain by processing events sequentially.

**Strategy:**
1. Store the last processed ledger number in a database
2. Poll `getEvents` starting from that ledger
3. Process events and update counters
4. Save the new last processed ledger

**Implementation Example (Node.js):**

```javascript
const { CronJob } = require('cron');
const { Client } = require('pg');

const db = new Client({ connectionString: process.env.DATABASE_URL });
const server = new StellarRpc.Server(process.env.SOROBAN_RPC_URL);

// Initialize counters in database
async function initializeCounters() {
  await db.query(`
    CREATE TABLE IF NOT EXISTS metrics_counters (
      contract_id TEXT PRIMARY KEY,
      event_type TEXT,
      counter BIGINT DEFAULT 0,
      last_ledger BIGINT DEFAULT 0,
      updated_at TIMESTAMP DEFAULT NOW()
    )
  `);
}

// Process events and update counters
async function processEvents(contractId, eventTypes) {
  // Get last processed ledger
  const { rows } = await db.query(
    'SELECT last_ledger FROM metrics_counters WHERE contract_id = $1',
    [contractId]
  );
  const startLedger = rows[0]?.last_ledger || 0;

  // Fetch events
  const events = await server.getEvents({
    filters: [{
      type: "contract",
      contractIds: [contractId],
      topics: eventTypes.map(t => [t])
    }],
    startLedger,
    limit: 1000
  });

  let maxLedger = startLedger;
  const counters = {};

  // Initialize counters
  for (const eventType of eventTypes) {
    counters[eventType] = 0;
  }

  // Process each event
  for (const event of events.events) {
    const topic = event.topic[0].value;
    const ledger = event.ledger;

    if (counters[topic] !== undefined) {
      counters[topic]++;
    }

    if (ledger > maxLedger) {
      maxLedger = ledger;
    }
  }

  // Update database
  for (const [eventType, count] of Object.entries(counters)) {
    if (count > 0) {
      await db.query(`
        INSERT INTO metrics_counters (contract_id, event_type, counter, last_ledger)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (contract_id, event_type)
        DO UPDATE SET
          counter = metrics_counters.counter + $3,
          last_ledger = $4,
          updated_at = NOW()
      `, [contractId, eventType, count, maxLedger]);
    }
  }

  console.log(`Processed ${events.events.length} events for ${contractId}`);
}

// Poll for events every 15 seconds
async function startEventPolling() {
  await db.connect();
  await initializeCounters();

  const job = new CronJob('*/15 * * * * *', async () => {
    try {
      // Poll router-middleware
      await processEvents(MIDDLEWARE_ID, ['pre_call', 'post_call']);

      // Poll router-quote
      await processEvents(QUOTE_ID, ['quote_generated', 'fee_estimated']);

      // Poll router-core
      await processEvents(CORE_ID, ['route_registered', 'route_updated', 'route_overwritten']);
    } catch (error) {
      console.error('Event polling error:', error);
    }
  });

  job.start();
  console.log('Event polling started');
}

startEventPolling();
```

**Counter Schema:**
```sql
CREATE TABLE metrics_counters (
  contract_id TEXT,
  event_type TEXT,
  counter BIGINT DEFAULT 0,
  last_ledger BIGINT DEFAULT 0,
  updated_at TIMESTAMP DEFAULT NOW(),
  PRIMARY KEY (contract_id, event_type)
);
```

**Querying Counters:**
```sql
-- Get current counter values
SELECT contract_id, event_type, counter, last_ledger
FROM metrics_counters
ORDER BY contract_id, event_type;

-- Calculate success rate from router-middleware
SELECT
  (SELECT counter FROM metrics_counters WHERE contract_id = $1 AND event_type = 'post_call')::FLOAT /
  NULLIF((SELECT counter FROM metrics_counters WHERE contract_id = $1 AND event_type = 'pre_call'), 0) * 100
  AS success_rate;
```

### Event Polling Best Practices

1. **Polling Interval:** Use 15–30 seconds for testnet, 30–60 seconds for mainnet to balance freshness with RPC load

2. **Batch Processing:** Process events in batches (limit: 1000) to avoid timeouts

3. **Ledger Gaps:** Handle ledger gaps by checking for missing sequences and re-fetching if necessary

4. **Idempotency:** Design counter updates to be idempotent (use UPSERT or ON CONFLICT)

5. **Backpressure:** If falling behind (events accumulating), increase polling frequency temporarily or process larger batches

6. **Error Handling:** Implement exponential backoff for RPC failures and log errors for investigation

7. **Persistence:** Save the last processed ledger after each successful batch to avoid re-processing

### Event Reference

**router-middleware events:**
- `pre_call`: Emitted before routing (caller, route)
- `post_call`: Emitted after routing (caller, route, success)

**router-quote events:**
- `quote_generated`: Emitted on quote generation (target, amount_in, amount_out, exchange_rate)
- `fee_estimated`: Emitted on fee estimation (operation, base_fee, resource_fee)

**router-core events:**
- `route_registered`: Emitted on route registration (name, address)
- `route_updated`: Emitted on route update (name, old_address, new_address)
- `route_overwritten`: Emitted on route overwrite (name, old_address, new_address)
- `admin_transferred`: Emitted on admin transfer (old_admin, new_admin)

**router-multicall events:**
- `call_result`: Emitted per call in batch (caller, target, function, success)
- `batch_executed`: Emitted on batch completion (total, succeeded, failed)
- `max_batch_size_updated`: Emitted on config change (old_size, new_size)

**router-timelock events:**
- `operation_queued`: Emitted on operation queue (op_id, description, target)
- `operation_executed`: Emitted on operation execution (op_id)
- `operation_cancelled`: Emitted on operation cancellation (op_id)

**router-access events:**
- `role_granted`: Emitted on role grant (role, target, expires_at)
- `role_revoked`: Emitted on role revoke (role, target)
