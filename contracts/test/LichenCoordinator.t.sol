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
    uint256 TIMEOUT = 1 hours;

    function setUp() public {
        coord = new LichenCoordinator(ALPHA, BETA, COLLATERAL);

        // fund agents
        vm.deal(alice, 100 ether);
        vm.deal(bob, 100 ether);
        vm.deal(carol, 100 ether);
        vm.deal(dave, 100 ether);
        vm.deal(worker, 100 ether);

        // deposit
        vm.prank(alice);
        coord.deposit{value: 10 ether}();
        vm.prank(bob);
        coord.deposit{value: 10 ether}();
        vm.prank(carol);
        coord.deposit{value: 10 ether}();
        vm.prank(dave);
        coord.deposit{value: 10 ether}();
        vm.prank(worker);
        coord.deposit{value: 10 ether}();
    }

    // ── helpers ──────────────────────────────────────────────────────────

    /// @dev convert a prediction like 0.75 to 64.64 fixed-point.
    function _pred(uint256 num, uint256 denom) internal pure returns (int128) {
        return ABDKMath64x64.divu(num, denom);
    }

    /// @dev create a task with default settings (maxRaters=3, minRaters=2, 1h timeout).
    function _createTask() internal returns (uint256 taskId) {
        vm.prank(worker);
        taskId = coord.createTask(
            keccak256("test prompt"),
            keccak256("test output"),
            3, // maxRaters
            2, // minRaters
            TIMEOUT
        );
    }

    /// @dev create a task with custom rater counts.
    function _createTaskCustom(uint8 maxRaters, uint8 minRaters) internal returns (uint256 taskId) {
        vm.prank(worker);
        taskId = coord.createTask(
            keccak256("test prompt"),
            keccak256("test output"),
            maxRaters,
            minRaters,
            TIMEOUT
        );
    }

    // ── deposit/withdraw ─────────────────────────────────────────────────

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

    // ── task creation ────────────────────────────────────────────────────

    function test_createTask() public {
        uint256 taskId = _createTask();
        (LichenCoordinator.Task memory t,) = coord.getTask(taskId);
        assertEq(t.maxRaters, 3);
        assertEq(t.minRaters, 2);
        assertEq(t.worker, worker);
        assertTrue(t.phase == LichenCoordinator.Phase.AwaitingRatings);
        assertEq(t.deadline, block.timestamp + TIMEOUT);
    }

    function test_createTask_invalidParams() public {
        vm.startPrank(worker);

        // maxRaters < 2
        vm.expectRevert("need >= 2 max raters");
        coord.createTask(keccak256("p"), keccak256("o"), 1, 1, TIMEOUT);

        // minRaters < 2
        vm.expectRevert("need >= 2 min raters");
        coord.createTask(keccak256("p"), keccak256("o"), 3, 1, TIMEOUT);

        // min > max
        vm.expectRevert("min > max");
        coord.createTask(keccak256("p"), keccak256("o"), 2, 3, TIMEOUT);

        // zero timeout
        vm.expectRevert("zero timeout");
        coord.createTask(keccak256("p"), keccak256("o"), 3, 2, 0);

        vm.stopPrank();
    }

    // ── rating submission ────────────────────────────────────────────────

    function test_submitRating_duplicate() public {
        uint256 taskId = _createTask();
        vm.prank(alice);
        coord.submitRating(taskId, true, _pred(75, 100));
        vm.prank(alice);
        vm.expectRevert("already rated");
        coord.submitRating(taskId, true, _pred(75, 100));
    }

    function test_submitRating_insufficientCollateral() public {
        uint256 taskId = _createTask();
        address broke = makeAddr("broke");
        vm.prank(broke);
        vm.expectRevert("insufficient collateral");
        coord.submitRating(taskId, true, _pred(75, 100));
    }

    function test_submitRating_workerCannotSelfRate() public {
        uint256 taskId = _createTask();
        vm.prank(worker);
        vm.expectRevert("worker cannot rate own task");
        coord.submitRating(taskId, true, _pred(75, 100));
    }

    function test_submitRating_maxRatersReached() public {
        uint256 taskId = _createTaskCustom(2, 2);

        vm.prank(alice);
        coord.submitRating(taskId, true, _pred(90, 100));
        vm.prank(bob);
        coord.submitRating(taskId, true, _pred(90, 100));

        // third rater should be rejected (auto-scored at 2)
        vm.prank(carol);
        vm.expectRevert(); // either "not awaiting ratings" (scored) or "max raters reached"
        coord.submitRating(taskId, true, _pred(90, 100));
    }

    function test_submitRating_afterDeadline() public {
        uint256 taskId = _createTask();

        // warp past deadline
        vm.warp(block.timestamp + TIMEOUT + 1);

        vm.prank(alice);
        vm.expectRevert("deadline passed");
        coord.submitRating(taskId, true, _pred(90, 100));
    }

    // ── auto-scoring (maxRaters reached) ─────────────────────────────────

    function test_autoScore_3raters_allGood() public {
        uint256 taskId = _createTask();

        vm.prank(alice);
        coord.submitRating(taskId, true, _pred(90, 100));
        vm.prank(bob);
        coord.submitRating(taskId, true, _pred(90, 100));
        vm.prank(carol);
        coord.submitRating(taskId, true, _pred(90, 100));

        (LichenCoordinator.Task memory t,) = coord.getTask(taskId);
        assertTrue(t.phase == LichenCoordinator.Phase.Scored);
        assertTrue(t.accepted);

        // all voted same with same prediction — equal scores
        int256 scoreA = coord.getScore(taskId, alice);
        int256 scoreB = coord.getScore(taskId, bob);
        int256 scoreC = coord.getScore(taskId, carol);
        assertEq(scoreA, scoreB);
        assertEq(scoreB, scoreC);
        assertEq(uint256(scoreA), COLLATERAL);

        // balances restored
        assertEq(coord.balances(alice), 10 ether);
    }

    function test_autoScore_surprisinglyPopular() public {
        uint256 taskId = _createTaskCustom(4, 2);

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

        int256 scoreAlice = coord.getScore(taskId, alice);
        int256 scoreDave = coord.getScore(taskId, dave);
        assertTrue(scoreAlice > scoreDave, "good voter should score higher");
    }

    function test_autoScore_goodPredictionRewarded() public {
        uint256 taskId = _createTaskCustom(2, 2);

        vm.prank(alice);
        coord.submitRating(taskId, true, _pred(95, 100));
        vm.prank(bob);
        coord.submitRating(taskId, true, _pred(50, 100));

        int256 scoreAlice = coord.getScore(taskId, alice);
        int256 scoreBob = coord.getScore(taskId, bob);
        assertTrue(scoreAlice > scoreBob, "better predictor should score higher");
    }

    // ── finalize (timeout with minRaters) ────────────────────────────────

    function test_finalizeTask_afterDeadlineWithMinRaters() public {
        uint256 taskId = _createTask(); // max=3, min=2

        // only 2 raters submit (under max, at min)
        vm.prank(alice);
        coord.submitRating(taskId, true, _pred(90, 100));
        vm.prank(bob);
        coord.submitRating(taskId, true, _pred(90, 100));

        // can't finalize before deadline
        vm.expectRevert("finalization conditions not met");
        coord.finalizeTask(taskId);

        // warp past deadline
        vm.warp(block.timestamp + TIMEOUT + 1);

        // now finalize should work
        coord.finalizeTask(taskId);

        (LichenCoordinator.Task memory t,) = coord.getTask(taskId);
        assertTrue(t.phase == LichenCoordinator.Phase.Scored);
        assertTrue(t.accepted);

        // both get equal payouts (same votes + predictions)
        int256 a = coord.getScore(taskId, alice);
        int256 b = coord.getScore(taskId, bob);
        assertEq(a, b);
        assertEq(uint256(a), COLLATERAL);
    }

    function test_finalizeTask_cannotDoubleFinalize() public {
        uint256 taskId = _createTask();

        vm.prank(alice);
        coord.submitRating(taskId, true, _pred(90, 100));
        vm.prank(bob);
        coord.submitRating(taskId, true, _pred(90, 100));

        vm.warp(block.timestamp + TIMEOUT + 1);
        coord.finalizeTask(taskId);

        // second finalize should revert
        vm.expectRevert("not awaiting ratings");
        coord.finalizeTask(taskId);
    }

    function test_finalizeTask_notEnoughRaters() public {
        uint256 taskId = _createTask(); // min=2

        // only 1 rater submitted
        vm.prank(alice);
        coord.submitRating(taskId, true, _pred(90, 100));

        vm.warp(block.timestamp + TIMEOUT + 1);

        // finalize should fail (1 < minRaters=2)
        vm.expectRevert("finalization conditions not met");
        coord.finalizeTask(taskId);
    }

    // ── cancel ───────────────────────────────────────────────────────────

    function test_cancelTask_underSubscribed() public {
        uint256 taskId = _createTask(); // min=2

        // only 1 rater submits
        vm.prank(alice);
        coord.submitRating(taskId, true, _pred(90, 100));

        uint256 aliceBalBefore = coord.balances(alice);

        // can't cancel before deadline
        vm.expectRevert("deadline not passed");
        coord.cancelTask(taskId);

        // warp past deadline
        vm.warp(block.timestamp + TIMEOUT + 1);

        // cancel
        coord.cancelTask(taskId);

        (LichenCoordinator.Task memory t,) = coord.getTask(taskId);
        assertTrue(t.phase == LichenCoordinator.Phase.Cancelled);

        // alice gets collateral refunded
        assertEq(coord.balances(alice), aliceBalBefore + COLLATERAL);

        // removed from active list
        uint256[] memory active = coord.getActiveTasks();
        assertEq(active.length, 0);
    }

    function test_cancelTask_noRaters() public {
        uint256 taskId = _createTask();

        vm.warp(block.timestamp + TIMEOUT + 1);
        coord.cancelTask(taskId);

        (LichenCoordinator.Task memory t,) = coord.getTask(taskId);
        assertTrue(t.phase == LichenCoordinator.Phase.Cancelled);
    }

    function test_cancelTask_enoughRatersToFinalize() public {
        uint256 taskId = _createTask(); // min=2

        vm.prank(alice);
        coord.submitRating(taskId, true, _pred(90, 100));
        vm.prank(bob);
        coord.submitRating(taskId, true, _pred(90, 100));

        vm.warp(block.timestamp + TIMEOUT + 1);

        // can't cancel — enough raters to finalize
        vm.expectRevert("enough raters to finalize");
        coord.cancelTask(taskId);
    }

    function test_cancelTask_cannotCancelScored() public {
        uint256 taskId = _createTaskCustom(2, 2);

        vm.prank(alice);
        coord.submitRating(taskId, true, _pred(90, 100));
        vm.prank(bob);
        coord.submitRating(taskId, true, _pred(90, 100));

        // already scored
        vm.warp(block.timestamp + TIMEOUT + 1);
        vm.expectRevert("not awaiting ratings");
        coord.cancelTask(taskId);
    }

    // ── active tasks ─────────────────────────────────────────────────────

    function test_activeTasks_removedAfterScoring() public {
        uint256 taskId = _createTask();

        uint256[] memory active = coord.getActiveTasks();
        assertEq(active.length, 1);
        assertEq(active[0], taskId);

        vm.prank(alice);
        coord.submitRating(taskId, true, _pred(90, 100));
        vm.prank(bob);
        coord.submitRating(taskId, true, _pred(90, 100));
        vm.prank(carol);
        coord.submitRating(taskId, true, _pred(90, 100));

        active = coord.getActiveTasks();
        assertEq(active.length, 0);
    }

    function test_activeTasks_removedAfterCancel() public {
        uint256 taskId = _createTask();

        uint256[] memory active = coord.getActiveTasks();
        assertEq(active.length, 1);

        vm.warp(block.timestamp + TIMEOUT + 1);
        coord.cancelTask(taskId);

        active = coord.getActiveTasks();
        assertEq(active.length, 0);
    }

    // ── zero-sum / payout invariants ─────────────────────────────────────

    function test_balances_zeroSum() public {
        uint256 totalBefore = coord.balances(alice) + coord.balances(bob)
            + coord.balances(carol) + coord.balances(dave);

        uint256 taskId = _createTaskCustom(4, 2);

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

        uint256 diff = totalBefore > totalAfter ? totalBefore - totalAfter : totalAfter - totalBefore;
        assertTrue(diff <= 4, "pool minus payouts must be near zero");
    }

    function test_allPayouts_nonNegative() public {
        uint256 taskId = _createTaskCustom(4, 2);

        vm.prank(alice);
        coord.submitRating(taskId, true, _pred(90, 100));
        vm.prank(bob);
        coord.submitRating(taskId, true, _pred(50, 100));
        vm.prank(carol);
        coord.submitRating(taskId, false, _pred(10, 100));
        vm.prank(dave);
        coord.submitRating(taskId, false, _pred(30, 100));

        assertTrue(coord.getScore(taskId, alice) >= 0, "alice payout negative");
        assertTrue(coord.getScore(taskId, bob) >= 0, "bob payout negative");
        assertTrue(coord.getScore(taskId, carol) >= 0, "carol payout negative");
        assertTrue(coord.getScore(taskId, dave) >= 0, "dave payout negative");
    }

    function test_higherScore_getsMore() public {
        uint256 taskId = _createTask();

        vm.prank(alice);
        coord.submitRating(taskId, true, _pred(95, 100)); // best
        vm.prank(bob);
        coord.submitRating(taskId, true, _pred(70, 100)); // ok
        vm.prank(carol);
        coord.submitRating(taskId, true, _pred(40, 100)); // worst

        int256 a = coord.getScore(taskId, alice);
        int256 b = coord.getScore(taskId, bob);
        int256 c = coord.getScore(taskId, carol);

        assertTrue(a > b, "alice should score higher than bob");
        assertTrue(b > c, "bob should score higher than carol");

        uint256 total = uint256(a) + uint256(b) + uint256(c);
        uint256 pool = 3 * COLLATERAL;
        assertTrue(pool - total <= 3, "pool must be fully distributed");
    }

    // ── worker reputation ────────────────────────────────────────────────

    function test_workerReputation_tracked() public {
        // task 1: approved (all good)
        uint256 t1 = _createTaskCustom(2, 2);
        vm.prank(alice);
        coord.submitRating(t1, true, _pred(80, 100));
        vm.prank(bob);
        coord.submitRating(t1, true, _pred(80, 100));

        (uint256 completed1, uint256 approvals1) = coord.getWorkerReputation(worker);
        assertEq(completed1, 1);
        assertEq(approvals1, 1);

        // task 2: rejected (all bad)
        uint256 t2 = _createTaskCustom(2, 2);
        vm.prank(alice);
        coord.submitRating(t2, false, _pred(20, 100));
        vm.prank(bob);
        coord.submitRating(t2, false, _pred(20, 100));

        (uint256 completed2, uint256 approvals2) = coord.getWorkerReputation(worker);
        assertEq(completed2, 2);
        assertEq(approvals2, 1);
    }

    // ── finalize with mixed rater counts ─────────────────────────────────

    function test_finalizeTask_withThreeOfFiveRaters() public {
        uint256 taskId = _createTaskCustom(5, 3);

        vm.prank(alice);
        coord.submitRating(taskId, true, _pred(90, 100));
        vm.prank(bob);
        coord.submitRating(taskId, true, _pred(85, 100));
        vm.prank(carol);
        coord.submitRating(taskId, true, _pred(80, 100));

        // 3 raters, need 3 min, but max is 5 — can't finalize yet
        vm.expectRevert("finalization conditions not met");
        coord.finalizeTask(taskId);

        // after deadline, should work
        vm.warp(block.timestamp + TIMEOUT + 1);
        coord.finalizeTask(taskId);

        (LichenCoordinator.Task memory t,) = coord.getTask(taskId);
        assertTrue(t.phase == LichenCoordinator.Phase.Scored);
        assertTrue(t.accepted);

        // best predictor gets most
        int256 a = coord.getScore(taskId, alice);
        int256 c = coord.getScore(taskId, carol);
        assertTrue(a > c, "better predictor should score higher");
    }

    // ── anyone can call finalize ─────────────────────────────────────────

    function test_finalizeTask_calledByAnyone() public {
        uint256 taskId = _createTask();

        vm.prank(alice);
        coord.submitRating(taskId, true, _pred(90, 100));
        vm.prank(bob);
        coord.submitRating(taskId, true, _pred(90, 100));

        vm.warp(block.timestamp + TIMEOUT + 1);

        // dave (not a rater) finalizes
        vm.prank(dave);
        coord.finalizeTask(taskId);

        (LichenCoordinator.Task memory t,) = coord.getTask(taskId);
        assertTrue(t.phase == LichenCoordinator.Phase.Scored);
    }
}
