// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {ABDKMath64x64} from "abdk/ABDKMath64x64.sol";

/// @title LichenCoordinator
/// @notice on-chain coordinator for the Lichen RBTS protocol.
///         manages task lifecycle, collects ratings, computes RBTS scores,
///         and redistributes staked ETH among raters.
///         supports an open rater marketplace where any registered rater
///         can self-select which tasks to rate.
contract LichenCoordinator {
    using ABDKMath64x64 for int128;

    // ── types ────────────────────────────────────────────────────────────

    enum Phase {
        AwaitingRatings,
        Scored,
        Cancelled
    }

    struct Task {
        bytes32 promptHash;
        address worker;
        bytes32 outputHash;
        uint8 maxRaters;
        uint8 minRaters;
        uint256 deadline;
        Phase phase;
        bool accepted;
    }

    struct Rating {
        address rater;
        bool signal;       // true = good
        int128 prediction; // 64.64 fixed-point, range [0, 1]
    }

    // ── state ────────────────────────────────────────────────────────────

    /// RBTS weighting parameters (64.64 fixed-point).
    int128 public immutable alpha;
    int128 public immutable beta;

    /// collateral locked per rating (in wei).
    uint256 public immutable collateralPerRating;

    /// agent ETH balances.
    mapping(address => uint256) public balances;

    /// task storage.
    uint256 public nextTaskId;
    mapping(uint256 => Task) public tasks;

    /// ratings per task.
    mapping(uint256 => Rating[]) public ratings;

    /// track whether an address has already rated a task.
    mapping(uint256 => mapping(address => bool)) public hasRated;

    /// RBTS scores after scoring (taskId => rater => payment in WAD).
    mapping(uint256 => mapping(address => int256)) public scores;

    /// worker reputation: tracks cumulative task outcomes per worker.
    struct WorkerRecord {
        uint256 tasksCompleted;
        uint256 approvals;
    }
    mapping(address => WorkerRecord) public workerReputation;

    /// list of active (non-scored, non-cancelled) task IDs for polling.
    uint256[] internal _activeTasks;
    mapping(uint256 => uint256) internal _activeIndex; // taskId => index+1 (0 = not active)

    // ── events ───────────────────────────────────────────────────────────

    event Deposited(address indexed agent, uint256 amount);
    event Withdrawn(address indexed agent, uint256 amount);
    event TaskCreated(
        uint256 indexed taskId,
        address indexed worker,
        bytes32 promptHash,
        uint8 maxRaters,
        uint8 minRaters,
        uint256 deadline
    );
    event RatingSubmitted(uint256 indexed taskId, address indexed rater, bool signal, uint256 ratingCount);
    event TaskFinalized(uint256 indexed taskId, uint256 raterCount, bool accepted);
    event TaskCancelled(uint256 indexed taskId);

    // ── constructor ──────────────────────────────────────────────────────

    /// @param _alpha  information score weight (64.64 fixed-point).
    /// @param _beta   prediction score weight (64.64 fixed-point).
    /// @param _collateral collateral per rating in wei.
    constructor(int128 _alpha, int128 _beta, uint256 _collateral) {
        alpha = _alpha;
        beta = _beta;
        collateralPerRating = _collateral;
    }

    // ── deposit / withdraw ───────────────────────────────────────────────

    function deposit() external payable {
        require(msg.value > 0, "zero deposit");
        balances[msg.sender] += msg.value;
        emit Deposited(msg.sender, msg.value);
    }

    function withdraw(uint256 amount) external {
        require(balances[msg.sender] >= amount, "insufficient balance");
        balances[msg.sender] -= amount;
        (bool ok,) = msg.sender.call{value: amount}("");
        require(ok, "transfer failed");
        emit Withdrawn(msg.sender, amount);
    }

    // ── task lifecycle ───────────────────────────────────────────────────

    /// @notice Create a task and submit the worker output in one call.
    ///         Any registered rater can then self-select to rate this task.
    /// @param promptHash Hash of the task prompt.
    /// @param outputHash Hash of the worker's output.
    /// @param maxRaters  Maximum number of raters (first-come-first-served).
    /// @param minRaters  Minimum raters needed for valid scoring after timeout.
    /// @param timeout    Seconds until deadline (block.timestamp + timeout).
    function createTask(
        bytes32 promptHash,
        bytes32 outputHash,
        uint8 maxRaters,
        uint8 minRaters,
        uint256 timeout
    ) external returns (uint256 taskId) {
        require(maxRaters >= 2, "need >= 2 max raters");
        require(minRaters >= 2, "need >= 2 min raters");
        require(minRaters <= maxRaters, "min > max");
        require(timeout > 0, "zero timeout");

        taskId = nextTaskId++;
        tasks[taskId] = Task({
            promptHash: promptHash,
            worker: msg.sender,
            outputHash: outputHash,
            maxRaters: maxRaters,
            minRaters: minRaters,
            deadline: block.timestamp + timeout,
            phase: Phase.AwaitingRatings,
            accepted: false
        });

        // add to active list
        _activeTasks.push(taskId);
        _activeIndex[taskId] = _activeTasks.length; // 1-indexed

        emit TaskCreated(taskId, msg.sender, promptHash, maxRaters, minRaters, block.timestamp + timeout);
    }

    /// @notice Submit a rating for an open task. First-come-first-served up to maxRaters.
    /// @param taskId    The task to rate.
    /// @param signal    true = good, false = bad.
    /// @param prediction Predicted fraction of "good" votes (64.64 fixed-point, [0, 1]).
    function submitRating(uint256 taskId, bool signal, int128 prediction) external {
        Task storage t = tasks[taskId];
        require(t.phase == Phase.AwaitingRatings, "not awaiting ratings");
        require(!hasRated[taskId][msg.sender], "already rated");
        require(msg.sender != t.worker, "worker cannot rate own task");
        require(ratings[taskId].length < t.maxRaters, "max raters reached");
        require(block.timestamp <= t.deadline, "deadline passed");
        require(prediction >= 0 && prediction <= ABDKMath64x64.fromUInt(1), "prediction out of range");
        require(balances[msg.sender] >= collateralPerRating, "insufficient collateral");

        // lock collateral
        balances[msg.sender] -= collateralPerRating;

        // store rating
        hasRated[taskId][msg.sender] = true;
        ratings[taskId].push(Rating({
            rater: msg.sender,
            signal: signal,
            prediction: prediction
        }));

        uint256 count = ratings[taskId].length;
        emit RatingSubmitted(taskId, msg.sender, signal, count);

        // auto-score when maxRaters reached
        if (count >= t.maxRaters) {
            _score(taskId);
        }
    }

    /// @notice Finalize a task after timeout if minRaters have been reached.
    ///         Can be called by anyone. Also handles auto-finalization if maxRaters reached.
    /// @param taskId The task to finalize.
    function finalizeTask(uint256 taskId) external {
        Task storage t = tasks[taskId];
        require(t.phase == Phase.AwaitingRatings, "not awaiting ratings");

        uint256 count = ratings[taskId].length;

        if (count >= t.maxRaters) {
            // maxRaters reached — score immediately
            _score(taskId);
        } else if (count >= t.minRaters && block.timestamp > t.deadline) {
            // minRaters met and deadline passed — score with what we have
            _score(taskId);
        } else {
            revert("finalization conditions not met");
        }
    }

    /// @notice Cancel an under-subscribed task after deadline.
    ///         Refunds collateral to any raters who submitted.
    /// @param taskId The task to cancel.
    function cancelTask(uint256 taskId) external {
        Task storage t = tasks[taskId];
        require(t.phase == Phase.AwaitingRatings, "not awaiting ratings");
        require(block.timestamp > t.deadline, "deadline not passed");

        uint256 count = ratings[taskId].length;
        require(count < t.minRaters, "enough raters to finalize");

        // refund collateral to raters who submitted
        Rating[] storage r = ratings[taskId];
        for (uint256 i = 0; i < count; i++) {
            balances[r[i].rater] += collateralPerRating;
        }

        t.phase = Phase.Cancelled;
        _removeActive(taskId);

        emit TaskCancelled(taskId);
    }

    // ── views ────────────────────────────────────────────────────────────

    function getTask(uint256 taskId)
        external
        view
        returns (Task memory task, Rating[] memory taskRatings)
    {
        task = tasks[taskId];
        taskRatings = ratings[taskId];
    }

    function getActiveTasks() external view returns (uint256[] memory) {
        return _activeTasks;
    }

    function getRatings(uint256 taskId) external view returns (Rating[] memory) {
        return ratings[taskId];
    }

    function getScore(uint256 taskId, address rater) external view returns (int256) {
        return scores[taskId][rater];
    }

    function getWorkerReputation(address worker) external view returns (uint256 tasksCompleted, uint256 approvals) {
        WorkerRecord storage rec = workerReputation[worker];
        return (rec.tasksCompleted, rec.approvals);
    }

    // ── internal: RBTS scoring ───────────────────────────────────────────

    /// @dev epsilon to avoid log(0), as 64.64 fixed-point.
    function _eps() internal pure returns (int128) {
        return int128(1); // smallest positive 64.64 value ≈ 5.4e-20
    }

    /// @dev clamp a 64.64 value to [eps, 1 - eps].
    function _clamp(int128 x) internal pure returns (int128) {
        int128 one = ABDKMath64x64.fromUInt(1);
        int128 lo = _eps();
        int128 hi = one.sub(lo);
        if (x < lo) return lo;
        if (x > hi) return hi;
        return x;
    }

    /// @dev quadratic prediction score: QPS(p, x) = 2px + 2(1-p)(1-x) - p² - (1-p)²
    function _qps(int128 p, int128 x) internal pure returns (int128) {
        int128 one = ABDKMath64x64.fromUInt(1);
        int128 two = ABDKMath64x64.fromUInt(2);
        int128 oneMinusP = one.sub(p);
        int128 oneMinusX = one.sub(x);

        // 2*p*x + 2*(1-p)*(1-x) - p*p - (1-p)*(1-p)
        return two.mul(p).mul(x)
            .add(two.mul(oneMinusP).mul(oneMinusX))
            .sub(p.mul(p))
            .sub(oneMinusP.mul(oneMinusP));
    }

    /// @dev compute RBTS scores and redistribute collateral.
    function _score(uint256 taskId) internal {
        Task storage t = tasks[taskId];
        Rating[] storage r = ratings[taskId];
        uint256 n = r.length;
        require(n >= 2, "need >= 2 ratings");

        int128 nFp = ABDKMath64x64.fromUInt(n);

        // count "good" votes and compute actual good fraction
        uint256 numGood = 0;
        int128 sumPredictions = int128(0);
        for (uint256 i = 0; i < n; i++) {
            if (r[i].signal) numGood++;
            sumPredictions = sumPredictions.add(r[i].prediction);
        }

        int128 actualGoodFrac = _clamp(ABDKMath64x64.fromUInt(numGood).div(nFp));
        int128 actualBadFrac = ABDKMath64x64.fromUInt(1).sub(actualGoodFrac);
        int128 avgPredGood = _clamp(sumPredictions.div(nFp));
        int128 avgPredBad = ABDKMath64x64.fromUInt(1).sub(avgPredGood);

        // bts acceptance: actual good fraction >= average predicted good fraction
        bool btsAccepted = actualGoodFrac >= avgPredGood;
        uint256 approval100 = (numGood * 100) / n;
        t.accepted = btsAccepted && (approval100 >= 50);
        t.phase = Phase.Scored;

        // update worker reputation
        workerReputation[t.worker].tasksCompleted++;
        if (t.accepted) {
            workerReputation[t.worker].approvals++;
        }

        // total collateral pool to redistribute
        uint256 totalPool = n * collateralPerRating;

        // compute raw RBTS payments (64.64)
        int128[] memory rawPayments = new int128[](n);
        int128 sumRaw = int128(0);

        for (uint256 i = 0; i < n; i++) {
            // information score: ln(actual_freq / avg_predicted_freq) for chosen signal
            int128 infoScore;
            if (r[i].signal) {
                infoScore = ABDKMath64x64.ln(actualGoodFrac.div(avgPredGood));
            } else {
                infoScore = ABDKMath64x64.ln(actualBadFrac.div(avgPredBad));
            }

            int128 predScore = _qps(r[i].prediction, actualGoodFrac);
            rawPayments[i] = alpha.mul(infoScore).add(beta.mul(predScore));
            sumRaw = sumRaw.add(rawPayments[i]);
        }

        // redistribute the collateral pool:
        // 1. shift all scores so the minimum becomes zero (all values >= 0)
        // 2. distribute pool proportionally to shifted scores
        int128 minPayment = rawPayments[0];
        for (uint256 i = 1; i < n; i++) {
            if (rawPayments[i] < minPayment) {
                minPayment = rawPayments[i];
            }
        }

        int128[] memory shifted = new int128[](n);
        int128 shiftedTotal = int128(0);
        for (uint256 i = 0; i < n; i++) {
            shifted[i] = rawPayments[i].sub(minPayment);
            shiftedTotal = shiftedTotal.add(shifted[i]);
        }

        if (shiftedTotal == 0) {
            // all scores identical — return collateral equally
            uint256 share = totalPool / n;
            for (uint256 i = 0; i < n; i++) {
                scores[taskId][r[i].rater] = int256(share);
                balances[r[i].rater] += share;
            }
        } else {
            for (uint256 i = 0; i < n; i++) {
                // payout_i = shifted[i] / shiftedTotal * totalPool
                uint256 payout = ABDKMath64x64.mulu(shifted[i].div(shiftedTotal), totalPool);
                scores[taskId][r[i].rater] = int256(payout);
                balances[r[i].rater] += payout;
            }
        }

        // remove from active list
        _removeActive(taskId);

        emit TaskFinalized(taskId, n, t.accepted);
    }

    /// @dev remove a task from the active list (swap-and-pop).
    function _removeActive(uint256 taskId) internal {
        uint256 idx1 = _activeIndex[taskId]; // 1-indexed
        if (idx1 == 0) return;
        uint256 idx = idx1 - 1;
        uint256 lastIdx = _activeTasks.length - 1;
        if (idx != lastIdx) {
            uint256 lastTaskId = _activeTasks[lastIdx];
            _activeTasks[idx] = lastTaskId;
            _activeIndex[lastTaskId] = idx + 1;
        }
        _activeTasks.pop();
        delete _activeIndex[taskId];
    }
}
