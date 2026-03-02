# Changes Summary

## Task 1: Wire coordinator to on-chain contract

### Added CLI flags
- `--onchain`: Enable on-chain backend
- `--rpc-url`: Ethereum RPC endpoint (required with --onchain)
- `--contract-address`: LichenCoordinator contract address (required with --onchain)
- `--private-key`: Private key for signing transactions (required with --onchain)

### Created TaskBackend trait
- New file: `crates/coordinator/src/backend.rs`
- Trait `TaskBackend` with methods for task operations
- `InMemoryBackend`: Original HashMap-based implementation
- `OnchainBackend`: New implementation using `OnchainClient`

### Protocol updates
- Added optional fields to `CreateTaskRequest`:
  - `output`: Expected output (for on-chain hashing)
  - `max_raters`: Maximum raters allowed
  - `min_raters`: Minimum raters required
  - `timeout_seconds`: Task timeout

### Architecture changes
- Coordinator now uses dependency injection for the backend
- Handlers use the backend trait instead of direct HashMap access
- Error mapping ensures correct HTTP status codes (409 for conflicts)

## Task 2: Agent events system

### SSE endpoint
- New endpoint: `GET /events/stream`
- Server-Sent Events (SSE) for real-time notifications

### Event types
- `task_created`: Emitted when a new task is created
- `task_rated`: Emitted when an agent submits a rating
- `task_scored`: Emitted when a task reaches scoring threshold

### Implementation
- Event broadcast channel in `AppState`
- Handlers emit events on state transitions
- SSE stream with automatic heartbeat keepalive

## Testing
- All existing tests pass
- Coordinator tests updated for new Args structure
- Client updated to provide default values for new fields
- Clippy warnings resolved