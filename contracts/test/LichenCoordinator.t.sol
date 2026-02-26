// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {Test, console} from "forge-std/Test.sol";
import {ABDKMath64x64} from "abdk/ABDKMath64x64.sol";
import {LichenCoordinator} from "../src/LichenCoordinator.sol";

contract LichenCoordinatorTest is Test {
    using ABDKMath64x64 for int128;

    LichenCoordinator public coord;

    address alice = makeAddr("alice");
    address bob = makeAddr("bob");
    address carol = makeAddr("carol");
    address dave = makeAddr("dave");
    address worker = makeAddr("worker");

    // alpha=1, beta=1 as 64.64 fixed-point
    int128 ALPHA = ABDKMath64x64.fromUInt(1);
    int128 BETA = ABDKMath64x64.fromUInt(1);
    uint256 COLLATERAL = 1 ether;

    function setUp() public {
        coord = new LichenCoordinator(ALPHA, BETA, COLLATERAL);

        // Fund agents
        vm.deal(alice, 100 ether);
        vm.deal(bob, 100 ether);
        vm.deal(carol, 100 ether);
        vm.deal(dave, 100 ether);
        vm.deal(worker, 10 ether);

        // Deposit
        vm.prank(alice);
        coord.deposit{value: 10 ether}();
        vm.prank(bob);
        coord.deposit{value: 10 ether}();
        vm.prank(carol);
        coord.deposit{value: 10 ether}();
        vm.prank(dave);
        coord.deposit{value: 10 ether}();
    }

    // ── Helpers ──────────────────────────────────────────────────────────

    /// @dev Convert a prediction like 0.75 to 64.64 fixed-point.
    ///      Pass numerator and denominator (e.g., 75, 100 for 0.75).
    function _pred(uint256 num, uint256 denom) internal pure returns (int128) {
        return ABDKMath64x64.divu(num, denom);
    }

    function _createAndSubmitWork() internal returns (uint256 taskId) {
        taskId = coord.createTask(keccak256("test prompt"), 3);
        vm.prank(worker);
        coord.submitResult(taskId, keccak256("test output"));
    }

    // ── Deposit/Withdraw ─────────────────────────────────────────────────

    function test_deposit() public view {
        assertEq(coord.balances(alice), 10 ether);
    }

    function test_withdraw() public {
        uint256 before = alice.balance;
        vm.prank(alice);
        coord.withdraw(5 ether);
        assertEq(coord.balances(alice), 5 ether);
        assertEq(alice.balance, before + 5 ether);
    }

    function test_withdraw_insufficient() public {
        vm.prank(alice);
        vm.expectRevert("insufficient balance");
        coord.withdraw(11 ether);
    }

    // ── Task Lifecycle ───────────────────────────────────────────────────

    function test_createTask() public {
        uint256 taskId = coord.createTask(keccak256("prompt"), 3);
        (LichenCoordinator.Task memory t,) = coord.getTask(taskId);
        assertEq(t.numRatersRequired, 3);
        assertTrue(t.phase == LichenCoordinator.Phase.AwaitingWork);
    }

    function test_submitResult() public {
        uint256 taskId = coord.createTask(keccak256("prompt"), 3);
        vm.prank(worker);
        coord.submitResult(taskId, keccak256("output"));
        (LichenCoordinator.Task memory t,) = coord.getTask(taskId);
        assertTrue(t.phase == LichenCoordinator.Phase.AwaitingRatings);
        assertEq(t.worker, worker);
    }

    function test_submitResult_wrongPhase() public {
        uint256 taskId = coord.createTask(keccak256("prompt"), 3);
        vm.prank(worker);
        coord.submitResult(taskId, keccak256("output"));
        // Try submitting again
        vm.prank(worker);
        vm.expectRevert("not awaiting work");
        coord.submitResult(taskId, keccak256("output2"));
    }

    function test_submitRating_wrongPhase() public {
        uint256 taskId = coord.createTask(keccak256("prompt"), 3);
        // No result submitted yet
        vm.prank(alice);
        vm.expectRevert("not awaiting ratings");
        coord.submitRating(taskId, true, _pred(75, 100));
    }

    function test_submitRating_duplicate() public {
        uint256 taskId = _createAndSubmitWork();
        vm.prank(alice);
        coord.submitRating(taskId, true, _pred(75, 100));
        vm.prank(alice);
        vm.expectRevert("already rated");
        coord.submitRating(taskId, true, _pred(75, 100));
    }

    function test_submitRating_insufficientCollateral() public {
        uint256 taskId = _createAndSubmitWork();
        address broke = makeAddr("broke");
        vm.prank(broke);
        vm.expectRevert("insufficient collateral");
        coord.submitRating(taskId, true, _pred(75, 100));
    }

    // ── Scoring ──────────────────────────────────────────────────────────

    function test_autoScore_3raters_allGood() public {
        uint256 taskId = _createAndSubmitWork();

        // All 3 vote GOOD with prediction 0.90
        vm.prank(alice);
        coord.submitRating(taskId, true, _pred(90, 100));
        vm.prank(bob);
        coord.submitRating(taskId, true, _pred(90, 100));
        vm.prank(carol);
        coord.submitRating(taskId, true, _pred(90, 100));

        // Should be scored now
        (LichenCoordinator.Task memory t,) = coord.getTask(taskId);
        assertTrue(t.phase == LichenCoordinator.Phase.Scored);
        assertTrue(t.accepted);

        // All voted the same with the same prediction — scores should be equal
        int256 scoreA = coord.getScore(taskId, alice);
        int256 scoreB = coord.getScore(taskId, bob);
        int256 scoreC = coord.getScore(taskId, carol);
        assertEq(scoreA, scoreB);
        assertEq(scoreB, scoreC);

        // Each should get back their collateral (zero-sum, equal scores)
        assertEq(uint256(scoreA), COLLATERAL);

        // Balances should be restored
        assertEq(coord.balances(alice), 10 ether);
    }

    function test_autoScore_surprisinglyPopular() public {
        // 3 vote GOOD predicting 0.50, 1 votes BAD predicting 0.50
        // "GOOD" is surprisingly popular → GOOD voters rewarded
        uint256 taskId = coord.createTask(keccak256("prompt"), 4);
        vm.prank(worker);
        coord.submitResult(taskId, keccak256("output"));

        vm.prank(alice);
        coord.submitRating(taskId, true, _pred(50, 100));
        vm.prank(bob);
        coord.submitRating(taskId, true, _pred(50, 100));
        vm.prank(carol);
        coord.submitRating(taskId, true, _pred(50, 100));
        vm.prank(dave);
        coord.submitRating(taskId, false, _pred(50, 100));

        (LichenCoordinator.Task memory t,) = coord.getTask(taskId);
        assertTrue(t.phase == LichenCoordinator.Phase.Scored);
        assertTrue(t.accepted);

        // GOOD voters should score higher than BAD voter
        int256 scoreAlice = coord.getScore(taskId, alice);
        int256 scoreDave = coord.getScore(taskId, dave);
        assertTrue(scoreAlice > scoreDave, "good voter should score higher");

        console.log("Good voter payout (wei):", uint256(scoreAlice));
        console.log("Bad voter payout (wei):", uint256(scoreDave));
    }

    function test_autoScore_goodPredictionRewarded() public {
        // Both vote GOOD, but alice predicts 0.95 (accurate) and bob predicts 0.50 (bad)
        uint256 taskId = coord.createTask(keccak256("prompt"), 2);
        vm.prank(worker);
        coord.submitResult(taskId, keccak256("output"));

        vm.prank(alice);
        coord.submitRating(taskId, true, _pred(95, 100));
        vm.prank(bob);
        coord.submitRating(taskId, true, _pred(50, 100));

        int256 scoreAlice = coord.getScore(taskId, alice);
        int256 scoreBob = coord.getScore(taskId, bob);
        assertTrue(scoreAlice > scoreBob, "better predictor should score higher");

        console.log("Good predictor payout:", uint256(scoreAlice));
        console.log("Bad predictor payout:", uint256(scoreBob));
    }

    function test_activeTasks_removedAfterScoring() public {
        uint256 taskId = _createAndSubmitWork();

        uint256[] memory active = coord.getActiveTasks();
        assertEq(active.length, 1);
        assertEq(active[0], taskId);

        // Score it
        vm.prank(alice);
        coord.submitRating(taskId, true, _pred(90, 100));
        vm.prank(bob);
        coord.submitRating(taskId, true, _pred(90, 100));
        vm.prank(carol);
        coord.submitRating(taskId, true, _pred(90, 100));

        active = coord.getActiveTasks();
        assertEq(active.length, 0);
    }

    function test_balances_zeroSum() public {
        // Total balances before = 40 ether (4 agents × 10)
        uint256 totalBefore = coord.balances(alice) + coord.balances(bob)
            + coord.balances(carol) + coord.balances(dave);

        uint256 taskId = coord.createTask(keccak256("prompt"), 4);
        vm.prank(worker);
        coord.submitResult(taskId, keccak256("output"));

        vm.prank(alice);
        coord.submitRating(taskId, true, _pred(90, 100));
        vm.prank(bob);
        coord.submitRating(taskId, true, _pred(90, 100));
        vm.prank(carol);
        coord.submitRating(taskId, true, _pred(50, 100));
        vm.prank(dave);
        coord.submitRating(taskId, false, _pred(20, 100));

        uint256 totalAfter = coord.balances(alice) + coord.balances(bob)
            + coord.balances(carol) + coord.balances(dave);

        // Should be approximately equal (rounding may cause tiny differences)
        uint256 diff = totalBefore > totalAfter ? totalBefore - totalAfter : totalAfter - totalBefore;
        assertTrue(diff < 1e15, "balances should be approximately zero-sum");

        console.log("Total before:", totalBefore);
        console.log("Total after:", totalAfter);
        console.log("Diff (wei):", diff);
    }

    function test_workerReputation_tracked() public {
        // Task 1: approved (all vote good)
        uint256 t1 = coord.createTask(keccak256("task1"), 2);
        vm.prank(alice);
        coord.submitResult(t1, keccak256("output1"));
        vm.prank(bob);
        coord.submitRating(t1, true, _pred(80, 100));
        vm.prank(carol);
        coord.submitRating(t1, true, _pred(80, 100));

        (uint256 completed1, uint256 approvals1) = coord.getWorkerReputation(alice);
        assertEq(completed1, 1);
        assertEq(approvals1, 1);

        // Task 2: rejected (all vote bad)
        uint256 t2 = coord.createTask(keccak256("task2"), 2);
        vm.prank(alice);
        coord.submitResult(t2, keccak256("output2"));
        vm.prank(bob);
        coord.submitRating(t2, false, _pred(20, 100));
        vm.prank(carol);
        coord.submitRating(t2, false, _pred(20, 100));

        (uint256 completed2, uint256 approvals2) = coord.getWorkerReputation(alice);
        assertEq(completed2, 2);
        assertEq(approvals2, 1);

        console.log("Worker tasks:", completed2);
        console.log("Worker approvals:", approvals2);
    }
}
