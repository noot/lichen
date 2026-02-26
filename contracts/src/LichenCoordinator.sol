// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {ABDKMath64x64} from "abdk/ABDKMath64x64.sol";

/// @title LichenCoordinator
/// @notice On-chain coordinator for the Lichen RBTS protocol.
///         Manages task lifecycle, collects ratings, computes RBTS scores,
///         and redistributes staked ETH among raters.
contract LichenCoordinator {
    using ABDKMath64x64 for int128;

    // ── Types ────────────────────────────────────────────────────────────

    enum Phase {
        AwaitingWork,
        AwaitingRatings,
        Scored
    }

    struct Task {
        bytes32 promptHash;
        address worker;
        bytes32 outputHash;
        uint8 numRatersRequired;
        Phase phase;
        bool accepted;
    }

    struct Rating {
        address rater;
        bool signal;       // true = GOOD
        int128 prediction; // 64.64 fixed-point, range [0, 1]
    }

    // ── State ────────────────────────────────────────────────────────────

    /// RBTS weighting parameters (64.64 fixed-point).
    int128 public immutable alpha;
    int128 public immutable beta;

    /// Collateral locked per rating (in wei).
    uint256 public immutable collateralPerRating;

    /// Agent ETH balances.
    mapping(address => uint256) public balances;

    /// Task storage.
    uint256 public nextTaskId;
    mapping(uint256 => Task) public tasks;

    /// Ratings per task.
    mapping(uint256 => Rating[]) public ratings;

    /// Track whether an address has already rated a task.
    mapping(uint256 => mapping(address => bool)) public hasRated;

    /// RBTS scores after scoring (taskId => rater => payment in WAD).
    mapping(uint256 => mapping(address => int256)) public scores;

    /// Worker reputation: tracks cumulative task outcomes per worker.
    struct WorkerRecord {
        uint256 tasksCompleted;
        uint256 approvals;
    }
    mapping(address => WorkerRecord) public workerReputation;

    /// List of active (non-scored) task IDs for polling.
    uint256[] internal _activeTasks;
    mapping(uint256 => uint256) internal _activeIndex; // taskId => index+1 (0 = not active)

    // ── Events ───────────────────────────────────────────────────────────

    event Deposited(address indexed agent, uint256 amount);
    event Withdrawn(address indexed agent, uint256 amount);
    event TaskCreated(uint256 indexed taskId, bytes32 promptHash, uint8 numRaters);
    event ResultSubmitted(uint256 indexed taskId, address indexed worker, bytes32 outputHash);
    event RatingSubmitted(uint256 indexed taskId, address indexed rater, bool signal);
    event TaskScored(uint256 indexed taskId, bool accepted);

    // ── Constructor ──────────────────────────────────────────────────────

    /// @param _alpha  Information score weight (64.64 fixed-point).
    /// @param _beta   Prediction score weight (64.64 fixed-point).
    /// @param _collateral Collateral per rating in wei.
    constructor(int128 _alpha, int128 _beta, uint256 _collateral) {
        alpha = _alpha;
        beta = _beta;
        collateralPerRating = _collateral;
    }

    // ── Deposit / Withdraw ───────────────────────────────────────────────

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

    // ── Task Lifecycle ───────────────────────────────────────────────────

    function createTask(bytes32 promptHash, uint8 numRaters) external returns (uint256 taskId) {
        require(numRaters >= 2, "need >= 2 raters");
        taskId = nextTaskId++;
        tasks[taskId] = Task({
            promptHash: promptHash,
            worker: address(0),
            outputHash: bytes32(0),
            numRatersRequired: numRaters,
            phase: Phase.AwaitingWork,
            accepted: false
        });
        // Add to active list
        _activeTasks.push(taskId);
        _activeIndex[taskId] = _activeTasks.length; // 1-indexed
        emit TaskCreated(taskId, promptHash, numRaters);
    }

    function submitResult(uint256 taskId, bytes32 outputHash) external {
        Task storage t = tasks[taskId];
        require(t.phase == Phase.AwaitingWork, "not awaiting work");
        t.worker = msg.sender;
        t.outputHash = outputHash;
        t.phase = Phase.AwaitingRatings;
        emit ResultSubmitted(taskId, msg.sender, outputHash);
    }

    function submitRating(uint256 taskId, bool signal, int128 prediction) external {
        Task storage t = tasks[taskId];
        require(t.phase == Phase.AwaitingRatings, "not awaiting ratings");
        require(!hasRated[taskId][msg.sender], "already rated");
        require(prediction >= 0 && prediction <= ABDKMath64x64.fromUInt(1), "prediction out of range");
        require(balances[msg.sender] >= collateralPerRating, "insufficient collateral");

        // Lock collateral
        balances[msg.sender] -= collateralPerRating;

        // Store rating
        hasRated[taskId][msg.sender] = true;
        ratings[taskId].push(Rating({
            rater: msg.sender,
            signal: signal,
            prediction: prediction
        }));
        emit RatingSubmitted(taskId, msg.sender, signal);

        // Auto-score when all ratings are in
        if (ratings[taskId].length >= t.numRatersRequired) {
            _score(taskId);
        }
    }

    // ── Views ────────────────────────────────────────────────────────────

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

    // ── Internal: RBTS Scoring ───────────────────────────────────────────

    /// @dev Epsilon to avoid log(0), as 64.64 fixed-point.
    function _eps() internal pure returns (int128) {
        return int128(1); // smallest positive 64.64 value ≈ 5.4e-20
    }

    /// @dev Clamp a 64.64 value to [eps, 1 - eps].
    function _clamp(int128 x) internal pure returns (int128) {
        int128 one = ABDKMath64x64.fromUInt(1);
        int128 lo = _eps();
        int128 hi = one.sub(lo);
        if (x < lo) return lo;
        if (x > hi) return hi;
        return x;
    }

    /// @dev Quadratic Prediction Score: QPS(p, x) = 2px + 2(1-p)(1-x) - p² - (1-p)²
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

    /// @dev Compute RBTS scores and redistribute collateral.
    function _score(uint256 taskId) internal {
        Task storage t = tasks[taskId];
        Rating[] storage r = ratings[taskId];
        uint256 n = r.length;
        require(n >= 2, "need >= 2 ratings");

        int128 nFp = ABDKMath64x64.fromUInt(n);

        // Count "good" votes and compute actual good fraction
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

        // BTS acceptance: actual good fraction >= average predicted good fraction
        bool btsAccepted = actualGoodFrac >= avgPredGood;
        uint256 approval100 = (numGood * 100) / n;
        t.accepted = btsAccepted && (approval100 >= 50);
        t.phase = Phase.Scored;

        // Update worker reputation
        workerReputation[t.worker].tasksCompleted++;
        if (t.accepted) {
            workerReputation[t.worker].approvals++;
        }

        // Total collateral pool to redistribute
        uint256 totalPool = n * collateralPerRating;

        // Compute raw RBTS payments (64.64)
        int128[] memory rawPayments = new int128[](n);
        int128 sumRaw = int128(0);

        for (uint256 i = 0; i < n; i++) {
            // Information score: ln(actual_freq / avg_predicted_freq) for chosen signal
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

        // Zero-sum redistribution matching the off-chain Rust implementation:
        // 1. Center payments around the mean
        // 2. Normalize by sum of absolute deviations so full pool is redistributed
        // 3. Floor negatives to 0, then redistribute clipped amount to positive scorers
        int128 mean = sumRaw.div(nFp);

        // Compute centered values and sum of absolute deviations
        int128[] memory centered = new int128[](n);
        int128 totalAbs = int128(0);
        for (uint256 i = 0; i < n; i++) {
            centered[i] = rawPayments[i].sub(mean);
            int128 absVal = centered[i] < 0 ? centered[i].neg() : centered[i];
            totalAbs = totalAbs.add(absVal);
        }

        if (totalAbs == 0) {
            // All scores identical — return collateral equally
            uint256 share = totalPool / n;
            for (uint256 i = 0; i < n; i++) {
                scores[taskId][r[i].rater] = int256(share);
                balances[r[i].rater] += share;
            }
        } else {
            // payout_i = (centered[i] / totalAbs) * totalPool
            // This can be negative, so we do two passes:
            // Pass 1: compute raw payouts, floor negatives to 0, track clipped amount
            int256[] memory payouts = new int256[](n);
            int256 clipped = 0;
            int256 positiveSum = 0;

            for (uint256 i = 0; i < n; i++) {
                // centered[i] / totalAbs * totalPool
                int256 raw = ABDKMath64x64.muli(centered[i].div(totalAbs), int256(totalPool));
                payouts[i] = raw;
                if (raw < 0) {
                    clipped += -raw;  // track how much we floored
                    payouts[i] = 0;
                } else {
                    positiveSum += raw;
                }
            }

            // Pass 2: redistribute clipped amount proportionally to positive scorers
            for (uint256 i = 0; i < n; i++) {
                if (payouts[i] > 0 && positiveSum > 0) {
                    // Add proportional share of clipped amount
                    payouts[i] += (clipped * payouts[i]) / positiveSum;
                }

                scores[taskId][r[i].rater] = payouts[i];
                balances[r[i].rater] += uint256(payouts[i]);
            }
        }

        // Remove from active list
        _removeActive(taskId);

        emit TaskScored(taskId, t.accepted);
    }

    /// @dev Remove a task from the active list (swap-and-pop).
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
