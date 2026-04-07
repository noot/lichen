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

        // fund agents
        vm.deal(alice, 100 ether);
        vm.deal(bob, 100 ether);
        vm.deal(carol, 100 ether);
        vm.deal(dave, 100 ether);
        vm.deal(worker, 10 ether);

        // deposit
        vm.prank(alice);
        coord.deposit{value: 10 ether}();
        vm.prank(bob);
        coord.deposit{value: 10 ether}();
        vm.prank(carol);
        coord.deposit{value: 10 ether}();
        vm.prank(dave);
        coord.deposit{value: 10 ether}();
    }

    // ── helpers ──────────────────────────────────────────────────────────

    /// @dev convert a prediction like 0.75 to 64.64 fixed-point.
    ///      pass numerator and denominator (e.g., 75, 100 for 0.75).
    function _pred(uint256 num, uint256 denom) internal pure returns (int128) {
        return ABDKMath64x64.divu(num, denom);
    }

    function _createAndSubmitWork() internal returns (uint256 taskId) {
        taskId = coord.createTask(keccak256("test prompt"), 3);
        vm.prank(worker);
        coord.submitResult(taskId, keccak256("test output"));
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

    // ── task lifecycle ───────────────────────────────────────────────────

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
        // try submitting again
        vm.prank(worker);
        vm.expectRevert("not awaiting work");
        coord.submitResult(taskId, keccak256("output2"));
    }

    function test_submitRating_wrongPhase() public {
        uint256 taskId = coord.createTask(keccak256("prompt"), 3);
        // no result submitted yet
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

    // ── scoring ──────────────────────────────────────────────────────────

    function test_autoScore_3raters_allGood() public {
        uint256 taskId = _createAndSubmitWork();

        // all 3 vote good with prediction 0.90
        vm.prank(alice);
        coord.submitRating(taskId, true, _pred(90, 100));
        vm.prank(bob);
        coord.submitRating(taskId, true, _pred(90, 100));
        vm.prank(carol);
        coord.submitRating(taskId, true, _pred(90, 100));

        // should be scored now
        (LichenCoordinator.Task memory t,) = coord.getTask(taskId);
        assertTrue(t.phase == LichenCoordinator.Phase.Scored);
        assertTrue(t.accepted);

        // all voted the same with the same prediction — scores should be equal
        int256 scoreA = coord.getScore(taskId, alice);
        int256 scoreB = coord.getScore(taskId, bob);
        int256 scoreC = coord.getScore(taskId, carol);
        assertEq(scoreA, scoreB);
        assertEq(scoreB, scoreC);

        // each should get back their collateral (equal scores)
        assertEq(uint256(scoreA), COLLATERAL);

        // balances should be restored
        assertEq(coord.balances(alice), 10 ether);
    }

    function test_autoScore_surprisinglyPopular() public {
        // 3 vote good predicting 0.50, 1 votes bad predicting 0.50
        // "good" is surprisingly popular → good voters rewarded
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

        // good voters should score higher than bad voter
        int256 scoreAlice = coord.getScore(taskId, alice);
        int256 scoreDave = coord.getScore(taskId, dave);
        assertTrue(scoreAlice > scoreDave, "good voter should score higher");

        console.log("good voter payout (wei):", uint256(scoreAlice));
        console.log("bad voter payout (wei):", uint256(scoreDave));
    }

    function test_autoScore_goodPredictionRewarded() public {
        // both vote good, but alice predicts 0.95 (accurate) and bob predicts 0.50 (bad)
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

        console.log("good predictor payout:", uint256(scoreAlice));
        console.log("bad predictor payout:", uint256(scoreBob));
    }

    function test_activeTasks_removedAfterScoring() public {
        uint256 taskId = _createAndSubmitWork();

        uint256[] memory active = coord.getActiveTasks();
        assertEq(active.length, 1);
        assertEq(active[0], taskId);

        // score it
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
        // total balances before = 40 ether (4 agents x 10)
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

        // pool is fully redistributed — rounding may lose a few wei
        uint256 diff = totalBefore > totalAfter ? totalBefore - totalAfter : totalAfter - totalBefore;
        assertTrue(diff <= 4, "pool minus payouts must be near zero");
    }

    function test_allPayouts_nonNegative() public {
        uint256 taskId = coord.createTask(keccak256("prompt"), 4);
        vm.prank(worker);
        coord.submitResult(taskId, keccak256("output"));

        // mixed signals and predictions
        vm.prank(alice);
        coord.submitRating(taskId, true, _pred(90, 100));
        vm.prank(bob);
        coord.submitRating(taskId, true, _pred(50, 100));
        vm.prank(carol);
        coord.submitRating(taskId, false, _pred(10, 100));
        vm.prank(dave);
        coord.submitRating(taskId, false, _pred(30, 100));

        // every rater should have a score >= 0
        assertTrue(coord.getScore(taskId, alice) >= 0, "alice payout negative");
        assertTrue(coord.getScore(taskId, bob) >= 0, "bob payout negative");
        assertTrue(coord.getScore(taskId, carol) >= 0, "carol payout negative");
        assertTrue(coord.getScore(taskId, dave) >= 0, "dave payout negative");
    }

    function test_lowestScorer_getsZero() public {
        // 2 raters: one votes with the majority prediction, one against
        uint256 taskId = coord.createTask(keccak256("prompt"), 2);
        vm.prank(worker);
        coord.submitResult(taskId, keccak256("output"));

        // alice: good signal, accurate prediction
        vm.prank(alice);
        coord.submitRating(taskId, true, _pred(95, 100));
        // bob: good signal, terrible prediction
        vm.prank(bob);
        coord.submitRating(taskId, true, _pred(5, 100));

        int256 scoreBob = coord.getScore(taskId, bob);
        assertEq(scoreBob, 0, "lowest scorer should get zero");

        // alice should get the entire pool
        int256 scoreAlice = coord.getScore(taskId, alice);
        // pool = 2 * 1 ether = 2 ether, minus possible rounding dust
        assertTrue(uint256(scoreAlice) >= 2 ether - 2, "top scorer should get full pool");
    }

    function test_equalScores_splitEvenly() public {
        uint256 taskId = _createAndSubmitWork();

        // all 3 vote identically
        vm.prank(alice);
        coord.submitRating(taskId, true, _pred(70, 100));
        vm.prank(bob);
        coord.submitRating(taskId, true, _pred(70, 100));
        vm.prank(carol);
        coord.submitRating(taskId, true, _pred(70, 100));

        int256 a = coord.getScore(taskId, alice);
        int256 b = coord.getScore(taskId, bob);
        int256 c = coord.getScore(taskId, carol);

        assertEq(a, b, "equal scores must produce equal payouts");
        assertEq(b, c, "equal scores must produce equal payouts");
        assertEq(uint256(a), COLLATERAL, "each gets back their collateral");
    }

    function test_higherScore_getsMore_shiftToMin() public {
        // 3 raters with different predictions, same signal. the one with the
        // best prediction (closest to actual good frac = 1.0) should get the most.
        uint256 taskId = _createAndSubmitWork();

        vm.prank(alice);
        coord.submitRating(taskId, true, _pred(95, 100)); // best prediction
        vm.prank(bob);
        coord.submitRating(taskId, true, _pred(70, 100)); // ok prediction
        vm.prank(carol);
        coord.submitRating(taskId, true, _pred(40, 100)); // worst prediction

        int256 a = coord.getScore(taskId, alice);
        int256 b = coord.getScore(taskId, bob);
        int256 c = coord.getScore(taskId, carol);

        assertTrue(a > b, "alice (best predictor) should score higher than bob");
        assertTrue(b > c, "bob should score higher than carol (worst predictor)");

        // pool fully distributed (minus rounding dust)
        uint256 total = uint256(a) + uint256(b) + uint256(c);
        uint256 pool = 3 * COLLATERAL;
        assertTrue(pool - total <= 3, "pool must be fully distributed");
    }

    function test_workerReputation_tracked() public {
        // task 1: approved (all vote good)
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

        // task 2: rejected (all vote bad)
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

        console.log("worker tasks:", completed2);
        console.log("worker approvals:", approvals2);
    }
}
