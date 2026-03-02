#!/bin/bash

# Demo script showing the new on-chain backend and SSE event stream

echo "=== Demo: Coordinator with On-chain Backend and Events ==="
echo

echo "1. Starting coordinator with in-memory backend (default)..."
echo "Command: cargo run -p coordinator"
echo

echo "2. Starting coordinator with on-chain backend..."
echo "Command: cargo run -p coordinator -- --onchain --rpc-url http://localhost:8545 --contract-address 0x5FbDB2315678afecb367f032d93F642f64180aa3 --private-key 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
echo

echo "3. Subscribe to SSE events..."
echo "Command: curl -N http://localhost:3000/events/stream"
echo

echo "Example event output:"
echo '
event: task_created
data: {"type":"task_created","task_id":"123e4567-e89b-12d3-a456-426614174000","prompt":"Write a haiku"}

event: task_rated
data: {"type":"task_rated","task_id":"123e4567-e89b-12d3-a456-426614174000","agent_id":"rater1"}

event: task_scored
data: {"type":"task_scored","task_id":"123e4567-e89b-12d3-a456-426614174000","accepted":true}
'
echo

echo "Note: The on-chain backend requires:"
echo "- A running Ethereum node (e.g., anvil from Foundry)"
echo "- Deployed LichenCoordinator contract"
echo "- Funded account for gas fees"